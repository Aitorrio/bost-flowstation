use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

use crate::bluestation::SecretField;

/// Asterisk SIP/RTP bridge configuration.
#[derive(Debug, Clone)]
pub struct CfgAsterisk {
    pub enabled: bool,
    pub outbound_prefix: String,
    pub strip_outbound_prefix: bool,
    pub inbound_prefix: String,
    pub register: bool,
    pub codec: String,
    pub service_numbers: Vec<String>,
    pub rtp_port_min: u16,
    pub rtp_port_max: u16,
    pub bind_addr: String,
    pub bind_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub contact_host: String,
    pub from_domain: String,
    pub local_user: String,
    pub auth_user: String,
    pub password: SecretField,
    pub realm: String,
    pub options_interval_secs: u64,
    /// RTP packet duration in ms. Real trunks expect 20 ms / 160 byte G.711 packets.
    pub ptime_ms: u16,
    /// How much SIP -> TETRA audio may wait for its downlink slot before the oldest is
    /// dropped. Rounded down to whole 60 ms blocks; anything queued past this is delay the
    /// call never gets back.
    pub dl_jitter_ms: u16,
    /// Extra source hosts allowed to talk SIP to us besides the resolved Asterisk peer.
    pub allow_from: Vec<String>,
    /// Cap on dialogs that have not been answered yet, per bridge.
    pub max_pending_dialogs: usize,
    /// Inbound INVITEs accepted per source address per minute (0 disables the limit).
    pub max_invites_per_minute: u32,
}

impl Default for CfgAsterisk {
    fn default() -> Self {
        apply_asterisk_patch(CfgAsteriskDto::default()).expect("default asterisk config must be valid")
    }
}

#[derive(Deserialize)]
pub struct CfgAsteriskDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_outbound_prefix")]
    pub outbound_prefix: String,
    #[serde(default = "default_strip_outbound_prefix")]
    pub strip_outbound_prefix: bool,
    #[serde(default = "default_inbound_prefix")]
    pub inbound_prefix: String,
    #[serde(default = "default_register")]
    pub register: bool,
    #[serde(default = "default_codec")]
    pub codec: String,
    #[serde(default)]
    pub service_numbers: Vec<String>,
    #[serde(default = "default_rtp_port_min")]
    pub rtp_port_min: u16,
    #[serde(default = "default_rtp_port_max")]
    pub rtp_port_max: u16,
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    #[serde(default = "default_bind_port")]
    pub bind_port: u16,
    #[serde(default = "default_remote_host")]
    pub remote_host: String,
    #[serde(default = "default_remote_port")]
    pub remote_port: u16,
    #[serde(default = "default_contact_host")]
    pub contact_host: String,
    #[serde(default = "default_from_domain")]
    pub from_domain: String,
    #[serde(default = "default_local_user")]
    pub local_user: String,
    #[serde(default = "default_auth_user")]
    pub auth_user: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_realm")]
    pub realm: String,
    #[serde(default = "default_options_interval_secs")]
    pub options_interval_secs: u64,
    #[serde(default = "default_ptime_ms")]
    pub ptime_ms: u16,
    #[serde(default = "default_dl_jitter_ms")]
    pub dl_jitter_ms: u16,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default = "default_max_pending_dialogs")]
    pub max_pending_dialogs: usize,
    #[serde(default = "default_max_invites_per_minute")]
    pub max_invites_per_minute: u32,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for CfgAsteriskDto {
    fn default() -> Self {
        Self {
            enabled: false,
            outbound_prefix: default_outbound_prefix(),
            strip_outbound_prefix: default_strip_outbound_prefix(),
            inbound_prefix: default_inbound_prefix(),
            register: default_register(),
            codec: default_codec(),
            service_numbers: Vec::new(),
            rtp_port_min: default_rtp_port_min(),
            rtp_port_max: default_rtp_port_max(),
            bind_addr: default_bind_addr(),
            bind_port: default_bind_port(),
            remote_host: default_remote_host(),
            remote_port: default_remote_port(),
            contact_host: default_contact_host(),
            from_domain: default_from_domain(),
            local_user: default_local_user(),
            auth_user: default_auth_user(),
            password: String::new(),
            realm: default_realm(),
            options_interval_secs: default_options_interval_secs(),
            ptime_ms: default_ptime_ms(),
            dl_jitter_ms: default_dl_jitter_ms(),
            allow_from: Vec::new(),
            max_pending_dialogs: default_max_pending_dialogs(),
            max_invites_per_minute: default_max_invites_per_minute(),
            extra: HashMap::new(),
        }
    }
}

