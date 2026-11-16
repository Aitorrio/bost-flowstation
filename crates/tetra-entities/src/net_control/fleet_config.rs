//! Fleet Control config (`config-v1`): the Brew server reads and changes the station's config.
//!
//! - [`ControlCommand::GetConfig`] returns the settings of the dashboard's Infrastructure page as
//!   JSON ([`config_json`]), the file as TOML ([`redacted_text`]) and the config hash
//!   ([`hash_text`]). Passwords and keys never leave the station (only `password_set`, `"***"`).
//! - [`ControlCommand::SetConfig`] applies a change ([`apply_changes`]) checked like a local save,
//!   only on the config it was made on (`base_hash`), saves the old file (`<config>.fleet-backup`),
//!   writes the new one, notes what waits for confirmation (`<config>.fleet-pending.json`) and
//!   restarts the service. After the restart the station has `confirm_secs` (default 180) to get a
//!   [`ControlCommand::ConfirmConfig`]; without it the watchdog ([`boot`]) restores the old file and
//!   restarts again, so a change that cuts the station off (wrong login, wrong server, no network)
//!   undoes itself. [`ControlCommand::RollbackConfig`] undoes it at once.
//!
//! Protocol: `Docs/fleet-control.md` (station side), brew-server `docs/Fleet-Control.md`.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use tetra_config::bluestation::SharedConfig;
use toml::{Table, Value as TomlValue};

use super::commands::{ControlCommand, ControlResponse, JsonDoc};
use crate::net_dashboard::{cells, ota_channel, roaming};

/// Capability: `GetConfig`, `SetConfig`, `ConfirmConfig`, `RollbackConfig`, `config_hash` in the hello.
pub const CAP_CONFIG: &str = "config-v1";
/// Confirmation window after the restart when `SetConfig.confirm_secs` is 0.
pub const DEFAULT_CONFIRM_SECS: u32 = 180;
/// Bounds of the confirmation window.
pub const MIN_CONFIRM_SECS: u32 = 60;
pub const MAX_CONFIRM_SECS: u32 = 900;
/// Time between the answer to `SetConfig` and the service restart.
const RESTART_DELAY: Duration = Duration::from_secs(3);
/// A started service gets at least this long before the watchdog may roll back.
const MIN_AFTER_BOOT: Duration = Duration::from_secs(20);

/// `code` values of this module (besides `fleet::code`).
pub mod code {
    /// `SetConfig`: the station's config is not the one the change was made on.
    pub const CONFLICT: &str = "conflict";
    /// `SetConfig`: the change does not give a valid config.
    pub const INVALID: &str = "invalid";
    /// `ConfirmConfig` / `RollbackConfig`: nothing waits for confirmation.
    pub const NOT_PENDING: &str = "not_pending";
    /// `ConfirmConfig`: the change was written by this run and the restart has not happened yet.
    pub const NOT_RESTARTED: &str = "not_restarted";
}

static CONFIG_PATH: Mutex<Option<String>> = Mutex::new(None);

/// The armed confirmation window of this process.
static ARMED: Mutex<Option<Instant>> = Mutex::new(None);

/// This process wrote the waiting change: it runs the old config, so it must not confirm
/// (the hello it re-announces before the restart already says "pending").
static WRITTEN_HERE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Path of the station's config file (set by [`boot`]), `None` before.
pub fn config_path() -> Option<String> {
    CONFIG_PATH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn backup_path(config: &str) -> String {
    format!("{config}.fleet-backup")
}

fn pending_path(config: &str) -> String {
    format!("{config}.fleet-pending.json")
}

/// What waits for confirmation (`<config>.fleet-pending.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingFile {
    pub old_hash: String,
    pub new_hash: String,
    pub confirm_secs: u32,
    #[serde(default)]
    pub by: String,
    /// When the new file was written (Unix seconds).
    pub written_unix: u64,
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Remember the config path and, when a change waits for confirmation, arm the watchdog: the
/// window runs from the restart (`written_unix` + restart delay + `confirm_secs`), at least
/// [`MIN_AFTER_BOOT`] from now. Called once at start, before the stack comes up.
pub fn boot(config: &str) {
    *CONFIG_PATH.lock().unwrap_or_else(|e| e.into_inner()) = Some(config.to_string());
    let Some(p) = read_pending(config) else { return };
    let secs = window_left(&p, now_unix());
    let left = Duration::from_secs(secs).max(MIN_AFTER_BOOT);
    tracing::warn!("Fleet Control: config change by {:?} waits for confirmation from the Brew server ({} s left)", p.by, left.as_secs());
    arm(Instant::now() + left);
}

/// Seconds of the window left at `now` (Unix seconds).
pub fn window_left(p: &PendingFile, now: u64) -> u64 {
    let end = p.written_unix + RESTART_DELAY.as_secs() + p.confirm_secs as u64;
    end.saturating_sub(now)
}

fn read_pending(config: &str) -> Option<PendingFile> {
    let text = std::fs::read_to_string(pending_path(config)).ok()?;
    match serde_json::from_str(&text) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("Fleet Control: cannot read {}: {e}", pending_path(config));
            None
        }
    }
}

fn arm(deadline: Instant) {
    let mut slot = ARMED.lock().unwrap_or_else(|e| e.into_inner());
    let start = slot.is_none();
    *slot = Some(deadline);
    drop(slot);
    if start {
        let _ = std::thread::Builder::new().name("fleet-config-watchdog".into()).spawn(watchdog);
    }
}

fn watchdog() {
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let due = match *ARMED.lock().unwrap_or_else(|e| e.into_inner()) {
            None => return,
            Some(deadline) => Instant::now() >= deadline,
        };
        if due {
            tracing::warn!("Fleet Control: config change not confirmed in time, restoring the old config");
            if let Err(e) = rollback() {
                tracing::error!("Fleet Control: rollback failed: {e}");
                *ARMED.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
            return;
        }
    }
}

