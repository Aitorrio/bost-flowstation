//! Dashboard Dual-Carrier support (cell_info switch + Soapy passband helpers).
//!
//! Enabling dual carrier is a config-file operation applied via controlled restart. The secondary
//! carrier number is remembered across OFF; `dual_carrier_enabled` is the operational switch.
//! When enabling, we also persist an effective `sample_rate` (and midway TX/RX centers) so
//! `StackConfig::validate` can prove both carriers fit the SDR passband — matching the Fs the
//! device already uses at runtime when the key was omitted from TOML.

/// Default Fs when TOML omits `sample_rate` (SXceiver / MuCell device default).
pub const DEFAULT_SAMPLE_RATE_HZ: f64 = 600_000.0;
/// TETRA carrier channel spacing.
pub const CARRIER_CHANNEL_HZ: f64 = 25_000.0;

/// Current dual-carrier configuration as read straight from the TOML file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DualCarrierState {
    /// The `dual_carrier_enabled` switch (absent = true for backward compatibility).
    pub enabled: bool,
    /// The configured `secondary_carrier` number, if any (preserved even while disabled).
    pub secondary_carrier: Option<u16>,
}

impl DualCarrierState {
    /// Dual carrier is operationally active only when switched on AND a carrier is configured.
    pub fn active(&self) -> bool {
        self.enabled && self.secondary_carrier.is_some()
    }
}

/// Read the dual-carrier switch + configured secondary carrier from the TOML file.
pub fn read_dual_carrier(config_path: &str) -> DualCarrierState {
    let txt = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut in_cell = false;
    let mut enabled = true;
    let mut secondary_carrier = None;

    for line in txt.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') && trimmed.contains(']') {
            in_cell = trimmed.starts_with("[cell_info]");
            continue;
        }
        if !in_cell || trimmed.starts_with('#') {
            continue;
        }
        if let Some(v) = active_value(trimmed, "secondary_carrier") {
            secondary_carrier = value_token(v).parse::<u16>().ok();
        } else if let Some(v) = active_value(trimmed, "dual_carrier_enabled") {
            enabled = value_token(v) == "true";
        }
    }
    DualCarrierState {
        enabled,
        secondary_carrier,
    }
}

/// For an active (uncommented) `key = <value>` line, return the trimmed value part; else None.
fn active_value<'a>(trimmed: &'a str, key: &str) -> Option<&'a str> {
    if !trimmed.starts_with(key) {
        return None;
    }
    trimmed[key.len()..].trim_start().strip_prefix('=').map(str::trim)
}

/// Strip a trailing `# inline comment` and surrounding whitespace from a TOML scalar value.
fn value_token(v: &str) -> &str {
    v.split('#').next().unwrap_or(v).trim()
}

/// Max `|secondary − main|` (carrier numbers) when LO is placed midway between the two carriers.
/// Both must fit in the SDR passband: `|f1−f2| ≤ Fs` ⇒ Δcarriers ≤ Fs / 25 kHz.
pub fn max_carrier_delta(sample_rate_hz: f64) -> u16 {
    if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
        return 1;
    }
    let d = (sample_rate_hz / CARRIER_CHANNEL_HZ).floor() as i64;
    d.clamp(1, 3998) as u16
}

/// Clamp a requested secondary carrier into the passband around `main` for the given Fs.
/// Never returns `main`; prefers `main+1` when the request equals main.
pub fn clamp_secondary_carrier(main: u16, want: u16, sample_rate_hz: f64) -> u16 {
    let max_d = max_carrier_delta(sample_rate_hz) as i32;
    let m = main as i32;
    let lo = (m - max_d).max(0);
    let hi = (m + max_d).min(3999);
    if lo >= hi {
        return if m < 3999 { (m + 1) as u16 } else { (m - 1) as u16 };
    }
    let mut s = want as i32;
    if s == m {
        s = if m + 1 <= hi { m + 1 } else { m - 1 };
    }
    s.clamp(lo, hi) as u16
}

