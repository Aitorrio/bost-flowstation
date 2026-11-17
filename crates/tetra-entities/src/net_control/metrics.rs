//! Fleet Control (`metrics-v1`): the station reports its load and temperatures to the server once a
//! minute, for the server's graphs and alarms (Fleet Control phase 7).
//!
//! [`ControlResponse::FleetMetrics`] carries the CPU load averaged over the last interval, the CPU
//! temperature, per cell the RF state and the SDR temperature, the registered radios, the calls in
//! progress and the stack health. It is sent only while the server offers Fleet Control and the
//! switch is on; older servers never see it. Protocol: `Docs/fleet-control.md`.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::commands::{CellMetrics, ControlResponse};

/// Capability: the station sends [`ControlResponse::FleetMetrics`].
pub const CAP_METRICS: &str = "metrics-v1";

/// How often the station sends its metrics.
pub const METRICS_INTERVAL: Duration = Duration::from_secs(60);

/// The first report after a connect or after the switch went on comes this much later (it needs a
/// CPU load sample and the SDR's first temperature reading).
pub const FIRST_METRICS_DELAY: Duration = Duration::from_secs(5);

/// An SDR temperature older than this is not reported (the SDR stopped answering).
const SDR_TEMP_MAX_AGE: Duration = Duration::from_secs(120);

/// Latest SDR temperature per cell, fed by the telemetry fan-out.
static SDR_TEMPS: Mutex<BTreeMap<u8, (f32, Instant)>> = Mutex::new(BTreeMap::new());

/// Note the SDR temperature of `cell` (from `SdrHealth` telemetry). `None` (no sensor) is ignored.
pub fn note_sdr_temp(cell: u8, temperature_c: Option<f32>) {
    let Some(t) = temperature_c.filter(|t| t.is_finite()) else { return };
    SDR_TEMPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cell, (t, Instant::now()));
}

fn sdr_temp(cell: u8, now: Instant) -> Option<f32> {
    SDR_TEMPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&cell)
        .filter(|(_, at)| now.duration_since(*at) <= SDR_TEMP_MAX_AGE)
        .map(|(t, _)| (*t * 10.0).round() / 10.0)
}

/// CPU load over the time between two calls, from `/proc/stat`.
#[derive(Debug, Default)]
pub struct CpuSampler {
    last: Option<(u64, u64)>,
}

impl CpuSampler {
    /// Percent busy since the previous call (`None` on the first call or without `/proc/stat`).
    pub fn sample(&mut self) -> Option<f32> {
        let now = std::fs::read_to_string("/proc/stat").ok().and_then(|s| parse_cpu_line(&s))?;
        let pct = self.last.and_then(|prev| cpu_pct(prev, now));
        self.last = Some(now);
        pct
    }
}

/// `(total, idle)` jiffies of the first line of `/proc/stat` (idle includes iowait).
pub fn parse_cpu_line(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().next()?;
    if !line.starts_with("cpu ") {
        return None;
    }
    let nums: Vec<u64> = line.split_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
    if nums.len() < 4 {
        return None;
    }
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    // guest / guest_nice are already part of user / nice.
    let total: u64 = nums.iter().take(8).sum();
    Some((total, idle))
}

/// Percent busy between two `(total, idle)` samples, one decimal.
pub fn cpu_pct(prev: (u64, u64), now: (u64, u64)) -> Option<f32> {
    let dt = now.0.checked_sub(prev.0)?;
    let di = now.1.checked_sub(prev.1)?;
    if dt == 0 {
        return None;
    }
    let busy = dt.saturating_sub(di) as f32 * 100.0 / dt as f32;
    Some((busy * 10.0).round() / 10.0)
}

/// CPU temperature: the first sane thermal zone (Raspberry Pi `cpu-thermal`, x86 `x86_pkg_temp`),
/// else a hwmon CPU driver (`coretemp`, `k10temp`, `cpu_thermal`).
pub fn cpu_temp_c() -> Option<f32> {
    let read_milli = |p: &std::path::Path| -> Option<f32> {
        let t = std::fs::read_to_string(p).ok()?.trim().parse::<i64>().ok()? as f32 / 1000.0;
        (t > 0.0 && t < 150.0).then_some(t)
    };
    let mut zones: Vec<_> = std::fs::read_dir("/sys/class/thermal")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    zones.sort();
    // Prefer a zone that names the CPU, else the first one.
    let typed = |p: &std::path::Path| std::fs::read_to_string(p.join("type")).unwrap_or_default().to_ascii_lowercase();
    let cpu_zone = zones
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("thermal_zone")))
        .find(|p| {
            let t = typed(p);
            t.contains("cpu") || t.contains("x86_pkg") || t.contains("soc")
        })
        .and_then(|p| read_milli(&p.join("temp")));
    if cpu_zone.is_some() {
        return cpu_zone.map(|t| (t * 10.0).round() / 10.0);
    }
    let mut hwmons: Vec<_> = std::fs::read_dir("/sys/class/hwmon")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    hwmons.sort();
    for h in &hwmons {
        let name = std::fs::read_to_string(h.join("name")).unwrap_or_default();
        if matches!(name.trim(), "coretemp" | "k10temp" | "cpu_thermal" | "zenpower") {
            if let Some(t) = read_milli(&h.join("temp1_input")) {
                return Some((t * 10.0).round() / 10.0);
            }
        }
    }
    zones
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("thermal_zone")))
        .find_map(|p| read_milli(&p.join("temp")))
        .map(|t| (t * 10.0).round() / 10.0)
}

