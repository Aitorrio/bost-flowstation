//! First-run / Setup wizard persistence and privileged helper invocations.
//!
//! `setup.json` lives next to `config.toml`. The dashboard never runs free-form
//! shell — driver install and systemd ensure go through `bost-setup-helper.sh`.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const HELPER_CANDIDATES: &[&str] = &[
    "/usr/local/sbin/bost-setup-helper.sh",
    "/usr/local/bin/bost-setup-helper.sh",
    "/opt/bost-flowstation/contrib/install/bost-setup-helper.sh",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupState {
    pub setup_complete: bool,
    #[serde(default)]
    pub skipped: bool,
    #[serde(default)]
    pub completed_at: Option<u64>,
    #[serde(default)]
    pub skipped_at: Option<u64>,
    #[serde(default)]
    pub version: u32,
}

impl Default for SetupState {
    fn default() -> Self {
        Self {
            setup_complete: false,
            skipped: false,
            completed_at: None,
            skipped_at: None,
            version: 1,
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn setup_json_path(config_path: &str) -> PathBuf {
    Path::new(config_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("setup.json")
}

pub fn read_setup_state(config_path: &str) -> SetupState {
    let path = setup_json_path(config_path);
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => SetupState::default(),
    }
}

pub fn write_setup_state(config_path: &str, state: &SetupState) -> Result<(), String> {
    let path = setup_json_path(config_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    fs::write(&path, body + "\n").map_err(|e| e.to_string())
}

pub fn mark_complete(config_path: &str, skipped: bool) -> Result<SetupState, String> {
    let mut state = read_setup_state(config_path);
    state.setup_complete = true;
    state.skipped = skipped;
    let ts = now_unix();
    if skipped {
        state.skipped_at = Some(ts);
    } else {
        state.completed_at = Some(ts);
    }
    write_setup_state(config_path, &state)?;
    Ok(state)
}

/// Parse `SoapySDRUtil --find` into a JSON-friendly device list.
pub fn scan_sdr_devices() -> JsonValue {
    let candidates = ["SoapySDRUtil", "/usr/bin/SoapySDRUtil", "/usr/local/bin/SoapySDRUtil"];
    for bin in candidates {
        let out = Command::new(bin).arg("--find").output();
        let Ok(out) = out else { continue };
        if !out.status.success() && out.stdout.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let devices = parse_soapy_find(&text);
        return serde_json::json!({
            "ok": true,
            "tool": bin,
            "raw": text.trim(),
            "devices": devices,
        });
    }
    serde_json::json!({
        "ok": false,
        "error": "SoapySDRUtil not found (install soapysdr-tools)",
        "devices": [],
    })
}

fn parse_soapy_find(text: &str) -> Vec<JsonValue> {
    let mut devices = Vec::new();
    let mut current: Option<serde_json::Map<String, JsonValue>> = None;

    fn refresh_device(map: &mut serde_json::Map<String, JsonValue>) {
        let args: Vec<String> = map
            .iter()
            .filter(|(k, _)| *k != "device")
            .filter_map(|(k, v)| v.as_str().map(|s| format!("{k}={s}")))
            .collect();
        if !args.is_empty() {
            map.insert("device".into(), JsonValue::String(args.join(",")));
        }
    }

    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();
        if lower.contains("found device") {
            if let Some(dev) = current.take() {
                devices.push(JsonValue::Object(dev));
            }
            let mut map = serde_json::Map::new();
            let rest = if let Some(idx) = lower.find("found device") {
                trimmed[idx + "found device".len()..].trim_start_matches([' ', ':'])
            } else {
                ""
            };
            for part in rest.split(',') {
                let part = part.trim();
                if let Some((k, v)) = part.split_once('=') {
                    map.insert(k.trim().to_string(), JsonValue::String(v.trim().to_string()));
                }
            }
            refresh_device(&mut map);
            current = Some(map);
            continue;
        }
        if let Some(ref mut map) = current {
            if let Some((k, v)) = trimmed.split_once('=') {
                let key = k.trim().trim_matches(|c| c == '"' || c == '\'');
                let val = v.trim().trim_matches(|c| c == '"' || c == '\'' || c == ',');
                if !key.is_empty() {
                    map.insert(key.to_string(), JsonValue::String(val.to_string()));
                    refresh_device(map);
                }
            }
        }
    }
    if let Some(dev) = current {
        devices.push(JsonValue::Object(dev));
    }
    devices
}

fn find_helper() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BOST_SETUP_HELPER") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    HELPER_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

fn run_helper(args: &[&str]) -> Result<String, String> {
    let helper = find_helper().ok_or_else(|| {
        "bost-setup-helper.sh not installed (re-run contrib/install/install-bost.sh)".to_string()
    })?;

    // Prefer passwordless sudo (sudoers drop-in). If sudo fails (e.g. service runs as
    // root but sudoers only lists the build user), fall back to direct exec.
    let sudo_out = Command::new("sudo").arg("-n").arg(&helper).args(args).output();
    let output = match sudo_out {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let sudo_err = format!(
                "sudo: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            match Command::new(&helper).args(args).output() {
                Ok(direct) if direct.status.success() => direct,
                Ok(direct) => {
                    let direct_err = String::from_utf8_lossy(&direct.stderr).trim().to_string();
                    return Err(if !direct_err.is_empty() {
                        format!("{sudo_err}; direct: {direct_err}")
                    } else {
                        sudo_err
                    });
                }
                Err(e) => return Err(format!("{sudo_err}; direct exec failed: {e}")),
            }
        }
        Err(_) => Command::new(&helper)
            .args(args)
            .output()
            .map_err(|e| format!("failed to run helper: {e}"))?,
    };

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        Ok(if stdout.is_empty() { stderr } else { stdout })
    } else {
        let msg = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("helper exited {}", output.status)
        };
        Err(msg)
    }
}