fn default_outbound_prefix() -> String {
    "91".to_string()
}

fn default_strip_outbound_prefix() -> bool {
    true
}

fn default_inbound_prefix() -> String {
    "T".to_string()
}

fn default_register() -> bool {
    true
}

fn default_codec() -> String {
    "PCMU".to_string()
}

fn default_rtp_port_min() -> u16 {
    30000
}

fn default_rtp_port_max() -> u16 {
    30100
}

// Loopback by default: anything reachable on this port can place calls into the TETRA cell.
fn default_bind_addr() -> String {
    "127.0.0.1".to_string()
}

fn default_bind_port() -> u16 {
    5062
}

fn default_remote_host() -> String {
    "127.0.0.1".to_string()
}

fn default_remote_port() -> u16 {
    5060
}

fn default_contact_host() -> String {
    "127.0.0.1".to_string()
}

fn default_from_domain() -> String {
    "127.0.0.1".to_string()
}

fn default_local_user() -> String {
    "flowstation".to_string()
}

fn default_auth_user() -> String {
    "flowstation".to_string()
}

fn default_realm() -> String {
    "asterisk".to_string()
}

fn default_options_interval_secs() -> u64 {
    30
}

fn default_ptime_ms() -> u16 {
    20
}

// Two 60 ms blocks: enough to ride out a trunk burst, short enough that nobody hears it.
fn default_dl_jitter_ms() -> u16 {
    120
}

fn default_max_pending_dialogs() -> usize {
    8
}

fn default_max_invites_per_minute() -> u32 {
    30
}

pub fn apply_asterisk_patch(src: CfgAsteriskDto) -> Result<CfgAsterisk, String> {
    if src.enabled {
        if src.bind_port == 0 {
            return Err("asterisk: bind_port cannot be 0".to_string());
        }
        if src.remote_port == 0 {
            return Err("asterisk: remote_port cannot be 0".to_string());
        }
        if src.rtp_port_min == 0 || src.rtp_port_max == 0 || src.rtp_port_min > src.rtp_port_max {
            return Err("asterisk: rtp_port_min/rtp_port_max must define a valid non-zero range".to_string());
        }
        if src.remote_host.trim().is_empty() {
            return Err("asterisk: remote_host cannot be empty when enabled".to_string());
        }
        if src.contact_host.trim().is_empty() {
            return Err("asterisk: contact_host cannot be empty when enabled".to_string());
        }
        if src.local_user.trim().is_empty() {
            return Err("asterisk: local_user cannot be empty when enabled".to_string());
        }
        if src.auth_user.trim().is_empty() {
            return Err("asterisk: auth_user cannot be empty when enabled".to_string());
        }
    }

    let codec = src.codec.trim().to_ascii_uppercase();
    if codec != "PCMU" {
        return Err("asterisk: only codec = \"PCMU\" is currently supported".to_string());
    }

    if !matches!(src.ptime_ms, 20 | 40 | 60) {
        return Err("asterisk: ptime_ms must be 20, 40 or 60".to_string());
    }

    // Anything queued here is delay the call carries for good, so refuse to buffer a call late.
    if src.dl_jitter_ms > 500 {
        return Err("asterisk: dl_jitter_ms must be 500 or less".to_string());
    }

    let allow_from = src
        .allow_from
        .into_iter()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .collect();

    let service_numbers = src
        .service_numbers
        .into_iter()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();

    Ok(CfgAsterisk {
        enabled: src.enabled,
        outbound_prefix: src.outbound_prefix,
        strip_outbound_prefix: src.strip_outbound_prefix,
        inbound_prefix: src.inbound_prefix,
        register: src.register,
        codec,
        service_numbers,
        rtp_port_min: src.rtp_port_min,
        rtp_port_max: src.rtp_port_max,
        bind_addr: src.bind_addr,
        bind_port: src.bind_port,
        remote_host: src.remote_host,
        remote_port: src.remote_port,
        contact_host: src.contact_host,
        from_domain: src.from_domain,
        local_user: src.local_user,
        auth_user: src.auth_user,
        password: SecretField::from(src.password),
        realm: src.realm,
        options_interval_secs: src.options_interval_secs,
        ptime_ms: src.ptime_ms,
        dl_jitter_ms: src.dl_jitter_ms,
        allow_from,
        max_pending_dialogs: src.max_pending_dialogs,
        max_invites_per_minute: src.max_invites_per_minute,
    })
}
