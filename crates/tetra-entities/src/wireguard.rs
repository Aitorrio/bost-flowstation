//! WireGuard tunnel management for the dashboard Network tab.
//!
//! The tunnel lives in a plain `wg-quick` config file, `/etc/wireguard/<iface>.conf` (mode 0600),
//! and is controlled with `wg-quick up/down` and `wg show`. Those need root: when the station does
//! not run as root every privileged step goes through `sudo -n` (passwordless sudo), the same way
//! the service restart does.
//!
//! Only the fields the dashboard manages are ever written: Interface (`PrivateKey`, `Address`,
//! `ListenPort`, `MTU`, `DNS`) and Peer (`PublicKey`, `PresharedKey`, `Endpoint`, `AllowedIPs`,
//! `PersistentKeepalive`). Every value is validated before it reaches the file, so a newline or a
//! stray `PostUp` can never be smuggled into a root-executed config. An existing file that carries
//! other directives (`PostUp`, `Table`, `FwMark`, ...) is not overwritten unless the caller says so.
//! Private and preshared keys are write-only: they are never returned to the browser.

use std::io::Write;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};

pub const DEFAULT_IFACE: &str = "wg0";
const CMD_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Peer {
    pub public_key: String,
    /// Input only. `None` keeps the stored one (matched by public key), `Some("")` removes it.
    #[serde(default, skip_serializing)]
    pub preshared_key: Option<String>,
    /// Output only: whether a preshared key is stored for this peer.
    #[serde(default, skip_deserializing)]
    pub has_preshared_key: bool,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub allowed_ips: Vec<String>,
    #[serde(default)]
    pub persistent_keepalive: Option<u16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub listen_port: Option<u16>,
    #[serde(default)]
    pub mtu: Option<u16>,
    #[serde(default)]
    pub dns: Vec<String>,
    #[serde(default)]
    pub peers: Vec<Peer>,
}

/// A config as stored on disk: the managed fields, the secrets, and directives we do not manage.
#[derive(Debug, Default)]
struct Stored {
    cfg: Config,
    private_key: Option<String>,
    preshared: Vec<(String, String)>, // (peer public key, psk)
    unmanaged: Vec<String>,
}

// ───────────────────────────── validation ─────────────────────────────

/// Interface names: what the kernel accepts, restricted to the characters wg-quick handles safely.
pub fn valid_iface(s: &str) -> bool {
    !s.is_empty() && s.len() <= 15 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '='))
}

/// A WireGuard key: 32 bytes, standard base64 (44 characters ending in `=`).
pub fn valid_key(s: &str) -> bool {
    s.len() == 44 && base64::engine::general_purpose::STANDARD.decode(s).map(|b| b.len() == 32).unwrap_or(false)
}

/// `address/prefix` (prefix optional only when `need_prefix` is false).
pub fn valid_cidr(s: &str, need_prefix: bool) -> bool {
    let (addr, prefix) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (s, None),
    };
    let Ok(ip) = addr.parse::<IpAddr>() else { return false };
    match prefix {
        None => !need_prefix,
        Some(p) => {
            let max = if ip.is_ipv4() { 32 } else { 128 };
            p.parse::<u8>().map(|n| n <= max).unwrap_or(false)
        }
    }
}

/// `host:port` / `[v6]:port`; host is an IP or a DNS name.
pub fn valid_endpoint(s: &str) -> bool {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let Some((h, p)) = rest.split_once("]:") else { return false };
        if h.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        (h, p)
    } else {
        let Some((h, p)) = s.rsplit_once(':') else { return false };
        let ok_host = h.parse::<std::net::Ipv4Addr>().is_ok()
            || (!h.is_empty() && h.len() <= 253 && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') && !h.starts_with(['-', '.']));
        if !ok_host {
            return false;
        }
        (h, p)
    };
    let _ = host;
    matches!(port.parse::<u16>(), Ok(p) if p > 0)
}

