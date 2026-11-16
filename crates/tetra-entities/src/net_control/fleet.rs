//! Fleet Control: station management by the Brew server (the SwMI) over the control channel.
//!
//! The station opts in with the local switch `[command] fleet_control` (default off, changed on
//! the dashboard's Infrastructure page at run time). The server offers the feature by sending
//! `X-Brew-Extensions: fleet-control` in the 101 answer of the control channel; a station that
//! sees no such header sends nothing new, so older servers never see these messages.
//!
//! Phase 1 (this file): [`ControlResponse::StationHello`] after every connect and on every
//! change of the switch, [`ControlResponse::CommandResult`] as the general answer, and the
//! round-trip command [`ControlCommand::FleetPing`]. The existing commands (kick, DGNA, SDS,
//! restart, ...) do not depend on the switch. Protocol: `Docs/fleet-control.md`.
//!
//! Phase 2 (`update-v1`): [`ControlCommand::CheckUpdate`] / [`ControlCommand::StartUpdate`] run
//! the dashboard's OTA from the station's own source (`net_dashboard::ota_remote`), answered with
//! [`ControlResponse::UpdateInfo`] / [`ControlResponse::CommandResult`]; every OTA run reports
//! [`ControlResponse::UpdateProgress`].
//!
//! Maintenance (`maintenance-v1`): [`ControlCommand::SetMaintenance`] switches a flag with an
//! expiry; while it is on, the dashboard shows a red banner on every page. A live update shows
//! its own banner without a flag. Restart (`restart-v1`): [`ControlCommand::RestartStation`]
//! restarts the service or reboots the host.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tetra_config::bluestation::SharedConfig;

use super::commands::{ControlCommand, ControlResponse};

/// Name in `X-Brew-Extensions` with which a server offers Fleet Control.
pub const EXT_FLEET_CONTROL: &str = "fleet-control";

/// Version of the Fleet Control messages ([`ControlResponse::StationHello`] `protocol`).
pub const FLEET_PROTOCOL: u32 = 1;

/// Capability of phase 1: `FleetPing`, `CommandResult`.
pub const CAP_FLEET: &str = "fleet-v1";
/// Capability of phase 2: `CheckUpdate`, `StartUpdate`, `UpdateInfo`, `UpdateProgress`. Listed only
/// while the station can run an OTA (its dashboard is running).
pub const CAP_UPDATE: &str = "update-v1";
/// Live update: `StartUpdate` with `source`, the station runs its installer with that repository
/// and branch and keeps the radio on until the service restart. Listed with `update-v1`.
pub const CAP_UPDATE_LIVE: &str = "update-live-v1";
/// Maintenance flag (`SetMaintenance`, banner on the dashboard).
pub const CAP_MAINTENANCE: &str = "maintenance-v1";
/// Restart of the service or reboot of the host (`RestartStation`).
pub const CAP_RESTART: &str = "restart-v1";

/// Capability names the station lists in its hello while the switch is on.
pub fn capabilities(ota_available: bool) -> Vec<String> {
    let mut caps = vec![CAP_FLEET.to_string(), CAP_MAINTENANCE.to_string(), CAP_RESTART.to_string(), super::metrics::CAP_METRICS.to_string()];
    if super::fleet_config::config_path().is_some() {
        caps.push(super::fleet_config::CAP_CONFIG.to_string());
    }
    if ota_available {
        caps.push(CAP_UPDATE.to_string());
        caps.push(CAP_UPDATE_LIVE.to_string());
    }
    caps
}

/// `code` values of [`ControlResponse::CommandResult`].
pub mod code {
    /// Done.
    pub const OK: &str = "ok";
    /// Refused: Fleet Control is switched off on this station.
    pub const FLEET_CONTROL_OFF: &str = "fleet_control_off";
    /// Refused: an update is already running.
    pub const BUSY: &str = "busy";
    /// Refused: a call is in progress (and `during_call` was not set).
    pub const CALL_ACTIVE: &str = "call_active";
    /// Refused: this station cannot do it (no dashboard, so no OTA).
    pub const UNAVAILABLE: &str = "unavailable";
    /// Refused: the live update source is not a plain https repository and branch name.
    pub const INVALID_SOURCE: &str = "invalid_source";
}

