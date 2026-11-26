//! "System IP info": the host's name, LAN address and interface, public (WAN) address and Wi-Fi
//! network.
//!
//! A monitor thread looks the addresses up while the Telegram category "System IP info" is on
//! (`[telegram_alerts] alert_system_ip`). Once the station is online after a start it sends them
//! once ("is online"), then again whenever one of them changes ("network changed"). The WAN
//! address comes from a public "what is my IP" service (`ifconfig.me`, `api.ipify.org` as a
//! fallback), so nothing is looked up while the category is off.
//!
//! Like the WX fetch and the Telegram client, everything here is blocking and runs on its own
//! thread, never on the stack loop.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tetra_config::bluestation::SharedConfig;

use crate::net_telegram::TelegramAlertSink;

/// The host's addresses at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SystemIpInfo {
    pub hostname: String,
    /// Address of the interface that carries the default route.
    pub lan_ip: Option<String>,
    /// That interface (`eth0`, `wlan0`, ...).
    pub iface: Option<String>,
    /// Public address as seen from the internet.
    pub wan_ip: Option<String>,
    /// Wi-Fi network, when the default route goes over Wi-Fi.
    pub ssid: Option<String>,
}

/// Services asked for the WAN address, in order. Each answers the bare address as text.
const WAN_SERVICES: &[&str] = &["https://ifconfig.me/ip", "https://api.ipify.org"];
const WAN_TIMEOUT: Duration = Duration::from_secs(5);
/// After a start or a change, wait this long at most for the WAN address before sending without it.
pub const SETTLE: Duration = Duration::from_secs(120);
/// Poll interval while waiting for the network (after a start or a change).
const FAST_POLL: Duration = Duration::from_secs(5);
/// Poll interval for the LAN side once settled (cheap: no subprocess, no request).
const SLOW_POLL: Duration = Duration::from_secs(30);
/// How often the WAN address is looked up again once settled.
const WAN_RECHECK: Duration = Duration::from_secs(300);

/// Last addresses the monitor saw (`None` until the first lookup).
static LATEST: Mutex<Option<SystemIpInfo>> = Mutex::new(None);

