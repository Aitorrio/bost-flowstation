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

use tetra_config::bluestation::{debug_with_secrets, AirChange, Retirement, SharedConfig, StackConfig};
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

/// Compares the full values, secrets included (a changed password or key is a change).
fn differs<T: std::fmt::Debug>(a: &T, b: &T) -> bool {
    debug_with_secrets(a) != debug_with_secrets(b)
}

/// The SDR settings that cannot change live: everything but the gains, the antennas (while
/// set) and the ppm correction; on an SDR of its own (`own`) also the frequencies, which follow
/// the cell's carrier.
fn soapy_fixed_for(s: &tetra_config::bluestation::sec_phy_soapy::CfgSoapySdr, own: bool) -> String {
    let mut s = s.clone();
    if own {
        s.ul_freq = 0.0;
        s.dl_freq = 0.0;
        s.rx_center_freq = None;
        s.tx_center_freq = None;
    }
    s.rx_gains.clear();
    s.tx_gains.clear();
    s.rx_ant = s.rx_ant.as_ref().map(|_| String::new());
    s.tx_ant = s.tx_ant.as_ref().map(|_| String::new());
    s.ppm_err = 0.0;
    debug_with_secrets(&s)
}

/// A gain that is no longer set would go back to a device default the station does not know.
fn gain_removed(old: Option<&tetra_config::bluestation::sec_phy_soapy::CfgSoapySdr>, new: Option<&tetra_config::bluestation::sec_phy_soapy::CfgSoapySdr>) -> bool {
    match (old, new) {
        (Some(o), Some(n)) => o.rx_gains.keys().any(|k| !n.rx_gains.contains_key(k)) || o.tx_gains.keys().any(|k| !n.tx_gains.contains_key(k)),
        _ => false,
    }
}

fn soapy_fixed(s: &tetra_config::bluestation::sec_phy_soapy::CfgSoapySdr) -> String {
    soapy_fixed_for(s, false)
}

fn sdr_fixed(p: &tetra_config::bluestation::CfgPhyIo, own: bool) -> String {
    let mut p = p.clone();
    let soapy = p.soapysdr.take();
    format!("{} {:?}", debug_with_secrets(&p), soapy.as_ref().map(|s| soapy_fixed_for(s, own)))
}

/// Cells that share their SDR with another cell (same `device`), as `open_shared_sdrs` groups
/// them at start-up.
fn shared_sdr_cells(c: &StackConfig) -> std::collections::HashSet<CellId> {
    let device = |s: Option<&tetra_config::bluestation::sec_phy_soapy::CfgSoapySdr>| {
        s.and_then(|s| s.device.as_deref()).map(|d| d.trim().to_string()).filter(|d| !d.is_empty())
    };
    let cells: Vec<(CellId, Option<String>)> = std::iter::once((CellId::PRIMARY, device(c.phy_io.soapysdr.as_ref())))
        .chain(c.extra_cells.iter().map(|x| (x.id, device(x.soapysdr.as_ref()))))
        .collect();
    cells
        .iter()
        .filter(|(id, d)| d.is_some() && cells.iter().any(|(o, od)| o != id && od == d))
        .map(|(id, _)| *id)
        .collect()
}

/// Additional cells `new` adds and removes compared with `old`.
fn cell_set_change(old: &StackConfig, new: &StackConfig) -> (Vec<CellId>, Vec<CellId>) {
    let ids = |c: &StackConfig| c.extra_cells.iter().map(|x| x.id).collect::<Vec<_>>();
    let (o, n) = (ids(old), ids(new));
    (n.iter().filter(|id| !o.contains(id)).copied().collect(), o.iter().filter(|id| !n.contains(id)).copied().collect())
}