/// Does this server offer Fleet Control (its `X-Brew-Extensions`)?
pub fn offered(server_extensions: &[String]) -> bool {
    server_extensions.iter().any(|e| e == EXT_FLEET_CONTROL)
}

/// The hello for the current config and switch.
pub fn station_hello(shared: &SharedConfig) -> ControlResponse {
    let cfg = shared.config();
    let enabled = shared.state_read().fleet_control.enabled;
    ControlResponse::StationHello {
        protocol: FLEET_PROTOCOL,
        version: tetra_core::STACK_VERSION.to_string(),
        brew_login: cfg.brew.as_ref().and_then(|b| b.username.clone()),
        mcc: cfg.net.mcc,
        mnc: cfg.net.mnc,
        fleet_control: enabled,
        capabilities: if enabled { capabilities(crate::net_dashboard::ota_remote::available()) } else { Vec::new() },
        config_hash: if enabled { super::fleet_config::current_hash() } else { None },
        config_pending: if enabled { super::fleet_config::pending_secs() } else { None },
    }
}

/// The answer to a Fleet Control command the worker handles itself, or `None` for every other
/// command (those go to the stack as before). `enabled`: the local switch.
pub fn answer(command: &ControlCommand, enabled: bool) -> Option<ControlResponse> {
    match command {
        ControlCommand::FleetPing { handle } => Some(if enabled {
            result(*handle, true, code::OK, "Fleet Control is on")
        } else {
            off(*handle)
        }),
        ControlCommand::SetMaintenance { handle, on, minutes, by, reason } => Some(if enabled {
            let mut slot = MAINTENANCE.lock().unwrap_or_else(|e| e.into_inner());
            let text = apply_maintenance(&mut slot, *on, *minutes, by, reason, Instant::now());
            tracing::info!("Fleet Control: {text}");
            result(*handle, true, code::OK, &text)
        } else {
            off(*handle)
        }),
        _ => None,
    }
}

/// Longest maintenance time per `SetMaintenance`.
pub const MAX_MAINTENANCE_MIN: u32 = 240;
/// Maintenance time when `minutes` is 0.
pub const DEFAULT_MAINTENANCE_MIN: u32 = 10;

/// The maintenance flag set by the server.
#[derive(Debug, Clone, PartialEq)]
pub struct Maintenance {
    pub by: String,
    pub reason: String,
    pub until: Instant,
}

static MAINTENANCE: Mutex<Option<Maintenance>> = Mutex::new(None);

/// Switch the flag in `slot`; returns the text for the answer.
pub fn apply_maintenance(slot: &mut Option<Maintenance>, on: bool, minutes: u32, by: &str, reason: &str, now: Instant) -> String {
    if !on {
        *slot = None;
        return "Maintenance off".into();
    }
    let minutes = if minutes == 0 { DEFAULT_MAINTENANCE_MIN } else { minutes.min(MAX_MAINTENANCE_MIN) };
    let clip = |s: &str| s.trim().chars().take(120).collect::<String>();
    *slot = Some(Maintenance { by: clip(by), reason: clip(reason), until: now + Duration::from_secs(minutes as u64 * 60) });
    format!("Maintenance on for {minutes} min")
}

/// The flag in `slot` if it has not run out (a run-out flag is cleared).
pub fn current_maintenance(slot: &mut Option<Maintenance>, now: Instant) -> Option<Maintenance> {
    if slot.as_ref().is_some_and(|m| m.until <= now) {
        *slot = None;
    }
    slot.clone()
}

