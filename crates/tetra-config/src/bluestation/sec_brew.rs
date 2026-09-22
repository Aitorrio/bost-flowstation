use std::{collections::HashMap, time::Duration};

use serde::Deserialize;
use toml::Value;

use crate::bluestation::SecretField;

/// Brew protocol (TetraPack/BrandMeister) configuration
#[derive(Debug, Clone)]
pub struct CfgBrew {
    /// TetraPack server hostname or IP
    pub host: String,
    /// TetraPack server port
    pub port: u16,
    /// Use TLS (wss:// / https://)
    pub tls: bool,
    /// Optional username for HTTP Digest auth
    pub username: Option<String>,
    /// Optional password for HTTP Digest auth
    pub password: Option<SecretField>,
    /// Reconnection delay
    pub reconnect_delay: Duration,
    /// Extra initial jitter playout delay in frames (added on top of adaptive baseline)
    pub jitter_initial_latency_frames: u8,

    /// Set to true when SDS between local and Brew clients is enabled
    pub feature_sds_enabled: bool,
    /// If true, RSSI measurements are exported to the Brew server as Service (0xf4) JSON messages.
    /// Disabled by default. Enable only if the Brew server supports and expects RSSI data.
    pub feature_rssi_export: bool,
    /// If true, UL LIP SDS (PID 10) is re-forwarded to Brew at [`Self::lip_forward_issi`], regardless
    /// of the radio's original destination (local / 9999 / other). Independent of SDS forwarding.
    pub feature_lip_forward: bool,
    /// Brew destination ISSI for sniffed LIP reports. Ignored when `feature_lip_forward` is false.
    pub lip_forward_issi: Option<u32>,
    /// If present, restrict Brew call to these remote SSIs
    pub whitelisted_ssis: Option<Vec<u32>>,
    /// Optional PBX gateway ISSIs that should be routable over Brew even if they don't match
    /// normal Tetrapack subscriber ISSI constraints.
    pub pbx_gateway_issis: Option<Vec<u32>>,
    /// Seconds the transport must stay down before SYSINFO advertises site-trunking
    /// (`system_wide_services=false`). Short 4G/5G blips do not flip the cell announcement.
    /// Active Brew calls are still released immediately on transport loss. Clamped 0..=60; default 3.
    pub backhaul_hysteresis_secs: u64,
}

#[derive(Default, Deserialize)]
pub struct CfgBrewDto {
    /// TetraPack server hostname or IP
    pub host: String,
    /// TetraPack server port
    #[serde(default = "default_brew_port")]
    pub port: u16,
    /// Use TLS (wss:// / https://)
    pub tls: bool,
    /// Optional username for HTTP Digest auth
    pub username: u32,
    /// Optional password for HTTP Digest auth
    pub password: String,
    /// Reconnection delay in seconds
    #[serde(default = "default_brew_reconnect_delay")]
    pub reconnect_delay_secs: u64,
    /// Extra initial jitter playout delay in frames (added on top of adaptive baseline)
    #[serde(default)]
    pub jitter_initial_latency_frames: u8,

    /// If present, restrict Brew call to these remote SSIs
    pub whitelisted_ssis: Option<Vec<u32>>,

    /// Set to true when SDS between local and Brew clients is enabled
    #[serde(default = "default_brew_feature_sds_enabled")]
    pub feature_sds_enabled: bool,

    /// Export RSSI measurements to the Brew server as Service JSON messages. Default: false.
    #[serde(default)]
    pub feature_rssi_export: bool,

    /// Re-forward UL LIP (PID 10) to Brew at `lip_forward_issi`. Default: false.
    #[serde(default)]
    pub feature_lip_forward: bool,

    /// Destination ISSI on Brew for sniffed LIP (any original dest).
    #[serde(default)]
    pub lip_forward_issi: Option<u32>,

    /// Optional PBX gateway ISSIs that should be routable over Brew even if they don't match
    /// normal Tetrapack subscriber ISSI constraints.
    #[serde(alias = "pbx_gateway_issi")]
    pub pbx_gateway_issis: Option<Vec<u32>>,

    /// Delay before announcing site-trunking after Brew transport drop. Default: 3.
    #[serde(default = "default_brew_backhaul_hysteresis_secs")]
    pub backhaul_hysteresis_secs: u64,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn default_brew_port() -> u16 {
    443
}

fn default_brew_reconnect_delay() -> u64 {
    15
}

fn default_brew_feature_sds_enabled() -> bool {
    true
}

fn default_brew_backhaul_hysteresis_secs() -> u64 {
    3
}

/// Convert a CfgBrewDto (from TOML) into a CfgBrew (used in the stack config)
pub fn apply_brew_patch(src: CfgBrewDto) -> CfgBrew {
    CfgBrew {
        host: src.host,
        port: src.port,
        tls: src.tls,
        username: Some(src.username.to_string()),
        password: Some(SecretField::from(src.password)),
        reconnect_delay: Duration::from_secs(src.reconnect_delay_secs),
        jitter_initial_latency_frames: src.jitter_initial_latency_frames,
        feature_sds_enabled: src.feature_sds_enabled,
        feature_rssi_export: src.feature_rssi_export,
        feature_lip_forward: src.feature_lip_forward,
        lip_forward_issi: src.lip_forward_issi.filter(|&i| i > 0 && i <= 0xFF_FFFF),
        whitelisted_ssis: src.whitelisted_ssis,
        pbx_gateway_issis: src.pbx_gateway_issis,
        backhaul_hysteresis_secs: src.backhaul_hysteresis_secs.clamp(0, 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backhaul_hysteresis_defaults_and_clamps() {
        let dto: CfgBrewDto = toml::from_str(
            r#"
            host = "x"
            tls = false
            username = 1
            password = "p"
            "#,
        )
        .unwrap();
        let c = apply_brew_patch(dto);
        assert_eq!(c.backhaul_hysteresis_secs, 3);

        let dto: CfgBrewDto = toml::from_str(
            r#"
            host = "x"
            tls = false
            username = 1
            password = "p"
            backhaul_hysteresis_secs = 999
            "#,
        )
        .unwrap();
        assert_eq!(apply_brew_patch(dto).backhaul_hysteresis_secs, 60);
    }
}
