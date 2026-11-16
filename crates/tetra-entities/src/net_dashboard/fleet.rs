//! Dashboard side of Fleet Control (see `net_control::fleet` and `Docs/fleet-control.md`).
//!
//! The switch sits under the Brew line on the Infrastructure page (above the collapsible Server
//! features list, so it is always visible), the status field in the sidebar
//! (under Health). It works at once: the config gets `[command] fleet_control`, the running
//! station's state flag changes and the control worker sends a new hello. Only when the control
//! channel is not configured yet the dashboard asks for it in a small dialog (server, port, TLS,
//! user, password); saving that writes `[command]` and restarts the service, because the control
//! channel is started at boot.

use serde_json::{Value as JsonValue, json};
use tetra_config::bluestation::{SharedConfig, StackConfig, parsing};
use toml::{Table, Value as TomlValue};

use crate::net_control::fleet::EXT_FLEET_CONTROL;

/// State of `[command]` in the config file.
fn command_state(table: &Table) -> &'static str {
    match table.get("command").and_then(|v| v.as_table()) {
        None => "missing",
        Some(t) if t.get("enabled").and_then(|v| v.as_bool()) == Some(false) => "off",
        Some(t) if t.get("host").and_then(|v| v.as_str()).is_none_or(|h| h.trim().is_empty()) => "missing",
        Some(_) => "on",
    }
}

/// Fleet Control as the dashboard shows it (`/api/fleet-control`, `fleet_control` in
/// `/api/roaming`). `offered`: the Brew link or the control channel offers it; the switch is shown
/// only then. `enabled`: the switch as the running station has it. `command`: `on`, `off`
/// (`enabled = false`) or `missing`. `brew_login` pre-fills the user field of the dialog (the
/// server address is never pre-filled).
pub fn status_json(table: &Table, cfg: &StackConfig, shared: Option<&SharedConfig>) -> JsonValue {
    let (fc, brew_offered) = match shared {
        Some(s) => {
            let st = s.state_read();
            (st.fleet_control.clone(), st.brew_server_extensions.iter().any(|e| e == EXT_FLEET_CONTROL))
        }
        None => (Default::default(), false),
    };
    let configured = table.get("command").and_then(|v| v.as_table()).and_then(|t| t.get("fleet_control")).and_then(|v| v.as_bool()).unwrap_or(false);
    json!({
        "offered": fc.offered || brew_offered,
        "enabled": if shared.is_some() { fc.enabled } else { configured },
        "connected": fc.connected,
        "command": command_state(table),
        "brew_login": cfg.brew.as_ref().and_then(|b| b.username.clone()),
        "maintenance": banner_maintenance(fc.enabled && shared.is_some(), crate::net_control::fleet::maintenance(), std::time::Instant::now()),
        "live_update": crate::net_dashboard::ota_remote::live_running().map(|p| json!({"stage": p.stage.as_str(), "percent": p.percent})),
    })
}

/// GET /api/fleet-banner: only what the red maintenance banner needs. This route is open to
/// visitors without a session (login page, public overview), so it carries nothing else: no
/// server address, no login, no switch state.
pub fn banner_json(shared: Option<&SharedConfig>) -> JsonValue {
    let enabled = shared.is_some_and(|s| s.state_read().fleet_control.enabled);
    json!({
        "maintenance": banner_maintenance(enabled, crate::net_control::fleet::maintenance(), std::time::Instant::now()),
        "live_update": crate::net_dashboard::ota_remote::live_running().map(|p| json!({"stage": p.stage.as_str(), "percent": p.percent})),
    })
}

/// `maintenance` for the dashboard banner: `{by, reason, left_secs}` while the server's flag is
/// on (and Fleet Control is on), else null.
pub fn banner_maintenance(enabled: bool, m: Option<crate::net_control::fleet::Maintenance>, now: std::time::Instant) -> JsonValue {
    match m {
        Some(m) if enabled => json!({"by": m.by, "reason": m.reason, "left_secs": m.until.saturating_duration_since(now).as_secs()}),
        _ => JsonValue::Null,
    }
}