/// The station's maintenance flag now (`None`: off or run out).
pub fn maintenance() -> Option<Maintenance> {
    current_maintenance(&mut MAINTENANCE.lock().unwrap_or_else(|e| e.into_inner()), Instant::now())
}

/// Handle `RestartStation` (`None` for every other command). `calls`: calls in progress;
/// `updating`: an OTA runs; `restart(host)` schedules it (the real one: [`schedule_restart`]).
pub fn restart_command(command: &ControlCommand, enabled: bool, calls: usize, updating: bool, restart: impl FnOnce(bool)) -> Option<ControlResponse> {
    let ControlCommand::RestartStation { handle, host, during_call } = command else { return None };
    let handle = *handle;
    if !enabled {
        return Some(off(handle));
    }
    if updating {
        return Some(result(handle, false, code::BUSY, "An update is running"));
    }
    if calls > 0 && !during_call {
        return Some(result(handle, false, code::CALL_ACTIVE, "A call is in progress"));
    }
    restart(*host);
    Some(result(handle, true, code::OK, if *host { "Host reboot in 5 s" } else { "Service restart in 5 s" }))
}

/// Restart the service or reboot the host in 5 s (time for the answer to leave).
pub fn schedule_restart(host: bool) {
    tracing::warn!("Fleet Control: {} requested by the Brew server", if host { "host reboot" } else { "service restart" });
    if host {
        crate::service_control::schedule_host_reboot(Duration::from_secs(5));
    } else {
        crate::service_control::schedule_service_action(crate::service_control::ServiceAction::Restart, Duration::from_secs(5));
    }
}

/// What the worker does with a Fleet Control update command.
#[derive(Debug)]
pub enum UpdateAction {
    /// Answer at once with this.
    Reply(ControlResponse),
    /// Run the update check off the loop and answer `handle` with an `UpdateInfo` when done.
    Check { handle: u32, auth: Option<crate::net_dashboard::source_auth::SourceToken> },
}

/// Handle `CheckUpdate` / `StartUpdate` (`None` for every other command). `enabled`: the switch;
/// `ota_available`: the station can run an OTA; `start` starts it (the real one is
/// [`crate::net_dashboard::ota_remote::start`]).
pub fn update_command(
    command: &ControlCommand,
    enabled: bool,
    ota_available: bool,
    start: impl FnOnce(bool, Option<&crate::net_control::commands::UpdateSource>, Option<&crate::net_dashboard::source_auth::SourceToken>) -> Result<(), crate::net_dashboard::ota_remote::StartRefused>,
) -> Option<UpdateAction> {
    use crate::net_dashboard::ota_remote::StartRefused;
    let handle = match command {
        ControlCommand::CheckUpdate { handle, .. } | ControlCommand::StartUpdate { handle, .. } => *handle,
        _ => return None,
    };
    if !enabled {
        return Some(UpdateAction::Reply(off(handle)));
    }
    if !ota_available {
        return Some(UpdateAction::Reply(result(handle, false, code::UNAVAILABLE, "No OTA on this station (dashboard not running)")));
    }
    Some(match command {
        ControlCommand::CheckUpdate { auth, .. } => UpdateAction::Check { handle, auth: auth.clone() },
        ControlCommand::StartUpdate { during_call, source, auth, .. } => UpdateAction::Reply(match start(*during_call, source.as_ref(), auth.as_ref()) {
            Ok(()) => result(handle, true, code::OK, if source.is_some() { "Live update started" } else { "Update started" }),
            Err(StartRefused::InvalidSource) => result(handle, false, code::INVALID_SOURCE, "Live update: only a plain https:// repository and branch name"),
            Err(StartRefused::Busy) => result(handle, false, code::BUSY, "An update is already running"),
            Err(StartRefused::CallActive) => result(handle, false, code::CALL_ACTIVE, "A call is in progress"),
            Err(StartRefused::Unavailable) => result(handle, false, code::UNAVAILABLE, "No OTA on this station"),
        }),
        _ => unreachable!("matched above"),
    })
}