pub fn install_driver(driver: &str) -> Result<String, String> {
    let driver = driver.trim().to_ascii_lowercase();
    match driver.as_str() {
        "sx" | "lime" | "mucell" | "uhd" | "pluto" => run_helper(&["install-driver", &driver]),
        _ => Err("unsupported driver (use sx, lime, mucell, uhd or pluto)".into()),
    }
}

fn systemd_unit_state() -> (String, String, String) {
    let unit = crate::service_control::resolve_service_unit();
    let enabled = Command::new("systemctl")
        .args(["is-enabled", &unit])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let active = Command::new("systemctl")
        .args(["is-active", &unit])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    (unit, enabled, active)
}

pub fn systemd_action(action: &str) -> Result<String, String> {
    let action = action.trim().to_ascii_lowercase();
    match action.as_str() {
        "enable" | "enable-service" => {
            let (unit, enabled, active) = systemd_unit_state();
            if enabled == "enabled" {
                return Ok(format!(
                    "already enabled ({unit}; active={active})"
                ));
            }
            // Try direct systemctl first (service usually runs as root).
            let direct = Command::new("systemctl").args(["enable", &unit]).output();
            if let Ok(out) = direct {
                if out.status.success() {
                    let (_, enabled2, active2) = systemd_unit_state();
                    return Ok(format!(
                        "enabled {unit} (enabled={enabled2}, active={active2})"
                    ));
                }
            }
            run_helper(&["enable-service"])
        }
        "restart" | "restart-service" => {
            // Prefer in-process lifecycle restart (same as dashboard Restart button).
            crate::service_control::schedule_service_action(
                crate::service_control::ServiceAction::Restart,
                std::time::Duration::from_secs(1),
            );
            Ok("restart scheduled".into())
        }
        "status" => {
            let (unit, enabled, active) = systemd_unit_state();
            Ok(serde_json::json!({
                "unit": unit,
                "enabled": enabled,
                "active": active,
            })
            .to_string())
        }
        _ => Err("unsupported systemd action (enable|restart|status)".into()),
    }
}

