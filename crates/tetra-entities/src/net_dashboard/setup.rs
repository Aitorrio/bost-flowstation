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
        "sx" | "lime" => run_helper(&["install-driver", &driver]),
        _ => Err("unsupported driver (use sx or lime)".into()),
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
