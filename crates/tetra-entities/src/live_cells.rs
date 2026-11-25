//! Additional cells started and stopped by a live configuration reload.
//!
//! The service binary owns the cell threads and their SDRs, so it installs the handler that
//! starts or stops a cell ([`install`]); the reload ([`crate::config_reload::reload_with`])
//! only says which cells come and go. Without a handler (tests, a station that cannot do it)
//! adding or removing a cell restarts the station.

use std::sync::OnceLock;

use tetra_config::bluestation::{Retirement, SharedConfig};
use tetra_core::CellId;

/// A cell to start or stop.
pub enum CellChange {
    /// Start a new cell with this (validated) config.
    Add { id: CellId, cfg: SharedConfig },
    /// Take a cell off the air: it releases its calls and, per `how`, its registrations, then
    /// stops and closes its SDR.
    Remove { id: CellId, how: Retirement },
}

type Handler = Box<dyn Fn(CellChange) + Send + Sync>;

static HANDLER: OnceLock<Handler> = OnceLock::new();

/// Install the handler that starts and stops cells (once, by the service binary).
pub fn install(handler: Handler) {
    let _ = HANDLER.set(handler);
}

/// True when cells can be added and removed without a restart.
pub fn available() -> bool {
    #[cfg(test)]
    if TEST_HANDLER.with(|h| h.borrow().is_some()) {
        return true;
    }
    HANDLER.get().is_some()
}

/// Start or stop a cell; ignored without a handler.
pub fn apply(change: CellChange) {
    #[cfg(test)]
    {
        let mut change = Some(change);
        TEST_HANDLER.with(|h| {
            if let Some(handler) = h.borrow().as_ref() {
                handler(change.take().expect("one change"));
            }
        });
        let Some(change) = change else { return };
        if let Some(handler) = HANDLER.get() {
            handler(change);
        }
    }
    #[cfg(not(test))]
    if let Some(handler) = HANDLER.get() {
        handler(change);
    }
}

#[cfg(test)]
thread_local! {
    static TEST_HANDLER: std::cell::RefCell<Option<Handler>> = std::cell::RefCell::new(None);
}

/// Tests: a handler for this thread only (`None` removes it).
#[cfg(test)]
pub fn set_test_handler(handler: Option<Handler>) {
    TEST_HANDLER.with(|h| *h.borrow_mut() = handler);
}
