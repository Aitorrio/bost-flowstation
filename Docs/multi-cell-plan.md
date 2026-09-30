# Multi-cell (N SDRs, one station) — implementation plan

Status: **draft / planning** (branch `beta`). Goal: one `bluestation-bs` process drives N SDRs, each SDR
being one TETRA cell (optionally dual-carrier), sharing one subscriber database, one set of network
links (Brew / Asterisk / LST / DAPNET / Telegram) and one dashboard, with group calls spanning cells
and radios roaming between cells.

## Where we start from

| Area | Today | Blocker for N cells |
|---|---|---|
| Process layout | `build_bs_stack` builds one entity chain on one `MessageRouter` | Router is keyed by `TetraEntity` → only one `Umac`, `Mm`, `Cmce`… per router |
| PHY | `try_attach_phy` opens one `RxTxDevSoapySdr` | One device, one sample clock, one TDMA timebase |
| Config | One `[cell_info]`, one `[phy_io.soapysdr]` | No notion of "cell id" |
| Mutable state | `SharedConfig.state` (`StackState`) holds `SubscriberRegistry` **and** `timeslot_alloc` | Registry is fine to share; timeslot allocator is per-cell resource |
| Globals | `rf_status`, `health::registry`, `service_control`, `DETECTED_SDR_NAME`, dashboard caches | Single-valued; must become per-cell or aggregated |
| Mobility | `neighbor_cells_ca` broadcast in D-NWRK-BROADCAST; D-NEW-CELL PDU exists | No inter-cell registration / call handling |
| systemd | `ExecStartPre` resets **all** USB devices | Fine for one process (resets all SDRs together) |

## Architecture target

```
                     ┌──────────────── Site core (1 per process) ────────────────┐
                     │ SubscriberRegistry · Group→cells map · Call switch (CMCE-X)│
                     │ Brew · Asterisk · LST · DAPNET · Telegram · Dashboard      │
                     └───────▲──────────────────▲──────────────────▲──────────────┘
                   site bus  │ (crossbeam chans)│                  │
     ┌───────────────────────┴──┐  ┌────────────┴─────────────┐  ┌─┴──── … cell N
     │ Cell 0 thread            │  │ Cell 1 thread            │
     │ Router: PHY→LMAC→UMAC→   │  │ Router: PHY→LMAC→UMAC→   │
     │ LLC→MLE→MM→CMCE→SNDCP    │  │ LLC→MLE→MM→CMCE→SNDCP    │
     │ SDR #0 (serial A)        │  │ SDR #1 (serial B)        │
     └──────────────────────────┘  └──────────────────────────┘
```

Key decision: **one router + one RT thread per cell**, not one router for all cells. Each SDR has its own
clock and TDMA timebase; keeping them in separate loops avoids cross-cell timing coupling and keeps the
existing entity code (which assumes one of each entity) almost untouched. Cells talk to the site core
over bounded channels, never share `&mut` state.

---

## Phase 0 — Groundwork & measurements (≈1 week)
- Benchmark CPU of one cell on Pi 4/5 (per-thread %), decide supported max (likely 2 on Pi 4, 3–4 on Pi 5).
- Test two SDRs on one USB bus (LimeSDR Mini 2 + SXceiver) for throughput/underruns.
- Inventory every `static`/`OnceLock` and every `cfg.config().cell` / `phy_io` use (14 files) → tag
  each as *per-cell* or *site-wide*.
- Add `CellId(u8)` type in `tetra-core`.

**Exit:** written inventory + CPU budget; no behaviour change.

## Phase 1 — Config schema (≈1 week)
- New optional `[[cells]]` array; each entry: `id`, `cell_info` (carriers, colour code, LA, BS id…),
  `soapysdr` (`device` serial mandatory when >1 cell, gains, fs, centers).
- Legacy single `[cell_info]` + `[phy_io.soapysdr]` auto-maps to `cells = [{ id = 0, … }]` — existing
  configs keep working unchanged.
- Validation: unique ids, unique device serials, no overlapping carriers, same MCC/MNC, distinct
  (LA or colour code) per cell, each cell passes the existing passband check.
- `StackConfig::cell(id)` accessor; `SharedConfig` gains a per-cell view (`CellConfig`) that entities
  use instead of `config().cell`.

**Exit:** parser + validator + unit tests; stack still runs only `cells[0]`.

