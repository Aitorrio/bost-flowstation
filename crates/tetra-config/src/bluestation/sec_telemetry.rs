use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

/// Telemetry endpoint configuration
#[derive(Debug, Clone)]
pub struct CfgTelemetry {
    /// Telemetry server hostname or IP
    pub host: String,
    /// Telemetry server port
    pub port: u16,
    /// Use TLS (wss://)
    pub use_tls: bool,
    /// Optional path to a DER-encoded CA certificate for self-signed TLS
    pub ca_cert: Option<String>,
    /// Optional (username, password) for HTTP Basic authentication
    pub credentials: Option<(String, String)>,
    /// Optional station name reported with the location (shown on the Brew server map)
    pub site_name: Option<String>,
    /// Optional station position (latitude, longitude) in decimal degrees, reported to the
    /// telemetry server. `None` when unset or (0, 0); nothing is reported then.
    pub site_location: Option<(f64, f64)>,
}

#[derive(Deserialize)]
pub struct CfgTelemetryDto {
    /// Telemetry server hostname or IP
    pub host: String,
    /// Telemetry server port
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
    /// Optional station name reported with the location
    pub site_name: Option<String>,
    /// Optional station latitude, decimal degrees (-90..90)
    pub latitude: Option<f64>,
    /// Optional station longitude, decimal degrees (-180..180)
    pub longitude: Option<f64>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Convert a [`CfgTelemetryDto`] (from TOML) into a [`CfgTelemetry`].
///
/// Returns an error string if `ca_cert` is set but `use_tls` is `false`.
pub fn apply_telemetry_patch(src: CfgTelemetryDto) -> Result<CfgTelemetry, String> {
    if src.ca_cert.is_some() && !src.use_tls {
        return Err("telemetry: ca_cert requires use_tls = true".to_string());
    }

    let site_location = match (src.latitude, src.longitude) {
        (None, None) => None,
        (Some(lat), Some(lon)) => {
            if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
                return Err("telemetry: latitude must be -90..90 and longitude -180..180".to_string());
            }
            // (0, 0) is "no fix", never a real station position
            (lat != 0.0 || lon != 0.0).then_some((lat, lon))
        }
        _ => return Err("telemetry: both latitude and longitude must be set, or neither".to_string()),
    };

    Ok(CfgTelemetry {
        site_name: src.site_name.filter(|n| !n.trim().is_empty()),
        site_location,
        host: src.host,
        port: src.port,
        use_tls: src.use_tls,
        credentials: match (src.username, src.password) {
            (Some(u), Some(p)) => Some((u, p)),
            (None, None) => None,
            _ => return Err("telemetry: both username and password must be set for credentials".to_string()),
        },
        ca_cert: src.ca_cert,
    })
}
