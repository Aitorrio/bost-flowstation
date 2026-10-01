# Air interface encryption — security class 2 (SCK)

Status: **design draft**, nothing implemented. Target radios: Motorola MTH800 / MTH850.

Normative reference: ETSI EN 300 392-7 (TETRA V+D security), clause 6 (air interface
encryption), with the PDU encodings in EN 300 392-2 (clause 16 MM, clause 21 MAC). Every item
marked **[verify]** below is from memory or from the existing parsers and must be checked against
the spec text before it is coded — a single wrong bit means the radio silently fails to decode.

## 1. What class 2 is

- One **static cipher key (SCK)**, 80 bits, shared by the infrastructure and every radio. Up to 32
  SCKs are numbered by **SCKN** (1–32, sent as 5 bits); the network says which one is in use.
- No authentication. Anyone with the SCK can join; class 2 protects against passive listening only.
- Signalling and traffic (voice) on the air are encrypted; the backhaul (site switch, Brew,
  Asterisk, LST) stays in clear, because each cell decrypts at its own MAC.

## 2. Crypto chain (EN 300 392-7 clause 6.2) [verify]

1. **ECK = TB5(SCK, CC, CN, LA)** — the encryption key is modified per cell by colour code, carrier
   number and location area, so every cell of a multi-cell station gets its own ECK from the same
   SCK. Computed once per cell at start-up.
2. **IV (29 bits)** from TDMA time: timeslot − 1 (2), frame number (5), multiframe number (6),
   low 15 bits of the hyperframe number, direction bit (DL/UL). The hyperframe count must
   therefore be exact on both ends (see §4).
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
  SCKN) the radio proposes. [verify field layout]
- D-LOCATION-UPDATE-ACCEPT: return the agreed ciphering parameters in the security element.
  The registration exchange itself goes in clear; encryption starts after accept. [verify the
  exact switch-on point, EN 300 392-7 clause 6.5]
- Per-ISSI state in the client registry: `encrypted: bool`, KSG, SCKN.
- Reject (or fall back to clear, per `allow_clear`) a radio proposing another SCKN or KSG.

### 4a. Mixed-cell policy (decided)

Clear radios may register, but they talk **only with other clear radios**; encrypted radios
talk only with encrypted radios. The station never bridges between the two, so nothing an
encrypted radio says is ever sent in clear.

- **Individual calls**: refused when caller and called differ in mode (D-RELEASE, cause
  "requested service not available" [verify cause code]).
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
  AIE is on; clear for broadcast-to-all control PDUs that must stay readable (sync, sysinfo,
  D-NWRK-BROADCAST — confirm which are exempt). [verify]

### UMAC (signalling)
- MAC-RESOURCE / MAC-DATA / MAC-FRAG / MAC-END: set `encryption_mode`, encrypt the TM-SDU
  (header and, per the spec, possibly the address via ESI) with the slot's keystream, before
  channel coding. Fragmented PDUs: every fragment uses its own slot's IV. [verify what is
  covered — header vs SDU, and whether class 2 uses ESI for addresses]
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
