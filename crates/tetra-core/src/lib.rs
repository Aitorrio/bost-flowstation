//! Core utilities for TETRA BlueStation
//!
//! This crate provides fundamental types and utilities used across the TETRA stack

/// Short git commit hash, set at compile time (e.g. "2aad62c8"). No `g` prefix: the empty `--match=`
/// makes `git describe --always` emit the bare abbreviated commit hash, not a tag-relative name.
pub const GIT_HASH: &str = git_version::git_version!(
    args = ["--always", "--dirty=-modified", "--match=", "--abbrev=8"],
    fallback = "unknown"
);

/// Product branding for this fork (UI / banners).
pub const PRODUCT_NAME: &str = "Bost FlowStation";

/// Our release line (independent of upstream crate package version).
pub const BOST_VERSION: &str = "0.1.36";

/// Upstream project this fork is based on.
pub const UPSTREAM_NAME: &str = "FlowStation";

/// Upstream FlowStation version this Bost release tracks (workspace package version).
pub const UPSTREAM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Numeric product version shown in the UI (no git hash), e.g. "v0.1.1".
pub const PRODUCT_VERSION: &str = const_format::formatcp!("v{}", BOST_VERSION);

/// Full build identity for OTA / diagnostics, e.g. "v0.1.1-2aad62c8".
pub const STACK_VERSION: &str = const_format::formatcp!("v{}-{}", BOST_VERSION, GIT_HASH);

/// Provenance line, e.g. "based on FlowStation v0.4.0".
pub const VERSION_BASED_ON: &str =
    const_format::formatcp!("based on {} v{}", UPSTREAM_NAME, UPSTREAM_VERSION);

/// Public source repository for this fork.
pub const PRODUCT_REPO_URL: &str = "https://github.com/Aitorrio/bost-flowstation";
pub const PRODUCT_REPO_LABEL: &str = "github.com/Aitorrio/bost-flowstation";

/// Git clone URL used by OTA (`git remote set-url origin …`).
pub const PRODUCT_REPO_GIT: &str = "https://github.com/Aitorrio/bost-flowstation.git";

/// Branch OTA fetches / fast-forwards from.
pub const PRODUCT_OTA_BRANCH: &str = "bost";

pub mod address;
pub mod bitbuffer;
pub mod debug;
pub mod direction;
pub mod freqs;
pub mod pdu_parse_error;
pub mod phy_types;
pub mod ranges;
pub mod sap_fields;
pub mod tdma_time;
pub mod tetra_common;
pub mod tetra_entities;
pub mod timeslot_alloc;
pub mod tx_receipt;
pub mod typed_pdu_fields;

// Re-export commonly used items
pub use address::*;
pub use bitbuffer::BitBuffer;
pub use direction::Direction;
pub use pdu_parse_error::PduParseErr;
pub use phy_types::*;
pub use sap_fields::*;
pub use tdma_time::TdmaTime;
pub use tetra_common::*;
pub use timeslot_alloc::*;
pub use tx_receipt::*;
