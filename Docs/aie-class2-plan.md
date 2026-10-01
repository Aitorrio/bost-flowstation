# Air interface encryption — security class 2 (SCK)

Status: **design draft**, nothing implemented. Target radios: Motorola MTH800 / MTH850.

Normative reference: **ETSI TS 100 392-7 V4.2.1 (2026-04)** (TETRA V+D security), clause 6
(air interface encryption) and Annex A, with the PDU encodings in EN 300 392-2 (clause 16 MM,
clause 21 MAC). Local copy (not committed): `Docs/spec/ts_10039207v040201p.pdf`. The MAC PDU
field positions come from EN 300 392-2 and the existing parsers; they are not checked here.

## 0. Checked against TS 100 392-7 V4.2.1

| Item | Spec | Result |
|---|---|---|
| Ciphering parameters, 10 bits | Table A.46 | KSG number (4), security class (1: 0 = class 2), SCK number (5) — as built |
| KSG number | Table 6.2 | 0 = TEA1 … 3 = TEA4 (set A, 80-bit CK); 4–6 = TEA5–7 (set B, 192-bit CKX); 8–11 proprietary |
| SCK number | Table A.96 | 0 = SCK 1 … 31 = SCK 32 — as built |
| SCK-VN | Table A.102, cl. 4.2.4.0b | 16 bits; LSB carried in every encrypted MAC-RESOURCE |
| Negotiation | cl. 6.6.2.1.2, 6.7.4 | Every registration on a class 2 cell carries cipher control = 1 + parameters; mixed class 1/2 cells are allowed (radio registers at the highest class it can). A refusal is a D-LOCATION UPDATE REJECT with cause 13–16 or 18 **that carries the cell's preferred parameters** — now done |
| Encrypted registration | cl. 6.6.2.2 | A radio that holds the broadcast SCKN may encrypt its very first registration (ITSI attach). The BS must therefore be able to decrypt an uplink before the radio is known |
| IV (TEA set A) | cl. 6.3.2.1 | 29 bits, numbered from the LSB: IV(0..1) = TN−1, IV(2..6) = FN (1–18), IV(7..12) = MN (1–60), IV(13..27) = 15 LSBs of the hyperframe, IV(28) = 0 DL / 1 UL |
| ECK | cl. 6.3.2.2, fig. 6.2 | ECK = TB5(CK, CN, LA-id, CC) — per carrier, so each cell and each carrier gets its own |
| Broadcast | cl. 6.3.2.0a | Hyperframe (IV(13..27)) is broadcast in SYSINFO **in turn with SCK-VN** on a schedule chosen by the SwMI |
| DL encryption mode | Table 6.6 | 00 clear, 01 reserved, 10 encrypted SCK-VN even, 11 encrypted SCK-VN odd |
| UL | cl. 6.5.2 | One "encrypted" bit in every uplink MAC header |
| Never encrypted | cl. 6.7.1.1 | SYNC, SYSINFO (TMB-SAP), ACCESS-DEFINE |
| What is encrypted | cl. 6.7.1.2 | MAC-RESOURCE and DL MAC-END: everything after the channel allocation flag (and the TM-SDU); KSS is per timeslot (max 432 bits on π/4-DQPSK) |
| Address | cl. 4.2.6, 6.7.1.2 | With TEA set A, whenever a MAC PDU is encrypted its SSI is replaced by the **ESI = TA61(SSI, SCK)** — for individual, group and broadcast addresses. Event label, usage marker, USSI and SMI are not encrypted |
| Key changes | cl. 6.3.2.0 | Change the SCK within 23 days to avoid IV reuse (recommendation) |

**Consequence for phase 2:** besides TB5 and TEA1, the station needs **TA61** (ESI). The BS
keeps an ESI ↔ SSI table for every registered ISSI, every group in use and the broadcast address,
recomputed when the SCK changes, and uses it to recognise encrypted uplink addresses. TB5, TA61
and TEA1 are ETSI-confidential and are not in this specification.

## 1. What class 2 is

- One **static cipher key (SCK)**, 80 bits, shared by the infrastructure and every radio. Up to 32
  SCKs are numbered by **SCKN** (1–32, sent as 5 bits); the network says which one is in use.
- No authentication. Anyone with the SCK can join; class 2 protects against passive listening only.
- Signalling and traffic (voice) on the air are encrypted; the backhaul (site switch, Brew,
  Asterisk, LST) stays in clear, because each cell decrypts at its own MAC.

## 2. Crypto chain (TS 100 392-7 clause 6.3)

1. **ECK = TB5(SCK, CN, LA-id, CC)** — the encryption key is modified per carrier by colour
   code, carrier number and location area, so every cell (and carrier) of a station gets its own
   ECK from the same SCK. Computed once per carrier at start-up.