/// Seconds left to confirm the last `SetConfig`, `None` when nothing waits.
pub fn pending_secs() -> Option<u32> {
    let deadline = (*ARMED.lock().unwrap_or_else(|e| e.into_inner()))?;
    Some(deadline.saturating_duration_since(Instant::now()).as_secs().min(u32::MAX as u64) as u32)
}

/// Keep the new config: drop the backup and the pending note. `false` when nothing waited.
pub fn confirm() -> Result<(), &'static str> {
    if WRITTEN_HERE.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(code::NOT_RESTARTED);
    }
    let was = ARMED.lock().unwrap_or_else(|e| e.into_inner()).take().is_some();
    let Some(config) = config_path() else { return if was { Ok(()) } else { Err(code::NOT_PENDING) } };
    let had_file = std::fs::remove_file(pending_path(&config)).is_ok();
    let _ = std::fs::remove_file(backup_path(&config));
    if was || had_file {
        tracing::info!("Fleet Control: config change confirmed by the Brew server");
    }
    if was || had_file { Ok(()) } else { Err(code::NOT_PENDING) }
}

/// Put the old config back and restart. `Err` when nothing waited or the backup is missing.
pub fn rollback() -> Result<(), String> {
    let config = config_path().ok_or("no config path")?;
    restore_files(&config)?;
    *ARMED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    tracing::warn!("Fleet Control: old config restored; restarting the service");
    crate::service_control::schedule_service_action(crate::service_control::ServiceAction::Restart, RESTART_DELAY);
    Ok(())
}

/// Copy the backup over the config and remove the pending note and the backup.
pub fn restore_files(config: &str) -> Result<(), String> {
    let backup = backup_path(config);
    if !std::path::Path::new(&pending_path(config)).exists() && !std::path::Path::new(&backup).exists() {
        return Err("no config change waits for confirmation".into());
    }
    let old = std::fs::read_to_string(&backup).map_err(|e| format!("cannot read {backup}: {e}"))?;
    crate::net_dashboard::server::atomic_write(config, &old).map_err(|e| format!("cannot write {config}: {e}"))?;
    let _ = std::fs::remove_file(pending_path(config));
    let _ = std::fs::remove_file(&backup);
    Ok(())
}

/// Save `old` as the backup, note what waits, write `new`. The pending note is written before
/// the config, so a crash in between still rolls back.
pub fn write_with_backup(config: &str, old: &str, new: &str, pending: &PendingFile) -> Result<(), String> {
    use crate::net_dashboard::server::atomic_write;
    atomic_write(&backup_path(config), old).map_err(|e| format!("cannot save the old config: {e}"))?;
    let note = serde_json::to_string_pretty(pending).map_err(|e| e.to_string())?;
    atomic_write(&pending_path(config), &note).map_err(|e| format!("cannot write {}: {e}", pending_path(config)))?;
    atomic_write(config, new).map_err(|e| format!("cannot write the config: {e}"))
}

// ---------------------------------------------------------------------------
// Hash and redaction
// ---------------------------------------------------------------------------

/// Hash of the config: of the parsed TOML (so comments and layout do not count), without the
/// Fleet Control switch (`[command] fleet_control` is switched on the station without being a
/// config change). 16 hex digits.
pub fn hash_text(text: &str) -> String {
    let norm = match toml::from_str::<Table>(text) {
        Ok(mut t) => {
            if let Some(c) = t.get_mut("command").and_then(|v| v.as_table_mut()) {
                c.remove("fleet_control");
            }
            toml::to_string(&t).unwrap_or_else(|_| text.to_string())
        }
        Err(_) => text.to_string(),
    };
    format!("{:x}", md5::compute(norm.as_bytes()))[..16].to_string()
}

/// A key whose value is secret (passwords, tokens, cipher keys).
fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    if k.ends_with("_path") || k.ends_with("_file") || k.ends_with("_dir") || k.ends_with("_url") {
        return false;
    }
    ["pass", "secret", "token", "key", "auth"].iter().any(|w| k.contains(w)) || ["sck", "cck", "gck", "dck", "k", "ki", "ks", "psk"].contains(&k.as_str())
}

fn redact_value(v: &mut TomlValue, secret: bool) {
    match v {
        TomlValue::Table(t) => redact_table(t),
        TomlValue::Array(a) => a.iter_mut().for_each(|x| redact_value(x, secret)),
        TomlValue::String(s) if secret && !s.is_empty() => *s = "***".into(),
        TomlValue::Integer(_) if secret => *v = TomlValue::String("***".into()),
        _ => {}
    }
}

fn redact_table(t: &mut Table) {
    for (k, v) in t.iter_mut() {
        redact_value(v, is_secret_key(k));
    }
}

