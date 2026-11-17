//! Fleet Control remote update (`update-v1`): the Brew server checks for and starts the same OTA
//! as the dashboard's Update button, over the control channel. Protocol: `Docs/fleet-control.md`.
//!
//! The OTA state is one per process ([`shared_update_state`]), so a run started from the server
//! shows on the dashboard and the other way round, and only one run can be active. The
//! dashboard registers the config path, the source directory override and its live state at
//! start ([`register`]); without a dashboard there is no OTA and the station does not offer
//! `update-v1`.
//!
//! The station always updates from its own OTA source (`[dashboard] ota_channel`, by default the
//! repository and branch it was installed from); the server only says "check" and "update".
//!
//! **Live update** (`update-live-v1`): `StartUpdate` with a `source` runs the installer
//! (`contrib/install/install-bost.sh`) with that repository and branch, the same as the install
//! command in a terminal, and keeps the radio on during the build; only the service restart at
//! the end takes the station off the air. The installer sets the station's OTA source to that
//! repository and branch.

use std::sync::{Arc, Mutex, OnceLock};

use crate::net_dashboard::server::{SharedUpdateState, UpdatePhase, UpdateState};
use crate::net_dashboard::state::DashboardState;

/// Step of an OTA run, as reported in `UpdateProgress.stage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaStage {
    /// Checking the source and fetching it from the repository.
    Download,
    /// `cargo build` (the radio is off the air from here on: the OTA frees the SDR for the build).
    Build,
    /// The new program is put in place.
    Install,
    /// Built and installed; the service restarts in a few seconds.
    Restart,
    /// Nothing to do: the running build is already the newest of the source.
    UpToDate,
    /// The run stopped with an error (the station keeps or restores the old version).
    Failed,
}

impl OtaStage {
    pub fn as_str(self) -> &'static str {
        match self {
            OtaStage::Download => "download",
            OtaStage::Build => "build",
            OtaStage::Install => "install",
            OtaStage::Restart => "restart",
            OtaStage::UpToDate => "up_to_date",
            OtaStage::Failed => "failed",
        }
    }
}

/// What the dashboard gives Fleet Control at start.
struct Registered {
    config_path: String,
    source_dir_override: Option<String>,
    dashboard: DashboardState,
}

static UPDATE: OnceLock<SharedUpdateState> = OnceLock::new();
static REGISTERED: Mutex<Option<Registered>> = Mutex::new(None);

/// The process-wide OTA state (dashboard and Fleet Control).
pub(crate) fn shared_update_state() -> SharedUpdateState {
    UPDATE.get_or_init(|| Arc::new(Mutex::new(UpdateState::new()))).clone()
}

/// Called by the dashboard when it starts.
pub(crate) fn register(config_path: String, source_dir_override: Option<String>, dashboard: DashboardState) {
    *REGISTERED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Registered { config_path, source_dir_override, dashboard });
}

/// Can this station run a remote update (a dashboard is running)?
pub fn available() -> bool {
    REGISTERED.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Result of a remote update check.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckOutcome {
    pub ok: bool,
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub channel: String,
    /// Repository of the OTA source, without credentials.
    pub repo: String,
    pub branch: String,
    pub message: String,
}

/// Run the station's update check against its own OTA source, fresh (no cache). Blocking
/// (GitHub, up to ~45 s): call it off the control worker's loop.
pub fn check() -> CheckOutcome {
    let Some(config_path) = REGISTERED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| r.config_path.clone()) else {
        return CheckOutcome {
            ok: false,
            current: tetra_core::STACK_VERSION.to_string(),
            latest: None,
            update_available: false,
            channel: String::new(),
            repo: String::new(),
            branch: String::new(),
            message: "no dashboard on this station, no OTA".into(),
        };
    };
    let source = crate::net_dashboard::ota_channel::resolve_ota_source(&config_path);
    let json = crate::net_dashboard::server::update_check_json(&config_path, true, false);
    check_outcome_from_json(&json, &source.repo_display)
}