#[derive(Debug, Deserialize)]
pub struct SetupApplyRequest {
    /// Optional visual-config style body (phy_io / net_info / cell_info / brew).
    #[serde(default)]
    pub visual: Option<JsonValue>,
    /// Soapy device args, e.g. `driver=sx` or `driver=lime,serial=…`.
    #[serde(default)]
    pub device: Option<String>,
    /// When true, set `phy_io.backend = "SoapySdr"`.
    #[serde(default)]
    pub enable_rf: bool,
    /// Schedule a controlled restart after writing.
    #[serde(default)]
    pub restart: bool,
}

/// Merge setup choices into config.toml (validated).
pub fn apply_setup(config_path: &str, req: &SetupApplyRequest) -> Result<(), String> {
    if let Some(ref visual) = req.visual {
        crate::net_dashboard::profiles::write_visual_config(config_path, visual)?;
    }

    if req.enable_rf || req.device.as_ref().is_some_and(|d| !d.trim().is_empty()) {
        let current = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
        let mut table: toml::Table = toml::from_str(&current).map_err(|e| format!("parse config: {e}"))?;

        let phy = table
            .entry("phy_io".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let phy_tbl = phy
            .as_table_mut()
            .ok_or_else(|| "phy_io must be a table".to_string())?;

        if req.enable_rf {
            phy_tbl.insert("backend".into(), toml::Value::String("SoapySdr".into()));
        }

        if let Some(ref device) = req.device {
            let device = device.trim();
            if !device.is_empty() {
                let soap = phy_tbl
                    .entry("soapysdr".to_string())
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()));
                let soap_tbl = soap
                    .as_table_mut()
                    .ok_or_else(|| "phy_io.soapysdr must be a table".to_string())?;
                soap_tbl.insert("device".into(), toml::Value::String(device.to_string()));
            }
        }

        let rendered = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
        // Reuse the same validate+write path as the raw config editor.
        crate::net_dashboard::profiles::validate_and_write_toml(config_path, &rendered)?;
    }

    if req.restart {
        crate::service_control::schedule_service_action(
            crate::service_control::ServiceAction::Restart,
            std::time::Duration::from_secs(1),
        );
    }
    Ok(())
}

/// The station default language (`[dashboard] language`), read from the config file so a change
/// from the Setup wizard applies to the next page load without a restart.
pub fn station_language(config_path: &str) -> Option<String> {
    let text = fs::read_to_string(config_path).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    let lang = table.get("dashboard")?.get("language")?.as_str()?.trim();
    tetra_config::bluestation::DASHBOARD_LANGUAGES.contains(&lang).then(|| lang.to_string())
}

/// The page with `{{STATION_LANG}}` filled in (empty when no station language is set).
pub fn with_station_language(page: &str, config_path: &str) -> String {
    page.replacen("{{STATION_LANG}}", station_language(config_path).as_deref().unwrap_or(""), 1)
}

/// Store the station default language (`[dashboard] language`). No restart needed.
pub fn set_station_language(config_path: &str, language: &str) -> Result<String, String> {
    let lang = language.trim().to_ascii_lowercase();
    if !tetra_config::bluestation::DASHBOARD_LANGUAGES.contains(&lang.as_str()) {
        return Err(format!("unknown language {lang:?}"));
    }
    let current = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    let mut table: toml::Table = toml::from_str(&current).map_err(|e| format!("parse config: {e}"))?;
    let dash = table
        .entry("dashboard".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| "dashboard must be a table".to_string())?;
    if dash.get("language").and_then(|v| v.as_str()) == Some(lang.as_str()) {
        return Ok(lang);
    }
    dash.insert("language".into(), toml::Value::String(lang.clone()));
    let rendered = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
    crate::net_dashboard::profiles::validate_and_write_toml(config_path, &rendered)?;
    Ok(lang)
}

