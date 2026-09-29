//! Dashboard port migration and surgical TOML patches.
//!
//! OTA migration is soft: only purely legacy configs (`port = 8080` with no `https_port`)
//! become `80` + `https_port = 443`. Configs that already chose an HTTPS port are left alone.
//!
//! Runtime may still open fail-soft redirects on 8080/8443 when the canonical HTTPS port is 443.

/// Previous HTTP default before HTTPS-canonical dashboard.
pub const LEGACY_HTTP_PORT: u16 = 8080;
/// Previous HTTPS sidecar default / high-port preset.
pub const LEGACY_HTTPS_PORT: u16 = 8443;

/// Standard preset: HTTP :80 redirects to HTTPS :443.
pub const PRESET_STANDARD_HTTP: u16 = 80;
pub const PRESET_STANDARD_HTTPS: u16 = 443;
/// High-port preset: HTTPS only on :8443 (`port = 0` disables HTTP).
pub const PRESET_HIGH_HTTP: u16 = 0;
pub const PRESET_HIGH_HTTPS: u16 = 8443;

/// Rewrite purely legacy `[dashboard] port = 8080` (no `https_port`) → `80` + `https_port = 443`.
/// Returns `true` when the file was modified.
pub fn migrate_legacy_dashboard_ports(config_path: &str) -> bool {
    let Ok(original) = std::fs::read_to_string(config_path) else {
        return false;
    };
    let patched = compute_toml(&original);
    if patched == original {
        return false;
    }
    match std::fs::write(config_path, &patched) {
        Ok(()) => {
            tracing::info!(
                "Dashboard: migrated pure-legacy ports in {} (HTTP {}→80, added https_port=443)",
                config_path,
                LEGACY_HTTP_PORT
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                "Dashboard: could not write port migration to {}: {}",
                config_path,
                e
            );
            false
        }
    }
}

/// Testable TOML rewrite for soft legacy migration.
pub fn compute_toml(original: &str) -> String {
    // First pass: does [dashboard] already declare https_port?
    let mut in_dash = false;
    let mut has_https = false;
    let mut has_legacy_http = false;
    for raw in original.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with('[') && trimmed.contains(']') {
            in_dash = trimmed.starts_with("[dashboard]");
            continue;
        }
        if !in_dash {
            continue;
        }
        if active_value(trimmed, "https_port").is_some() {
            has_https = true;
        }
        if let Some(v) = active_value(trimmed, "port") {
            if value_token(v) == LEGACY_HTTP_PORT.to_string() {
                has_legacy_http = true;
            }
        }
    }

    // Only rewrite when this is the old FlowStation-style pair: port=8080 and no https_port key.
    if !has_legacy_http || has_https {
        return original.to_string();
    }

    let mut out: Vec<String> = Vec::new();
    in_dash = false;
    let mut wrote_https = false;

    for raw in original.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with('[') && trimmed.contains(']') {
            if in_dash && !wrote_https {
                out.push("https_port = 443".to_string());
                wrote_https = true;
            }
            in_dash = trimmed.starts_with("[dashboard]");
            out.push(raw.to_string());
            continue;
        }
        if in_dash {
            if let Some(v) = active_value(trimmed, "port") {
                let indent = &raw[..raw.len() - raw.trim_start().len()];
                if value_token(v) == LEGACY_HTTP_PORT.to_string() {
                    out.push(format!("{indent}port = 80"));
                } else {
                    out.push(raw.to_string());
                }
                continue;
            }
        }
        out.push(raw.to_string());
    }
    if in_dash && !wrote_https {
        out.push("https_port = 443".to_string());
    }

    let mut s = out.join("\n");
    if original.ends_with('\n') && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Rewrite (or insert) `port` / `https_port` under `[dashboard]`.