/// Parse the dashboard's update check JSON.
pub fn check_outcome_from_json(json: &str, repo: &str) -> CheckOutcome {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let failed = v.get("check_failed").and_then(|x| x.as_bool()).unwrap_or(true);
    let available = v.get("update_available").and_then(|x| x.as_bool()).unwrap_or(false);
    CheckOutcome {
        ok: !failed,
        current: s("current").unwrap_or_else(|| tetra_core::STACK_VERSION.to_string()),
        latest: latest_label(s("remote_version").as_deref(), s("latest").as_deref()),
        update_available: available,
        channel: s("channel").unwrap_or_default(),
        repo: repo.to_string(),
        branch: s("branch").unwrap_or_default(),
        message: if failed {
            "update check failed (no connection to the repository?)".into()
        } else if available {
            "newer version available".into()
        } else {
            "up to date".into()
        },
    }
}

/// Newest version in the same form as `STACK_VERSION` (`v0.5.9-d081d936`): the version number
/// of the source plus the commit of its tip (`latest` is `branch@commit`), so stations on the same
/// version number but other commits can be told apart.
fn latest_label(remote_version: Option<&str>, latest: Option<&str>) -> Option<String> {
    let commit = latest.and_then(|l| l.rsplit_once('@').map(|(_, c)| c)).filter(|c| !c.is_empty());
    match (remote_version.map(|v| v.trim().trim_start_matches('v')).filter(|v| !v.is_empty()), commit) {
        (Some(v), Some(c)) => Some(format!("v{v}-{c}")),
        (Some(v), None) => Some(format!("v{v}")),
        (None, _) => latest.map(str::to_string),
    }
}

/// Why a remote update did not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartRefused {
    /// No dashboard, so no OTA on this station.
    Unavailable,
    /// An update is already running.
    Busy,
    /// A call is in progress and the server did not ask to update during calls.
    CallActive,
    /// Live update: not a plain `https://` repository or branch name.
    InvalidSource,
}

/// Only plain `https://` repository URLs without credentials and plain branch names reach the
/// installer (it gets them as environment variables, never through a shell line).
pub fn valid_live_source(repo: &str, branch: &str) -> bool {
    let ok_repo = repo.strip_prefix("https://").is_some_and(|rest| {
        !rest.is_empty() && !rest.contains('@') && rest.chars().all(|c| c.is_ascii_alphanumeric() || "-._/:~".contains(c))
    });
    let ok_branch = !branch.is_empty()
        && branch.len() <= 100
        && !branch.starts_with('-')
        && !branch.contains("..")
        && branch.chars().all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c));
    ok_repo && ok_branch
}

/// Number of calls in progress on this station (dashboard view).
pub fn active_calls() -> usize {
    REGISTERED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|r| r.dashboard.read().map(|s| s.calls.len()).unwrap_or(0))
        .unwrap_or(0)
}

/// Start the OTA like the dashboard's Update button. `during_call`: start even while a call is
/// in progress (the radio goes off the air for the build). `source`: live update from that
/// repository and branch with the radio kept on. `auth`: read token for the source, held only
/// while this update runs.
pub fn start(
    during_call: bool,
    source: Option<&crate::net_control::commands::UpdateSource>,
    auth: Option<&crate::net_dashboard::source_auth::SourceToken>,
) -> Result<(), StartRefused> {
    let (config_path, source_dir) = {
        let reg = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = reg.as_ref() else { return Err(StartRefused::Unavailable) };
        (r.config_path.clone(), r.source_dir_override.clone())
    };
    if let Some(s) = source {
        if !valid_live_source(s.repo.trim(), s.branch.trim()) {
            return Err(StartRefused::InvalidSource);
        }
    }
    let update = shared_update_state();
    try_begin(&update, during_call, active_calls())?;
    let u = Arc::clone(&update);
    let auth = auth.cloned();
    let spawned = match source {
        Some(s) => {
            let (repo, branch) = (s.repo.trim().to_string(), s.branch.trim().to_string());
            tracing::info!("Fleet Control: live update from {repo} ({branch}) started by the Brew server");
            std::thread::Builder::new()
                .name("ota-live-update".into())
                .spawn(move || {
                    let _auth = crate::net_dashboard::source_auth::scope(auth.as_ref());
                    crate::net_dashboard::server::run_live_update(u, source_dir, repo, branch)
                })
        }
        None => {
            tracing::info!("Fleet Control: OTA update started by the Brew server");
            std::thread::Builder::new()
                .name("ota-update".into())
                .spawn(move || {
                    let _auth = crate::net_dashboard::source_auth::scope(auth.as_ref());
                    crate::net_dashboard::server::run_update(u, config_path, source_dir)
                })
        }
    };
    if spawned.is_err()
    {
        let mut st = update.lock().unwrap_or_else(|e| e.into_inner());
        st.phase = UpdatePhase::Done { success: false };
        st.stage = OtaStage::Failed;
        return Err(StartRefused::Busy);
    }
    Ok(())
}