/// `UpdateInfo` for a finished check.
pub fn update_info(handle: u32, c: crate::net_dashboard::ota_remote::CheckOutcome) -> ControlResponse {
    ControlResponse::UpdateInfo {
        handle,
        ok: c.ok,
        current: c.current,
        latest: c.latest,
        update_available: c.update_available,
        channel: c.channel,
        repo: c.repo,
        branch: c.branch,
        message: c.message,
    }
}

/// `UpdateProgress` for the current OTA state.
pub fn update_progress(p: &crate::net_dashboard::ota_remote::Progress) -> ControlResponse {
    ControlResponse::UpdateProgress {
        stage: p.stage.as_str().to_string(),
        percent: p.percent,
        message: p.message.clone(),
        version: tetra_core::STACK_VERSION.to_string(),
        elapsed_secs: p.elapsed_secs,
    }
}

/// Should the worker send this progress now? `last`: stage and percent last sent, with the time.
pub fn progress_due(p: &crate::net_dashboard::ota_remote::Progress, last: Option<&(String, u8, std::time::Instant)>, now: std::time::Instant) -> bool {
    match last {
        None => p.running || matches!(p.stage, crate::net_dashboard::ota_remote::OtaStage::Restart),
        Some((stage, percent, at)) => {
            stage != p.stage.as_str() || *percent != p.percent || (p.running && now.duration_since(*at) >= std::time::Duration::from_secs(10))
        }
    }
}

fn off(handle: u32) -> ControlResponse {
    result(handle, false, code::FLEET_CONTROL_OFF, "Fleet Control is switched off on this station")
}