pub fn patch_dashboard_ports(original: &str, http_port: u16, https_port: u16) -> String {
    let port_line = format!("port = {http_port}");
    let https_line = format!("https_port = {https_port}");

    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 4);

    let mut in_dashboard = false;
    let mut wrote_port = false;
    let mut wrote_https = false;
    let mut dashboard_seen = false;

    for &line in &lines {
        let trimmed = line.trim_start();

        if trimmed.starts_with('[') && trimmed.contains(']') {
            if in_dashboard {
                if !wrote_port {
                    out.push(port_line.clone());
                    wrote_port = true;
                }
                if !wrote_https {
                    out.push(https_line.clone());
                    wrote_https = true;
                }
            }
            in_dashboard = trimmed.starts_with("[dashboard]");
            if in_dashboard {
                dashboard_seen = true;
            }
            out.push(line.to_string());
            continue;
        }

        if in_dashboard {
            if active_value(trimmed, "port").is_some() {
                out.push(port_line.clone());
                wrote_port = true;
                continue;
            }
            if active_value(trimmed, "https_port").is_some() {
                out.push(https_line.clone());
                wrote_https = true;
                continue;
            }
        }

        out.push(line.to_string());
    }

    if in_dashboard {
        if !wrote_port {
            out.push(port_line.clone());
        }
        if !wrote_https {
            out.push(https_line.clone());
        }
    }

    if !dashboard_seen {
        if !out.is_empty() && !out.last().map(|l| l.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("[dashboard]".to_string());
        out.push(port_line);
        out.push(https_line);
    }

    let mut new_content = out.join("\n");
    if original.ends_with('\n') {
        new_content.push('\n');
    }
    new_content
}

/// Classify a port pair as a known preset name.
pub fn preset_name(http_port: u16, https_port: u16) -> &'static str {
    if http_port == PRESET_STANDARD_HTTP && https_port == PRESET_STANDARD_HTTPS {
        "standard"
    } else if http_port == PRESET_HIGH_HTTP && https_port == PRESET_HIGH_HTTPS {
        "high"
    } else {
        "custom"
    }
}

/// Resolve preset name to `(http_port, https_port)`.
pub fn ports_for_preset(preset: &str) -> Option<(u16, u16)> {
    match preset {
        "standard" => Some((PRESET_STANDARD_HTTP, PRESET_STANDARD_HTTPS)),
        "high" => Some((PRESET_HIGH_HTTP, PRESET_HIGH_HTTPS)),
        _ => None,
    }
}

fn active_value<'a>(trimmed: &'a str, key: &str) -> Option<&'a str> {
    if trimmed.starts_with('#') {
        return None;
    }
    let rest = trimmed.strip_prefix(key)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=')?;
    Some(rest.trim())
}

fn value_token(v: &str) -> String {
    v.split(['#', '\r'])
        .next()
        .unwrap_or(v)
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_8080_without_https_port() {
        let src = "[dashboard]\nport = 8080\nusername = \"admin\"\n";
        let out = compute_toml(src);
        assert!(out.contains("port = 80"));
        assert!(!out.contains("port = 8080"));
        assert!(out.contains("https_port = 443"));
        assert!(out.contains("username = \"admin\""));
    }

    #[test]
    fn leaves_custom_http_port_untouched() {
        let src = "[dashboard]\nport = 9080\n";
        let out = compute_toml(src);
        assert_eq!(out, src);
    }

    #[test]
    fn keeps_8080_when_https_port_present() {
        let src = "[dashboard]\nport = 8080\nhttps_port = 8443\n";
        let out = compute_toml(src);
        assert_eq!(out, src);
        assert!(out.contains("port = 8080"));
        assert!(out.contains("https_port = 8443"));
    }

    #[test]
    fn patch_ports_standard_and_high() {
        let src = "[dashboard]\nport = 80\nhttps_port = 443\nusername = \"admin\"\n";
        let high = patch_dashboard_ports(src, 0, 8443);
        assert!(high.contains("port = 0"));
        assert!(high.contains("https_port = 8443"));
        assert!(high.contains("username = \"admin\""));
        let back = patch_dashboard_ports(&high, 80, 443);
        assert!(back.contains("port = 80"));
        assert!(back.contains("https_port = 443"));
    }

    #[test]
    fn preset_helpers() {
        assert_eq!(preset_name(80, 443), "standard");
        assert_eq!(preset_name(0, 8443), "high");
        assert_eq!(preset_name(8080, 8443), "custom");
        assert_eq!(ports_for_preset("high"), Some((0, 8443)));
    }
}