/// Mark the state as running unless something stops it (testable without a thread).
fn try_begin(update: &SharedUpdateState, during_call: bool, calls: usize) -> Result<(), StartRefused> {
    let mut st = update.lock().unwrap_or_else(|e| e.into_inner());
    if st.phase == UpdatePhase::Running {
        return Err(StartRefused::Busy);
    }
    if calls > 0 && !during_call {
        return Err(StartRefused::CallActive);
    }
    st.start();
    Ok(())
}

/// Progress of the current or last OTA run of this process.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub running: bool,
    pub stage: OtaStage,
    /// 0–100, estimated (build: compiled units against the last build).
    pub percent: u8,
    /// Last log line (shortened).
    pub message: String,
    pub elapsed_secs: u64,
}

/// A live update (radio on) running now: its progress (the dashboard shows a banner).
pub fn live_running() -> Option<Progress> {
    let st = shared_update_state();
    let st = st.lock().unwrap_or_else(|e| e.into_inner());
    if !st.live {
        return None;
    }
    progress_of(&st).filter(|p| p.running)
}

/// `None` until an update ran in this process.
pub fn progress() -> Option<Progress> {
    let st = shared_update_state();
    let st = st.lock().unwrap_or_else(|e| e.into_inner());
    progress_of(&st)
}

pub(crate) fn progress_of(st: &UpdateState) -> Option<Progress> {
    let running = match st.phase {
        UpdatePhase::Idle => return None,
        UpdatePhase::Running => true,
        UpdatePhase::Done { .. } => false,
    };
    let percent = match st.stage {
        OtaStage::Download => 5,
        OtaStage::Build => build_percent(st.compiled, st.units_expected),
        OtaStage::Install => 92,
        OtaStage::Restart | OtaStage::UpToDate => 100,
        OtaStage::Failed => 0,
    };
    let mut message = st.log.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
    if message.chars().count() > 160 {
        message = message.chars().take(157).collect::<String>() + "…";
    }
    Some(Progress { running, stage: st.stage, percent, message, elapsed_secs: st.started.map(|t| t.elapsed().as_secs()).unwrap_or(0) })
}

/// Build progress 10–90 % from the compiled units against the expected count.
pub fn build_percent(compiled: u32, expected: u32) -> u8 {
    if expected == 0 {
        return 10;
    }
    let share = (compiled.min(expected) as u64 * 80 / expected as u64) as u8;
    10 + share.min(80)
}

const UNITS_FILE: &str = "target/.ota-build-units";

/// Units the next build will compile: the count of the last successful build, else every
/// package in `Cargo.lock` (a first build compiles them all).
pub(crate) fn expected_units(src_dir: &std::path::Path) -> u32 {
    if let Some(n) = std::fs::read_to_string(src_dir.join(UNITS_FILE)).ok().and_then(|s| s.trim().parse::<u32>().ok()).filter(|n| *n > 0) {
        return n;
    }
    std::fs::read_to_string(src_dir.join("Cargo.lock")).map(|s| s.matches("[[package]]").count() as u32).unwrap_or(0)
}