/// The config file as TOML with every secret value replaced by `"***"` (comments dropped).
pub fn redacted_text(text: &str) -> Result<String, String> {
    let mut t: Table = toml::from_str(text).map_err(|e| format!("config does not parse: {e}"))?;
    redact_table(&mut t);
    toml::to_string(&t).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// A Brew link (`[brew]` or `[networks.<n>.brew]`) without its password.
fn brew_json(t: Option<&Table>) -> JsonValue {
    let Some(t) = t else { return JsonValue::Null };
    json!({
        "host": t.get("host").and_then(|v| v.as_str()),
        "port": t.get("port").and_then(|v| v.as_integer()),
        "tls": t.get("tls").and_then(|v| v.as_bool()).unwrap_or(true),
        "username": t.get("username").and_then(|v| v.as_integer()),
        "password_set": t.get("password").and_then(|v| v.as_str()).is_some_and(|p| !p.is_empty()),
    })
}

/// The settings for the server's edit page: the Infrastructure page's JSON (`roaming_json`)
/// without what only the station's own page needs (the server's cell directory and networks),
/// plus the Brew links with their logins, the SDR of each cell, the carrier setup and the OTA
/// source. Passwords never: only `password_set`.
pub fn config_json(config: &str, shared: Option<&SharedConfig>) -> Result<JsonValue, String> {
    let text = std::fs::read_to_string(config).map_err(|e| format!("cannot read config: {e}"))?;
    let table: Table = toml::from_str(&text).map_err(|e| format!("config does not parse: {e}"))?;
    let mut v = roaming::roaming_json(config, shared)?;
    if let Some(o) = v.as_object_mut() {
        o.remove("directory");
        o.remove("server_networks");
        o.remove("max_cells");
    }
    let networks = table.get("networks").and_then(|n| n.as_table());
    if let Some(list) = v.get_mut("networks").and_then(|n| n.as_array_mut()) {
        for n in list.iter_mut() {
            let name = n.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string();
            let entry = networks.and_then(|t| t.get(&name)).and_then(|e| e.as_table());
            n["brew"] = brew_json(entry.and_then(|e| e.get("brew")).and_then(|b| b.as_table()));
        }
    }
    // The classic link (`[brew]`); with `[networks]` the links are on the networks.
    v["brew"]["link"] = brew_json(table.get("brew").and_then(|b| b.as_table()));
    fn soapy(t: Option<&Table>) -> Option<&Table> {
        t.and_then(|s| s.get("soapysdr")).and_then(|s| s.as_table())
    }
    let primary_soapy = soapy(table.get("phy_io").and_then(|p| p.as_table()));
    if let Some(list) = v.get_mut("cells").and_then(|c| c.as_array_mut()) {
        for c in list.iter_mut() {
            let id = c.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u8;
            let s = if id == 0 { primary_soapy } else { soapy(roaming::extra_cell_entry(&table, id)) };
            c["device"] = json!(s.and_then(|s| s.get("device")).and_then(|d| d.as_str()));
            if id == 0 {
                // A carrier change on the primary needs a plain single-carrier setup.
                c["carrier_fixed"] = json!(carrier_locked(&table));
            }
        }
    }
    let source = ota_channel::resolve_from(&text, || ota_channel::installed_source(config));
    v["ota"] = json!({
        "channel": source.channel,
        "repo": ota_channel::dashboard_value(&text, "ota_repo"),
        "branch": ota_channel::dashboard_value(&text, "ota_branch"),
        "effective_repo": source.repo_display,
        "effective_branch": source.branch,
    });
    // Edited through `raw` patches (the server's Home Mode Display and ISSI whitelist cards).
    let ci = table.get("cell_info").and_then(|c| c.as_table());
    v["home_mode_display"] = ci.and_then(|c| c.get("home_mode_display")).map(|h| serde_json::to_value(h).unwrap_or(JsonValue::Null)).unwrap_or(JsonValue::Null);
    let sec = table.get("security").and_then(|s| s.as_table());
    v["access"] = json!({
        "issi_whitelist": sec.and_then(|s| s.get("issi_whitelist")).map(|l| serde_json::to_value(l).unwrap_or(JsonValue::Null)).unwrap_or(json!([])),
        "whitelist_mode": sec.and_then(|s| s.get("whitelist_mode")).and_then(|m| m.as_str()).unwrap_or("auto"),
    });
    v["net_info"] = json!({
        "mcc": table.get("net_info").and_then(|n| n.get("mcc")).and_then(|x| x.as_integer()),
        "mnc": table.get("net_info").and_then(|n| n.get("mnc")).and_then(|x| x.as_integer()),
    });
    Ok(v)
}

/// Why the primary carrier cannot be changed from the server (`None`: it can): a second carrier
/// or an SDR centre frequency is set, which only the station's own pages adjust together.
fn carrier_locked(table: &Table) -> Option<&'static str> {
    let ci = table.get("cell_info").and_then(|c| c.as_table());
    let soapy = table.get("phy_io").and_then(|p| p.get("soapysdr")).and_then(|s| s.as_table());
    if ci.is_some_and(|c| c.contains_key("secondary_carrier")) {
        return Some("dual carrier is set up: change the carrier on the station");
    }
    if soapy.is_some_and(|s| s.contains_key("tx_center_freq") || s.contains_key("rx_center_freq")) {
        return Some("an SDR centre frequency is set: change the carrier on the station");
    }
    None
}

// ---------------------------------------------------------------------------
// Changing
// ---------------------------------------------------------------------------

/// Keys `apply_roaming` takes as they are.
const ROAMING_KEYS: &[&str] =
    &["reselect", "rxlev_access_min_dbm", "migration", "presence_check_secs", "multi_network_enabled", "networks", "telemetry", "command", "cells"];
/// Keys only Fleet Control adds.
const FLEET_KEYS: &[&str] = &["cells_add", "cells_remove", "brew", "ota", "net_info", "raw"];