impl Config {
    /// Validate every field. Returns a human-readable error for the first problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.addresses.is_empty() {
            return Err("at least one interface address (e.g. 10.8.0.2/24) is required".into());
        }
        for a in &self.addresses {
            if !valid_cidr(a, true) {
                return Err(format!("invalid interface address '{a}' (expected address/prefix)"));
            }
        }
        if self.listen_port == Some(0) {
            return Err("listen port must be 1..65535".into());
        }
        if let Some(m) = self.mtu {
            if !(576..=9000).contains(&m) {
                return Err(format!("MTU {m} out of range (576..9000)"));
            }
        }
        for d in &self.dns {
            if !valid_cidr(d, false) || d.contains('/') {
                return Err(format!("invalid DNS server '{d}' (expected an IP address)"));
            }
        }
        if self.peers.is_empty() {
            return Err("at least one peer is required".into());
        }
        for (i, p) in self.peers.iter().enumerate() {
            let n = i + 1;
            if !valid_key(&p.public_key) {
                return Err(format!("peer {n}: invalid public key"));
            }
            if let Some(k) = p.preshared_key.as_deref() {
                if !k.is_empty() && !valid_key(k) {
                    return Err(format!("peer {n}: invalid preshared key"));
                }
            }
            if let Some(e) = p.endpoint.as_deref() {
                if !e.is_empty() && !valid_endpoint(e) {
                    return Err(format!("peer {n}: invalid endpoint '{e}' (expected host:port)"));
                }
            }
            if p.allowed_ips.is_empty() {
                return Err(format!("peer {n}: allowed IPs / networks are required (e.g. 10.8.0.0/24)"));
            }
            for a in &p.allowed_ips {
                if !valid_cidr(a, false) {
                    return Err(format!("peer {n}: invalid allowed IP '{a}'"));
                }
            }
            if self.peers[..i].iter().any(|o| o.public_key == p.public_key) {
                return Err(format!("peer {n}: duplicate public key"));
            }
        }
        Ok(())
    }
}

// ───────────────────────────── file format ─────────────────────────────

fn render(cfg: &Config, private_key: &str, preshared: &[(String, String)]) -> String {
    let mut s = String::from("# Managed by the Bost FlowStation dashboard (Network tab). Edit there.\n[Interface]\n");
    s.push_str(&format!("PrivateKey = {private_key}\n"));
    s.push_str(&format!("Address = {}\n", cfg.addresses.join(", ")));
    if let Some(p) = cfg.listen_port {
        s.push_str(&format!("ListenPort = {p}\n"));
    }
    if let Some(m) = cfg.mtu {
        s.push_str(&format!("MTU = {m}\n"));
    }
    if !cfg.dns.is_empty() {
        s.push_str(&format!("DNS = {}\n", cfg.dns.join(", ")));
    }
    for p in &cfg.peers {
        s.push_str("\n[Peer]\n");
        s.push_str(&format!("PublicKey = {}\n", p.public_key));
        if let Some((_, k)) = preshared.iter().find(|(pk, _)| *pk == p.public_key) {
            s.push_str(&format!("PresharedKey = {k}\n"));
        }
        if let Some(e) = p.endpoint.as_deref().filter(|e| !e.is_empty()) {
            s.push_str(&format!("Endpoint = {e}\n"));
        }
        s.push_str(&format!("AllowedIPs = {}\n", p.allowed_ips.join(", ")));
        if let Some(k) = p.persistent_keepalive.filter(|k| *k > 0) {
            s.push_str(&format!("PersistentKeepalive = {k}\n"));
        }
    }
    s
}