/// Remember the unit count of a successful build.
pub(crate) fn remember_units(src_dir: &std::path::Path, compiled: u32) {
    if compiled > 0 {
        let _ = std::fs::write(src_dir.join(UNITS_FILE), compiled.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_percent_is_estimated_and_bounded() {
        assert_eq!(build_percent(0, 0), 10);
        assert_eq!(build_percent(0, 40), 10);
        assert_eq!(build_percent(20, 40), 50);
        assert_eq!(build_percent(40, 40), 90);
        assert_eq!(build_percent(55, 40), 90, "more units than last time never passes 90 %");
    }

    #[test]
    fn progress_follows_the_stages() {
        let mut st = UpdateState::new();
        assert!(progress_of(&st).is_none(), "nothing ran yet");
        st.start();
        st.append_for_test("=== OTA Update ===");
        let p = progress_of(&st).unwrap();
        assert!(p.running && p.stage == OtaStage::Download && p.percent == 5);
        st.set_stage_for_test(OtaStage::Build);
        st.units_expected = 4;
        st.append_for_test("   Compiling tetra-core v0.1.0");
        st.append_for_test("   Compiling tetra-pdus v0.1.0");
        st.append_for_test("warning: something");
        let p = progress_of(&st).unwrap();
        assert_eq!((p.stage, p.percent), (OtaStage::Build, 50));
        assert_eq!(p.message, "warning: something");
        st.set_stage_for_test(OtaStage::Restart);
        st.finish_for_test(true);
        let p = progress_of(&st).unwrap();
        assert!(!p.running && p.stage == OtaStage::Restart && p.percent == 100);
        st.start();
        st.finish_for_test(false);
        assert_eq!(progress_of(&st).unwrap().stage, OtaStage::Failed, "a failed run reports failed");
    }

    #[test]
    fn compiling_lines_outside_the_build_do_not_count() {
        let mut st = UpdateState::new();
        st.start();
        st.append_for_test("   Compiling something in a fetch log");
        assert_eq!(st.compiled, 0);
    }

    #[test]
    fn start_refused_while_busy_or_in_a_call() {
        let u: SharedUpdateState = Arc::new(Mutex::new(UpdateState::new()));
        assert_eq!(try_begin(&u, false, 1), Err(StartRefused::CallActive));
        assert!(u.lock().unwrap().phase == UpdatePhase::Idle, "a refused start changes nothing");
        assert_eq!(try_begin(&u, true, 1), Ok(()), "during_call starts in a call");
        assert_eq!(try_begin(&u, true, 0), Err(StartRefused::Busy));
        u.lock().unwrap().finish_for_test(true);
        assert_eq!(try_begin(&u, false, 0), Ok(()));
    }

    #[test]
    fn live_source_only_plain_https_and_branch_names() {
        assert!(valid_live_source("https://github.com/sebastianmadl/bost-flowstation", "fleet-control"));
        assert!(valid_live_source("https://github.com/Aitorrio/bost-flowstation.git", "main"));
        assert!(valid_live_source("https://git.example.org:8443/x/y", "feature/a_b.1"));
        assert!(!valid_live_source("git@github.com:a/b.git", "main"), "ssh needs keys");
        assert!(!valid_live_source("https://user:pw@github.com/a/b", "main"), "no credentials");
        assert!(!valid_live_source("https://github.com/a/b;reboot", "main"));
        assert!(!valid_live_source("https://github.com/a/b $(id)", "main"));
        assert!(!valid_live_source("https://github.com/a/b", "-x"));
        assert!(!valid_live_source("https://github.com/a/b", "a..b"));
        assert!(!valid_live_source("https://github.com/a/b", "a b"));
        assert!(!valid_live_source("https://github.com/a/b", ""));
        assert!(!valid_live_source("http://github.com/a/b", "main"));
    }

    #[test]
    fn live_stage_follows_the_installer_log() {
        let mut st = UpdateState::new();
        st.start();
        st.live = true;
        st.append_for_test("==> Updating /opt/bost-flowstation → origin/fleet-control");
        assert_eq!(st.stage, OtaStage::Download);
        st.append_for_test("==> Building bluestation-bs (release, --features asterisk) — this can take a while on a Pi");
        assert_eq!(st.stage, OtaStage::Build);
        st.append_for_test("   Compiling tetra-core v0.1.0");
        assert_eq!(st.compiled, 1);
        st.append_for_test("==> Installed /usr/local/bin/bluestation-bs");
        assert_eq!(st.stage, OtaStage::Install);
        let mut normal = UpdateState::new();
        normal.start();
        normal.append_for_test("==> Building bluestation-bs (release)");
        assert_eq!(normal.stage, OtaStage::Download, "the normal OTA sets its stages itself");
    }

    #[test]
    fn installer_gets_the_source_runs_as_a_normal_task_and_reports_stages() {
        let dir = std::env::temp_dir().join(format!("ota-live-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("Cargo.lock"), "[[package]]\n[[package]]\n").unwrap();
        let script = dir.join("install.sh");
        std::fs::write(
            &script,
            "echo \"==> Updating $BOST_SRC → origin/$BOST_BRANCH\"\necho \"repo=$BOST_REPO\"\n\
             echo \"nice=$(awk '{print $19}' /proc/$$/stat) $(chrt -p $$ 2>/dev/null | head -1)\"\n\
             read -t 1 x && echo \"stdin=$x\"\n\
             echo '==> Building bluestation-bs (release)'\nsleep 0.3\necho '   Compiling a v1' >&2\necho '   Compiling b v1' >&2\necho '==> Installed /usr/local/bin/bluestation-bs'\n",
        )
        .unwrap();
        let u: SharedUpdateState = Arc::new(Mutex::new(UpdateState::new()));
        u.lock().unwrap().start();
        u.lock().unwrap().live = true;
        assert!(crate::net_dashboard::server::run_installer_for_test(&u, &dir, &script, "https://github.com/sebastianmadl/bost-flowstation", "fleet-control"));
        let st = u.lock().unwrap();
        assert!(st.log.contains("origin/fleet-control") && st.log.contains("repo=https://github.com/sebastianmadl/bost-flowstation"), "{}", st.log);
        assert!(st.log.contains("nice=10"), "lower priority: {}", st.log);
        // Started from a real-time process (the service), the installer must not inherit it.
        assert!(!st.log.contains("SCHED_FIFO") && !st.log.contains("SCHED_RR"), "normal scheduling: {}", st.log);
        assert!(!st.log.contains("stdin="), "no terminal: the installer never waits for input");
        assert_eq!((st.stage, st.compiled, st.units_expected), (OtaStage::Install, 2, 2), "{}", st.log);
        drop(st);
        assert_eq!(expected_units(&dir), 2, "the unit count is kept for the next run");
        std::fs::write(&script, "echo boom; exit 3\n").unwrap();
        u.lock().unwrap().start();
        assert!(!crate::net_dashboard::server::run_installer_for_test(&u, &dir, &script, "https://x/y", "main"));
        assert_eq!(u.lock().unwrap().stage, OtaStage::Failed);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn check_json_is_parsed() {
        let json = r#"{"current":"v0.5.9-aaa","latest":"main@bbb","update_available":true,"check_failed":false,"channel":"installed","branch":"fleet-control","remote_version":"0.5.9"}"#;
        let c = check_outcome_from_json(json, "https://github.com/sebastianmadl/bost-flowstation");
        assert!(c.ok && c.update_available);
        assert_eq!(c.latest.as_deref(), Some("v0.5.9-bbb"), "version number plus the tip commit");
        let real = r#"{"current":"v0.5.9-2bb45158","latest":"fleet-control@d081d936","update_available":true,"check_failed":false,"channel":"installed","branch":"fleet-control","remote_version":"0.5.9"}"#;
        assert_eq!(check_outcome_from_json(real, "").latest.as_deref(), Some("v0.5.9-d081d936"));
        let no_version = r#"{"current":"v1","latest":"main@abc12345","update_available":true,"check_failed":false}"#;
        assert_eq!(check_outcome_from_json(no_version, "").latest.as_deref(), Some("main@abc12345"));
        assert_eq!((c.channel.as_str(), c.branch.as_str()), ("installed", "fleet-control"));
        let failed = check_outcome_from_json(r#"{"current":"v1","check_failed":true,"update_available":false}"#, "");
        assert!(!failed.ok && !failed.update_available);
        assert!(!check_outcome_from_json("not json", "").ok);
    }

    #[test]
    fn expected_units_from_last_build_or_lockfile() {
        let dir = std::env::temp_dir().join(format!("ota-units-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("Cargo.lock"), "[[package]]\nname=\"a\"\n[[package]]\nname=\"b\"\n").unwrap();
        assert_eq!(expected_units(&dir), 2);
        remember_units(&dir, 7);
        assert_eq!(expected_units(&dir), 7);
        remember_units(&dir, 0);
        assert_eq!(expected_units(&dir), 7, "an empty count is not stored");
        std::fs::remove_dir_all(&dir).ok();
    }
}