/// Apply a `SetConfig.changes` object to the config text; returns the new text, checked like a
/// local save. Every key is optional:
/// - the keys of the Infrastructure page's save (`POST /api/roaming`, see
///   [`roaming::apply_roaming`]): `reselect`, `rxlev_access_min_dbm`, `migration`,
///   `presence_check_secs`, `multi_network_enabled`, `networks`, `telemetry`, `command`, `cells`
///   (per cell `id`, `location_area`, `network`, `neighbours`, and here also `main_carrier` and
///   `colour_code`);
/// - `cells_remove: [id]` and `cells_add: [{device, main_carrier, colour_code?, location_area?}]`
///   (like the Cells card), applied first;
/// - `brew: {host, port, tls, username, password}`: the classic `[brew]` link (with `[networks]`
///   the links are set per network);
/// - `net_info: {mcc, mnc}` (classic config only);
/// - `ota: {channel, repo?, branch?}` (`[dashboard] ota_channel` / `ota_repo` / `ota_branch`);
/// - `raw: [{path: [key | index, ..], value?, remove?}]`: single values of the config file (the
///   server's config editor and the Home Mode Display / ISSI whitelist cards send only what
///   changed), applied last. Keys and passwords are refused here.
///
/// An empty password keeps the saved one. A key this station does not know is refused, so an
/// older station never drops part of a change silently.
pub fn apply_changes(original: &str, changes: &JsonValue) -> Result<String, String> {
    let obj = changes.as_object().ok_or("changes must be a JSON object")?;
    if let Some(k) = obj.keys().find(|k| !ROAMING_KEYS.contains(&k.as_str()) && !FLEET_KEYS.contains(&k.as_str())) {
        return Err(format!("unknown setting {k:?} (this station's software does not know it)"));
    }
    let mut text = original.to_string();

    for id in obj.get("cells_remove").and_then(|v| v.as_array()).map(Vec::as_slice).unwrap_or_default() {
        let id = id.as_u64().and_then(|i| u8::try_from(i).ok()).ok_or("cells_remove: cell ids")?;
        text = cells::remove_cell_toml(&text, id)?;
    }
    for (i, c) in obj.get("cells_add").and_then(|v| v.as_array()).map(Vec::as_slice).unwrap_or_default().iter().enumerate() {
        let what = format!("cells_add[{i}]");
        let device = c.get("device").and_then(|v| v.as_str()).unwrap_or_default();
        let carrier = c.get("main_carrier").and_then(|v| v.as_u64()).filter(|n| *n < 4096).ok_or(format!("{what}.main_carrier (0-4095) is required"))?;
        let cc = match c.get("colour_code").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => Some(roaming::int_in(v, &format!("{what}.colour_code"), 0, 63)? as u8),
        };
        let la = match c.get("location_area").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => Some(roaming::int_in(v, &format!("{what}.location_area"), 0, 16383)? as u16),
        };
        text = cells::add_cell_toml(&text, device, carrier as u16, cc, la)?.0;
    }

    let roaming_part: serde_json::Map<String, JsonValue> =
        obj.iter().filter(|(k, _)| ROAMING_KEYS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    if !roaming_part.is_empty() {
        text = roaming::apply_roaming(&text, &JsonValue::Object(roaming_part))?;
    }

    let carriers: Vec<&JsonValue> = obj
        .get("cells")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter(|c| c.get("main_carrier").is_some() || c.get("colour_code").is_some()).collect())
        .unwrap_or_default();
    if !carriers.is_empty() || obj.contains_key("brew") || obj.contains_key("net_info") {
        let mut table: Table = toml::from_str(&text).map_err(|e| format!("config does not parse: {e}"))?;
        for c in carriers {
            apply_cell_radio(&mut table, c)?;
        }
        if let Some(b) = obj.get("brew").filter(|b| !b.is_null()) {
            if table.contains_key("networks") {
                return Err("brew: this station uses [networks]; set the Brew link on the network".into());
            }
            roaming::apply_brew_link(&mut table, b, "brew")?;
        }
        if let Some(n) = obj.get("net_info").filter(|n| !n.is_null()) {
            if table.contains_key("networks") {
                return Err("net_info: this station uses [networks]; set MCC / MNC on the network".into());
            }
            let ni = roaming::table_mut(&mut table, "net_info")?;
            if let Some(v) = n.get("mcc") {
                ni.insert("mcc".into(), TomlValue::Integer(roaming::int_in(v, "net_info.mcc", 0, 1023)?));
            }
            if let Some(v) = n.get("mnc") {
                ni.insert("mnc".into(), TomlValue::Integer(roaming::int_in(v, "net_info.mnc", 0, 16383)?));
            }
        }
        text = toml::to_string(&table).map_err(|e| e.to_string())?;
    }

    if let Some(o) = obj.get("ota").filter(|o| !o.is_null()) {
        let channel = o.get("channel").and_then(|v| v.as_str()).ok_or("ota.channel is required")?;
        if !["installed", "stable", "beta", "custom"].contains(&channel) {
            return Err(format!("ota.channel: installed, stable, beta or custom, not {channel:?}"));
        }
        text = ota_channel::ota_source_text(&text, channel, o.get("repo").and_then(|v| v.as_str()), o.get("branch").and_then(|v| v.as_str()))?;
    }

    if let Some(list) = obj.get("raw").filter(|r| !r.is_null()) {
        let mut table: Table = toml::from_str(&text).map_err(|e| format!("config does not parse: {e}"))?;
        apply_raw(&mut table, list)?;
        text = toml::to_string(&table).map_err(|e| e.to_string())?;
    }

    cells::validated(&text)?;
    Ok(text)
}

/// One step of a `raw` path: a table key or an array index.
enum Step<'a> {
    Key(&'a str),
    Index(usize),
}

/// Apply `raw` patches (see [`apply_changes`]). Missing tables on the way are created; an index
/// must exist (arrays are replaced as a whole, or element by element when the length stays).
fn apply_raw(table: &mut Table, list: &JsonValue) -> Result<(), String> {
    let list = list.as_array().ok_or("raw must be a list")?;
    let mut root = TomlValue::Table(std::mem::take(table));
    let result = apply_raw_to(&mut root, list);
    if let TomlValue::Table(t) = root {
        *table = t;
    }
    result
}

