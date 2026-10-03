//! Process-wide RF / PHY status for the dashboard and setup wizard.
//!
//! The stack can run with the dashboard up even when no SDR is open (setup mode or
//! open failure). Consumers read this status without needing a PHY entity.

use std::collections::BTreeMap;
use std::sync::{Mutex, RwLock};

use tetra_core::CellId;
use std::sync::atomic::{AtomicBool, Ordering};

/// High-level RF availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RfState {
    /// SDR open and PHY registered.
    Online,
    /// Intentionally disabled (`phy_io.backend = None`).
    Offline,
    /// Wanted SoapySDR but open failed (or unsupported backend).
    Error,
    /// Still trying to open the radio.
    Starting,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RfStatus {
    pub state: RfState,
    pub detail: String,
    pub backend: String,
}

impl Default for RfStatus {
    fn default() -> Self {
        Self {
            state: RfState::Starting,
            detail: "RF not initialised yet".into(),
            backend: "unknown".into(),
        }
    }
}

static STATUS: RwLock<RfStatus> = RwLock::new(RfStatus {
    state: RfState::Starting,
    detail: String::new(),
    backend: String::new(),
});

/// Status of the additional cells (multi-cell). The primary cell keeps using `STATUS`.
static EXTRA_CELLS: Mutex<BTreeMap<CellId, RfStatus>> = Mutex::new(BTreeMap::new());

/// Set by the OTA thread; the PHY polls this and drops the SDR so the cell leaves the air
/// before a long `cargo build` (avoids ghost registrations when RF dies mid-compile).
static OTA_RF_OFF: AtomicBool = AtomicBool::new(false);

/// Writes go to the status of the cell the calling thread runs (see [`crate::cell_context`]).
fn write_status(state: RfState, detail: impl Into<String>, backend: impl Into<String>) {
    let cell = crate::cell_context::current();
    if !cell.is_primary() {
        if let Ok(mut g) = EXTRA_CELLS.lock() {
            g.insert(
                cell,
                RfStatus {
                    state,
                    detail: detail.into(),
                    backend: backend.into(),
                },
            );
        }
        return;
    }
    if let Ok(mut g) = STATUS.write() {
        g.state = state;
        g.detail = detail.into();
        g.backend = backend.into();
    }
}

pub fn set_starting(backend: &str) {
    write_status(RfState::Starting, "Opening SDR…", backend);
}

pub fn set_online(backend: &str, detail: impl Into<String>) {
    write_status(RfState::Online, detail, backend);
}

pub fn set_offline(detail: impl Into<String>) {
    write_status(RfState::Offline, detail, "None");
}

pub fn set_error(backend: &str, detail: impl Into<String>) {
    write_status(RfState::Error, detail, backend);
}

/// Status of the primary cell.
pub fn get() -> RfStatus {
    STATUS.read().map(|g| g.clone()).unwrap_or_default()
}

/// Status of every cell, primary first. Additional cells appear once their thread reports.
pub fn get_all() -> Vec<(CellId, RfStatus)> {
    let mut v = vec![(CellId::PRIMARY, get())];
    if let Ok(g) = EXTRA_CELLS.lock() {
        v.extend(g.iter().map(|(id, s)| (*id, s.clone())));
    }
    v
}

/// Ask the running PHY to power off the SDR for an in-process OTA build.
/// Cleared only by process exit (the service restarts after install).
pub fn request_ota_rf_off() {
    OTA_RF_OFF.store(true, Ordering::SeqCst);
}

#[inline]
pub fn ota_rf_off_requested() -> bool {
    OTA_RF_OFF.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_cell_status_does_not_touch_primary() {
        let before = get();
        std::thread::spawn(|| {
            crate::cell_context::set_current(CellId(5));
            set_error("SoapySdr", "test: no device");
        })
        .join()
        .unwrap();

        let primary = get();
        assert_eq!(primary.state, before.state);
        assert_eq!(primary.detail, before.detail);
        let (_, cell5) = get_all().into_iter().find(|(id, _)| *id == CellId(5)).unwrap();
        assert_eq!(cell5.state, RfState::Error);
        assert_eq!(cell5.detail, "test: no device");
    }
}
