//! Wall-clock synchronisation self-check.
//!
//! Every FlowStation timestamp that leaves the host — log lines, roaming/handover records on the
//! brew-server, telemetry — is taken from the system wall clock. Correlating events across cells
//! and the brew-server is only meaningful if those clocks agree, which is the NTP daemon's job
//! (chrony / ntpd / systemd-timesyncd). This module asks the kernel, once per health tick, whether
//! the clock is actually disciplined and how far off it estimates it is, and logs a warning when it
//! is not — so an operator reading logs after an incident knows whether the timestamps can be
//! trusted. Observe-only: a privileged-free `adjtimex` read, no clock is ever set.
//!
//! FlowStation-original work.

use std::time::{Duration, Instant};

/// A single reading of the kernel clock-discipline state.
pub struct ClockSample {
    /// The kernel considers the clock NTP-synchronised (not `STA_UNSYNC`, no `TIME_ERROR`).
    pub synced: bool,
    /// Current NTP offset estimate, milliseconds.
    pub offset_ms: f64,
    /// Estimated error bound, milliseconds.
    pub esterror_ms: f64,
}

/// Read the kernel clock state via `adjtimex(2)` with an all-zero (read-only) request, so no
/// capability is needed and the clock is never modified. `None` if the read fails or the platform
/// has no such interface — in which case we simply say nothing rather than warn on a guess.
#[cfg(target_os = "linux")]
pub fn sample() -> Option<ClockSample> {
    // SAFETY: a zeroed `timex` with `modes == 0` is a well-defined read-only adjtimex request; the
    // struct is caller-owned and fully initialised by the kernel on success.
    let mut tx: libc::timex = unsafe { std::mem::zeroed() };
    tx.modes = 0;
    let state = unsafe { libc::adjtimex(&mut tx) };
    if state == -1 {
        return None;
    }
    // With STA_NANO the offset is nanoseconds, otherwise microseconds; esterror is always µs.
    let offset_ms = if tx.status & libc::STA_NANO != 0 {
        tx.offset as f64 / 1_000_000.0
    } else {
        tx.offset as f64 / 1_000.0
    };
    let esterror_ms = tx.esterror as f64 / 1_000.0;
    let synced = state != libc::TIME_ERROR && tx.status & libc::STA_UNSYNC == 0;
    Some(ClockSample { synced, offset_ms, esterror_ms })
}

#[cfg(not(target_os = "linux"))]
pub fn sample() -> Option<ClockSample> {
    None
}

/// Offset past which the clock is flagged even while the kernel still calls it synchronised.
const OFFSET_WARN_MS: f64 = 50.0;
/// While the clock stays bad, re-warn at most this often (the state is also logged on every flip).
const REWARN_AFTER: Duration = Duration::from_secs(300);

/// Rate-limiting state for [`check_and_log`]; one per health-monitor thread.
#[derive(Default)]
pub struct WarnState {
    /// `Some(true)` healthy, `Some(false)` bad, `None` before the first sample.
    last_good: Option<bool>,
    last_warn: Option<Instant>,
}

/// Sample the clock and log a warning when it is unsynchronised or the offset is large. Logs the
/// state on every healthy⇄bad transition (so boot prints one status line), and while bad re-warns
/// no more than every [`REWARN_AFTER`]. Does nothing on platforms without `adjtimex`.
pub fn check_and_log(state: &mut WarnState) {
    let Some(s) = sample() else { return };
    let good = s.synced && s.offset_ms.abs() <= OFFSET_WARN_MS;
    let flipped = state.last_good != Some(good);
    let now = Instant::now();

    if !good {
        let due = state.last_warn.is_none_or(|t| now.duration_since(t) >= REWARN_AFTER);
        if flipped || due {
            if !s.synced {
                tracing::warn!(
                    "CLOCK: system clock not NTP-synchronised (STA_UNSYNC); offset≈{:.1} ms, esterror≈{:.1} ms — timestamps across cells and the brew-server may not line up",
                    s.offset_ms,
                    s.esterror_ms
                );
            } else {
                tracing::warn!(
                    "CLOCK: NTP offset {:.1} ms exceeds {:.0} ms (esterror≈{:.1} ms) — check chrony/ntp on this host",
                    s.offset_ms,
                    OFFSET_WARN_MS,
                    s.esterror_ms
                );
            }
            state.last_warn = Some(now);
        }
    } else if flipped {
        tracing::info!(
            "CLOCK: system clock NTP-synchronised (offset {:.1} ms, esterror≈{:.1} ms)",
            s.offset_ms,
            s.esterror_ms
        );
        state.last_warn = None;
    }

    state.last_good = Some(good);
}