/// Name of an RF state on the wire.
pub fn rf_name(state: crate::rf_status::RfState) -> &'static str {
    match state {
        crate::rf_status::RfState::Online => "online",
        crate::rf_status::RfState::Offline => "off",
        crate::rf_status::RfState::Error => "error",
        crate::rf_status::RfState::Starting => "starting",
    }
}

/// The metrics report from the given readings (kept apart from the sources for the tests).
pub fn report(
    cpu_pct: Option<f32>,
    cpu_temp_c: Option<f32>,
    cells: Vec<CellMetrics>,
    radios: usize,
    calls: usize,
    health: &str,
) -> ControlResponse {
    ControlResponse::FleetMetrics {
        interval_secs: METRICS_INTERVAL.as_secs() as u32,
        cpu_pct,
        cpu_temp_c,
        cells,
        radios: radios as u32,
        calls: calls as u32,
        health: health.to_string(),
    }
}

/// The metrics report from the live sources.
pub fn collect(cpu: &mut CpuSampler) -> ControlResponse {
    let now = Instant::now();
    let cells = crate::rf_status::get_all()
        .into_iter()
        .map(|(id, s)| CellMetrics {
            cell: id.0,
            rf: rf_name(s.state).to_string(),
            sdr_temp_c: sdr_temp(id.0, now),
            detail: if s.state == crate::rf_status::RfState::Error {
                s.detail.chars().take(200).collect()
            } else {
                String::new()
            },
        })
        .collect();
    let health = crate::health::registry().overall_level().as_str().to_ascii_lowercase();
    report(
        cpu.sample(),
        cpu_temp_c(),
        cells,
        crate::health::registry().registered_radios(),
        crate::net_dashboard::ota_remote::active_calls(),
        &health,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_load_from_two_proc_stat_samples() {
        let a = parse_cpu_line("cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 1 2 3 4\n").unwrap();
        let b = parse_cpu_line("cpu  200 0 200 1300 200 0 0 0 0 0\n").unwrap();
        assert_eq!(a, (1000, 800));
        assert_eq!(b, (1900, 1500));
        // 900 jiffies, 700 idle -> 22.2 % busy.
        assert_eq!(cpu_pct(a, b), Some(22.2));
        assert_eq!(cpu_pct(a, a), None);
        assert_eq!(cpu_pct(b, a), None);
        assert_eq!(parse_cpu_line("intr 1 2 3"), None);
    }

    #[test]
    fn report_carries_every_reading() {
        let r = report(
            Some(12.5),
            Some(48.0),
            vec![CellMetrics {
                cell: 0,
                rf: "online".into(),
                sdr_temp_c: Some(41.0),
                detail: String::new(),
            }],
            3,
            1,
            "ok",
        );
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["FleetMetrics"]["interval_secs"], 60);
        assert_eq!(json["FleetMetrics"]["cpu_pct"], 12.5);
        assert_eq!(json["FleetMetrics"]["cells"][0]["sdr_temp_c"], 41.0);
        assert_eq!(json["FleetMetrics"]["radios"], 3);
        assert_eq!(json["FleetMetrics"]["calls"], 1);
        assert_eq!(json["FleetMetrics"]["health"], "ok");
    }

    #[test]
    fn sdr_temperature_is_kept_per_cell_and_ages_out() {
        note_sdr_temp(6, Some(52.34));
        note_sdr_temp(6, None); // a reading without a sensor keeps the last value
        let now = Instant::now();
        assert_eq!(sdr_temp(6, now), Some(52.3));
        assert_eq!(sdr_temp(6, now + SDR_TEMP_MAX_AGE + Duration::from_secs(1)), None);
        assert_eq!(sdr_temp(5, now), None);
    }
}
