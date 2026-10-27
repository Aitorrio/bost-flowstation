use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use toml::Value;

/// An explicitly configured private source network allowed to reach authenticated HTTP.
///
/// Host bits are masked on parsing. IPv4-mapped IPv6 addresses are canonicalised to IPv4;
/// a mapped prefix must include the complete 96-bit mapping prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardHttpNetwork {
    network: IpAddr,
    prefix: u8,
}

impl DashboardHttpNetwork {
    pub fn contains(&self, peer: IpAddr) -> bool {
        match (self.network, canonical_peer(peer)) {
            (IpAddr::V4(network), IpAddr::V4(peer)) => u32::from(peer) & ipv4_mask(self.prefix) == u32::from(network),
            (IpAddr::V6(network), IpAddr::V6(peer)) => u128::from(peer) & ipv6_mask(self.prefix) == u128::from(network),
            _ => false,
        }
    }
}

impl FromStr for DashboardHttpNetwork {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || value.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err("expected an IP address or CIDR without whitespace".into());
        }
        let (address, prefix_text) = match value.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (value, None),
        };
        let address = address
            .parse::<IpAddr>()
            .map_err(|_| "expected a literal IPv4 or IPv6 address (no hostnames or zone identifiers)".to_string())?;
        let width = if address.is_ipv4() { 32 } else { 128 };
        let mut prefix = match prefix_text {
            Some(text) if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) => {
                text.parse::<u8>().map_err(|_| "invalid CIDR prefix".to_string())?
            }
            Some(_) => return Err("invalid CIDR prefix".into()),
            None => width,
        };
        if prefix > width {
            return Err(format!("CIDR prefix must not exceed {width}"));
        }
        let address = match address {
            IpAddr::V6(address) => match address.to_ipv4_mapped() {
                Some(mapped) => {
                    if prefix < 96 {
                        return Err("IPv4-mapped IPv6 CIDRs must have a prefix of at least 96".into());
                    }
                    prefix -= 96;
                    IpAddr::V4(mapped)
                }
                None => IpAddr::V6(address),
            },
            address => address,
        };
        let network = match address {
            IpAddr::V4(address) => {
                let network = u32::from(address) & ipv4_mask(prefix);
                let private_ranges = [
                    (u32::from(Ipv4Addr::new(10, 0, 0, 0)), 8),
                    (u32::from(Ipv4Addr::new(172, 16, 0, 0)), 12),
                    (u32::from(Ipv4Addr::new(192, 168, 0, 0)), 16),
                    (u32::from(Ipv4Addr::new(127, 0, 0, 0)), 8),
                ];
                if !private_ranges
                    .into_iter()
                    .any(|(base, bits)| prefix >= bits && network & ipv4_mask(bits) == base)
                {
                    return Err("HTTP source networks must be wholly within RFC1918 or IPv4 loopback space".into());
                }
                IpAddr::V4(Ipv4Addr::from(network))
            }
            IpAddr::V6(address) => {
                let network = u128::from(address) & ipv6_mask(prefix);
                let is_ula = prefix >= 7 && network & ipv6_mask(7) == (0xfcu128 << 120);
                let is_loopback = prefix == 128 && address == Ipv6Addr::LOCALHOST;
                if !is_ula && !is_loopback {
                    return Err("HTTP source networks must be wholly within IPv6 ULA or loopback space".into());
                }
                IpAddr::V6(Ipv6Addr::from(network))
            }
        };
        Ok(Self { network, prefix })
    }
}

impl fmt::Display for DashboardHttpNetwork {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.network, self.prefix)
    }
}

fn canonical_peer(peer: IpAddr) -> IpAddr {
    match peer {
        IpAddr::V6(address) => address.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(peer),
        peer => peer,
    }
}

fn ipv4_mask(prefix: u8) -> u32 {
    if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) }
}

fn ipv6_mask(prefix: u8) -> u128 {
    if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) }
}

