//! Process-wide RF / PHY status for the dashboard and setup wizard.
//!
//! The stack can run with the dashboard up even when no SDR is open (setup mode or
//! open failure). Consumers read this status without needing a PHY entity.

use std::sync::RwLock;
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

/// Set by the OTA thread; the PHY polls this and drops the SDR so the cell leaves the air
/// before a long `cargo build` (avoids ghost registrations when RF dies mid-compile).
static OTA_RF_OFF: AtomicBool = AtomicBool::new(false);

fn write_status(state: RfState, detail: impl Into<String>, backend: impl Into<String>) {
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

pub fn get() -> RfStatus {
    STATUS.read().map(|g| g.clone()).unwrap_or_default()
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