/// Produce a new TOML body with `dual_carrier_enabled` (and, when `secondary_carrier` is `Some`,
/// the active `secondary_carrier` key) set inside `[cell_info]`, preserving everything else
/// including comments. When `secondary_carrier` is `None`, any existing `secondary_carrier` line is
/// left untouched (so the configured number is remembered while the switch is off).
pub fn compute_toml(original: &str, enabled: bool, secondary_carrier: Option<u16>) -> String {
    let enabled_line = format!("dual_carrier_enabled = {enabled}");
    let secondary_line = secondary_carrier.map(|c| format!("secondary_carrier = {c}"));

    let mut out: Vec<String> = Vec::new();
    let mut in_cell = false;
    let mut cell_seen = false;
    let mut wrote_enabled = false;
    let mut wrote_secondary = secondary_line.is_none();

    let is_active_key = |trimmed: &str, key: &str| {
        !trimmed.starts_with('#') && trimmed.starts_with(key) && trimmed[key.len()..].trim_start().starts_with('=')
    };

    let flush_missing = |out: &mut Vec<String>, wrote_enabled: &mut bool, wrote_secondary: &mut bool| {
        if !*wrote_enabled {
            out.push(enabled_line.clone());
            *wrote_enabled = true;
        }
        if !*wrote_secondary {
            if let Some(ref s) = secondary_line {
                out.push(s.clone());
            }
            *wrote_secondary = true;
        }
    };

    for line in original.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with('[') && trimmed.contains(']') {
            if in_cell {
                flush_missing(&mut out, &mut wrote_enabled, &mut wrote_secondary);
            }
            in_cell = trimmed.starts_with("[cell_info]");
            if in_cell {
                cell_seen = true;
            }
            out.push(line.to_string());
            continue;
        }

        if in_cell {
            if !wrote_enabled && is_active_key(trimmed, "dual_carrier_enabled") {
                out.push(enabled_line.clone());
                wrote_enabled = true;
                continue;
            }
            if !wrote_secondary && is_active_key(trimmed, "secondary_carrier") {
                if let Some(ref s) = secondary_line {
                    out.push(s.clone());
                }
                wrote_secondary = true;
                continue;
            }
        }

        out.push(line.to_string());
    }

    if in_cell {
        flush_missing(&mut out, &mut wrote_enabled, &mut wrote_secondary);
    }

    if !cell_seen {
        if !out.is_empty() && !out.last().map(|l| l.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("[cell_info]".to_string());
        out.push(enabled_line.clone());
        if let Some(ref s) = secondary_line {
            out.push(s.clone());
        }
    }

    let mut new_content = out.join("\n");
    if original.ends_with('\n') {
        new_content.push('\n');
    }
    new_content
}

/// Upsert scalar keys inside `[phy_io.soapysdr]` (create the section if missing).
fn upsert_soapysdr_keys(original: &str, keys: &[(&str, String)]) -> String {
    if keys.is_empty() {
        return original.to_string();
    }
    let mut out: Vec<String> = Vec::new();
    let mut in_soapy = false;
    let mut soapy_seen = false;
    let mut wrote: Vec<bool> = keys.iter().map(|_| false).collect();

    let is_active_key = |trimmed: &str, key: &str| {
        !trimmed.starts_with('#') && trimmed.starts_with(key) && trimmed[key.len()..].trim_start().starts_with('=')
    };

    let flush = |out: &mut Vec<String>, wrote: &mut [bool]| {
        for (i, ((k, v), done)) in keys.iter().zip(wrote.iter_mut()).enumerate() {
            let _ = i;
            if !*done {
                out.push(format!("{k} = {v}"));
                *done = true;
            }
        }
    };

    for line in original.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') && trimmed.contains(']') {
            if in_soapy {
                flush(&mut out, &mut wrote);
            }
            in_soapy = trimmed.starts_with("[phy_io.soapysdr]");
            if in_soapy {
                soapy_seen = true;
            }
            out.push(line.to_string());
            continue;
        }
        if in_soapy {
            let mut replaced = false;
            for (i, (k, v)) in keys.iter().enumerate() {
                if !wrote[i] && is_active_key(trimmed, k) {
                    out.push(format!("{k} = {v}"));
                    wrote[i] = true;
                    replaced = true;
                    break;
                }
            }
            if replaced {
                continue;
            }
        }
        out.push(line.to_string());
    }
    if in_soapy {
        flush(&mut out, &mut wrote);
    }
    if !soapy_seen {
        if !out.is_empty() && !out.last().map(|l| l.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("[phy_io.soapysdr]".to_string());
        flush(&mut out, &mut wrote);
    }
    let mut new_content = out.join("\n");
    if original.ends_with('\n') {
        new_content.push('\n');
    }
    new_content
}

