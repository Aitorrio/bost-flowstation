# Group call timers (Hangtime & Call timeout)

These settings are under **Config → TETRA identity → Advanced network / timers**. Each field has a **?** help control in the dashboard (live settings and Cell/Brew profile sheets).

## Hangtime (`hangtime_secs`, default 5)

After the last speaker releases PTT, the group call stays allocated for this many seconds (“group in use” on many terminals). Another PTT inside that window **reuses the same call** (same `call_id`) without a full setup cycle.

## Call timeout (`call_timeout_secs`, default 120)

ETSI-style **T310**: absolute maximum duration of **one group call**, measured from call establishment — **not** the length of each PTT.

Because hangtime (and quick turn-taking on LST Dispatch / Brew) keeps the call alive between overs, a long QSO can hit this ceiling. The BS then sends **D-RELEASE**; Motorola and others may show **PTT denied**.

| Value | Meaning |
|-------|---------|
| *(empty / reset in UI)* | Engine default **120** s |
| **0** | Unlimited (until hangtime / disconnect) |
| **600**, **1800**, … | Longer absolute ceiling for dispatch QSOs |

This measurement mode is intentional and aligned with FlowStation / ETSI absolute call time-out. Raising or clearing the limit is an **operator setting**, not a stack bug.

See also comments in [`example_config/config.toml`](../example_config/config.toml) under `[cell_info]`.
