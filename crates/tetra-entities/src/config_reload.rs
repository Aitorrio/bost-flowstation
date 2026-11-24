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

/// Set by the start-up code when the LST dispatch entity exists. Like [`ASTERISK_RUNNING`]: with it
/// the console's settings apply live, without it switching it on needs a restart.
pub static LST_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    // The dashboard's login, public overview and private-HTTP allowlist follow live; the sockets
    // it listens on do not (a wrong port would cut off the only management channel).
    let listener = |c: &StackConfig| c.dashboard.as_ref().map(|d| (d.port, d.https_port, d.bind.clone(), d.source_dir.clone()));
    check(differs(&listener(old), &listener(new)), "dashboard listener (bind address, ports, source_dir)");
    // Brew settings follow live. The network layout is live on a station of one cell: networks no
    // cell serves are only configuration, and the primary's core link is attached or dropped
    // through the router. With several cells the links between them (site switch, cell links) are
    // built once at start-up, so any change of layout restarts.
    let lst_on = |c: &StackConfig| c.lst_dispatch.as_ref().is_some_and(|l| l.enabled);
    let single = old.extra_cells.is_empty() && new.extra_cells.is_empty();
    let link_change = old.brew.is_some() != new.brew.is_some();
    let layout = |c: &StackConfig| (c.networks_explicit, c.network.clone(), c.networks.iter().map(|n| (n.name.clone(), n.brew.is_some(), n.via.clone())).collect::<Vec<_>>(), c.extra_cells.iter().map(|x| (x.id, x.network.clone())).collect::<Vec<_>>());
    if single {
        check(old.network != new.network, "the network the cell serves");
        check(link_change && (lst_on(old) || lst_on(new)), "core link and LST dispatch exchange the Brew slot");
        check(link_change && !crate::network_swap::available(), "core link gained or lost (not available on this station)");
    } else {
        check(differs(&layout(old), &layout(new)), "networks: added, removed, or a network gaining or losing its Brew link (several cells)");
    }
    check(
        new.lst_dispatch.as_ref().is_some_and(|l| l.enabled)
            && differs(&old.lst_dispatch, &new.lst_dispatch)
            && !LST_RUNNING.load(std::sync::atomic::Ordering::Relaxed),
        "LST dispatch (not running: it was off at start-up)",
    );
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
    let single_cell = old.extra_cells.is_empty() && new.extra_cells.is_empty();
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
    let (log_changed, name_changed) = (differs(&old.debug_log, &new.debug_log), differs(&old.service_name, &new.service_name));
    let (log_file, service_name) = (new.debug_log.clone(), new.service_name.clone());
    let dashboard = new.dashboard.clone();
    let link = (old.brew.is_some(), new.brew.is_some());
    primary.replace_config(new)?;
    match link {
        (false, true) if single_cell => {
            tracing::info!("Core link gained: starting the Brew entity");
            crate::network_swap::attach(primary);
        }
        (true, false) if single_cell => {
            tracing::info!("Core link removed: stopping the Brew entity");
            crate::network_swap::detach(primary);
            crate::health::registry().set_brew_configured(false);
        }
        _ => {}
    }
    if link == (false, true) {
        crate::health::registry().set_brew_configured(true);
    }
    crate::net_dashboard::server::apply_live_credentials(dashboard.as_ref());
    if log_changed && let Err(e) = tetra_core::debug::set_log_file(log_file.as_deref()) {
        tracing::warn!("debug_log: {e}; keeping the previous log file");
    }
    if name_changed {
        match service_name.as_deref() {
            Some(name) => crate::service_control::set_configured_service_unit(name),
            None => crate::service_control::clear_configured_service_unit(),
        }
    }
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
    fn log_file_service_name_and_dashboard_login_apply_live() {
        let mut new = base();
        new.debug_log = Some("/tmp/flowstation-test.log".into());
        new.service_name = Some("tetra".into());
        if let Some(d) = new.dashboard.as_mut() {
            d.username = Some("op".into());
            d.password = Some("secret".into());
            d.public_overview = !d.public_overview;
        }
        assert!(restart_reasons(&base(), &new).is_empty(), "{:?}", restart_reasons(&base(), &new));
    }

    #[test]
    fn dashboard_ports_and_a_brew_link_appearing_still_need_a_restart() {
        let mut ports = base();
        if let Some(d) = ports.dashboard.as_mut() {
            d.https_port += 1;
            assert!(!restart_reasons(&base(), &ports).is_empty());
        }
        let mut brew = base();
        if brew.brew.is_some() {
            brew.brew = None;
            assert!(!restart_reasons(&base(), &brew).is_empty(), "removing the core link changes which entities exist");
        }
    }

    #[test]
    fn a_network_no_cell_serves_is_only_configuration_on_one_cell() {
        let mut new = base();
        new.networks.push(tetra_config::bluestation::CfgNetwork {
            name: "spare".into(),
            net: new.net.clone(),
            brew: None,
            via: None,
        });
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