/// Build TOML for enabling dual carrier: cell flags + sample_rate + midway centers.
/// `sample_rate_hz` should be the effective Fs (config or device default).
pub fn build_enabled_toml(
    original: &str,
    secondary: u16,
    sample_rate_hz: f64,
    main_dl_hz: f64,
    main_ul_hz: f64,
    sec_dl_hz: f64,
    sec_ul_hz: f64,
) -> String {
    let with_cell = compute_toml(original, true, Some(secondary));
    let tx_c = (main_dl_hz + sec_dl_hz) / 2.0;
    let rx_c = (main_ul_hz + sec_ul_hz) / 2.0;
    upsert_soapysdr_keys(
        &with_cell,
        &[
            ("sample_rate", format!("{sample_rate_hz}")),
            ("tx_center_freq", format!("{tx_c}")),
            ("rx_center_freq", format!("{rx_c}")),
        ],
    )
}

/// Apply dual-carrier disable (flag only; secondary + RF keys kept).
pub fn write_dual_carrier(config_path: &str, enabled: bool, secondary_carrier: Option<u16>) -> std::io::Result<()> {
    let original = std::fs::read_to_string(config_path)?;
    let new_content = compute_toml(&original, enabled, secondary_carrier);
    let backup = format!("{config_path}.dualcarrier.bak");
    let _ = std::fs::copy(config_path, &backup);
    std::fs::write(config_path, new_content)
}

/// Write a fully prepared dual-enable TOML body (already includes RF keys).
pub fn write_toml_body(config_path: &str, new_content: &str) -> std::io::Result<()> {
    let backup = format!("{config_path}.dualcarrier.bak");
    let _ = std::fs::copy(config_path, &backup);
    std::fs::write(config_path, new_content)
}

/// Effective sample rate from parsed config (TOML key or device default).
pub fn effective_sample_rate_hz(fs_from_toml: Option<f64>) -> f64 {
    fs_from_toml
        .filter(|f| f.is_finite() && *f > 0.0)
        .unwrap_or(DEFAULT_SAMPLE_RATE_HZ)
}

/// Read `sample_rate` from TOML soapysdr if present (line scan; no full parse).
pub fn read_sample_rate_from_toml(config_path: &str) -> Option<f64> {
    let txt = std::fs::read_to_string(config_path).ok()?;
    let mut in_soapy = false;
    for line in txt.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') && trimmed.contains(']') {
            in_soapy = trimmed.starts_with("[phy_io.soapysdr]");
            continue;
        }
        if !in_soapy || trimmed.starts_with('#') {
            continue;
        }
        if let Some(v) = active_value(trimmed, "sample_rate") {
            return value_token(v).parse::<f64>().ok();
        }
    }
    None
}

/// Sync dual-carrier + Soapy RF keys into the active Cell profile JSON (best-effort).
pub fn sync_active_cell_profile(
    config_path: &str,
    enabled: bool,
    secondary: Option<u16>,
    sample_rate_hz: Option<f64>,
    tx_center_hz: Option<f64>,
    rx_center_hz: Option<f64>,
) -> Result<(), String> {
    use crate::net_dashboard::profiles::{profile_file_for_cell, read_active};
    use serde_json::{json, Map, Value as JsonValue};

    let active = read_active(config_path);
    let path = profile_file_for_cell(config_path, &active.cell)?;
    let txt = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut root: JsonValue = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "cell profile root must be an object".to_string())?;

    let cell = obj
        .entry("cell_info".to_string())
        .or_insert_with(|| JsonValue::Object(Map::new()));
    let cell_obj = cell
        .as_object_mut()
        .ok_or_else(|| "cell_info must be an object".to_string())?;
    cell_obj.insert("dual_carrier_enabled".into(), JsonValue::Bool(enabled));
    if let Some(s) = secondary {
        cell_obj.insert("secondary_carrier".into(), json!(s));
    }

    if enabled {
        let phy = obj
            .entry("phy_io".to_string())
            .or_insert_with(|| json!({"backend":"SoapySdr","soapysdr":{}}));
        let phy_obj = phy
            .as_object_mut()
            .ok_or_else(|| "phy_io must be an object".to_string())?;
        let soapy = phy_obj
            .entry("soapysdr".to_string())
            .or_insert_with(|| JsonValue::Object(Map::new()));
        let soapy_obj = soapy
            .as_object_mut()
            .ok_or_else(|| "soapysdr must be an object".to_string())?;
        if let Some(fs) = sample_rate_hz {
            soapy_obj.insert("sample_rate".into(), json!(fs));
        }
        if let Some(tx) = tx_center_hz {
            soapy_obj.insert("tx_center_freq".into(), json!(tx));
        }
        if let Some(rx) = rx_center_hz {
            soapy_obj.insert("rx_center_freq".into(), json!(rx));
        }
    }

    let rendered = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(&path, rendered).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
