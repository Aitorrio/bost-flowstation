//! End-to-end encryption (E2EE) for TETRA payloads — SFPG Recommendation 02
//! aligned, AES-based.
//!
//! This protects the *payload* from the sending radio to the receiving radio;
//! the base station and any bridge never see cleartext. It is distinct from
//! air-interface encryption (AIE: TEA/OTAR in [`crate::ksg`]/[`crate::taa1`]).
//!
//! DISABLED BY DEFAULT. Like AIE, E2EE must only be enabled on networks where
//! encryption is lawful (licensed private/commercial systems) — never on
//! amateur allocations, where obscuring encryption is not permitted. The
//! operator who enables it owns that decision.
//!
//! No external crypto crates and no `unsafe`: AES is implemented from FIPS-197
//! in [`aes`], AES-CMAC from RFC 4493 in [`cmac`], and the SDS payload format
//! lives in [`sds`]. Correctness is pinned by the standards' known-answer tests.

pub mod aes;
pub mod cmac;
pub mod sds;

pub use sds::{E2eeAlg, E2eeError, Tek, TekStore, is_protected, open_sds, seal_sds};
