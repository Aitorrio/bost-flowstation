//! Which cell the current thread belongs to.
//!
//! In a multi-cell station every cell's stack runs on its own thread. Process-wide state that
//! still describes a single cell (health registry gauges, RF status, detected SDR name) is kept
//! for the primary cell only; code writing it checks [`is_primary`] or keys by [`current`].

use std::cell::Cell;
use tetra_core::CellId;

thread_local! {
    static CURRENT: Cell<CellId> = const { Cell::new(CellId::PRIMARY) };
}

/// Mark the calling thread as running `id`'s stack. Threads default to the primary cell.
pub fn set_current(id: CellId) {
    CURRENT.with(|c| c.set(id));
}

pub fn current() -> CellId {
    CURRENT.with(|c| c.get())
}

pub fn is_primary() -> bool {
    current().is_primary()
}
