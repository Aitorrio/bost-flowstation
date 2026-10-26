# End-to-end encryption (E2EE) for SDS — setup guide

FlowStation can end-to-end encrypt SDS payloads: the message is protected from
the sending party to the receiving party, so the base station and any bridge
never see cleartext. This is **SFPG Recommendation 02-aligned, AES-based**, and
is distinct from the air-interface encryption (AIE: TEA/OTAR) — AIE protects the
radio link, E2EE protects the payload end to end.

## Legality — read this first

Like AIE, E2EE encrypts traffic to make it unintelligible to listeners. That is
**not permitted on amateur radio allocations** (UK and most jurisdictions). It is
for **licensed private / commercial networks** only. The feature ships **disabled
by default**; enabling it is the operator's decision and the operator is
responsible for having a lawful basis to transmit encrypted traffic.

## What it does today

- Confidentiality: **AES-CTR** (AES-128 or AES-256).
- Integrity: **AES-CMAC** (encrypt-then-MAC), so a wrong key or a tampered
  message is rejected rather than silently decrypted to garbage.
- Keyed by a **TEK** (Traffic Encryption Key) identified by a 16-bit id carried
  in each message, so multiple keys can coexist and be rotated.

The crypto is implemented from the public standards (FIPS-197, NIST SP 800-38A,
RFC 4493) with no external crypto libraries, and pinned by the standards'
known-answer vectors.

## Interoperability

The exact SFPG Rec 02 on-air framing is a members-only document, so this
implementation uses a FlowStation-internal wire format. It interoperates
**between FlowStation-aware endpoints**; it is **not guaranteed byte-compatible
with a COTS E2EE radio** (Motorola/Sepura E2EE modules) without the Rec 02 spec
and captured vectors.

## config.toml

Disabled by default. To enable, add a `[security.e2ee]` section with one or more
TEKs:

```toml
[security.e2ee]
enabled = true

[[security.e2ee.teks]]
id = 1                      # 16-bit key id, carried in each protected message
algorithm = "aes256"        # "aes128" or "aes256" (default: aes256)
key = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
```

### Rules the loader enforces
- `algorithm` must be `aes128` or `aes256`; the `key` must be the matching
  length (32 hex digits for AES-128, 64 for AES-256).
- TEK `id`s must be unique.
- `enabled = true` needs at least one TEK.
- Key material is redacted from all logs and debug output.

## Key management

TEKs can be provisioned inline as above, or generated and versioned by the TETRA
KMF (SFPG Rec 02 support is planned there, alongside the existing Rec 01 AIE key
interchange) and distributed by OTAR.

## Current scope and the remaining step

This release provides the crypto, the config, and the entity-side service
(building the key store and sealing/opening payloads). **Wiring it into the live
SDS send/receive dispatch** — inbound auto-decrypt of protected messages and an
outbound per-peer policy (which radios/groups get E2EE, mirroring the AIE
`clear_groups` approach so clear radios are never sent ciphertext) — is the
remaining integration step and is intentionally conservative: auto-encrypting
everything would break radios without the key.

See also: [install-and-setup.md](install-and-setup.md).

---

*End-to-end encryption (SDS) by **David Harcus, MM7FDM**.*