[net]
mcc = 1

[cell_info]
main_carrier = 1521                 # comment kept
# secondary_carrier = 1522          # optional, commented by default
duplex_spacing = 4

[phy_io.soapysdr]
tx_freq = 438025000.0
rx_freq = 433025000.0

[security]
issi_whitelist = []
";

    #[test]
    fn enable_inserts_both_keys_and_keeps_comments() {
        let out = compute_toml(SAMPLE, true, Some(1522));
        assert!(out.contains("dual_carrier_enabled = true"));
        assert!(out.contains("secondary_carrier = 1522"));
        assert!(out.contains("# secondary_carrier = 1522          # optional, commented by default"));
        assert!(out.contains("[security]"));
        assert!(out.contains("main_carrier = 1521"));
        let cell_idx = out.find("[cell_info]").unwrap();
        let sec_idx = out.find("[security]").unwrap();
        let enabled_idx = out.find("dual_carrier_enabled = true").unwrap();
        assert!(cell_idx < enabled_idx && enabled_idx < sec_idx);
    }

    #[test]
    fn disable_sets_flag_and_keeps_existing_secondary() {
        let enabled = compute_toml(SAMPLE, true, Some(1522));
        let disabled = compute_toml(&enabled, false, None);
        assert!(disabled.contains("dual_carrier_enabled = false"));
        assert!(!disabled.contains("dual_carrier_enabled = true"));
        assert!(disabled.contains("secondary_carrier = 1522"));
    }

    #[test]
    fn toggling_is_idempotent_no_duplicate_keys() {
        let once = compute_toml(SAMPLE, true, Some(1522));
        let twice = compute_toml(&once, true, Some(1530));
        assert_eq!(twice.matches("dual_carrier_enabled =").count(), 1);
        assert_eq!(
            twice.lines().filter(|l| l.trim_start().starts_with("secondary_carrier =")).count(),
            1
        );
        assert!(twice.contains("secondary_carrier = 1530"));
    }

    #[test]
    fn read_back_round_trips() {
        let dir = std::env::temp_dir();
        let path = dir.join("dc_test_roundtrip.toml");
        let path_str = path.to_str().unwrap();
        std::fs::write(&path, compute_toml(SAMPLE, true, Some(1522))).unwrap();
        let st = read_dual_carrier(path_str);
        assert_eq!(
            st,
            DualCarrierState {
                enabled: true,
                secondary_carrier: Some(1522)
            }
        );
        assert!(st.active());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_cell_section_is_appended() {
        let out = compute_toml("[net]\nmcc = 1\n", true, Some(1522));
        assert!(out.contains("[cell_info]"));
        assert!(out.contains("dual_carrier_enabled = true"));
        assert!(out.contains("secondary_carrier = 1522"));
    }

    #[test]
    fn max_delta_for_600k_is_24() {
        assert_eq!(max_carrier_delta(600_000.0), 24);
    }

    #[test]
    fn clamp_keeps_secondary_in_passband() {
        let main = 1536u16;
        assert_eq!(clamp_secondary_carrier(main, 1537, 600_000.0), 1537);
        assert_eq!(clamp_secondary_carrier(main, 2000, 600_000.0), 1536 + 24);
        assert_eq!(clamp_secondary_carrier(main, 1000, 600_000.0), 1536 - 24);
        assert_ne!(clamp_secondary_carrier(main, main, 600_000.0), main);
    }

    #[test]
    fn build_enabled_injects_fs_and_centers() {
        let out = build_enabled_toml(
            SAMPLE,
            1522,
            600_000.0,
            438_025_000.0,
            433_025_000.0,
            438_050_000.0,
            433_050_000.0,
        );
        assert!(out.contains("dual_carrier_enabled = true"));
        assert!(out.contains("secondary_carrier = 1522"));
        assert!(out.contains("sample_rate = 600000"));
        assert!(out.contains("tx_center_freq = 438037500"));
        assert!(out.contains("rx_center_freq = 433037500"));
    }
}