fn result(handle: u32, ok: bool, code: &str, message: &str) -> ControlResponse {
    ControlResponse::CommandResult { handle, ok, code: code.to_string(), message: message.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net_control::codec::ControlCodecJson;
    use crate::net_control::commands::UpdateSource;

    #[test]
    fn ping_is_answered_only_while_the_switch_is_on() {
        let on = answer(&ControlCommand::FleetPing { handle: 7 }, true).unwrap();
        assert!(matches!(on, ControlResponse::CommandResult { handle: 7, ok: true, ref code, .. } if code == code::OK));
        let off = answer(&ControlCommand::FleetPing { handle: 8 }, false).unwrap();
        assert!(matches!(off, ControlResponse::CommandResult { handle: 8, ok: false, ref code, .. } if code == code::FLEET_CONTROL_OFF));
    }

    #[test]
    fn other_commands_are_not_fleet_commands() {
        // The existing commands keep working without the switch: the worker routes them as before.
        assert!(answer(&ControlCommand::KickMs { issi: 1 }, false).is_none());
        assert!(answer(&ControlCommand::RestartService, false).is_none());
    }

    use crate::net_dashboard::ota_remote::{CheckOutcome, OtaStage, Progress, StartRefused};

    fn code_of(a: Option<UpdateAction>) -> (bool, String) {
        match a {
            Some(UpdateAction::Reply(ControlResponse::CommandResult { ok, code, .. })) => (ok, code),
            other => panic!("expected a CommandResult, got {other:?}"),
        }
    }

    #[test]
    fn update_commands_need_the_switch_and_an_ota() {
        let never = |_, _: Option<&UpdateSource>, _: Option<&crate::net_dashboard::source_auth::SourceToken>| -> Result<(), StartRefused> { panic!("must not start") };
        assert_eq!(code_of(update_command(&ControlCommand::StartUpdate { handle: 1, during_call: false, source: None, auth: None }, false, true, never)), (false, "fleet_control_off".into()));
        assert_eq!(code_of(update_command(&ControlCommand::CheckUpdate { handle: 2, auth: None }, false, true, never)), (false, "fleet_control_off".into()));
        assert_eq!(code_of(update_command(&ControlCommand::StartUpdate { handle: 3, during_call: false, source: None, auth: None }, true, false, never)), (false, "unavailable".into()));
        assert!(matches!(update_command(&ControlCommand::CheckUpdate { handle: 4, auth: None }, true, true, never), Some(UpdateAction::Check { handle: 4, .. })));
        assert!(update_command(&ControlCommand::FleetPing { handle: 5 }, true, true, never).is_none());
    }

    #[test]
    fn start_update_outcomes() {
        let mut seen = None;
        let ok = update_command(&ControlCommand::StartUpdate { handle: 1, during_call: true, source: None, auth: None }, true, true, |d, s, _| {
            seen = Some((d, s.cloned()));
            Ok(())
        });
        assert_eq!(code_of(ok), (true, "ok".into()));
        assert_eq!(seen, Some((true, None)), "during_call is passed on");
        let busy = update_command(&ControlCommand::StartUpdate { handle: 2, during_call: false, source: None, auth: None }, true, true, |_, _, _| Err(StartRefused::Busy));
        assert_eq!(code_of(busy), (false, "busy".into()));
        let call = update_command(&ControlCommand::StartUpdate { handle: 3, during_call: false, source: None, auth: None }, true, true, |_, _, _| Err(StartRefused::CallActive));
        assert_eq!(code_of(call), (false, "call_active".into()));
        let src = UpdateSource { repo: "https://github.com/sebastianmadl/bost-flowstation".into(), branch: "fleet-control".into() };
        let mut got = None;
        let live = update_command(&ControlCommand::StartUpdate { handle: 4, during_call: false, source: Some(src.clone()), auth: None }, true, true, |_, s, _| {
            got = s.cloned();
            Ok(())
        });
        assert_eq!(code_of(live), (true, "ok".into()));
        assert_eq!(got, Some(src.clone()), "the live source is passed on");
        let bad = update_command(&ControlCommand::StartUpdate { handle: 5, during_call: false, source: Some(src), auth: None }, true, true, |_, _, _| Err(StartRefused::InvalidSource));
        assert_eq!(code_of(bad), (false, "invalid_source".into()));
    }

    #[test]
    fn maintenance_flag_runs_out_and_is_bounded() {
        let t0 = Instant::now();
        let mut slot = None;
        assert_eq!(apply_maintenance(&mut slot, true, 0, " admin ", "new carrier", t0), "Maintenance on for 10 min");
        let m = current_maintenance(&mut slot, t0 + Duration::from_secs(60)).unwrap();
        assert_eq!((m.by.as_str(), m.reason.as_str()), ("admin", "new carrier"));
        assert!(current_maintenance(&mut slot, t0 + Duration::from_secs(600)).is_none(), "runs out after 10 min");
        assert!(slot.is_none(), "a run-out flag is cleared");
        assert_eq!(apply_maintenance(&mut slot, true, 9999, "", "", t0), "Maintenance on for 240 min");
        assert_eq!(apply_maintenance(&mut slot, false, 0, "", "", t0), "Maintenance off");
        assert!(slot.is_none());
    }

    #[test]
    fn set_maintenance_needs_the_switch() {
        let cmd = ControlCommand::SetMaintenance { handle: 3, on: true, minutes: 5, by: "admin".into(), reason: String::new() };
        let off = answer(&cmd, false).unwrap();
        assert!(matches!(off, ControlResponse::CommandResult { ok: false, ref code, .. } if code == code::FLEET_CONTROL_OFF));
        assert!(maintenance().is_none(), "switch off: nothing set");
        let on = answer(&cmd, true).unwrap();
        assert!(matches!(on, ControlResponse::CommandResult { handle: 3, ok: true, .. }));
        assert_eq!(maintenance().map(|m| m.by), Some("admin".to_string()));
        answer(&ControlCommand::SetMaintenance { handle: 4, on: false, minutes: 0, by: String::new(), reason: String::new() }, true);
        assert!(maintenance().is_none());
        let codec = ControlCodecJson;
        let c = codec.decode_command(br#"{"SetMaintenance":{"handle":5,"on":true}}"#).unwrap();
        assert!(matches!(c, ControlCommand::SetMaintenance { handle: 5, on: true, minutes: 0, .. }));
    }

    #[test]
    fn restart_outcomes() {
        let never = |_| panic!("must not restart");
        let cmd = |host, during_call| ControlCommand::RestartStation { handle: 1, host, during_call };
        assert_eq!(code_of(restart_command(&cmd(true, false), false, 0, false, never).map(UpdateAction::Reply)), (false, "fleet_control_off".into()));
        assert_eq!(code_of(restart_command(&cmd(true, false), true, 0, true, never).map(UpdateAction::Reply)), (false, "busy".into()));
        assert_eq!(code_of(restart_command(&cmd(true, false), true, 2, false, never).map(UpdateAction::Reply)), (false, "call_active".into()));
        let mut asked = None;
        assert_eq!(code_of(restart_command(&cmd(true, true), true, 2, false, |h| asked = Some(h)).map(UpdateAction::Reply)), (true, "ok".into()));
        assert_eq!(asked, Some(true), "reboot during a call when asked to");
        let mut asked = None;
        assert_eq!(code_of(restart_command(&cmd(false, false), true, 0, false, |h| asked = Some(h)).map(UpdateAction::Reply)), (true, "ok".into()));
        assert_eq!(asked, Some(false), "service restart");
        assert!(restart_command(&ControlCommand::FleetPing { handle: 1 }, true, 0, false, never).is_none());
        let codec = ControlCodecJson;
        let c = codec.decode_command(br#"{"RestartStation":{"handle":7,"host":true}}"#).unwrap();
        assert!(matches!(c, ControlCommand::RestartStation { handle: 7, host: true, during_call: false }));
    }

    #[test]
    fn capabilities_list_update_only_with_an_ota() {
        assert_eq!(capabilities(false), vec!["fleet-v1", "maintenance-v1", "restart-v1", "metrics-v1"]);
        assert_eq!(capabilities(true), vec!["fleet-v1", "maintenance-v1", "restart-v1", "metrics-v1", "update-v1", "update-live-v1"]);
    }

    fn prog(running: bool, stage: OtaStage, percent: u8) -> Progress {
        Progress { running, stage, percent, message: String::new(), elapsed_secs: 0 }
    }

    #[test]
    fn progress_sent_on_change_and_every_ten_seconds() {
        let t0 = std::time::Instant::now();
        assert!(!progress_due(&prog(false, OtaStage::UpToDate, 100), None, t0), "an old finished run is not reported after a restart");
        assert!(progress_due(&prog(true, OtaStage::Download, 5), None, t0));
        let last = ("build".to_string(), 30u8, t0);
        assert!(!progress_due(&prog(true, OtaStage::Build, 30), Some(&last), t0 + std::time::Duration::from_secs(3)));
        assert!(progress_due(&prog(true, OtaStage::Build, 31), Some(&last), t0));
        assert!(progress_due(&prog(true, OtaStage::Build, 30), Some(&last), t0 + std::time::Duration::from_secs(10)));
        assert!(progress_due(&prog(false, OtaStage::Restart, 100), Some(&last), t0), "the end is always sent");
        let done = ("restart".to_string(), 100u8, t0);
        assert!(!progress_due(&prog(false, OtaStage::Restart, 100), Some(&done), t0 + std::time::Duration::from_secs(60)));
    }

    #[test]
    fn update_wire_shapes() {
        let codec = ControlCodecJson;
        let c = codec.decode_command(br#"{"CheckUpdate":{"handle":9}}"#).unwrap();
        assert!(matches!(c, ControlCommand::CheckUpdate { handle: 9, .. }));
        let s = codec.decode_command(br#"{"StartUpdate":{"handle":10}}"#).unwrap();
        assert!(matches!(s, ControlCommand::StartUpdate { handle: 10, during_call: false, source: None, .. }), "during_call defaults to false");
        let s = codec.decode_command(br#"{"StartUpdate":{"handle":11,"during_call":true}}"#).unwrap();
        assert!(matches!(s, ControlCommand::StartUpdate { handle: 11, during_call: true, source: None, auth: None }));
        let s = codec.decode_command(br#"{"StartUpdate":{"handle":12,"source":{"repo":"https://github.com/sebastianmadl/bost-flowstation","branch":"fleet-control"}}}"#).unwrap();
        match s {
            ControlCommand::StartUpdate { handle: 12, during_call: false, source: Some(src), auth: None } => {
                assert_eq!((src.repo.as_str(), src.branch.as_str()), ("https://github.com/sebastianmadl/bost-flowstation", "fleet-control"));
            }
            other => panic!("unexpected {other:?}"),
        }
        let info = update_info(
            9,
            CheckOutcome {
                ok: true,
                current: "v0.5.9-aaa".into(),
                latest: Some("v0.5.9-bbb".into()),
                update_available: true,
                channel: "installed".into(),
                repo: "https://github.com/sebastianmadl/bost-flowstation".into(),
                branch: "fleet-control".into(),
                message: "newer version available".into(),
            },
        );
        let json: serde_json::Value = serde_json::from_slice(&codec.encode_response(&info)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"UpdateInfo": {"handle": 9, "ok": true, "current": "v0.5.9-aaa", "latest": "v0.5.9-bbb", "update_available": true, "channel": "installed", "repo": "https://github.com/sebastianmadl/bost-flowstation", "branch": "fleet-control", "message": "newer version available"}})
        );
        let p = update_progress(&Progress { running: true, stage: OtaStage::Build, percent: 42, message: "Compiling x".into(), elapsed_secs: 120 });
        let json: serde_json::Value = serde_json::from_slice(&codec.encode_response(&p)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"UpdateProgress": {"stage": "build", "percent": 42, "message": "Compiling x", "version": tetra_core::STACK_VERSION, "elapsed_secs": 120}})
        );
    }

    #[test]
    fn offered_only_with_the_extension() {
        assert!(!offered(&[]));
        assert!(!offered(&["bts-info".to_string()]));
        assert!(offered(&["bts-info".to_string(), EXT_FLEET_CONTROL.to_string()]));
    }

    #[test]
    fn json_wire_shape() {
        // Externally tagged, as brew-server's `control.rs` parses it (docs: Docs/fleet-control.md).
        let codec = ControlCodecJson;
        let hello = ControlResponse::StationHello {
            protocol: 1,
            version: "v0.5.9-abc".into(),
            brew_login: Some("1000001".into()),
            mcc: 232,
            mnc: 20,
            fleet_control: true,
            capabilities: vec!["fleet-v1".into()],
            config_hash: None,
            config_pending: None,
        };
        let json: serde_json::Value = serde_json::from_slice(&codec.encode_response(&hello)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"StationHello": {"protocol": 1, "version": "v0.5.9-abc", "brew_login": "1000001", "mcc": 232, "mnc": 20, "fleet_control": true, "capabilities": ["fleet-v1"]}})
        );
        let res = result(3, false, code::FLEET_CONTROL_OFF, "off");
        let json: serde_json::Value = serde_json::from_slice(&codec.encode_response(&res)).unwrap();
        assert_eq!(json, serde_json::json!({"CommandResult": {"handle": 3, "ok": false, "code": "fleet_control_off", "message": "off"}}));
        let ping = codec.decode_command(br#"{"FleetPing":{"handle":5}}"#).unwrap();
        assert!(matches!(ping, ControlCommand::FleetPing { handle: 5 }));
    }
}
