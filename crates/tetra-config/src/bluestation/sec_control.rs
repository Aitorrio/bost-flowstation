use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

/// Control endpoint configuration
#[derive(Debug, Clone)]
pub struct CfgControl {
    /// Control server hostname or IP
    pub host: String,
    /// Control server port
    pub port: u16,
    /// Use TLS (wss://)
    pub use_tls: bool,
    /// Optional path to a DER-encoded CA certificate for self-signed TLS
    pub ca_cert: Option<String>,
    /// Optional (username, password) for HTTP Basic authentication
    pub credentials: Option<(String, String)>,
    /// Fleet Control: the server may manage this station (`fleet_control = true`). Off by
    /// default; switched on the Infrastructure page. See `Docs/fleet-control.md`.
    pub fleet_control: bool,
}

#[derive(Deserialize)]
pub struct CfgControlDto {
    /// Control server hostname or IP
    pub host: String,
    /// Control server port
    pub port: u16,
    /// Use TLS (wss://)
    #[serde(default)]
    pub use_tls: bool,
    /// Optional path to a DER-encoded CA certificate for self-signed TLS
    pub ca_cert: Option<String>,
    /// Optional username for HTTP Basic auth
    pub username: Option<String>,
    /// Optional password for HTTP Basic auth
    pub password: Option<String>,

    /// Switch for the whole section (default on): `enabled = false` keeps the values in the
    /// config but does not connect, so the dashboard can turn it off without losing them.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Fleet Control switch (default off): the Brew server may manage this station over the
    /// control channel once it offers `fleet-control`.
    #[serde(default)]
    pub fleet_control: bool,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn default_enabled() -> bool {
    true
}

/// Convert a [`CfgControlDto`] (from TOML) into a [`CfgControl`].
///
/// Returns an error string if `ca_cert` is set but `use_tls` is `false`.
pub fn apply_control_patch(src: CfgControlDto) -> Result<CfgControl, String> {
    if src.ca_cert.is_some() && !src.use_tls {
        return Err("control: ca_cert requires use_tls = true".to_string());
    }

    Ok(CfgControl {
        host: src.host,
        port: src.port,
        use_tls: src.use_tls,
        credentials: match (src.username, src.password) {
            (Some(u), Some(p)) => Some((u, p)),
            (None, None) => None,
            _ => return Err("control: both username and password must be set for credentials".to_string()),
        },
        ca_cert: src.ca_cert,
        fleet_control: src.fleet_control,
    })
}
