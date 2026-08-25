//! Persist and read `[dashboard].ota_channel` (`stable` | `beta`).
//!
//! Line-oriented edit of the active `config.toml` (same style as dual_carrier): preserves
//! comments and unrelated keys. Default when absent is `stable` → git branch `bost`.

use tetra_core::{normalize_ota_channel, ota_branch_for_channel};

/// Read the configured OTA channel (`"stable"` or `"beta"`). Missing/invalid → `"stable"`.
pub fn read_ota_channel(config_path: &str) -> String {
    let txt = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut in_dash = false;
    for line in txt.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') && trimmed.contains(']') {
            in_dash = trimmed.starts_with("[dashboard]");
            continue;
        }
        if !in_dash || trimmed.starts_with('#') {
            continue;
        }
        if let Some(v) = active_value(trimmed, "ota_channel") {
            let raw = value_token(v).trim_matches('"').trim_matches('\'');
            return normalize_ota_channel(raw).to_string();
        }
    }
    "stable".to_string()
}

/// Persist `ota_channel` under `[dashboard]`. Creates the section if missing.
pub fn write_ota_channel(config_path: &str, channel: &str) -> std::io::Result<()> {
    let channel = normalize_ota_channel(channel);
    let original = std::fs::read_to_string(config_path).unwrap_or_default();
    let patched = compute_toml(&original, channel);
    // Same style as dual_carrier: direct write. Config path is owned by the service.
    std::fs::write(config_path, patched)
}

/// Testable TOML rewrite.
pub fn compute_toml(original: &str, channel: &str) -> String {
    let channel = normalize_ota_channel(channel);
    let line = format!("ota_channel = \"{channel}\"");
    let mut out: Vec<String> = Vec::new();
    let mut in_dash = false;
    let mut wrote = false;
    let mut saw_dash = false;

    for raw in original.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with('[') && trimmed.contains(']') {
            if in_dash && !wrote {
                out.push(line.clone());
                wrote = true;
            }
            in_dash = trimmed.starts_with("[dashboard]");
            if in_dash {
                saw_dash = true;
            }
            out.push(raw.to_string());
            continue;
        }
        if in_dash && active_value(trimmed, "ota_channel").is_some() {
            if !wrote {
                let indent = &raw[..raw.len() - raw.trim_start().len()];
                out.push(format!("{indent}{line}"));
                wrote = true;
            }
            continue;
        }
        out.push(raw.to_string());
    }
    if in_dash && !wrote {
        out.push(line.clone());
        wrote = true;
    }
    if !saw_dash {
        if !out.is_empty() && !out.last().map(|s| s.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("[dashboard]".to_string());
        out.push(line);
        wrote = true;
    }
    debug_assert!(wrote);
    let mut body = out.join("\n");
    if !original.is_empty() && original.ends_with('\n') && !body.ends_with('\n') {
        body.push('\n');
    }
    body
}

pub fn channel_json(config_path: &str) -> String {
    let channel = read_ota_channel(config_path);
    let branch = ota_branch_for_channel(&channel);
    format!(
        "{{\"channel\":\"{channel}\",\"branch\":\"{branch}\",\"channels\":[\
{{\"id\":\"stable\",\"branch\":\"bost\",\"label\":\"Estable (Bost)\"}},\
{{\"id\":\"beta\",\"branch\":\"beta\",\"label\":\"Beta\"}}\
]}}"
    )
}

fn active_value<'a>(trimmed: &'a str, key: &str) -> Option<&'a str> {
    if !trimmed.starts_with(key) {
        return None;
    }
    trimmed[key.len()..]
        .trim_start()
        .strip_prefix('=')
        .map(str::trim)
}

fn value_token(v: &str) -> &str {
    v.split('#').next().unwrap_or(v).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_stable() {
        assert_eq!(read_ota_channel("/no/such/file.toml"), "stable");
    }

    #[test]
    fn insert_into_existing_dashboard() {
        let src = "[dashboard]\nport = 8080\n";
        let out = compute_toml(src, "beta");
        assert!(out.contains("ota_channel = \"beta\""));
        assert!(out.contains("port = 8080"));
    }

    #[test]
    fn replace_existing() {
        let src = "[dashboard]\nota_channel = \"stable\"\n";
        let out = compute_toml(src, "beta");
        assert_eq!(out.matches("ota_channel").count(), 1);
        assert!(out.contains("ota_channel = \"beta\""));
    }
}