/// Why the cells `new` adds or removes cannot start or stop live (`None` = they can). A cell on
/// an SDR of its own, serving the primary's network, starts and stops in its own thread and
/// joins or leaves the running site switch; anything that changes how the cells are wired
/// together restarts.
fn cell_set_refusal(old: &StackConfig, new: &StackConfig, own: &dyn Fn(CellId) -> bool) -> Option<&'static str> {
    let (added, removed) = cell_set_change(old, new);
    if added.is_empty() && removed.is_empty() {
        return None;
    }
    if !crate::live_cells::available() {
        return Some("additional cells added or removed (not available on this station)");
    }
    if added.iter().chain(&removed).any(|id| !own(*id)) {
        return Some("a cell on a shared SDR added or removed");
    }
    if added.iter().any(|id| new.network_of_cell(*id).name != new.network) || removed.iter().any(|id| old.network_of_cell(*id).name != old.network) {
        return Some("a cell of another network than the primary's added or removed");
    }
    if old.network_is_linked(&old.network) != new.network_is_linked(&new.network) {
        return Some("a cell added or removed so that the cells start or stop sharing the network link");
    }
    if !added.is_empty() && new.asterisk.enabled && new.network_is_linked(&new.network) {
        return Some("a cell added while the Asterisk bridge serves several cells");
    }
    if old.brew.is_some() != new.brew.is_some() {
        return Some("a cell added or removed together with the core link");
    }
    None
}

