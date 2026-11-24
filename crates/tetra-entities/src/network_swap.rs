//! Gaining or losing a core link at run time, for a station of one cell.
//!
//! The core link (Brew) is the entity in the router's Brew slot. On a single-cell station nothing
//! else depends on it, so a live reload can build it, or drop it, between two ticks through the
//! router's mailbox. With several cells the link sits inside the site switch and the cell links
//! that join the cells, which are built once at start-up, so those stations restart instead.

use std::sync::OnceLock;

use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;

use crate::{RouterCommand, RouterMailbox, TetraEntityTrait};

type Build = Box<dyn Fn(&SharedConfig) -> Box<dyn TetraEntityTrait> + Send + Sync>;

struct Swap {
    mailbox: RouterMailbox,
    build: Build,
}

static SWAP: OnceLock<Swap> = OnceLock::new();

/// Set by the start-up code of a single-cell station: the primary router's mailbox and how to
/// build the Brew-slot entity for the primary's config.
pub fn register(mailbox: RouterMailbox, build: Build) {
    let _ = SWAP.set(Swap { mailbox, build });
}

/// Whether the Brew slot can be filled or emptied at run time.
pub fn available() -> bool {
    SWAP.get().is_some()
}

/// Put the core link in the Brew slot (`cfg` already has the new `[brew]`).
pub fn attach(cfg: &SharedConfig) {
    if let Some(swap) = SWAP.get() {
        swap.mailbox.send(RouterCommand::Register((swap.build)(cfg)));
    }
}

/// Take the core link out of the Brew slot. The entity's worker stops when it is dropped.
pub fn detach(cfg: &SharedConfig) {
    if let Some(swap) = SWAP.get() {
        swap.mailbox.send(RouterCommand::Remove(TetraEntity::Brew));
        cfg.state_write().network_connected = false;
    }
}