/// Setup wizard "Frequency" step: write TX/RX frequency (`[phy_io.soapysdr]`) and band, carrier,
/// offset, duplex, reverse operation, colour code and location area (`[cell_info]`). The whole
/// config is validated before it is written; nothing is restarted.
pub fn set_station_frequency(config_path: &str, v: &JsonValue) -> Result<(), String> {
    let int = |k: &str| v.get(k).and_then(|x| x.as_i64()).ok_or_else(|| format!("{k} missing or not a number"));
    let ranged = |k: &str, lo: i64, hi: i64| -> Result<i64, String> {
        let n = int(k)?;
        if (lo..=hi).contains(&n) { Ok(n) } else { Err(format!("{k} must be {lo}..{hi}")) }
    };
    let tx = ranged("tx_freq", 100_000_000, 1_000_000_000)?;
    let rx = ranged("rx_freq", 100_000_000, 1_000_000_000)?;
    let band = ranged("freq_band", 1, 15)?;
    let carrier = ranged("main_carrier", 0, 3999)?;
    let offset = int("freq_offset")?;
    if ![0, 6250, -6250, 12500].contains(&offset) {
        return Err("freq_offset must be 0, 6250, -6250 or 12500".into());
    }
    let duplex_id = ranged("duplex_spacing", 0, 7)?;
    let custom = match v.get("custom_duplex_spacing") {
        None | Some(JsonValue::Null) => None,
        Some(x) => Some(x.as_i64().filter(|n| (25_000..=100_000_000).contains(n))
            .ok_or_else(|| "custom_duplex_spacing must be 25000..100000000 Hz".to_string())?),
    };
    let reverse = v.get("reverse_operation").and_then(|x| x.as_bool()).unwrap_or(false);
    let colour = ranged("colour_code", 0, 63)?;
    let la = ranged("location_area", 1, 16383)?;

    let current = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    let mut table: toml::Table = toml::from_str(&current).map_err(|e| format!("parse config: {e}"))?;
    fn sub<'a>(t: &'a mut toml::Table, k: &str) -> Result<&'a mut toml::Table, String> {
        t.entry(k.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| format!("{k} must be a table"))
    }
    let soapy = sub(sub(&mut table, "phy_io")?, "soapysdr")?;
    soapy.insert("tx_freq".into(), toml::Value::Integer(tx));
    soapy.insert("rx_freq".into(), toml::Value::Integer(rx));
    let cell = sub(&mut table, "cell_info")?;
    for (k, n) in [
        ("freq_band", band),
        ("main_carrier", carrier),
        ("freq_offset", offset),
        ("duplex_spacing", duplex_id),
        ("colour_code", colour),
        ("location_area", la),
    ] {
        cell.insert(k.into(), toml::Value::Integer(n));
    }
    cell.insert("reverse_operation".into(), toml::Value::Boolean(reverse));
    match custom {
        Some(hz) => {
            cell.insert("custom_duplex_spacing".into(), toml::Value::Integer(hz));
        }
        None => {
            cell.remove("custom_duplex_spacing");
        }
    }
    let rendered = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
    crate::net_dashboard::profiles::validate_and_write_toml(config_path, &rendered)
}