/// GET /api/fleet-control payload.
pub fn status_from_file(config_path: &str, shared: Option<&SharedConfig>) -> Result<JsonValue, String> {
    let text = std::fs::read_to_string(config_path).map_err(|e| format!("cannot read config: {e}"))?;
    let table: Table = toml::from_str(&text).map_err(|e| format!("config does not parse: {e}"))?;
    let cfg = parsing::from_toml_str(&text).map_err(|e| format!("config does not parse: {e}"))?;
    Ok(status_json(&table, &cfg, shared))
}

/// What a switch request does.
#[derive(Debug, PartialEq)]
pub enum FleetChange {
    /// The control channel is not configured: the dashboard opens the dialog. Nothing written.
    NeedCommand,
    /// Write this config. `restart`: the control channel was (re)configured, the service has to
    /// restart to connect it; otherwise the switch is applied to the running station at once.
    Write { text: String, enabled: bool, restart: bool },
}

/// POST /api/fleet-control: `{"enabled": bool, "command": {host, port, use_tls, username,
/// password}?}`. `command` comes from the dialog; an empty password there takes the Brew password
/// (`brew_password`, never sent to the browser), as Brew and control logins are kept the same.
pub fn apply(original: &str, body: &JsonValue, brew_password: Option<&str>) -> Result<FleetChange, String> {
    let obj = body.as_object().ok_or_else(|| "body must be a JSON object".to_string())?;
    let enabled = obj.get("enabled").and_then(|v| v.as_bool()).ok_or_else(|| "enabled must be true or false".to_string())?;
    let mut table: Table = toml::from_str(original).map_err(|e| format!("config does not parse: {e}"))?;
    let mut restart = false;
    match obj.get("command").filter(|v| !v.is_null()) {
        Some(cmd) => {
            let mut cmd = cmd.as_object().ok_or_else(|| "command must be an object".to_string())?.clone();
            cmd.insert("enabled".into(), json!(true));
            let typed = cmd.get("password").and_then(|v| v.as_str()).is_some_and(|p| !p.is_empty());
            let stored = table
                .get("command")
                .and_then(|v| v.as_table())
                .and_then(|t| t.get("password"))
                .and_then(|v| v.as_str())
                .is_some_and(|p| !p.is_empty());
            let user = cmd.get("username").and_then(|v| v.as_str()).is_some_and(|u| !u.trim().is_empty());
            if user && !typed && !stored {
                match brew_password.filter(|p| !p.is_empty()) {
                    Some(p) => {
                        cmd.insert("password".into(), json!(p));
                    }
                    None => return Err("command.password: no Brew password to take over, please type one".into()),
                }
            }
            crate::net_dashboard::roaming::apply_link_section(&mut table, "command", &JsonValue::Object(cmd))?;
            restart = true;
        }
        None if enabled && command_state(&table) != "on" => return Ok(FleetChange::NeedCommand),
        None => {}
    }
    match table.get_mut("command").and_then(|v| v.as_table_mut()) {
        Some(section) if enabled => {
            section.insert("fleet_control".into(), TomlValue::Boolean(true));
        }
        Some(section) => {
            section.remove("fleet_control");
        }
        None => {} // switching off without a control channel: nothing to keep
    }
    let text = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
    parsing::from_toml_str(&text).map_err(|e| format!("resulting config does not parse: {e}"))?;
    Ok(FleetChange::Write { text, enabled, restart })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = include_str!("../../../../example_config/config.toml");

    fn with_command(extra: &str) -> String {
        format!("{BASE}\n[command]\nhost = \"brew.example\"\nport = 443\nuse_tls = true\nusername = \"1000001\"\npassword = \"pw\"\n{extra}")
    }

    fn written(c: FleetChange) -> (Table, bool, bool) {
        match c {
            FleetChange::Write { text, enabled, restart } => (toml::from_str(&text).unwrap(), enabled, restart),
            other => panic!("expected a write, got {other:?}"),
        }
    }

    #[test]
    fn switch_on_without_control_channel_asks_for_it() {
        let base = BASE.replace("[command]", "[command_unused]");
        assert_eq!(apply(&base, &json!({"enabled": true}), Some("x")).unwrap(), FleetChange::NeedCommand);
        let off = with_command("enabled = false\n");
        assert_eq!(apply(&off, &json!({"enabled": true}), Some("x")).unwrap(), FleetChange::NeedCommand);
    }

    #[test]
    fn switch_works_without_restart_when_the_channel_exists() {
        let (t, enabled, restart) = written(apply(&with_command(""), &json!({"enabled": true}), None).unwrap());
        assert!(enabled && !restart);
        assert_eq!(t["command"]["fleet_control"].as_bool(), Some(true));
        let on = with_command("fleet_control = true\n");
        let (t, enabled, restart) = written(apply(&on, &json!({"enabled": false}), None).unwrap());
        assert!(!enabled && !restart);
        assert!(t["command"].get("fleet_control").is_none());
        assert_eq!(t["command"]["host"].as_str(), Some("brew.example"), "the channel stays");
    }

    #[test]
    fn dialog_writes_the_channel_and_takes_the_brew_password() {
        let base = BASE.replace("[command]", "[command_unused]");
        let body = json!({"enabled": true, "command": {"host": "fleet.example.org", "port": 443, "use_tls": true, "username": "1000001", "password": ""}});
        let (t, enabled, restart) = written(apply(&base, &body, Some("brewpw")).unwrap());
        assert!(enabled && restart);
        let c = t["command"].as_table().unwrap();
        assert_eq!(c["host"].as_str(), Some("fleet.example.org"));
        assert_eq!(c["port"].as_integer(), Some(443));
        assert_eq!(c["use_tls"].as_bool(), Some(true));
        assert_eq!(c["username"].as_str(), Some("1000001"));
        assert_eq!(c["password"].as_str(), Some("brewpw"));
        assert_eq!(c["fleet_control"].as_bool(), Some(true));
        assert!(c.get("enabled").is_none(), "the channel is switched on");
        // A typed password wins; no Brew password and none typed is refused.
        let mut typed = body.clone();
        typed["command"]["password"] = json!("own");
        let (t, ..) = written(apply(&base, &typed, Some("brewpw")).unwrap());
        assert_eq!(t["command"]["password"].as_str(), Some("own"));
        assert!(apply(&base, &body, None).is_err());
        // The server address is required.
        let mut nohost = body.clone();
        nohost["command"]["host"] = json!("");
        assert!(apply(&base, &nohost, Some("brewpw")).is_err());
    }

    #[test]
    fn status_reports_the_command_section() {
        let cfg = parsing::from_toml_str(&with_command("fleet_control = true\n")).unwrap();
        let table: Table = toml::from_str(&with_command("fleet_control = true\n")).unwrap();
        let s = status_json(&table, &cfg, None);
        assert_eq!(s["command"], "on");
        assert_eq!(s["enabled"], true);
        assert_eq!(s["offered"], false);
        let shared = SharedConfig::from_parts(cfg, None);
        assert!(shared.state_read().fleet_control.enabled, "the running switch starts from the config");
        shared.state_write().fleet_control.offered = true;
        assert_eq!(status_json(&table, &shared.config(), Some(&shared))["offered"], true);
    }

    #[test]
    fn banner_shows_maintenance_only_while_fleet_control_is_on() {
        let now = std::time::Instant::now();
        let m = crate::net_control::fleet::Maintenance { by: "admin".into(), reason: "carrier".into(), until: now + std::time::Duration::from_secs(90) };
        assert_eq!(banner_maintenance(true, Some(m.clone()), now), json!({"by": "admin", "reason": "carrier", "left_secs": 90}));
        assert_eq!(banner_maintenance(false, Some(m), now), JsonValue::Null, "switch off: no banner");
        assert_eq!(banner_maintenance(true, None, now), JsonValue::Null);
    }
}
