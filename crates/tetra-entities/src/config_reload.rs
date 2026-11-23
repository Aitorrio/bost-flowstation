//! Live configuration reload: apply a changed `config.toml` to the running station without a
//! restart. Every cell's [`SharedConfig`] is swapped for the new snapshot; entities that read the
//! config per use see it at once, the ones that cache derived values (UMAC broadcast PDUs and
//! cipher) rebuild when [`SharedConfig::config_generation`] changes.
//!
//! What cannot change at run time is listed by [`restart_reasons`]: the SDR and everything that
//! retunes it, the set of cells, and the
//! network connections opened at start-up (Brew, LST) and the dashboard listener. A change in any of those is not applied at all
//! (all or nothing), so the running station never ends up half on the old file and half on the
//! new one; the caller then restarts as before.

use tetra_config::bluestation::{SharedConfig, StackConfig};
use tetra_core::CellId;

/// Set by the start-up code when the Asterisk entity exists. With it, Asterisk settings (including
/// switching it off and on again) apply live; without it, switching it on needs a restart because
/// there is nothing to switch on.
pub static ASTERISK_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Result of [`reload`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReloadReport {
    /// Cells whose configuration was swapped.
    pub cells: usize,
    /// Settings that need a restart; when non-empty nothing was applied.
    pub restart: Vec<String>,
}

fn differs<T: std::fmt::Debug>(a: &T, b: &T) -> bool {
    format!("{a:?}") != format!("{b:?}")
}

/// Why `new` cannot replace `old` without a restart (empty = it can).
pub fn restart_reasons(old: &StackConfig, new: &StackConfig) -> Vec<String> {
    let mut r = Vec::new();
    let mut check = |changed: bool, what: &str| {
        if changed {
            r.push(what.to_string());
        }
    };
    // The SDR and its settings.
    check(differs(&old.phy_io, &new.phy_io), "SDR / hardware (phy_io)");
    check(differs(&old.stack_mode, &new.stack_mode), "stack_mode");
    // Radio parameters of every cell that retune the SDR or seed the scrambling code.
    let rf = |c: &StackConfig| {
        c.cells()
            .iter()
            .map(|(id, cell, _)| {
                (
                    *id,
                    cell.main_carrier,
                    cell.secondary_carrier,
                    cell.freq_band,
                    cell.freq_offset_hz,
                    cell.duplex_spacing_id,
                    cell.custom_duplex_spacing,
                    cell.reverse_operation,
                )
            })
            .collect::<Vec<_>>()
    };
    check(differs(&rf(old), &rf(new)), "cell carrier or frequency plan (retunes the SDR)");
    check(
        differs(
            &old.extra_cells.iter().map(|c| (c.id, c.soapysdr.clone())).collect::<Vec<_>>(),
            &new.extra_cells.iter().map(|c| (c.id, c.soapysdr.clone())).collect::<Vec<_>>(),
        ),
        "additional cells or their SDRs",
    );
    // Connections and listeners opened once at start-up.
    check(differs(&old.dashboard, &new.dashboard), "dashboard listener (ports, TLS, users)");
    check(differs(&old.debug_log, &new.debug_log), "debug_log");
    check(differs(&old.service_name, &new.service_name), "service_name");
    check(differs(&old.brew, &new.brew) || differs(&old.networks, &new.networks), "Brew / network connections");
    check(differs(&old.lst_dispatch, &new.lst_dispatch), "LST dispatch");
    check(
        new.asterisk.enabled && differs(&old.asterisk, &new.asterisk) && !ASTERISK_RUNNING.load(std::sync::atomic::Ordering::Relaxed),
        "Asterisk bridge (not running: it was off at start-up)",
    );
    r
}

/// Apply `new` (already parsed, validated and given its sibling neighbours) to the primary cell
/// and to every additional cell. Returns the settings that block a live reload without applying
/// anything, or the number of cells swapped.
pub fn reload(primary: &SharedConfig, extra: &[(CellId, SharedConfig)], new: StackConfig) -> Result<ReloadReport, String> {
    let old = primary.config();
    let restart = restart_reasons(&old, &new);
    if !restart.is_empty() {
        return Ok(ReloadReport { cells: 0, restart });
    }
    // Build every cell's config first so a bad one fails before anything is swapped.
    let mut derived = Vec::new();
    for (id, shared) in extra {
        let cell = new.for_extra_cell(*id).ok_or_else(|| format!("cell {id:?} is not in the new configuration"))?;
        cell.validate()?;
        derived.push((shared, cell));
    }
    new.validate()?;
    if differs(&old.security, &new.security) || differs(&old.extra_cells.iter().map(|c| &c.security).collect::<Vec<_>>(), &new.extra_cells.iter().map(|c| &c.security).collect::<Vec<_>>()) {
        tracing::warn!("Security settings changed live: radios that are already registered keep their old keys until they register again");
    }
    primary.replace_config(new)?;
    let cells = 1 + derived.len();
    for (shared, cell) in derived {
        shared.replace_config(cell)?;
    }
    tracing::info!("Configuration reloaded live ({cells} cell(s))");
    Ok(ReloadReport { cells, restart: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_config::bluestation::parsing;

    fn base() -> StackConfig {
        parsing::from_toml_str(include_str!("../../../example_config/config.toml")).expect("example config parses")
    }

    #[test]
    fn identical_config_needs_no_restart() {
        assert!(restart_reasons(&base(), &base()).is_empty());
    }

    #[test]
    fn rf_change_needs_restart_and_is_not_applied() {
        let shared = SharedConfig::from_parts(base(), None);
        let mut new = base();
        new.cell.main_carrier += 1;
        let report = reload(&shared, &[], new).unwrap();
        assert!(!report.restart.is_empty());
        assert_eq!(shared.config_generation(), 0);
    }

    #[test]
    fn colour_code_and_services_apply_live() {
        let mut new = base();
        new.cell.colour_code = (new.cell.colour_code + 1) % 64;
        new.telemetry = None;
        new.control = None;
        new.telegram = None;
        new.health.enabled = !new.health.enabled;
        assert!(restart_reasons(&base(), &new).is_empty(), "{:?}", restart_reasons(&base(), &new));
    }

    #[test]
    fn plain_setting_applies_live_and_bumps_generation() {
        let shared = SharedConfig::from_parts(base(), None);
        let clone = shared.clone();
        let mut new = base();
        new.cell.location_area = new.cell.location_area.wrapping_add(1) & 0x3FFF;
        let la = new.cell.location_area;
        let report = reload(&shared, &[], new).unwrap();
        assert_eq!(report, ReloadReport { cells: 1, restart: Vec::new() });
        assert_eq!(clone.config().cell.location_area, la, "every clone sees the new snapshot");
        assert_eq!(clone.config_generation(), 1);
    }
}
