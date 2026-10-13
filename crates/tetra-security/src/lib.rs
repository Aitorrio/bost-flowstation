//! TETRA air-interface security.
//!
//! Implements the algorithms ETSI released to the public domain in 2024:
//!
//! * **TAA1** — authentication and key management (ETSI TS 104 053-3), built on
//!   the HURDLE-II block cipher: `TA11`/`TA12` authenticate a mobile station,
//!   `TA21`/`TA22` authenticate the infrastructure, the rest derive and seal
//!   cipher keys (`TA31`/`TA32` SCK, `TA41` ESI, `TA51`/`TA52` GCK, `TA71` …).
//! * **TEA set A** — key stream generators TEA1, TEA2 and TEA3 (ETSI TS 104 053-1),
//!   used for air-interface encryption (AIE) of signalling and traffic. TEA1 is
//!   included for research and interoperability only: it keeps just 32 bits of
//!   its key (TETRA:BURST, 2023) and must not be relied on for protection.
//!
//! The procedures that use them (challenge/response, security classes, OTAR)
//! are specified in ETSI EN 300 392-7; the state machines live in
//! `tetra-entities`, this crate is pure computation with no I/O.
//!
//! Every algorithm is implemented from the published specification and
//! checked against the specification's worked examples in the unit tests.
//!
//! Note for amateur use: authentication is an access-control handshake and is
//! fine on amateur allocations; air-interface *encryption* is not permitted
//! under amateur licences and must only be enabled on licensed private networks.

#![forbid(unsafe_code)]

pub mod aie;
pub mod auth;
pub mod keys;
pub mod ksg;
pub mod taa1;

pub use keys::{AuthKey, CipherKey, KeyStore, Ssi};
pub use ksg::{Ksg, KsgId};