/// Last addresses the monitor saw.
pub fn latest() -> Option<SystemIpInfo> {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Host name from the kernel.
pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Address and interface of the default route (no subprocess).
pub fn lan() -> (Option<String>, Option<String>) {
    let ip = crate::sys_telemetry::primary_ip();
    let iface = ip.as_ref().and_then(|ip| {
        crate::host_network::list_ipv4_ifaces_fast()
            .into_iter()
            .find(|(_, a)| a == ip)
            .map(|(name, _)| name)
    });
    (ip, iface)
}

/// Wi-Fi network of `iface` when it is a wireless interface: `iwgetid -r`, else NetworkManager.
pub fn ssid(iface: Option<&str>) -> Option<String> {
    let iface = iface?;
    if !iface.starts_with("wl") {
        return None;
    }
    let from_iwgetid = std::process::Command::new("iwgetid")
        .args([iface, "-r"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    from_iwgetid.or_else(|| crate::wifi::status().ok().and_then(|s| s.connected_ssid))
}

/// The WAN address from a service's answer: one IPv4 or IPv6 address, nothing else.
pub fn parse_wan(body: &str) -> Option<String> {
    body.trim().parse::<std::net::IpAddr>().ok().map(|ip| ip.to_string())
}

fn wan(http: &reqwest::blocking::Client) -> Option<String> {
    WAN_SERVICES.iter().find_map(|url| {
        let resp = http.get(*url).send().ok()?;
        if !resp.status().is_success() {
            return None;
        }
        parse_wan(&resp.text().ok()?)
    })
}

/// Should the monitor look the addresses up? Only while somebody uses them, since the WAN lookup
/// asks a public service.
fn wanted(cfg: &SharedConfig) -> bool {
    let tg = cfg.effective_telegram();
    tg.alert_system_ip && tg.is_deliverable()
}

/// Is it time to send? `announced` is what was sent last (`None`: nothing yet since the start),
/// `waiting` how long `current` (or the change to it) has been waiting for the WAN address.
/// Returns `Some(changed)`: `false` for the first message, `true` for a change.
pub fn decide(announced: Option<&SystemIpInfo>, current: &SystemIpInfo, waiting: Duration) -> Option<bool> {
    if announced == Some(current) {
        return None;
    }
    // No address at all yet: the network is not up, keep waiting (nothing worth sending).
    if current.lan_ip.is_none() && current.wan_ip.is_none() {
        return None;
    }
    if current.wan_ip.is_none() && waiting < SETTLE {
        return None;
    }
    Some(announced.is_some())
}

/// Start the monitor thread. `sink` receives the messages for Telegram.
pub fn spawn_monitor(cfg: SharedConfig, sink: Option<TelegramAlertSink>) {
    let spawned = std::thread::Builder::new()
        .name("system-ip".into())
        .spawn(move || run(cfg, sink));
    if let Err(e) = spawned {
        tracing::warn!("System IP info: could not start the monitor thread: {}", e);
    }
}

fn run(cfg: SharedConfig, sink: Option<TelegramAlertSink>) {
    let http = reqwest::blocking::Client::builder()
        .timeout(WAN_TIMEOUT)
        .user_agent("FlowStation")
        .build()
        .unwrap_or_default();
    let mut announced: Option<SystemIpInfo> = None;
    let mut current: Option<SystemIpInfo> = None;
    let mut waiting_since: Option<Instant> = None;
    let mut last_wan: Option<Instant> = None;
    loop {
        if !wanted(&cfg) {
            // Switched off: the next switch-on sends the addresses again, as after a start.
            announced = None;
            current = None;
            waiting_since = None;
            last_wan = None;
            *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
            std::thread::sleep(Duration::from_secs(10));
            continue;
        }
        let (lan_ip, iface) = lan();
        let lan_changed = current.as_ref().is_none_or(|c| c.lan_ip != lan_ip || c.iface != iface);
        let settled = waiting_since.is_none() && announced.is_some();
        let wan_due = lan_changed || !settled || last_wan.is_none_or(|t| t.elapsed() >= WAN_RECHECK);
        let wan_ip = if wan_due {
            last_wan = Some(Instant::now());
            // A failed lookup with the same LAN keeps the last address (no "changed" for a
            // service that did not answer once).
            wan(&http).or_else(|| if lan_changed { None } else { current.as_ref().and_then(|c| c.wan_ip.clone()) })
        } else {
            current.as_ref().and_then(|c| c.wan_ip.clone())
        };
        let info = SystemIpInfo { hostname: hostname(), ssid: ssid(iface.as_deref()), lan_ip, iface, wan_ip };
        *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some(info.clone());

        if announced.as_ref() == Some(&info) {
            waiting_since = None;
        } else {
            let since = *waiting_since.get_or_insert_with(Instant::now);
            if let Some(changed) = decide(announced.as_ref(), &info, since.elapsed()) {
                tracing::info!(
                    "System IP info: {} LAN {} ({}) WAN {}",
                    if changed { "changed," } else { "online," },
                    info.lan_ip.as_deref().unwrap_or("n/a"),
                    info.iface.as_deref().unwrap_or("-"),
                    info.wan_ip.as_deref().unwrap_or("n/a")
                );
                if let Some(sink) = &sink {
                    sink.send_system_ip(info.clone(), changed);
                }
                announced = Some(info.clone());
                waiting_since = None;
            }
        }
        current = Some(info);
        std::thread::sleep(if waiting_since.is_some() || announced.is_none() { FAST_POLL } else { SLOW_POLL });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(lan: Option<&str>, wan: Option<&str>) -> SystemIpInfo {
        SystemIpInfo {
            hostname: "flowstationpi".into(),
            lan_ip: lan.map(Into::into),
            iface: lan.map(|_| "wlan0".into()),
            wan_ip: wan.map(Into::into),
            ssid: None,
        }
    }

    #[test]
    fn wan_answer_must_be_a_bare_address() {
        assert_eq!(parse_wan("203.0.113.7\n").as_deref(), Some("203.0.113.7"));
        assert_eq!(parse_wan(" 2001:db8::1 ").as_deref(), Some("2001:db8::1"));
        assert_eq!(parse_wan("<html>blocked</html>"), None);
        assert_eq!(parse_wan(""), None);
    }

    #[test]
    fn first_message_waits_for_the_wan_address() {
        let lan_only = info(Some("192.168.1.20"), None);
        assert_eq!(decide(None, &lan_only, Duration::from_secs(10)), None, "WAN not there yet");
        assert_eq!(decide(None, &lan_only, SETTLE), Some(false), "sent without WAN after the wait");
        let full = info(Some("192.168.1.20"), Some("203.0.113.7"));
        assert_eq!(decide(None, &full, Duration::ZERO), Some(false));
        assert_eq!(decide(None, &info(None, None), SETTLE * 10), None, "no network: nothing to send");
    }

    #[test]
    fn change_is_sent_once_and_only_when_different() {
        let a = info(Some("192.168.1.20"), Some("203.0.113.7"));
        assert_eq!(decide(Some(&a), &a, SETTLE), None);
        let new_wan = info(Some("192.168.1.20"), Some("198.51.100.9"));
        assert_eq!(decide(Some(&a), &new_wan, Duration::ZERO), Some(true));
        let new_lan_no_wan = info(Some("10.0.0.5"), None);
        assert_eq!(decide(Some(&a), &new_lan_no_wan, Duration::from_secs(5)), None, "waits for the WAN");
        assert_eq!(decide(Some(&a), &new_lan_no_wan, SETTLE), Some(true));
    }

    #[test]
    fn ssid_only_for_wireless_interfaces() {
        assert_eq!(ssid(None), None);
        assert_eq!(ssid(Some("eth0")), None);
    }
}