## Phase 2 — Per-cell stack instantiation (≈2 weeks)
- Refactor `build_bs_stack` → `build_cell_stack(cell_id, …) -> CellRuntime` (router + entities + PHY).
- Move `timeslot_alloc` from `StackState` into per-cell state.
- Spawn one RT thread per cell (FIFO priority as today), each opening its SDR by serial.
- Globals: `rf_status` and `health::registry` become maps keyed by `CellId`, with an aggregate view
  (site "online" = all/any cells online, configurable). `service_control` stays site-wide.
- With N=2 and **no** inter-cell features: two cells run, each only serves its own radios; network
  entities (Brew etc.) still attached to cell 0 only.

**Exit:** two SDRs transmitting two independent cells from one process; single-cell configs unchanged
(regression test: existing integration tests in `crates/tetra-entities/tests` pass).

## Phase 3 — Site core & site bus (≈2–3 weeks)
- Extract Brew / Asterisk / LST / DAPNET / Telegram / GeoAlarm out of the per-cell router into a
  site-core thread. Define `SiteMsg` (voice frames, SDS, call control events, registration events)
  with `CellId` tagging.
- `SubscriberRegistry` gains `current_cell: Option<CellId>` per ISSI; MM on each cell writes it on
  U-LOCATION-UPDATE.
- Site core builds `group → set<CellId>` from affiliations.
- Routing rules: incoming Brew/LST group call → every cell with members of that group; SDS to ISSI →
  the cell holding it; unknown location → broadcast/page all cells.

**Exit:** external network traffic reaches radios on any cell; SDS works across cells.

## Phase 4 — Inter-cell group & individual calls (≈3 weeks)
- CMCE "call switch" in site core: a group call started on cell A triggers D-SETUP on every other cell
  with members; UL voice from the talker's cell is fanned out as DL TCH to other cells (and to Brew).
- Floor control (U-TX-DEMAND / D-TX-GRANTED) arbitrated centrally so only one talker site-wide.
- Individual (P2P/duplex) calls between radios on different cells.
- Handle unequal TDMA timing between cells: voice is re-framed per cell (jitter buffer similar to
  `net_brew/components/jitter_buffer.rs`).

**Exit:** radio on cell 0 and radio on cell 1 in the same talkgroup hear each other; P2P works.

## Phase 5 — Mobility (≈2–3 weeks)
- Auto-populate `neighbor_cells_ca` from sibling cells (no manual config).
- Cell reselection: accept migrating U-LOCATION-UPDATE, move ISSI in registry, silently drop the stale
  entry on the old cell (MM state cleanup).
- Call restoration across cells (U-CALL-RESTORE / D-CALL-RESTORE already partly in
  `cc_bs/procedures/restoration.rs`) so an ongoing group call survives a cell change.
- Optional: announced handover via D-NEW-CELL (stretch goal; many terminals do fine with
  unannounced reselection + restore).

**Exit:** walking a radio between two cells keeps registration and rejoins an active call.

## Phase 6 — Dashboard, ops & packaging (≈2 weeks, parallelisable from Phase 2)
- Dashboard: cell selector / per-cell cards (RF status, carriers, load, registered radios, SDR name),
  site-wide views remain aggregated. `/api/btsinfo` returns a `cells` array.
- Config UI: add/remove cell, pick SDR by detected serial (Setup wizard enumerates all Soapy devices).
- Telemetry/control protocol: add `cell_id` to events.
- systemd: keep one unit; USB reset stays "all devices" (acceptable since one process owns all SDRs).
- Docs + CHANGELOG, beta release, then promote to stable.

---

## Risks
- **CPU on Raspberry Pi** — may cap practical N at 2; Phase 0 decides.
- **USB bandwidth / power** for two SDRs on one Pi (powered hub may be required).
- **Terminal behaviour** on reselection/restoration differs per vendor (Motorola MXP600/MTM800E/MTM5400
  must be tested each phase).
- **Regression of single-cell users** — every phase must keep legacy config working; gate multi-cell
  behind `[[cells]]` presence.
- **Panic containment**: one cell's caught panic must not degrade others (per-cell health counters).

## Suggested delivery
Phases 1–2 ship as one beta ("independent multi-cell"), 3–4 as the next ("linked multi-cell"),
5–6 as the release that promotes to stable. Rough total: 12–16 weeks for one developer.