fn apply_raw_to(root: &mut TomlValue, list: &[JsonValue]) -> Result<(), String> {
    for (i, p) in list.iter().enumerate() {
        let what = format!("raw[{i}]");
        let steps: Vec<Step> = p
            .get("path")
            .and_then(|v| v.as_array())
            .filter(|a| !a.is_empty())
            .ok_or(format!("{what}.path is required"))?
            .iter()
            .map(|s| match s {
                JsonValue::String(k) if !k.is_empty() => Ok(Step::Key(k.as_str())),
                JsonValue::Number(n) if n.as_u64().is_some() => Ok(Step::Index(n.as_u64().unwrap_or(0) as usize)),
                _ => Err(format!("{what}.path: keys and indexes only")),
            })
            .collect::<Result<_, _>>()?;
        let name = p["path"].as_array().map(|a| a.iter().map(|s| s.as_str().map(str::to_owned).unwrap_or_else(|| s.to_string())).collect::<Vec<_>>().join(".")).unwrap_or_default();
        if steps.iter().any(|s| matches!(s, Step::Key(k) if is_secret_key(k))) {
            return Err(format!("{name}: keys and passwords are not changed this way"));
        }
        let remove = p.get("remove").and_then(|v| v.as_bool()).unwrap_or(false);
        let value = if remove {
            None
        } else {
            let v = p.get("value").filter(|v| !v.is_null()).ok_or(format!("{what}.value is required"))?;
            if v.to_string().contains("\"***\"") {
                return Err(format!("{name}: a hidden value (***) cannot be written back"));
            }
            Some(TomlValue::try_from(v.clone()).map_err(|e| format!("{name}: {e}"))?)
        };
        let (last, parents) = steps.split_last().expect("not empty");
        let mut cur: &mut TomlValue = &mut *root;
        for s in parents {
            cur = match (s, cur) {
                (Step::Key(k), TomlValue::Table(t)) => t.entry(k.to_string()).or_insert_with(|| TomlValue::Table(Table::new())),
                (Step::Index(n), TomlValue::Array(a)) => a.get_mut(*n).ok_or(format!("{name}: no element {n}"))?,
                _ => return Err(format!("{name}: the path does not fit the config")),
            };
        }
        match (last, cur, value) {
            (Step::Key(k), TomlValue::Table(t), Some(v)) => {
                t.insert(k.to_string(), v);
            }
            (Step::Key(k), TomlValue::Table(t), None) => {
                t.remove(*k);
            }
            (Step::Index(n), TomlValue::Array(a), Some(v)) if *n < a.len() => a[*n] = v,
            (Step::Index(n), TomlValue::Array(a), Some(v)) if *n == a.len() => a.push(v),
            (Step::Index(n), TomlValue::Array(a), None) if *n < a.len() => {
                a.remove(*n);
            }
            _ => return Err(format!("{name}: the path does not fit the config")),
        }
    }
    Ok(())
}