/// Dashboard HTTP / HTTPS server configuration.
///
/// Canonical access is HTTPS on [`CfgDashboard::https_port`] (default 443).
/// [`CfgDashboard::port`] (default 80) redirects to HTTPS unless a private source network
/// has been explicitly allowed and dashboard credentials are configured.
/// Set `port = 0` to disable the HTTP listener (high-port / shared-host preset).
#[derive(Debug, Clone)]
pub struct CfgDashboard {
    /// Primary cleartext HTTP port (default: 80). Redirects to HTTPS unless an explicitly
    /// allowed private peer is using the authenticated HTTP mode.
    /// `0` disables the HTTP listener entirely.
    pub port: u16,
    /// HTTPS port for the real dashboard (default: 443).
    pub https_port: u16,
    /// Bind address (default: 0.0.0.0 — all interfaces, so the dashboard is reachable from the LAN).
    /// Set username/password to protect the dashboard, or bind to 127.0.0.1 for host-only access.
    /// The optional private HTTP mode additionally requires a matching direct peer address.
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
    /// OTA release channel: `"stable"` → git branch `main`, `"beta"` → `beta`. Default `stable`.
    pub ota_channel: String,
    /// Optional dashboard login credentials. POST /api/login accepts JSON `user`/`password`
    /// or a form body and issues an `fs_session` cookie for protected HTTP and WebSocket requests.
    /// When omitted, the dashboard is accessible without a password (default, home-network use).
    /// Explicitly allowed private HTTP management always requires both credentials and a session.
    ///
    /// HTTPS is the default transport. HTTP does not encrypt credentials or session cookies;
    /// restrict private HTTP to a trusted LAN or encrypted VPN, without public forwarding.
    pub username: Option<String>,
    /// Password paired with `username` for the cookie-session login.
    pub password: Option<String>,
    /// Explicit private source IPs/CIDRs permitted to use authenticated HTTP.
    /// Empty by default. This is source-address policy, not authentication; the HTTP
    /// listener must still enforce its current login/session gate on every request.
    /// Applies to `port` only. When a separate legacy 8080 listener is opened, it stays a redirect.
    pub http_allowed_networks: Vec<DashboardHttpNetwork>,
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
            http_allowed_networks: Vec::new(),
            public_overview: false,
            show_dgna_attachment_mode_picker: false,
        }
    }
}