2. **IV (29 bits)**, see §0. The hyperframe count must be exact on both ends.
3. **Keystream = KSG(ECK, IV)** — TEA*n* chosen by the KSG number (TEA1 = 1 … TEA4 = 4).
4. Ciphertext = plaintext XOR keystream, applied per burst/slot to the encrypted part of the
   MAC block (§5) or to the TCH bits (voice).

The TB5 and TEA*n* primitives are ETSI-confidential. The implementation keeps them behind one
trait (`Ksg::keystream(eck, iv, n_bits)`, `Tb5::eck(...)`) in a separate module, so the algorithm
source (licensed from ETSI or otherwise) is a single, replaceable file. Choosing that source is the
operator's legal / export-control decision; this project does not ship one by default.

**Which TEA the MTH800/850 run** depends on the radio's codeplug and licence options (typically
TEA1 on commercial units). Must be confirmed from the radio before writing the KSG.

## 3. Pre-check: do the radios offer SCK?

MM already logs `class_of_ms` at registration (`mm_bs.rs`, "MS … class_of_ms: …"). Register an
MTH800 to the current build and look for `sck:true` (and `aiv:true`). If the radio does not report
SCK capability, it has no AIE licence / codeplug and nothing below will work for it. The SCK
itself is loaded with Motorola's programming/key-loading tooling; the same 80-bit value and SCKN go
into `config.toml`.

## 4. Changes, layer by layer

### Config (`tetra-config`)
```toml
[security.aie]
class = 2                 # 1 = clear (default), 2 = SCK
ksg = 1                   # TEA1
sckn = 1                  # 1..32
sck = "0123456789ABCDEF0123"   # 80 bits hex — never shown in dashboard/telemetry/logs
sck_vn = 0                # SCK version number loaded with the key (16 bits)
```
Validation: sck exactly 20 hex digits, sckn 1–32, ksg 1–4. Extra cells inherit it. The key is
redacted from every serialisation path (dashboard config view, telemetry, profiles export).

### Broadcast (UMAC SYSINFO / MLE)
- SYSINFO: set "hyperframe / cipher key" flag to send the **hyperframe number** (not a CCK id),
  so radios lock their IV counter. Already parsed in `mac_sysinfo.rs` (`hyperframe_number`).
- Extended services security element (`sysinfo_ext_services.rs`): `class2_supported`, `sck_n`,
  `auth_required = false`, and the "AIE enabled" bit in the parent element. Currently hard-coded
  in `umac_bs.rs:196`.
- Mixed cell: class 1 is also advertised (`class1_supported`), so radios without AIE still
  register in clear. Config flag `allow_clear = true` (decided; see §4a).

### MM (registration)
- U-LOCATION-UPDATE-DEMAND: read the **ciphering parameters** (KSG number, security class,
  SCKN) the radio proposes (Table A.46).
- Accepted parameters need no echo in D-LOCATION-UPDATE-ACCEPT for TEA set A (the "Security
  downlink" element is only needed for TEA set B identity encryption, cl. 4.2.6). A radio that
  holds the SCK may already send the registration encrypted (cl. 6.6.2.2); the uplink MAC
  "encrypted" bit says so, and the BS decrypts on that bit, not on the registry.
- Per-ISSI state in the client registry: `encrypted: bool`, KSG, SCKN.
- Reject a radio proposing another SCKN or KSG, sending the cell's own parameters in the
  D-LOCATION UPDATE REJECT (cl. 6.6.2.1.2).

### 4a. Mixed-cell policy (decided)

Clear radios may register, but they talk **only with other clear radios**; encrypted radios
talk only with encrypted radios. The station never bridges between the two, so nothing an
encrypted radio says is ever sent in clear.

- **Individual calls**: refused when caller and called differ in mode (D-RELEASE, cause
  "requested service not available" (EN 300 392-2 disconnect cause) ).
- **SDS / status**: same rule, the station rejects delivery across modes.
- **Groups**: a group call goes out on one channel to every member of the GSSI, so it cannot be
  half clear and half encrypted. Each talkgroup therefore gets a mode in config
  (`[security.aie] clear_groups = [...]`; every other group is encrypted). A radio may only
  attach to / call groups of its own mode: the station drops the other mode's attachments
  (D-ATTACH/DETACH-GROUP-IDENTITY) and refuses calls on them.
- **Network side** (Brew, Asterisk, LST, other cells): an encrypted group or ISSI is still
  routed as today — the backhaul is outside the air interface. A clear radio cannot reach an
  encrypted one through the network either: the same check runs on calls arriving from the
  switch.
- Dashboard shows each radio's and each group's mode.

### LLC / MLE / CMCE plumbing
- Fill the existing `air_interface_encryption` fields (`tla`, `tma` SAPs; `TODO FIXME`s in
  `llc_bs_ms.rs`) from the destination's state: encrypted for a class-2 ISSI or any group once
  AIE is on. Only SYNC, SYSINFO and ACCESS-DEFINE are always clear (cl. 6.7.1.1); other
  broadcasts (D-NWRK-BROADCAST, home-mode SDS) are encrypted on a class-2-only cell, but on a
  mixed cell they must stay clear so class 1 radios can read them.

### UMAC (signalling)
- MAC-RESOURCE / DL MAC-END: set `encryption_mode` (10/11 by SCK-VN parity), replace the SSI
  with its ESI (TA61), encrypt everything after the channel allocation flag plus the TM-SDU with
  the slot's KSS, before channel coding (cl. 6.7.1.2). MAC-FRAG / MAC-DATA: TM-SDU. Fragmented
  PDUs: every fragment uses its own slot's IV. KSS allocation within a slot (PDU association,
  half slots): cl. 6.4.2.2 — read before coding.
