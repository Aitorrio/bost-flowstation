//! Host LAN overview + Ethernet profile control via NetworkManager (`nmcli`).
//!
//! Powers the dashboard «Red» page: list ethernet/wifi interfaces with IPv4,
//! mark the default-route address, and bring ethernet profiles up/down.
//! Wi-Fi scan/connect stays in [`crate::wifi`]; this module does not replace it.

use std::process::{Command, Stdio};
use std::time::Duration;

const NMCLI_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "msg")]
pub enum NetworkError {
    NotAvailable,
    Failed(String),
    Io(String),
    Timeout,
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkError::NotAvailable => write!(f, "NetworkManager (nmcli) not installed"),
            NetworkError::Failed(s) => write!(f, "nmcli failed: {}", s),
            NetworkError::Io(s) => write!(f, "nmcli exec error: {}", s),
            NetworkError::Timeout => write!(f, "nmcli timed out"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IfaceKind {
    Ethernet,
    Wifi,
    Other,
}

impl IfaceKind {
    fn from_nmcli(t: &str) -> Self {
        match t {
            "ethernet" => IfaceKind::Ethernet,
            "wifi" => IfaceKind::Wifi,
            _ => IfaceKind::Other,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct IfaceLink {
    pub name: String,
    pub kind: IfaceKind,
    pub state: String,
    /// Active connection profile name, if any (`--` → empty).
    pub connection: Option<String>,
    pub ipv4: Vec<String>,
    /// True when one of `ipv4` equals [`crate::sys_telemetry::primary_ip`].
    pub is_default: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NetworkStatus {
    pub interfaces: Vec<IfaceLink>,
    pub primary_ip: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EthernetProfile {
    pub uuid: String,
    pub name: String,
    pub active: bool,
}

/// Same probe as Wi-Fi: nmcli present and runnable.
pub fn available() -> bool {
    crate::wifi::available()
}

/// Snapshot of ethernet + wifi (and other) devices with IPv4 + default-route mark.
pub fn status() -> Result<NetworkStatus, NetworkError> {
    let primary_ip = crate::sys_telemetry::primary_ip();
    let dev_out = run_nmcli(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"])?;

    let mut interfaces = Vec::new();
    for line in dev_out.lines() {
        let fields = parse_terse_line(line);
        if fields.len() < 4 {
            continue;
        }
        let name = fields[0].clone();
        let kind = IfaceKind::from_nmcli(&fields[1]);
        // Skip loopback / purely virtual noise for the overview.
        if name == "lo" || fields[1] == "loopback" {
            continue;
        }
        // Keep ethernet + wifi always; skip bridge/tun/etc. from management UI noise
        // but still show them only if they have an IPv4 (useful for debugging).
        let state = fields[2].clone();
        let connection = {
            let c = fields[3].as_str();
            if c.is_empty() || c == "--" {
                None
            } else {
                Some(c.to_string())
            }
        };

        let ipv4 = ipv4_for_device(&name).unwrap_or_default();
        if kind == IfaceKind::Other && ipv4.is_empty() {
            continue;
        }

        let is_default = primary_ip
            .as_ref()
            .map(|p| ipv4.iter().any(|ip| ip == p))
            .unwrap_or(false);

        interfaces.push(IfaceLink {
            name,
            kind,
            state,
            connection,
            ipv4,
            is_default,
        });
    }

    // Stable order: ethernet first, then wifi, then other; by name within kind.
    interfaces.sort_by(|a, b| {
        kind_rank(a.kind)
            .cmp(&kind_rank(b.kind))
            .then(a.name.cmp(&b.name))
    });

    Ok(NetworkStatus {
        interfaces,
        primary_ip,
    })
}

fn kind_rank(k: IfaceKind) -> u8 {
    match k {
        IfaceKind::Ethernet => 0,
        IfaceKind::Wifi => 1,
        IfaceKind::Other => 2,
    }
}

fn ipv4_for_device(dev: &str) -> Result<Vec<String>, NetworkError> {
    let out = run_nmcli(&["-t", "-f", "IP4.ADDRESS", "device", "show", dev])?;
    let mut ips = Vec::new();
    for line in out.lines() {
        // Format: IP4.ADDRESS[1]:192.168.1.42/24  or key:value after first colon
        if let Some(rest) = line.split(':').nth(1) {
            if rest.is_empty() {
                continue;
            }
            let ip = rest.split('/').next().unwrap_or(rest).trim();
            if !ip.is_empty() {
                ips.push(ip.to_string());
            }
        }
    }
    Ok(ips)
}

/// Compact one-line summary for U-STATUS: `eth0=10.0.1.212* wlan0=10.0.1.228`.
/// `*` marks the address that matches the default route ([`primary_ip`]).
pub fn format_ip_status_line() -> String {
    match status() {
        Ok(st) => {
            let mut parts = Vec::new();
            for iface in &st.interfaces {
                if iface.kind != IfaceKind::Ethernet && iface.kind != IfaceKind::Wifi {
                    continue;
                }
                for ip in &iface.ipv4 {
                    let mark = if st.primary_ip.as_deref() == Some(ip.as_str()) {
                        "*"
                    } else {
                        ""
                    };
                    parts.push(format!("{}={}{}", iface.name, ip, mark));
                }
            }
            if parts.is_empty() {
                st.primary_ip
                    .unwrap_or_else(|| "n/a".to_string())
            } else {
                parts.join(" ")
            }
        }
        Err(_) => crate::sys_telemetry::primary_ip().unwrap_or_else(|| "n/a".to_string()),
    }
}

/// List saved ethernet profiles (`802-3-ethernet`).
pub fn list_ethernet_saved() -> Result<Vec<EthernetProfile>, NetworkError> {
    let out = run_nmcli(&["-t", "-f", "UUID,NAME,TYPE,ACTIVE", "connection", "show"])?;
    let mut profiles = Vec::new();
    for line in out.lines() {
        let fields = parse_terse_line(line);
        if fields.len() < 4 {
            continue;
        }
        if fields[2] != "802-3-ethernet" {
            continue;
        }
        profiles.push(EthernetProfile {
            uuid: fields[0].clone(),
            name: fields[1].clone(),
            active: fields[3] == "yes",
        });
    }
    Ok(profiles)
}

/// Bring up a saved ethernet profile.
pub fn ethernet_up(uuid: &str) -> Result<(), NetworkError> {
    run_nmcli(&["--wait", "12", "connection", "up", "uuid", uuid])?;
    Ok(())
}

/// Bring down a saved ethernet profile (does not forget it).
pub fn ethernet_down(uuid: &str) -> Result<(), NetworkError> {
    run_nmcli(&["connection", "down", "uuid", uuid])?;
    Ok(())
}

fn run_nmcli(args: &[&str]) -> Result<String, NetworkError> {
    let mut child = match Command::new("nmcli")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Err(NetworkError::NotAvailable);
            }
            return Err(NetworkError::Io(e.to_string()));
        }
    };

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output().map_err(|e| NetworkError::Io(e.to_string()))?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if status.success() {
                    return Ok(stdout);
                }
                return Err(NetworkError::Failed(if stderr.is_empty() {
                    format!("exit code {}", status.code().unwrap_or(-1))
                } else {
                    stderr
                }));
            }
            Ok(None) => {
                if start.elapsed() > NMCLI_TIMEOUT {
                    let _ = child.kill();
                    return Err(NetworkError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(NetworkError::Io(e.to_string())),
        }
    }
}

fn parse_terse_line(line: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![String::new()];
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.last_mut().unwrap().push(next);
            }
        } else if c == ':' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}
