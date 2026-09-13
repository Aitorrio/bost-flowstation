use serde::Deserialize;
use std::collections::HashMap;
use toml::Value;

/// Dashboard HTTP / HTTPS server configuration.
///
/// Canonical access is HTTPS on [`CfgDashboard::https_port`] (default 443).
/// [`CfgDashboard::port`] (default 80) is a cleartext listener that only redirects to HTTPS.
#[derive(Debug, Clone)]
pub struct CfgDashboard {
    /// Cleartext HTTP port used only to redirect to HTTPS (default: 80).
    pub port: u16,
    /// HTTPS port for the real dashboard (default: 443).
    pub https_port: u16,
    /// Bind address (default: 0.0.0.0 — all interfaces, so the dashboard is reachable from the LAN).
    /// Off-host access without credentials is read-only: control commands are only honoured from
    /// localhost or an authenticated session. Set username/password to allow off-host control, or set
    /// this to 127.0.0.1 to keep the dashboard on this host only.
    pub bind: String,
    /// Optional explicit path to the Bost FlowStation git source directory used for OTA updates.
    /// When unset, the dashboard auto-detects by:
    ///   1. Walking up from the running binary path until a `.git` directory is found
    ///   2. Trying well-known install paths (/opt/bost-flowstation, legacy FlowStation paths)
    ///   3. Falling back to the current working directory if it is a git repo
    /// Set this explicitly when the binary is installed outside the repo (e.g. /opt/tetra/
    /// with the git clone elsewhere), or when auto-detection picks the wrong directory.
    /// OTA pulls from github.com/Aitorrio/bost-flowstation (`ota_channel` selects the branch).
    pub source_dir: Option<String>,
    /// OTA release channel: `"stable"` → git branch `bost`, `"beta"` → `beta`. Default `stable`.
    pub ota_channel: String,
    /// Optional HTTP Basic Auth credentials.
    /// When both username and password are set, all dashboard requests require authentication.
    /// When omitted, the dashboard is accessible without a password (default, home-network use).
    ///
    /// SECURITY NOTE: HTTP Basic Auth sends credentials as base64 (not encrypted) on the wire.
    /// This protects against casual/accidental access on a LAN but is NOT secure over the
    /// public internet without TLS. For internet-facing deployments, put a reverse proxy
    /// with HTTPS in front of the dashboard.
    pub username: Option<String>,
    pub password: Option<String>,
    /// When true AND auth (username+password) is set, anonymous visitors get a read-only public
    /// overview page instead of being bounced to /login. Admin controls and raw config stay behind
    /// login. Default false = unchanged behaviour (auth is all-or-nothing). Inert without auth.
    pub public_overview: bool,
    /// Advanced DGNA control: when true, the dashboard shows the SS-DGNA attachment-mode picker.
    /// When false, operators always use the cell-level default attachment mode.
    pub show_dgna_attachment_mode_picker: bool,
}

impl Default for CfgDashboard {
    fn default() -> Self {
        Self {
            port: 80,
            https_port: 443,
            bind: "0.0.0.0".to_string(),
            source_dir: None,
            ota_channel: "stable".to_string(),
            username: None,
            password: None,
            public_overview: false,
            show_dgna_attachment_mode_picker: false,
        }
    }
}

#[derive(Deserialize)]
pub struct CfgDashboardDto {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_https_port")]
    pub https_port: u16,
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default)]
    pub source_dir: Option<String>,
    #[serde(default = "default_ota_channel")]
    pub ota_channel: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    // Mandatory DTO field (not optional): the DTO flattens unknown keys into `extra`, so without an
    // explicit field the TOML `public_overview` would be silently ignored.
    #[serde(default)]
    pub public_overview: bool,
    #[serde(default)]
    pub show_dgna_attachment_mode_picker: bool,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn default_port() -> u16 {
    80
}
fn default_https_port() -> u16 {
    443
}
fn default_bind() -> String {
    // All interfaces by default. The dashboard is normally opened from another machine on the LAN, so
    // a loopback-only default locks out the common setup and breaks every box that didn't set `bind`
    // explicitly. Control over the wire is still localhost-or-authenticated only (see the gate in
    // net_dashboard::server), so off-host without credentials is read-only — a wide default exposes a
    // view, not the control surface. Set `bind = "127.0.0.1"` to keep it on this host.
    "0.0.0.0".to_string()
}

fn default_ota_channel() -> String {
    "stable".to_string()
}

pub fn apply_dashboard_patch(src: CfgDashboardDto) -> Result<CfgDashboard, String> {
    if src.port == 0 {
        return Err("dashboard: port cannot be 0".to_string());
    }
    if src.https_port == 0 {
        return Err("dashboard: https_port cannot be 0".to_string());
    }
    if src.port == src.https_port {
        return Err("dashboard: port and https_port must differ".to_string());
    }
    // Validate source_dir if provided: must be an existing directory.
    if let Some(ref sd) = src.source_dir {
        if sd.trim().is_empty() {
            return Err("dashboard: source_dir cannot be empty (omit the field instead)".to_string());
        }
        let path = std::path::Path::new(sd);
        if !path.exists() {
            return Err(format!("dashboard: source_dir '{}' does not exist", sd));
        }
        if !path.is_dir() {
            return Err(format!("dashboard: source_dir '{}' is not a directory", sd));
        }
    }
    // Auth: either both username+password are set, or neither.
    match (&src.username, &src.password) {
        (Some(u), Some(p)) => {
            if u.trim().is_empty() {
                return Err("dashboard: username cannot be empty".to_string());
            }
            if p.is_empty() {
                return Err("dashboard: password cannot be empty".to_string());
            }
        }
        (None, None) => {}
        _ => return Err("dashboard: set both 'username' and 'password', or neither".to_string()),
    }
    let ota_channel = match src.ota_channel.trim().to_ascii_lowercase().as_str() {
        "stable" | "beta" => src.ota_channel.trim().to_ascii_lowercase(),
        other if other.is_empty() => "stable".to_string(),
        other => {
            return Err(format!(
                "dashboard: ota_channel must be \"stable\" or \"beta\" (got '{other}')"
            ));
        }
    };
    Ok(CfgDashboard {
        port: src.port,
        https_port: src.https_port,
        bind: src.bind,
        source_dir: src.source_dir,
        ota_channel,
        username: src.username,
        password: src.password,
        // public_overview is inert unless auth is set (with no auth the dashboard is already open),
        // so we accept it silently rather than erroring — keeps config validation lenient.
        public_overview: src.public_overview,
        show_dgna_attachment_mode_picker: src.show_dgna_attachment_mode_picker,
    })
}