pub fn status_payload(config_path: &str) -> JsonValue {
    let setup = read_setup_state(config_path);
    let rf = crate::rf_status::get();
    let scan = scan_sdr_devices();
    let unit = crate::service_control::resolve_service_unit();
    let helper = find_helper().map(|p| p.to_string_lossy().to_string());

    let backend = fs::read_to_string(config_path)
        .ok()
        .and_then(|s| toml::from_str::<toml::Table>(&s).ok())
        .and_then(|t| {
            t.get("phy_io")
                .and_then(|p| p.get("backend"))
                .and_then(|b| b.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".into());

    let device = fs::read_to_string(config_path)
        .ok()
        .and_then(|s| toml::from_str::<toml::Table>(&s).ok())
        .and_then(|t| {
            t.get("phy_io")
                .and_then(|p| p.get("soapysdr"))
                .and_then(|s| s.get("device"))
                .and_then(|d| d.as_str())
                .map(|s| s.to_string())
        });

    serde_json::json!({
        "setup": setup,
        "rf_status": rf,
        "config_backend": backend,
        "config_device": device,
        "devices": scan.get("devices").cloned().unwrap_or(JsonValue::Array(vec![])),
        "scan_ok": scan.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "scan_error": scan.get("error"),
        "service_unit": unit,
        "helper_path": helper,
        "setup_json": setup_json_path(config_path).to_string_lossy(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
config_version = "0.6"
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
"#;

    fn temp_config(text: &str) -> std::path::PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("fs-lang-test-{n}.toml"));
        fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn wizard_frequency_is_written_and_loads() {
        let p = temp_config(CONFIG);
        let path = p.to_str().unwrap();
        let body = serde_json::json!({
            "tx_freq": 438_200_000u64, "rx_freq": 430_600_000u64, "freq_band": 4, "main_carrier": 1528,
            "freq_offset": 0, "duplex_spacing": 7, "custom_duplex_spacing": 7_600_000,
            "reverse_operation": false, "colour_code": 5, "location_area": 12,
        });
        set_station_frequency(path, &body).unwrap();
        let cfg = tetra_config::bluestation::from_toml_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(cfg.cell.main_carrier, 1528);
        assert_eq!(cfg.cell.duplex_spacing_id, 7);
        assert_eq!(cfg.cell.custom_duplex_spacing, Some(7_600_000));
        assert_eq!((cfg.cell.colour_code, cfg.cell.location_area), (5, 12));
        let raw: toml::Table = toml::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(raw["phy_io"]["soapysdr"]["tx_freq"].as_integer(), Some(438_200_000));
        assert_eq!(raw["phy_io"]["backend"].as_str(), Some("None"), "RF stays off until the wizard enables it");

        // Table duplex again: the custom value is removed.
        let mut table_dup = body.clone();
        table_dup["duplex_spacing"] = 4.into();
        table_dup["custom_duplex_spacing"] = serde_json::Value::Null;
        table_dup["rx_freq"] = 433_200_000u64.into();
        set_station_frequency(path, &table_dup).unwrap();
        let cfg = tetra_config::bluestation::from_toml_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(cfg.cell.custom_duplex_spacing, None);

        for (k, v) in [("colour_code", 64), ("location_area", 0), ("freq_offset", 1000), ("main_carrier", 4000)] {
            let mut bad = body.clone();
            bad[k] = v.into();
            assert!(set_station_frequency(path, &bad).is_err(), "{k} = {v}");
        }
        let _ = fs::remove_file(&p);
        let _ = fs::remove_file(format!("{path}.bak"));
    }

    #[test]
    fn station_language_is_stored_and_filled_into_the_pages() {
        let p = temp_config(CONFIG);
        let path = p.to_str().unwrap();
        assert_eq!(station_language(path), None);
        assert_eq!(with_station_language("const L='{{STATION_LANG}}';", path), "const L='';");

        assert_eq!(set_station_language(path, " DE ").unwrap(), "de");
        assert_eq!(station_language(path).as_deref(), Some("de"));
        assert_eq!(with_station_language("const L='{{STATION_LANG}}';", path), "const L='de';");
        // The config still loads and carries it.
        let cfg = tetra_config::bluestation::from_toml_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(cfg.dashboard.and_then(|d| d.language).as_deref(), Some("de"));

        assert!(set_station_language(path, "xx").is_err());
        assert_eq!(station_language(path).as_deref(), Some("de"), "a refused language changes nothing");
        let _ = fs::remove_file(&p);
        let _ = fs::remove_file(format!("{path}.bak"));
    }

    #[test]
    fn both_pages_carry_the_station_language_placeholder() {
        assert!(crate::net_dashboard::html::DASHBOARD_HTML.contains("'{{STATION_LANG}}'"));
        assert!(crate::net_dashboard::html::LOGIN_HTML.contains("'{{STATION_LANG}}'"));
    }
}