/// `main_carrier` / `colour_code` of one `cells[]` entry. A new carrier also moves the cell's SDR
/// frequencies (the primary's band plan, like a new cell on the Cells card).
fn apply_cell_radio(table: &mut Table, c: &JsonValue) -> Result<(), String> {
    let id = c.get("id").and_then(|v| v.as_u64()).and_then(|v| u8::try_from(v).ok()).ok_or("cells[].id is required")?;
    let what = format!("cell {id}");
    let primary_text = toml::to_string(&*table).map_err(|e| e.to_string())?;
    let cfg = tetra_config::bluestation::parsing::from_toml_str(&primary_text).map_err(|e| format!("config does not parse: {e}"))?;
    let carrier = match c.get("main_carrier").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(roaming::int_in(v, &format!("{what} main_carrier"), 0, 4095)? as u16),
    };
    let current = if id == 0 { Some(cfg.cell.main_carrier) } else { cfg.extra_cells.iter().find(|e| e.id.0 == id).map(|e| e.cell.main_carrier) };
    let current = current.ok_or(format!("{what} is not in the config"))?;
    let new_carrier = carrier.filter(|c| *c != current);
    if new_carrier.is_some() && id == 0 {
        if let Some(why) = carrier_locked(table) {
            return Err(format!("{what}: {why}"));
        }
    }
    let freqs = match new_carrier {
        Some(n) => Some(cells::carrier_freqs(&cfg.cell, n)?),
        None => None,
    };
    let entry: &mut Table = if id == 0 { table } else { roaming::extra_cell_entry_mut(table, id).ok_or(format!("{what} is not in the config"))? };
    let ci = roaming::table_mut(entry, "cell_info")?;
    if let Some(n) = new_carrier {
        ci.insert("main_carrier".into(), TomlValue::Integer(n as i64));
    }
    if let Some(v) = c.get("colour_code").filter(|v| !v.is_null()) {
        ci.insert("colour_code".into(), TomlValue::Integer(roaming::int_in(v, &format!("{what} colour_code"), 0, 63)?));
    }
    if let Some((dl, ul)) = freqs {
        let soapy = if id == 0 { roaming::table_mut(roaming::table_mut(entry, "phy_io")?, "soapysdr")? } else { roaming::table_mut(entry, "soapysdr")? };
        soapy.insert("tx_freq".into(), TomlValue::Integer(dl as i64));
        soapy.insert("rx_freq".into(), TomlValue::Integer(ul as i64));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// What the station is doing when a config command arrives.
#[derive(Debug, Clone, Copy, Default)]
pub struct StationNow {
    /// Fleet Control switch.
    pub enabled: bool,
    /// Calls in progress.
    pub calls: usize,
    /// An OTA runs.
    pub updating: bool,
}

/// Decide a `SetConfig` on `original`: the new text, or the refusal (`code`, message).
pub fn plan_set_config(
    original: &str,
    base_hash: &str,
    changes: &JsonValue,
    during_call: bool,
    now: StationNow,
    pending: bool,
) -> Result<String, (&'static str, String)> {
    use super::fleet::code as fc;
    if pending {
        return Err((fc::BUSY, "A config change still waits for confirmation".into()));
    }
    if now.updating {
        return Err((fc::BUSY, "An update is running".into()));
    }
    if now.calls > 0 && !during_call {
        return Err((fc::CALL_ACTIVE, "A call is in progress".into()));
    }
    let hash = hash_text(original);
    if hash != base_hash {
        return Err((code::CONFLICT, format!("The config on the station changed (now {hash}, change made on {base_hash})")));
    }
    let new = apply_changes(original, changes).map_err(|e| (code::INVALID, e))?;
    if hash_text(&new) == hash {
        return Err((code::INVALID, "Nothing to change".into()));
    }
    Ok(new)
}

/// Handle the `config-v1` commands (`None` for every other command).
pub fn config_command(command: &ControlCommand, now: StationNow, shared: Option<&SharedConfig>) -> Option<ControlResponse> {
    let handle = match command {
        ControlCommand::GetConfig { handle }
        | ControlCommand::SetConfig { handle, .. }
        | ControlCommand::ConfirmConfig { handle }
        | ControlCommand::RollbackConfig { handle } => *handle,
        _ => return None,
    };
    if !now.enabled {
        return Some(match command {
            ControlCommand::GetConfig { .. } => info_refused(handle, super::fleet::code::FLEET_CONTROL_OFF, "Fleet Control is switched off on this station"),
            _ => result(handle, false, super::fleet::code::FLEET_CONTROL_OFF, "Fleet Control is switched off on this station"),
        });
    }
    let Some(config) = config_path() else {
        return Some(match command {
            ControlCommand::GetConfig { .. } => info_refused(handle, super::fleet::code::UNAVAILABLE, "No config file"),
            _ => result(handle, false, super::fleet::code::UNAVAILABLE, "No config file"),
        });
    };
    Some(match command {
        ControlCommand::GetConfig { .. } => get_config(handle, &config, shared),
        ControlCommand::SetConfig { base_hash, changes, confirm_secs, during_call, by, .. } => {
            let original = match std::fs::read_to_string(&config) {
                Ok(t) => t,
                Err(e) => return Some(result(handle, false, super::fleet::code::UNAVAILABLE, &format!("cannot read config: {e}"))),
            };
            match plan_set_config(&original, base_hash, &changes.value(), *during_call, now, pending_secs().is_some()) {
                Err((c, m)) => {
                    tracing::info!("Fleet Control: config change by {by:?} refused ({c}): {m}");
                    result(handle, false, c, &m)
                }
                Ok(new) => {
                    let secs = if *confirm_secs == 0 { DEFAULT_CONFIRM_SECS } else { (*confirm_secs).clamp(MIN_CONFIRM_SECS, MAX_CONFIRM_SECS) };
                    let pending = PendingFile { old_hash: hash_text(&original), new_hash: hash_text(&new), confirm_secs: secs, by: by.clone(), written_unix: now_unix() };
                    match write_with_backup(&config, &original, &new, &pending) {
                        Err(e) => result(handle, false, super::fleet::code::UNAVAILABLE, &e),
                        Ok(()) => {
                            tracing::warn!("Fleet Control: config changed by {by:?} ({} -> {}); restarting, {secs} s to confirm", pending.old_hash, pending.new_hash);
                            // Armed here too, so a restart that does not happen still rolls back.
                            arm(Instant::now() + RESTART_DELAY + Duration::from_secs(secs as u64));
                            WRITTEN_HERE.store(true, std::sync::atomic::Ordering::SeqCst);
                            crate::service_control::schedule_service_action(crate::service_control::ServiceAction::Restart, RESTART_DELAY);
                            result(handle, true, super::fleet::code::OK, &format!("Config written ({}), restarting; confirm within {secs} s", pending.new_hash))
                        }
                    }
                }
            }
        }
        ControlCommand::ConfirmConfig { .. } => {
            match confirm() {
                Ok(()) => result(handle, true, super::fleet::code::OK, "Config confirmed"),
                Err(code::NOT_RESTARTED) => result(handle, false, code::NOT_RESTARTED, "The station has not restarted with the new config yet"),
                Err(c) => result(handle, false, c, "No config change waits for confirmation"),
            }
        }
        ControlCommand::RollbackConfig { .. } => match rollback() {
            Ok(()) => result(handle, true, super::fleet::code::OK, "Old config restored, restarting"),
            Err(e) => result(handle, false, code::NOT_PENDING, &e),
        },
        _ => unreachable!("matched above"),
    })
}

fn get_config(handle: u32, config: &str, shared: Option<&SharedConfig>) -> ControlResponse {
    let read = || -> Result<(String, JsonValue, String), String> {
        let text = std::fs::read_to_string(config).map_err(|e| format!("cannot read config: {e}"))?;
        Ok((hash_text(&text), config_json(config, shared)?, redacted_text(&text)?))
    };
    match read() {
        Ok((hash, json, text)) => ControlResponse::ConfigInfo {
            handle,
            ok: true,
            code: super::fleet::code::OK.into(),
            message: String::new(),
            hash,
            config: JsonDoc::from_value(&json),
            text,
            pending: pending_secs(),
        },
        Err(e) => info_refused(handle, super::fleet::code::UNAVAILABLE, &e),
    }
}

fn info_refused(handle: u32, code: &str, message: &str) -> ControlResponse {
    ControlResponse::ConfigInfo {
        handle,
        ok: false,
        code: code.into(),
        message: message.into(),
        hash: String::new(),
        config: JsonDoc::default(),
        text: String::new(),
        pending: None,
    }
}

fn result(handle: u32, ok: bool, code: &str, message: &str) -> ControlResponse {
    ControlResponse::CommandResult { handle, ok, code: code.to_string(), message: message.to_string() }
}

/// The current config hash (`None` without a readable config).
pub fn current_hash() -> Option<String> {
    let config = config_path()?;
    std::fs::read_to_string(config).ok().map(|t| hash_text(&t))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1521
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[brew]
host = "brew.example.org"
port = 443
tls = true
username = 1000002
password = "topsecret"

[command]
host = "brew.example.org"
port = 9002
username = "1000002"
password = "alsosecret"
fleet_control = true
"#;

    #[test]
    fn hash_ignores_layout_comments_and_the_fleet_switch() {
        let h = hash_text(BASE);
        assert_eq!(h.len(), 16);
        let commented = format!("# a comment\n{}", BASE.replace("location_area = 1", "location_area   =   1 # here"));
        assert_eq!(hash_text(&commented), h, "comments and spaces do not count");
        assert_eq!(hash_text(&BASE.replace("fleet_control = true", "fleet_control = false")), h, "the switch does not count");
        assert_ne!(hash_text(&BASE.replace("location_area = 1", "location_area = 2")), h, "a real change does");
    }

    #[test]
    fn redacted_text_keeps_no_secret() {
        let r = redacted_text(BASE).unwrap();
        assert!(!r.contains("topsecret") && !r.contains("alsosecret"), "{r}");
        assert!(r.contains("brew.example.org") && r.contains("***"));
        let keys = "[security]\nsck = \"00112233445566778899\"\n[security.tea]\nkey = \"000102\"\nkey_path = \"/etc/k.pem\"\n";
        let r = redacted_text(keys).unwrap();
        assert!(!r.contains("00112233445566778899") && !r.contains("000102"), "{r}");
        assert!(r.contains("/etc/k.pem"), "paths stay");
    }

    #[test]
    fn unknown_settings_are_refused() {
        let e = apply_changes(BASE, &json!({"warp_drive": true})).unwrap_err();
        assert!(e.contains("warp_drive"), "{e}");
    }

    #[test]
    fn location_area_and_colour_code() {
        let t = apply_changes(BASE, &json!({"cells": [{"id": 0, "location_area": 7, "colour_code": 5}]})).unwrap();
        let cfg = tetra_config::bluestation::parsing::from_toml_str(&t).unwrap();
        assert_eq!((cfg.cell.location_area, cfg.cell.colour_code), (7, 5));
    }

    #[test]
    fn new_primary_carrier_moves_the_sdr_frequencies() {
        let with_sdr = BASE.replace("backend = \"None\"", "backend = \"None\"\n\n[phy_io.soapysdr]\ndevice = \"driver=sx1255\"\ntx_freq = 1\nrx_freq = 1");
        let t = apply_changes(&with_sdr, &json!({"cells": [{"id": 0, "main_carrier": 1530}]})).unwrap();
        let cfg = tetra_config::bluestation::parsing::from_toml_str(&t).unwrap();
        assert_eq!(cfg.cell.main_carrier, 1530);
        let (dl, ul) = cells::carrier_freqs(&cfg.cell, 1530).unwrap();
        let soapy = cfg.phy_io.soapysdr.unwrap();
        assert_eq!((soapy.dl_freq as u32, soapy.ul_freq as u32), (dl, ul));
    }

    #[test]
    fn carrier_is_locked_with_a_second_carrier() {
        let dual = BASE.replace("location_area = 1", "location_area = 1\nsecondary_carrier = 1522");
        let e = apply_changes(&dual, &json!({"cells": [{"id": 0, "main_carrier": 1530}]})).unwrap_err();
        assert!(e.contains("dual carrier"), "{e}");
    }

    #[test]
    fn brew_link_keeps_the_password_when_empty() {
        let t = apply_changes(BASE, &json!({"brew": {"host": "other.example.org", "port": 8443, "tls": true, "username": 1000009, "password": ""}})).unwrap();
        let table: Table = toml::from_str(&t).unwrap();
        let b = table["brew"].as_table().unwrap();
        assert_eq!(b["host"].as_str(), Some("other.example.org"));
        assert_eq!(b["username"].as_integer(), Some(1000009));
        assert_eq!(b["password"].as_str(), Some("topsecret"));
    }

    #[test]
    fn ota_source_and_net_info() {
        let t = apply_changes(BASE, &json!({"ota": {"channel": "custom", "repo": "https://github.com/sebastianmadl/bost-flowstation", "branch": "fleet-control"}, "net_info": {"mcc": 232, "mnc": 77}})).unwrap();
        assert_eq!(ota_channel::dashboard_value(&t, "ota_channel").as_deref(), Some("custom"));
        assert_eq!(ota_channel::dashboard_value(&t, "ota_branch").as_deref(), Some("fleet-control"));
        let cfg = tetra_config::bluestation::parsing::from_toml_str(&t).unwrap();
        assert_eq!((cfg.net.mcc, cfg.net.mnc), (232, 77));
        assert!(apply_changes(BASE, &json!({"ota": {"channel": "custom", "repo": "ftp://x", "branch": "b"}})).is_err());
    }

    #[test]
    fn telemetry_through_the_page_keys() {
        let t = apply_changes(BASE, &json!({"telemetry": {"enabled": true, "host": "t.example.org", "port": 9001, "use_tls": true, "username": "u", "password": "p", "site_name": "Linz"}})).unwrap();
        assert!(t.contains("site_name = \"Linz\""), "{t}");
    }

    fn now() -> StationNow {
        StationNow { enabled: true, calls: 0, updating: false }
    }

    #[test]
    fn set_config_is_refused_on_another_base() {
        let change = json!({"cells": [{"id": 0, "location_area": 2}]});
        let (c, _) = plan_set_config(BASE, "0000000000000000", &change, false, now(), false).unwrap_err();
        assert_eq!(c, code::CONFLICT);
        assert!(plan_set_config(BASE, &hash_text(BASE), &change, false, now(), false).is_ok());
    }

    #[test]
    fn set_config_waits_for_calls_updates_and_a_pending_change() {
        let change = json!({"cells": [{"id": 0, "location_area": 2}]});
        let h = hash_text(BASE);
        let busy_call = StationNow { calls: 1, ..now() };
        assert_eq!(plan_set_config(BASE, &h, &change, false, busy_call, false).unwrap_err().0, "call_active");
        assert!(plan_set_config(BASE, &h, &change, true, busy_call, false).is_ok(), "during_call");
        assert_eq!(plan_set_config(BASE, &h, &change, false, StationNow { updating: true, ..now() }, false).unwrap_err().0, "busy");
        assert_eq!(plan_set_config(BASE, &h, &change, false, now(), true).unwrap_err().0, "busy");
        assert_eq!(plan_set_config(BASE, &h, &json!({}), false, now(), false).unwrap_err().0, code::INVALID, "nothing to change");
        assert_eq!(plan_set_config(BASE, &h, &json!({"cells": [{"id": 0, "location_area": 99999}]}), false, now(), false).unwrap_err().0, code::INVALID);
    }

    #[test]
    fn backup_restore_round_trip() {
        let dir = std::env::temp_dir().join(format!("fleet-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml").to_string_lossy().to_string();
        std::fs::write(&cfg, BASE).unwrap();
        let new = apply_changes(BASE, &json!({"cells": [{"id": 0, "location_area": 3}]})).unwrap();
        let p = PendingFile { old_hash: hash_text(BASE), new_hash: hash_text(&new), confirm_secs: 180, by: "admin".into(), written_unix: 1000 };
        write_with_backup(&cfg, BASE, &new, &p).unwrap();
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), new);
        assert_eq!(read_pending(&cfg), Some(p.clone()));
        restore_files(&cfg).unwrap();
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), BASE, "old file back");
        assert!(read_pending(&cfg).is_none() && !std::path::Path::new(&backup_path(&cfg)).exists(), "notes removed");
        assert!(restore_files(&cfg).is_err(), "nothing left to restore");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raw_patches_change_single_values_and_never_secrets() {
        let base = format!("{BASE}\n[security]\nissi_whitelist = [1001]\n");
        let text = apply_changes(&base, &json!({"raw": [
            {"path": ["cell_info", "home_mode_display", "text"], "value": "OE4XXX"},
            {"path": ["cell_info", "home_mode_display", "interval_multiframes"], "value": 60},
            {"path": ["security", "issi_whitelist"], "value": [1001, 1002]},
            {"path": ["security", "whitelist_mode"], "value": "enforce"},
        ]})).unwrap();
        let t: Table = toml::from_str(&text).unwrap();
        assert_eq!(t["cell_info"]["home_mode_display"]["text"].as_str(), Some("OE4XXX"));
        assert_eq!(t["cell_info"]["home_mode_display"]["interval_multiframes"].as_integer(), Some(60));
        assert_eq!(t["security"]["issi_whitelist"].as_array().unwrap().len(), 2);
        assert_eq!(t["brew"]["password"].as_str(), Some("topsecret"), "untouched values stay");
        let removed = apply_changes(&text, &json!({"raw": [{"path": ["cell_info", "home_mode_display"], "remove": true}]})).unwrap();
        assert!(!removed.contains("home_mode_display"));
        for bad in [
            json!({"raw": [{"path": ["brew", "password"], "value": "x"}]}),
            json!({"raw": [{"path": ["security", "sck"], "value": "00"}]}),
            json!({"raw": [{"path": ["brew", "host"], "value": "***"}]}),
            json!({"raw": [{"path": ["cell_info", "main_carrier", "x"], "value": 1}]}),
        ] {
            assert!(apply_changes(&base, &bad).is_err(), "{bad}");
        }
        assert!(apply_changes(&base, &json!({"raw": [{"path": ["cell_info", "colour_code"], "value": "red"}]})).is_err(), "the result is validated");
    }

    #[test]
    fn config_json_shows_home_mode_display_and_whitelist() {
        let dir = std::env::temp_dir().join(format!("fleet-cfg-hmd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, format!("{BASE}\n[cell_info.home_mode_display]\ntext = \"Hello\"\n\n[security]\nissi_whitelist = [5]\nwhitelist_mode = \"enforce\"\n")).unwrap();
        let v = config_json(path.to_str().unwrap(), None).unwrap();
        assert_eq!(v["home_mode_display"]["text"], "Hello");
        assert_eq!(v["access"], json!({"issi_whitelist": [5], "whitelist_mode": "enforce"}));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_run_that_wrote_the_change_does_not_confirm_it() {
        // The hello re-announced before the restart says "pending"; a confirmation arriving then
        // must not end the window, or the new config would run without the rollback.
        WRITTEN_HERE.store(true, std::sync::atomic::Ordering::SeqCst);
        let r = confirm();
        WRITTEN_HERE.store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(r, Err(code::NOT_RESTARTED));
    }

    #[test]
    fn window_runs_from_the_restart() {
        let p = PendingFile { old_hash: String::new(), new_hash: String::new(), confirm_secs: 180, by: String::new(), written_unix: 1000 };
        assert_eq!(window_left(&p, 1000), 183);
        assert_eq!(window_left(&p, 1100), 83);
        assert_eq!(window_left(&p, 5000), 0);
    }

    #[test]
    fn config_doc_is_plain_json_on_the_wire() {
        let cmd = ControlCommand::SetConfig { handle: 1, base_hash: "ab".into(), changes: JsonDoc::from_value(&json!({"migration": true})), confirm_secs: 0, during_call: false, by: "x".into() };
        let wire = serde_json::to_value(&cmd).unwrap();
        assert_eq!(wire["SetConfig"]["changes"]["migration"], json!(true), "{wire}");
        let back: ControlCommand = serde_json::from_value(wire).unwrap();
        assert!(matches!(back, ControlCommand::SetConfig { ref changes, .. } if changes.value()["migration"] == json!(true)));
    }
}