impl CfgDashboard {
    /// Match the socket peer only; callers must not substitute forwarded HTTP headers.
    /// Current live credentials/session checks are still required by the server.
    pub fn permits_http_peer(&self, peer: IpAddr) -> bool {
        self.username.as_ref().is_some_and(|user| !user.trim().is_empty())
            && self.password.as_ref().is_some_and(|password| !password.is_empty())
            && self.http_allowed_networks.iter().any(|network| network.contains(peer))
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
    #[serde(default)]
    pub http_allowed_networks: Vec<String>,
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
    // port == 0 disables the cleartext HTTP redirect listener (high-port preset).
    if src.https_port == 0 {
        return Err("dashboard: https_port cannot be 0".to_string());
    }
    if src.port != 0 && src.port == src.https_port {
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
            return Err(format!("dashboard: ota_channel must be \"stable\" or \"beta\" (got '{other}')"));
        }
    };
    if src.http_allowed_networks.len() > 32 {
        return Err("dashboard: http_allowed_networks may contain at most 32 entries".into());
    }
    if !src.http_allowed_networks.is_empty() && (src.username.is_none() || src.password.is_none()) {
        return Err("dashboard: http_allowed_networks requires username and password".into());
    }
    let mut http_allowed_networks = Vec::new();
    for value in &src.http_allowed_networks {
        let network = value
            .parse::<DashboardHttpNetwork>()
            .map_err(|error| format!("dashboard: invalid http_allowed_networks entry {value:?}: {error}"))?;
        if !http_allowed_networks.contains(&network) {
            http_allowed_networks.push(network);
        }
    }
    Ok(CfgDashboard {
        port: src.port,
        https_port: src.https_port,
        bind: src.bind,
        source_dir: src.source_dir,
        ota_channel,
        username: src.username,
        password: src.password,
        http_allowed_networks,
        // public_overview is inert unless auth is set (with no auth the dashboard is already open),
        // so we accept it silently rather than erroring — keeps config validation lenient.
        public_overview: src.public_overview,
        show_dgna_attachment_mode_picker: src.show_dgna_attachment_mode_picker,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dto(networks: &[&str], authenticated: bool) -> CfgDashboardDto {
        let mut value: CfgDashboardDto = toml::from_str("").expect("default dashboard DTO");
        value.http_allowed_networks = networks.iter().map(|value| (*value).to_string()).collect();
        if authenticated {
            value.username = Some("operator".into());
            value.password = Some("test-password".into());
        }
        value
    }

    fn peer(address: &str) -> IpAddr {
        address.parse().expect("test IP address")
    }

    #[test]
    fn http_default_preserves_redirect_only_policy() {
        let config = apply_dashboard_patch(dto(&[], false)).expect("default config");
        assert!(config.http_allowed_networks.is_empty());
        assert!(!config.permits_http_peer(peer("10.0.0.10")));
        assert!(!config.permits_http_peer(peer("127.0.0.1")));
        assert_eq!(config.port, 80);
        assert_eq!(config.https_port, 443);
        assert!(CfgDashboard::default().http_allowed_networks.is_empty());
    }

    #[test]
    fn http_network_dto_field_is_not_flattened_into_extra() {
        let value: CfgDashboardDto =
            toml::from_str("username = 'operator'\npassword = 'test-password'\nhttp_allowed_networks = ['10.0.0.10']")
                .expect("dashboard config");
        assert!(!value.extra.contains_key("http_allowed_networks"));
        assert_eq!(value.http_allowed_networks, ["10.0.0.10"]);
        let config = apply_dashboard_patch(value).expect("validated config");
        assert!(config.permits_http_peer(peer("10.0.0.10")));
        assert!(!config.permits_http_peer(peer("10.0.0.11")));
        assert_eq!(config.http_allowed_networks[0].to_string(), "10.0.0.10/32");
    }

    #[test]
    fn http_private_ipv4_boundaries_are_matched_as_subnets() {
        let config = apply_dashboard_patch(dto(&["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"], true)).expect("private networks");
        for address in [
            "10.0.0.0",
            "10.255.255.255",
            "172.16.0.0",
            "172.31.255.255",
            "192.168.0.0",
            "192.168.255.255",
        ] {
            assert!(config.permits_http_peer(peer(address)), "{address}");
        }
        for address in [
            "9.255.255.255",
            "11.0.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "192.167.255.255",
            "192.169.0.0",
            "8.8.8.8",
        ] {
            assert!(!config.permits_http_peer(peer(address)), "{address}");
        }
    }

    #[test]
    fn http_networks_mask_host_bits_and_deduplicate_canonical_entries() {
        let config = apply_dashboard_patch(dto(&["10.2.3.99/24", "10.2.3.0/24"], true)).expect("network");
        assert_eq!(config.http_allowed_networks.len(), 1);
        assert_eq!(config.http_allowed_networks[0].to_string(), "10.2.3.0/24");
        assert!(config.permits_http_peer(peer("10.2.3.255")));
        assert!(!config.permits_http_peer(peer("10.2.4.0")));
    }

    #[test]
    fn http_ipv6_ula_and_loopback_are_explicit() {
        let config = apply_dashboard_patch(dto(&["fc00::/7", "::1", "127.0.0.0/8"], true)).expect("private IPv6");
        for address in ["fc00::", "fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", "::1", "127.255.255.254"] {
            assert!(config.permits_http_peer(peer(address)), "{address}");
        }
        for address in [
            "fbff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            "fe00::",
            "::",
            "::2",
            "fe80::1",
            "2001:db8::1",
        ] {
            assert!(!config.permits_http_peer(peer(address)), "{address}");
        }
        assert_eq!(config.http_allowed_networks[1].to_string(), "::1/128");
    }

    #[test]
    fn http_rejects_public_mixed_and_malformed_networks() {
        for network in [
            "0.0.0.0/0",
            "10.0.0.0/7",
            "172.16.0.0/11",
            "192.168.0.0/15",
            "127.0.0.0/7",
            "8.8.8.8",
            "100.64.0.0/10",
            "169.254.0.0/16",
            "198.18.0.0/15",
            "::/0",
            "::/127",
            "fc00::/6",
            "fe80::/10",
            "2001:db8::/32",
            "fe80::1%eth0",
            "10.0.0.1/33",
            "fd00::/129",
            "10.0.0.0/+8",
            "10.0.0.0/",
            "10.0.0.0/8/8",
            "10.0.0.0/8;true",
            "10.0.0.0/8\n",
            " 10.0.0.1",
            "http://10.0.0.1/",
            "localhost",
            "10.1/16",
            "::10.0.0.1",
        ] {
            assert!(apply_dashboard_patch(dto(&[network], true)).is_err(), "accepted {network:?}");
        }
    }

    #[test]
    fn http_ipv4_mapped_ipv6_cannot_expand_or_bypass_policy() {
        let config = apply_dashboard_patch(dto(&["::ffff:10.0.0.0/104", "::ffff:127.0.0.1"], true)).expect("mapped private networks");
        assert_eq!(config.http_allowed_networks[0].to_string(), "10.0.0.0/8");
        assert_eq!(config.http_allowed_networks[1].to_string(), "127.0.0.1/32");
        assert!(config.permits_http_peer(peer("10.0.0.10")));
        assert!(config.permits_http_peer(peer("::ffff:10.0.0.10")));
        assert!(config.permits_http_peer(peer("::ffff:127.0.0.1")));
        assert!(!config.permits_http_peer(peer("::ffff:11.0.0.1")));
        assert!(!config.permits_http_peer(peer("::10.0.0.10")));
        for network in ["::ffff:8.8.8.8", "::ffff:10.0.0.0/95", "::ffff:10.0.0.0/96"] {
            assert!(apply_dashboard_patch(dto(&[network], true)).is_err(), "accepted {network}");
        }
    }

    #[test]
    fn http_policy_requires_both_credentials_and_fails_closed_if_removed() {
        assert!(apply_dashboard_patch(dto(&["10.0.0.0/8"], false)).is_err());
        let mut config = apply_dashboard_patch(dto(&["10.0.0.0/8"], true)).expect("authenticated network");
        assert!(config.permits_http_peer(peer("10.0.0.10")));
        config.password = None;
        assert!(!config.permits_http_peer(peer("10.0.0.10")));
        config.password = Some(String::new());
        assert!(!config.permits_http_peer(peer("10.0.0.10")));
        config.password = Some("test-password".into());
        config.username = Some("  ".into());
        assert!(!config.permits_http_peer(peer("10.0.0.10")));
    }

    #[test]
    fn http_policy_caps_configured_entry_count_before_deduplication() {
        let maximum = vec!["10.0.0.10"; 32];
        assert!(apply_dashboard_patch(dto(&maximum, true)).is_ok());
        let excessive = vec!["10.0.0.10"; 33];
        let error = apply_dashboard_patch(dto(&excessive, true)).expect_err("entry limit");
        assert!(error.contains("at most 32"));
    }

    #[test]
    fn accepts_port_zero_high_preset() {
        let dto = CfgDashboardDto {
            port: 0,
            https_port: 8443,
            bind: "0.0.0.0".into(),
            source_dir: None,
            ota_channel: "stable".into(),
            username: None,
            password: None,
            http_allowed_networks: Vec::new(),
            public_overview: false,
            show_dgna_attachment_mode_picker: false,
            extra: HashMap::new(),
        };
        let c = apply_dashboard_patch(dto).expect("high preset");
        assert_eq!(c.port, 0);
        assert_eq!(c.https_port, 8443);
    }

    #[test]
    fn rejects_https_port_zero() {
        let dto = CfgDashboardDto {
            port: 80,
            https_port: 0,
            bind: "0.0.0.0".into(),
            source_dir: None,
            ota_channel: "stable".into(),
            username: None,
            password: None,
            http_allowed_networks: Vec::new(),
            public_overview: false,
            show_dgna_attachment_mode_picker: false,
            extra: HashMap::new(),
        };
        assert!(apply_dashboard_patch(dto).is_err());
    }
}