- Uplink: decrypt the TM-SDU of a MAC-ACCESS/MAC-DATA from an encrypted ISSI before LLC.
- Stealing (FACCH) on traffic slots follows the same rule.

### UMAC/PHY (voice)
- On a traffic channel of an encrypted call, XOR the 432 TCH bits (per slot) with the keystream
  on DL, and decrypt UL before handing ACELP frames to the switch/Brew. One keystream per
  slot, IV from the slot's TDMA time.
- Group calls: all members use the same SCK, so one DL keystream serves the group.

### Multi-cell
- Each cell computes its own ECK (different CN, maybe LA). Site switch, playout buffers and
  Brew routing are unchanged — they only ever see clear PDUs and voice.
- Handover (D-NEW-CELL): the target cell must treat the arriving ISSI as encrypted; carry the
  flag in the switch's prepare message.

### Dashboard / telemetry
- Show per-radio "AIE: SCK n / clear" and the cell's class. Never the key.

## 5. Phases

1. **Plumbing, no crypto**: config, SYSINFO bits, MM ciphering-parameter parse/accept,
   per-ISSI state, a `NullKsg` (zero keystream) so the whole path is exercised in tests. Radios
   still in clear (class 1). Unit tests on every PDU encoding change.
2. **KSG + signalling**: real TB5/TEA module, encrypt/decrypt MAC signalling. Test vectors first
   (from the spec annex / algorithm source), then a single MTH800: does it register and keep
   the cell?
3. **Voice**: TCH encryption, individual then group calls.
4. **Multi-cell + handover**, dashboard display, docs, changelog, release.

## 6. Testing

- Unit: known-answer tests for TB5 and TEA*n*; IV construction against hand-computed values;
  round-trip encrypt/decrypt of every MAC PDU type.
- On air: one MTH800 with the test SCK. Failure symptoms to expect: radio registers then loses
  the cell (signalling keystream wrong), or registers but no audio (TCH keystream wrong).
  A monitoring receiver (e.g. osmo-tetra with the same key) makes debugging far faster.

## 7. Open questions

- Which TEA the MTH800/850 are licensed for, and whether their codeplug has class 2 enabled.
- Source of the TB5/TEA implementation (operator decision).

## 8. Phase 1 — as built

- `[security.aie]` (`class`, `ksg`, `sckn`, `sck`, `clear_groups`) parsed and validated in
  `tetra-config` (`sec_security.rs`); the key is a `CipherKey` whose `Debug` is redacted.
  Class 3 is refused at load time.
- `crates/tetra-entities/src/aie/mod.rs` holds the policy: `effective()` (class 2 only when the
  named TEA is built in — none is yet, so a configured key is logged and the cell stays class 1),
  `sysinfo_security()` (unchanged broadcast without AIE), `registration_decision()`,
  `may_communicate()`, `may_use_group()`.
- Ciphering parameters decoded by `tetra_pdus::mm::fields::ciphering_parameters`. KSG number is
  sent as TEA*n* − 1 and SCK number as SCKN − 1 (confirmed, §0).
- MM: decision at U-LOCATION UPDATE DEMAND (reject causes 13–16 for a wrong KSG / key type /
  SCKN); per-radio mode in the client registry and in `SubscriberRegistry::encrypted`; attach
  requests for other-mode groups dropped (registration and U-ATTACH).
- CMCE: individual and group U-SETUP refused with "requested service not available" across
  modes; SDS and status between modes dropped.
- Not yet: the rule across cells of a multi-cell station (a radio on another cell has no local
  mode, so it is not blocked) — phase 4; a radio that changes mode on re-registration keeps its
  other-mode groups until it re-attaches.
- Note for phase 2: an uplink burst says itself whether it is encrypted (MAC `encryption_mode`),
  so UL decryption keys off the MAC header; the registered mode decides DL encryption.