fn split_list(v: &str) -> Vec<String> {
    v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

fn parse(text: &str) -> Stored {
    let mut st = Stored::default();
    #[derive(PartialEq)]
    enum Sec {
        None,
        Iface,
        Peer,
    }
    let mut sec = Sec::None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.eq_ignore_ascii_case("[interface]") {
            sec = Sec::Iface;
            continue;
        }
        if line.eq_ignore_ascii_case("[peer]") {
            sec = Sec::Peer;
            st.cfg.peers.push(Peer::default());
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            st.unmanaged.push(line.to_string());
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match (&sec, k.to_ascii_lowercase().as_str()) {
            (Sec::Iface, "privatekey") => st.private_key = Some(v.to_string()),
            (Sec::Iface, "address") => st.cfg.addresses.extend(split_list(v)),
            (Sec::Iface, "listenport") => st.cfg.listen_port = v.parse().ok(),
            (Sec::Iface, "mtu") => st.cfg.mtu = v.parse().ok(),
            (Sec::Iface, "dns") => st.cfg.dns.extend(split_list(v)),
            (Sec::Peer, "publickey") => {
                if let Some(p) = st.cfg.peers.last_mut() {
                    p.public_key = v.to_string();
                }
            }
            (Sec::Peer, "presharedkey") => {
                if let Some(p) = st.cfg.peers.last_mut() {
                    p.has_preshared_key = true;
                    st.preshared.push((p.public_key.clone(), v.to_string()));
                }
            }
            (Sec::Peer, "endpoint") => {
                if let Some(p) = st.cfg.peers.last_mut() {
                    p.endpoint = Some(v.to_string());
                }
            }
            (Sec::Peer, "allowedips") => {
                if let Some(p) = st.cfg.peers.last_mut() {
                    p.allowed_ips.extend(split_list(v));
                }
            }
            (Sec::Peer, "persistentkeepalive") => {
                if let Some(p) = st.cfg.peers.last_mut() {
                    p.persistent_keepalive = v.parse().ok();
                }
            }
            _ => st.unmanaged.push(line.to_string()),
        }
    }
    st
}

// ───────────────────────────── privileged helpers ─────────────────────────────

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

fn conf_path(iface: &str) -> PathBuf {
    PathBuf::from(format!("/etc/wireguard/{iface}.conf"))
}

/// Run a command (through `sudo -n` when not root), optionally feeding stdin. Returns stdout.
fn run(privileged: bool, program: &str, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let mut cmd = if privileged && !is_root() {
        let mut c = Command::new("sudo");
        c.arg("-n").arg(program);
        c
    } else {
        Command::new(program)
    };
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{program} not found (install wireguard-tools)")
        } else {
            e.to_string()
        }
    })?;
    if let (Some(input), Some(mut si)) = (stdin, child.stdin.take()) {
        let _ = si.write_all(input.as_bytes());
    }
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = child.wait_with_output().map_err(|e| e.to_string())?;
                if status.success() {
                    return Ok(String::from_utf8_lossy(&out.stdout).to_string());
                }
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                return Err(if err.is_empty() { format!("{program} exited with {}", status.code().unwrap_or(-1)) } else { err });
            }
            Ok(None) if start.elapsed() > CMD_TIMEOUT => {
                let _ = child.kill();
                return Err(format!("{program} timed out"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn read_conf(iface: &str) -> Result<Option<String>, String> {
    let path = conf_path(iface);
    match std::fs::read_to_string(&path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => match run(true, "cat", &[path.to_str().unwrap_or_default()], None) {
            Ok(t) => Ok(Some(t)),
            // /etc/wireguard itself is 0700 root, so "no such file" is indistinguishable from a
            // permission problem without sudo; sudo's own error tells them apart.
            Err(e) if e.contains("No such file") => Ok(None),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        },
    }
}

fn write_conf(iface: &str, content: &str) -> Result<(), String> {
    let path = conf_path(iface);
    let p = path.to_str().ok_or("bad path")?;
    // `umask 077` so the file never exists world-readable, even for an instant.
    run(true, "sh", &["-c", "umask 077; mkdir -p /etc/wireguard; cat > \"$1\"", "sh", p], Some(content))
        .map(|_| ())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn derive_public_key(private_key: &str) -> Option<String> {
    run(false, "wg", &["pubkey"], Some(&format!("{private_key}\n"))).ok().map(|s| s.trim().to_string()).filter(|s| valid_key(s))
}

fn generate_private_key() -> Result<String, String> {
    let k = run(false, "wg", &["genkey"], None)?.trim().to_string();
    if valid_key(&k) { Ok(k) } else { Err("wg genkey returned an invalid key".into()) }
}

// ───────────────────────────── status ─────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PeerStatus {
    pub public_key: String,
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<String>,
    /// Seconds since the last handshake, `None` = never.
    pub handshake_age_s: Option<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Status {
    pub up: bool,
    pub listen_port: Option<u16>,
    pub peers: Vec<PeerStatus>,
}

/// Parse `wg show <iface> dump` (tab-separated; first line = interface, then one line per peer).
fn parse_dump(dump: &str, now_unix: u64) -> Status {
    let mut st = Status { up: true, ..Default::default() };
    for (i, line) in dump.lines().enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        if i == 0 {
            st.listen_port = f.get(2).and_then(|p| p.parse().ok()).filter(|p| *p > 0);
            continue;
        }
        if f.len() < 8 {
            continue;
        }
        let hs: u64 = f[4].parse().unwrap_or(0);
        st.peers.push(PeerStatus {
            public_key: f[0].to_string(),
            endpoint: Some(f[2].to_string()).filter(|e| e != "(none)"),
            allowed_ips: if f[3] == "(none)" { Vec::new() } else { split_list(f[3]) },
            handshake_age_s: if hs == 0 { None } else { Some(now_unix.saturating_sub(hs)) },
            rx_bytes: f[5].parse().unwrap_or(0),
            tx_bytes: f[6].parse().unwrap_or(0),
        });
    }
    st
}

pub fn installed() -> bool {
    Command::new("wg").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn status(iface: &str) -> Status {
    match run(true, "wg", &["show", iface, "dump"], None) {
        Ok(d) => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            parse_dump(&d, now)
        }
        Err(_) => Status::default(),
    }
}

fn boot_enabled(iface: &str) -> bool {
    run(false, "systemctl", &["is-enabled", &format!("wg-quick@{iface}")], None).map(|s| s.trim() == "enabled").unwrap_or(false)
}

// ───────────────────────────── public API ─────────────────────────────

/// Everything the Network tab shows for one tunnel. Never contains a secret.
pub fn overview(iface: &str) -> Result<serde_json::Value, String> {
    if !valid_iface(iface) {
        return Err("invalid interface name".into());
    }
    let tools = installed();
    let stored = read_conf(iface)?.map(|t| parse(&t));
    let (config, has_private_key, public_key, unmanaged) = match &stored {
        Some(s) => (
            Some(s.cfg.clone()),
            s.private_key.is_some(),
            s.private_key.as_deref().and_then(derive_public_key),
            s.unmanaged.clone(),
        ),
        None => (None, false, None, Vec::new()),
    };
    Ok(serde_json::json!({
        "installed": tools,
        "iface": iface,
        "exists": stored.is_some(),
        "config": config,
        "has_private_key": has_private_key,
        "public_key": public_key,
        "unmanaged": unmanaged,
        "status": if tools { status(iface) } else { Status::default() },
        "boot": tools && boot_enabled(iface),
    }))
}

pub struct SaveOptions {
    /// New private key; `None` keeps the stored one.
    pub private_key: Option<String>,
    pub generate_private_key: bool,
    pub overwrite_unmanaged: bool,
    /// Restart the tunnel afterwards if it is up.
    pub apply: bool,
}

pub fn save(iface: &str, cfg: &Config, opt: &SaveOptions) -> Result<(), String> {
    if !valid_iface(iface) {
        return Err("invalid interface name".into());
    }
    cfg.validate()?;
    let existing = read_conf(iface)?.map(|t| parse(&t));
    if let Some(e) = &existing {
        if !e.unmanaged.is_empty() && !opt.overwrite_unmanaged {
            return Err(format!(
                "the existing config has directives this editor does not manage ({}); saving would drop them — confirm the overwrite to continue",
                e.unmanaged.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
            ));
        }
    }
    let private_key = if opt.generate_private_key {
        generate_private_key()?
    } else if let Some(k) = opt.private_key.as_deref().filter(|k| !k.is_empty()) {
        if !valid_key(k) {
            return Err("invalid private key".into());
        }
        k.to_string()
    } else if let Some(k) = existing.as_ref().and_then(|e| e.private_key.clone()) {
        k
    } else {
        return Err("a private key is required (paste one or generate a new one)".into());
    };
    // Preshared keys: a peer's new value wins, "" removes it, absent keeps the stored one.
    let mut preshared: Vec<(String, String)> = Vec::new();
    for p in &cfg.peers {
        match p.preshared_key.as_deref() {
            Some("") => {}
            Some(k) => preshared.push((p.public_key.clone(), k.to_string())),
            None => {
                if let Some((_, k)) = existing.as_ref().and_then(|e| e.preshared.iter().find(|(pk, _)| *pk == p.public_key)) {
                    preshared.push((p.public_key.clone(), k.clone()));
                }
            }
        }
    }
    write_conf(iface, &render(cfg, &private_key, &preshared))?;
    if opt.apply && status(iface).up {
        run(true, "wg-quick", &["down", iface], None)?;
        run(true, "wg-quick", &["up", iface], None)?;
    }
    Ok(())
}

pub fn set_up(iface: &str, up: bool) -> Result<(), String> {
    if !valid_iface(iface) {
        return Err("invalid interface name".into());
    }
    if read_conf(iface)?.is_none() {
        return Err("no WireGuard config saved yet".into());
    }
    run(true, "wg-quick", &[if up { "up" } else { "down" }, iface], None).map(|_| ())
}

pub fn set_boot(iface: &str, enable: bool) -> Result<(), String> {
    if !valid_iface(iface) {
        return Err("invalid interface name".into());
    }
    run(true, "systemctl", &[if enable { "enable" } else { "disable" }, &format!("wg-quick@{iface}")], None).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "gN65BkIKy1eCE9pP1wdc8ROUtkHLF2PfAqYdyYBz6EA=";
    const K2: &str = "HIgo9xNzJMWLKASShiTqIybxZ0U3wGLiUeJ1PKf8ykw=";

    fn cfg() -> Config {
        Config {
            addresses: vec!["10.8.0.2/24".into()],
            listen_port: Some(51820),
            mtu: Some(1380),
            dns: vec!["1.1.1.1".into()],
            peers: vec![Peer {
                public_key: K1.into(),
                endpoint: Some("vpn.example.org:51820".into()),
                allowed_ips: vec!["10.8.0.0/24".into(), "192.168.50.0/24".into()],
                persistent_keepalive: Some(25),
                ..Default::default()
            }],
        }
    }

    #[test]
    fn render_parse_roundtrip_keeps_everything() {
        let text = render(&cfg(), K2, &[(K1.into(), K2.into())]);
        let st = parse(&text);
        assert!(st.unmanaged.is_empty());
        assert_eq!(st.private_key.as_deref(), Some(K2));
        assert_eq!(st.cfg.mtu, Some(1380));
        assert_eq!(st.cfg.peers[0].allowed_ips, vec!["10.8.0.0/24", "192.168.50.0/24"]);
        assert!(st.cfg.peers[0].has_preshared_key);
        assert_eq!(st.preshared, vec![(K1.to_string(), K2.to_string())]);
    }

    #[test]
    fn unmanaged_directives_are_reported() {
        let st = parse("[Interface]\nPrivateKey = x\nAddress = 10.0.0.1/24\nPostUp = touch /tmp/x\n[Peer]\nPublicKey = y\nAllowedIPs = 0.0.0.0/0\nTable = off\n");
        assert_eq!(st.unmanaged, vec!["PostUp = touch /tmp/x", "Table = off"]);
    }

    #[test]
    fn validation_rejects_injection_and_bad_values() {
        assert!(cfg().validate().is_ok());
        let mut c = cfg();
        c.peers[0].allowed_ips = vec!["10.0.0.0/24\nPostUp = reboot".into()];
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.peers[0].endpoint = Some("host:51820\nPostUp = x".into());
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.mtu = Some(100);
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.addresses = vec!["10.8.0.2".into()]; // no prefix
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.peers[0].public_key = "short".into();
        assert!(c.validate().is_err());
        assert!(valid_iface("wg0") && !valid_iface("wg0;rm") && !valid_iface("../x") && !valid_iface(""));
        assert!(valid_endpoint("[2001:db8::1]:51820") && valid_endpoint("1.2.3.4:1") && !valid_endpoint("host") && !valid_endpoint("host:0"));
    }

    #[test]
    fn dump_parsing() {
        let dump = format!(
            "PRIV\tPUB\t51820\toff\n{K1}\t(none)\t203.0.113.5:51820\t10.8.0.0/24,192.168.50.0/24\t1000\t123\t456\t25\n{K2}\t(none)\t(none)\t(none)\t0\t0\t0\toff\n"
        );
        let st = parse_dump(&dump, 1090);
        assert!(st.up);
        assert_eq!(st.listen_port, Some(51820));
        assert_eq!(st.peers.len(), 2);
        assert_eq!(st.peers[0].handshake_age_s, Some(90));
        assert_eq!(st.peers[0].allowed_ips.len(), 2);
        assert_eq!((st.peers[0].rx_bytes, st.peers[0].tx_bytes), (123, 456));
        assert_eq!(st.peers[1].handshake_age_s, None);
        assert_eq!(st.peers[1].endpoint, None);
    }
}