/// Why `new` cannot replace `old` without a restart (empty = it can).
pub fn restart_reasons(old: &StackConfig, new: &StackConfig) -> Vec<String> {
    let mut r = Vec::new();
    let mut check = |changed: bool, what: &str| {
        if changed {
            r.push(what.to_string());
        }
    };
    // The SDR and its settings. Gains, antennas and the ppm correction follow live on the
    // running SDR (`RxTxDevSoapySdr::follow_config`); a gain or antenna that is removed would go
    // back to a device default the station does not know, so that restarts.
    // A cell on an SDR of its own may also move to another carrier (the SDR retunes, the cell
    // rebuilds as for a new colour code); cells sharing an SDR keep theirs.
    let shared = {
        let mut s = shared_sdr_cells(old);
        s.extend(shared_sdr_cells(new));
        s
    };
    let own = |id: CellId| !shared.contains(&id);
    // Cells in both configs; cells added or removed are judged by `cell_set_refusal`.
    let common = |id: CellId| id.is_primary() || (old.extra_cells.iter().any(|x| x.id == id) && new.extra_cells.iter().any(|x| x.id == id));
    if let Some(why) = cell_set_refusal(old, new, &own) {
        check(true, why);
    }
    check(differs(&sdr_fixed(&old.phy_io, own(CellId::PRIMARY)), &sdr_fixed(&new.phy_io, own(CellId::PRIMARY))), "SDR / hardware (phy_io)");
    let removed = gain_removed(old.phy_io.soapysdr.as_ref(), new.phy_io.soapysdr.as_ref())
        || old.extra_cells.iter().any(|o| gain_removed(o.soapysdr.as_ref(), new.extra_cells.iter().find(|n| n.id == o.id).and_then(|n| n.soapysdr.as_ref())));
    check(removed, "an SDR gain removed (back to the device default)");
    check(differs(&old.stack_mode, &new.stack_mode), "stack_mode");
    // Radio parameters of every cell that retune the SDR or seed the scrambling code.
    let rf = |c: &StackConfig| {
        c.cells()
            .iter()
            .filter(|(id, _, _)| common(*id))
            .map(|(id, cell, _)| {
                if own(*id) {
                    // Retuned live: only whether there is a second carrier must stay.
                    return format!("{id:?} own SDR, secondary {}", cell.secondary_carrier.is_some());
                }
                format!(
                    "{:?}",
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
                )
            })
            .collect::<Vec<_>>()
    };
    check(differs(&rf(old), &rf(new)), "carrier or frequency plan of a cell on a shared SDR, or a second carrier added or removed");
    // The cell's identity on air (colour code, MCC/MNC) follows live: every radio loses the
    // cell and registers anew (TS 100 392-2 clauses 8.2.5.2, 16.4.1.0), so each cell rebuilds its
    // registrations and calls in place (MM `reset_registrations`, CMCE `follow_cell_identity`)
    // while the other cells and the station keep running.
    // Air interface encryption. A change of the security class or of the cell's cipher key is
    // announced to the registered radios with D-CK CHANGE DEMAND and takes effect on one slot,
    // with SYSINFO changing on the same slot (EN 300 392-7 clauses 4.5.5.6 and 4.5.6; see
    // `reload_with` and `AirChange`). The key stream generator of a cell that stays encrypted
    // is not part of that procedure and restarts.
    // Who may register (whitelist, limits) and the subscriber keys K stay live: K is only used at
    // the next authentication (clause 4.4).
    let ksg = |c: &StackConfig| {
        c.cells()
            .iter()
            .filter(|(id, _, _)| common(*id))
            .map(|(id, _, _)| {
                let sec = c.extra_cells.iter().find(|x| x.id == *id).and_then(|x| x.security.as_ref()).unwrap_or(&c.security);
                (*id, crate::aie::running(sec.aie.as_ref()).map(|a| a.ksg))
            })
            .collect::<Vec<_>>()
    };
    let (old_ksg, new_ksg) = (ksg(old), ksg(new));
    let ksg_changed = old_ksg.iter().any(|(id, o)| new_ksg.iter().any(|(n_id, n)| n_id == id && o.is_some() && n.is_some() && o != n));
    check(ksg_changed, "air interface encryption: key stream generator (TEA) of an encrypted cell");
    check(
        differs(
            &old.extra_cells.iter().filter(|c| common(c.id)).map(|c| (c.id, c.soapysdr.as_ref().map(|s| soapy_fixed_for(s, own(c.id))))).collect::<Vec<_>>(),
            &new.extra_cells.iter().filter(|c| common(c.id)).map(|c| (c.id, c.soapysdr.as_ref().map(|s| soapy_fixed_for(s, own(c.id))))).collect::<Vec<_>>(),
        ),
        "the SDR of an additional cell",
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
    let layout = |c: &StackConfig| (c.networks_explicit, c.network.clone(), c.networks.iter().map(|n| (n.name.clone(), n.brew.is_some(), n.via.clone())).collect::<Vec<_>>(), c.extra_cells.iter().filter(|x| common(x.id)).map(|x| (x.id, x.network.clone())).collect::<Vec<_>>());
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

/// What a reload does with a cell's air interface encryption.
enum AirHold {
    /// Nothing to announce: `[security.aie]` swaps with the rest.
    None,
    /// Announce this change; the cell keeps the encryption in use until it takes effect.
    Set(AirChange),
    /// The same change is already announced: keep it.
    Keep,
    /// A change not yet announced is withdrawn (the file is back to what runs).
    Cancel,
}

/// A change of the security class or of the cell's cipher key cannot simply be swapped in: the
/// registered radios are told first with D-CK CHANGE DEMAND, and the change takes effect on the
/// slot named there, SYSINFO changing on the same slot (EN 300 392-7 clauses 4.5.5.6, 4.5.6).
/// So `new` keeps the encryption in use and MM announces and commits the change. A different
/// change while one is announced is refused (the radios were already told).
fn hold_air_change(shared: &SharedConfig, old: &StackConfig, new: &mut StackConfig) -> Result<AirHold, String> {
    let current = crate::aie::on_air(old.security.aie.as_ref());
    let wanted = crate::aie::on_air(new.security.aie.as_ref());
    let pending = shared.state_read().air_change.clone();
    let announced = pending.as_ref().filter(|p| p.at.is_some());
    if !differs(&current, &wanted) {
        return match (announced, pending.is_some()) {
            (Some(_), _) => Err("a security class or key change is already announced to the radios; save again once it is in effect".into()),
            (None, true) => Ok(AirHold::Cancel),
            (None, false) => Ok(AirHold::None),
        };
    }
    let target = new.security.aie.clone();
    new.security.aie = old.security.aie.clone();
    if let Some(p) = announced {
        if differs(&crate::aie::on_air(p.target.as_ref()), &wanted) {
            return Err("another security class or key change is already announced to the radios; save again once it is in effect".into());
        }
        return Ok(AirHold::Keep);
    }
    Ok(AirHold::Set(AirChange { target, lead_secs: new.security.change_lead_secs, at: None }))
}

fn apply_air_hold(shared: &SharedConfig, hold: AirHold) {
    let id = shared.config().cell_id;
    match hold {
        AirHold::Set(change) => {
            tracing::info!(
                "Live reload: {id:?} changes to security class {} — announcing it to the registered radios {} s ahead (D-CK CHANGE DEMAND)",
                crate::aie::on_air(change.target.as_ref()).map_or(1, |a| a.0),
                change.lead_secs
            );
            shared.state_write().air_change = Some(change);
        }
        AirHold::Cancel => {
            tracing::info!("Live reload: {id:?} security change withdrawn before it was announced");
            shared.state_write().air_change = None;
        }
        AirHold::None | AirHold::Keep => {}
    }
}

/// How a removed cell leaves: its radios keep their registrations when another cell of the
/// linked network serves the same location area (they reselect there without registering, TS
/// 100 392-2 clause 16.4.1.0, and the site switch counts them there), else they are dropped.
fn retirement(old: &StackConfig, new: &StackConfig, id: CellId) -> Retirement {
    let Some(la) = old.extra_cells.iter().find(|x| x.id == id).map(|x| x.cell.location_area) else {
        return Retirement::DropRegistrations;
    };
    let linked = new.network_is_linked(&new.network);
    let sibling = new.cells().iter().any(|(c, cell, _)| *c != id && cell.location_area == la && new.network_of_cell(*c).name == new.network);
    if linked && sibling { Retirement::KeepRegistrations } else { Retirement::DropRegistrations }
}

/// Apply `new` (already parsed, validated and given its sibling neighbours) to the primary cell
/// and to every additional cell. Returns the settings that block a live reload without applying
/// anything, or the number of cells swapped.
pub fn reload(primary: &SharedConfig, extra: &[(CellId, SharedConfig)], new: StackConfig) -> Result<ReloadReport, String> {
    reload_with(primary, extra, new, &|_| {})
}

/// [`reload`] with a hook that completes each additional cell's derived config before it is
/// validated and swapped (key management gives every cell the server key of its own LA, the
/// same as at start-up).
pub fn reload_with(
    primary: &SharedConfig,
    extra: &[(CellId, SharedConfig)],
    mut new: StackConfig,
    prepare_cell: &dyn Fn(&mut StackConfig),
) -> Result<ReloadReport, String> {
    let old = primary.config();
    let single_cell = old.extra_cells.is_empty() && new.extra_cells.is_empty();
    let restart = restart_reasons(&old, &new);
    if !restart.is_empty() {
        return Ok(ReloadReport { cells: 0, restart });
    }
    // Build every cell's config first so a bad one fails before anything is swapped.
    let (added, removed) = cell_set_change(&old, &new);
    let mut derived = Vec::new();
    for (id, shared) in extra {
        if removed.contains(id) {
            continue;
        }
        let mut cell = new.for_extra_cell(*id).ok_or_else(|| format!("cell {id:?} is not in the new configuration"))?;
        prepare_cell(&mut cell);
        cell.validate()?;
        let hold = hold_air_change(shared, &shared.config(), &mut cell)?;
        derived.push((shared, cell, hold));
    }
    let mut started = Vec::new();
    for id in &added {
        let mut cell = new.for_extra_cell(*id).ok_or_else(|| format!("cell {id:?} is not in the new configuration"))?;
        prepare_cell(&mut cell);
        started.push((*id, SharedConfig::try_from_parts(cell, None)?));
    }
    let stopped: Vec<(CellId, Retirement)> = removed.iter().map(|id| (*id, retirement(&old, &new, *id))).collect();
    let primary_hold = hold_air_change(primary, &old, &mut new)?;
    new.validate()?;
    if differs(&old.security, &new.security) || differs(&old.extra_cells.iter().map(|c| &c.security).collect::<Vec<_>>(), &new.extra_cells.iter().map(|c| &c.security).collect::<Vec<_>>()) {
        tracing::info!("Registration rules or subscriber keys changed live; they apply from each radio's next registration");
    }
    let (log_changed, name_changed) = (differs(&old.debug_log, &new.debug_log), differs(&old.service_name, &new.service_name));
    let (log_file, service_name) = (new.debug_log.clone(), new.service_name.clone());
    let dashboard = new.dashboard.clone();
    let link = (old.brew.is_some(), new.brew.is_some());
    primary.replace_config(new)?;
    apply_air_hold(primary, primary_hold);
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
    for (shared, cell, hold) in derived {
        shared.replace_config(cell)?;
        apply_air_hold(shared, hold);
    }
    for (id, how) in stopped {
        tracing::info!("Live reload: cell {id:?} removed, taking it off the air ({how:?})");
        crate::live_cells::apply(crate::live_cells::CellChange::Remove { id, how });
    }
    for (id, cfg) in started {
        tracing::info!("Live reload: cell {id:?} added, starting it");
        crate::live_cells::apply(crate::live_cells::CellChange::Add { id, cfg });
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

    /// The example config with additional cells (`[[cells]]` entries appended).
    fn with_cells(cells: &str) -> StackConfig {
        let text = format!("{}\n{cells}", include_str!("../../../example_config/config.toml"));
        let mut c = parsing::from_toml_str(&text).expect("config with cells parses");
        c.phy_io.soapysdr.as_mut().unwrap().device = Some("driver=zero".into());
        c.add_sibling_neighbours();
        c
    }

    const CELL_1: &str = "[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\ncolour_code = 2\n[cells.soapysdr]\ntx_freq = 438125000.0\nrx_freq = 433125000.0\ndevice = \"driver=one\"\n";
    const CELL_2: &str = "[[cells]]\nid = 2\n[cells.cell_info]\nmain_carrier = 1530\ncolour_code = 3\n[cells.soapysdr]\ntx_freq = 438250000.0\nrx_freq = 433250000.0\ndevice = \"driver=two\"\n";

    fn record_cell_changes() -> std::rc::Rc<std::cell::RefCell<Vec<String>>> {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        // The handler runs on this thread only, so a thread-local log is enough.
        thread_local! {
            static LOG: std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<Vec<String>>>>> = const { std::cell::RefCell::new(None) };
        }
        LOG.with(|l| *l.borrow_mut() = Some(seen.clone()));
        crate::live_cells::set_test_handler(Some(Box::new(|change| {
            let line = match change {
                crate::live_cells::CellChange::Add { id, cfg } => format!("add {id:?} carrier {}", cfg.config().cell.main_carrier),
                crate::live_cells::CellChange::Remove { id, how } => format!("remove {id:?} {how:?}"),
            };
            LOG.with(|l| l.borrow().as_ref().expect("log").borrow_mut().push(line));
        })));
        seen
    }

    #[test]
    fn a_cell_on_its_own_sdr_starts_and_stops_live_when_the_station_can() {
        let one = with_cells(CELL_1);
        one.validate().unwrap();
        let two = with_cells(&format!("{CELL_1}{CELL_2}"));
        crate::live_cells::set_test_handler(None);
        assert!(!restart_reasons(&one, &two).is_empty(), "without a cell handler adding a cell restarts");

        let seen = record_cell_changes();
        assert!(restart_reasons(&one, &two).is_empty(), "{:?}", restart_reasons(&one, &two));
        let primary = SharedConfig::from_parts(one.clone(), None);
        let cell1 = SharedConfig::from_parts(one.for_extra_cell(CellId(1)).unwrap(), None);
        let report = reload(&primary, &[(CellId(1), cell1.clone())], two.clone()).unwrap();
        assert!(report.restart.is_empty(), "{:?}", report.restart);
        assert_eq!(*seen.borrow(), vec!["add CellId(2) carrier 1530".to_string()]);
        // Cell 1 now lists its new sibling as a neighbour (live, like any neighbour change).
        assert!(cell1.config().cell.neighbor_cells_ca.len() > one.for_extra_cell(CellId(1)).unwrap().cell.neighbor_cells_ca.len());

        // Removing it again: independent cells (no network link) drop their registrations.
        seen.borrow_mut().clear();
        let cell2 = SharedConfig::from_parts(two.for_extra_cell(CellId(2)).unwrap(), None);
        let report = reload(&primary, &[(CellId(1), cell1), (CellId(2), cell2)], one).unwrap();
        assert!(report.restart.is_empty(), "{:?}", report.restart);
        assert_eq!(*seen.borrow(), vec!["remove CellId(2) DropRegistrations".to_string()]);
        crate::live_cells::set_test_handler(None);
    }

    #[test]
    fn a_cell_on_a_shared_sdr_or_one_that_links_the_cells_needs_a_restart() {
        let _seen = record_cell_changes();
        let one = base();
        let device = one.phy_io.soapysdr.as_ref().and_then(|s| s.device.clone()).unwrap_or_else(|| "driver=test".into());
        let mut shared = with_cells(CELL_1);
        shared.phy_io.soapysdr.as_mut().unwrap().device = Some(device.clone());
        shared.extra_cells[0].soapysdr = shared.phy_io.soapysdr.clone();
        let mut alone = one.clone();
        alone.phy_io.soapysdr.as_mut().unwrap().device = Some(device);
        assert!(restart_reasons(&alone, &shared).iter().any(|r| r.contains("shared SDR")), "{:?}", restart_reasons(&alone, &shared));

        // Two cells of a switched network down to one: the primary stops being site-linked.
        let mut two = with_cells(CELL_1);
        two.networks_explicit = true;
        let mut single = base();
        single.networks_explicit = true;
        assert!(restart_reasons(&two, &single).iter().any(|r| r.contains("sharing the network link")), "{:?}", restart_reasons(&two, &single));
        crate::live_cells::set_test_handler(None);
    }

    #[test]
    fn a_removed_linked_cell_keeps_its_radios_registered_only_when_its_location_area_stays_served() {
        let mut three = with_cells(&format!("{CELL_1}{CELL_2}"));
        three.networks_explicit = true;
        let la = three.cell.location_area;
        three.extra_cells[0].cell.location_area = la;
        three.extra_cells[1].cell.location_area = la.wrapping_add(1).max(1);
        let mut two = three.clone();
        two.extra_cells.retain(|c| c.id == CellId(1));
        assert_eq!(retirement(&three, &two, CellId(2)), Retirement::DropRegistrations);
        let mut other = three.clone();
        other.extra_cells.retain(|c| c.id == CellId(2));
        assert_eq!(retirement(&three, &other, CellId(1)), Retirement::KeepRegistrations);
        // Without a network link the cells are independent: nobody counts the radios elsewhere.
        let (mut a, mut b) = (three.clone(), other.clone());
        a.networks_explicit = false;
        b.networks_explicit = false;
        assert_eq!(retirement(&a, &b, CellId(1)), Retirement::DropRegistrations);
    }

    #[test]
    fn identical_config_needs_no_restart() {
        assert!(restart_reasons(&base(), &base()).is_empty());
    }

    #[test]
    fn rf_change_that_cannot_retune_needs_restart_and_is_not_applied() {
        let shared = SharedConfig::from_parts(base(), None);
        let mut new = base();
        new.cell.secondary_carrier = Some(new.cell.main_carrier + 2);
        let report = reload(&shared, &[], new).unwrap();
        assert!(!report.restart.is_empty(), "a second carrier needs another channelizer");
        assert_eq!(shared.config_generation(), 0);
        let mut fs = base();
        fs.phy_io.soapysdr.as_mut().unwrap().fs = Some(2_000_000.0);
        assert!(!restart_reasons(&base(), &fs).is_empty(), "sample rate");
    }

    #[test]
    fn a_cell_on_its_own_sdr_moves_to_another_carrier_live() {
        let mut new = base();
        new.cell.main_carrier += 2;
        let (_, dl, ul) = new.bs_phase_mod_carriers().unwrap()[0];
        let s = new.phy_io.soapysdr.as_mut().unwrap();
        s.dl_freq = dl as f64;
        s.ul_freq = ul as f64;
        new.validate().unwrap();
        assert!(restart_reasons(&base(), &new).is_empty(), "{:?}", restart_reasons(&base(), &new));
    }

    fn aie(class: u8, key: u8) -> tetra_config::bluestation::CfgAie {
        tetra_config::bluestation::CfgAie {
            class3: (class == 3).then(|| tetra_config::bluestation::CfgClass3 { cck: tetra_config::bluestation::CipherKey([key; 10]), cck_id: 1 }),
            enabled: class >= 2,
            ksg: 1,
            sckn: 0,
            sck: tetra_config::bluestation::CipherKey([key; 10]),
            sck_vn: 1,
            clear_groups: Vec::new(),
        }
    }

    #[test]
    fn colour_code_and_network_code_apply_live_as_a_cell_rebuild() {
        let mut cc = base();
        cc.cell.colour_code = (cc.cell.colour_code + 1) % 64;
        assert!(restart_reasons(&base(), &cc).is_empty(), "{:?}", restart_reasons(&base(), &cc));
        let mut mcc = base();
        mcc.net.mcc = (mcc.net.mcc + 1) & 0x3FF;
        assert!(restart_reasons(&base(), &mcc).is_empty());
        let mut mnc = base();
        mnc.net.mnc = (mnc.net.mnc + 1) & 0x3FFF;
        let shared = SharedConfig::from_parts(base(), None);
        let report = reload(&shared, &[], mnc).unwrap();
        assert!(report.restart.is_empty());
        assert_eq!(shared.config().net.mnc, (base().net.mnc + 1) & 0x3FFF);
    }

    #[test]
    fn security_class_and_cipher_keys_are_announced_tea_restarts() {
        let mut old = base();
        old.security.aie = Some(aie(2, 1));
        for (what, new_aie) in [("class 2 -> 3", Some(aie(3, 1))), ("class 2 -> 1", None), ("new SCK", Some(aie(2, 2)))] {
            let mut new = old.clone();
            new.security.aie = new_aie;
            assert!(restart_reasons(&old, &new).is_empty(), "{what}: {:?}", restart_reasons(&old, &new));
        }
        let mut tea = old.clone();
        tea.security.aie.as_mut().unwrap().ksg = 2;
        assert!(restart_reasons(&old, &tea).iter().any(|r| r.contains("TEA")), "{:?}", restart_reasons(&old, &tea));
        let mut from_clear = base();
        from_clear.security.aie = None;
        assert!(restart_reasons(&from_clear, &tea).is_empty(), "a cell going encrypted picks its TEA");
        let mut rules = old.clone();
        rules.security.issi_whitelist.push(1234567);
        rules.security.max_registered_clients += 1;
        assert!(restart_reasons(&old, &rules).is_empty(), "{:?}", restart_reasons(&old, &rules));
    }

    #[test]
    fn a_security_change_is_held_for_its_announcement() {
        let mut old = base();
        old.security.aie = Some(aie(2, 1));
        let shared = SharedConfig::from_parts(old.clone(), None);
        let mut new = old.clone();
        new.security.aie = Some(aie(2, 2));
        new.security.issi_whitelist.push(1234567);
        new.security.change_lead_secs = 45;
        reload(&shared, &[], new.clone()).unwrap();
        let running = shared.config();
        assert!(!differs(&running.security.aie, &old.security.aie), "the SCK in use stays until the change takes effect");
        assert_eq!(running.security.issi_whitelist, vec![1234567], "the rest applies at once");
        let change = shared.state_read().air_change.clone().expect("change held");
        assert!(!differs(&change.target, &new.security.aie));
        assert_eq!((change.lead_secs, change.at), (45, None));

        // Saved again before the announcement: the same change stays held; back to what runs: withdrawn.
        reload(&shared, &[], new.clone()).unwrap();
        assert!(shared.state_read().air_change.is_some());
        reload(&shared, &[], old.clone()).unwrap();
        assert!(shared.state_read().air_change.is_none(), "withdrawn");

        // Once announced, the same change is kept and another one is refused.
        reload(&shared, &[], new.clone()).unwrap();
        shared.state_write().air_change.as_mut().unwrap().at = Some(tetra_core::TdmaTime { t: 1, f: 1, m: 4, h: 1 });
        reload(&shared, &[], new.clone()).unwrap();
        assert!(shared.state_read().air_change.as_ref().unwrap().at.is_some());
        let mut other = new.clone();
        other.security.aie = None;
        assert!(reload(&shared, &[], other).is_err(), "the radios were told about another change");
        assert!(reload(&shared, &[], old).is_err(), "nor can it be taken back");
    }

    #[test]
    fn gains_antennas_and_ppm_apply_live_other_sdr_settings_restart() {
        let mut old = base();
        let soapy = old.phy_io.soapysdr.get_or_insert_with(|| panic!("example config has an SDR"));
        soapy.rx_gains.insert("LNA".into(), 20.0);
        soapy.rx_ant = Some("LNAL".into());
        let mut live = old.clone();
        {
            let s = live.phy_io.soapysdr.as_mut().unwrap();
            s.rx_gains.insert("LNA".into(), 30.0);
            s.tx_gains.insert("PAD".into(), 40.0);
            s.rx_ant = Some("LNAW".into());
            s.ppm_err += 1.5;
        }
        assert!(restart_reasons(&old, &live).is_empty(), "{:?}", restart_reasons(&old, &live));
        let mut removed = old.clone();
        removed.phy_io.soapysdr.as_mut().unwrap().rx_ant = None;
        assert!(!restart_reasons(&old, &removed).is_empty(), "back to an unknown device default");
        let mut gone = old.clone();
        gone.phy_io.soapysdr.as_mut().unwrap().rx_gains.clear();
        assert!(!restart_reasons(&old, &gone).is_empty());
        let mut device = old.clone();
        device.phy_io.soapysdr.as_mut().unwrap().device = Some("driver=other".into());
        assert!(!restart_reasons(&old, &device).is_empty());
    }

    #[test]
    fn a_changed_password_is_a_change() {
        let mut new = base();
        new.asterisk.password = tetra_config::bluestation::SecretField::from("other".to_string());
        assert!(differs(&base().asterisk, &new.asterisk), "the redacted Debug form hides the password");
        assert!(!format!("{:?}", new.asterisk).contains("other"), "plain Debug still redacts");
    }

    #[test]
    fn services_apply_live() {
        let mut new = base();
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
