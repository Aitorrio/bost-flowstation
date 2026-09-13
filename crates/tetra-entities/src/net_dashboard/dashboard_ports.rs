//! Migrate legacy dashboard ports (HTTP 8080 / HTTPS 8443) → 80 / 443.
//!
//! Line-oriented edit of `config.toml` (same style as `ota_channel`): preserves comments.
//! Safe for OTA: bookmarks to `:8080` / `:8443` still work via runtime redirect listeners.

/// Previous HTTP default before HTTPS-canonical dashboard.
pub const LEGACY_HTTP_PORT: u16 = 8080;
/// Previous HTTPS sidecar default.
pub const LEGACY_HTTPS_PORT: u16 = 8443;

/// Rewrite legacy `[dashboard] port = 8080` → `80` and ensure `https_port = 443`.
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
                "Dashboard: migrated legacy ports in {} (HTTP {}→80, HTTPS {}→443)",
                config_path,
                LEGACY_HTTP_PORT,
                LEGACY_HTTPS_PORT
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

/// Testable TOML rewrite.
pub fn compute_toml(original: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_dash = false;
    let mut saw_dash = false;
    let mut wrote_https = false;
    let mut changed_port = false;

    for raw in original.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with('[') && trimmed.contains(']') {
            if in_dash && !wrote_https {
                out.push("https_port = 443".to_string());
                wrote_https = true;
            }
            in_dash = trimmed.starts_with("[dashboard]");
            if in_dash {
                saw_dash = true;
            }
            out.push(raw.to_string());
            continue;
        }
        if in_dash {
            if let Some(v) = active_value(trimmed, "port") {
                let indent = &raw[..raw.len() - raw.trim_start().len()];
                let tok = value_token(v);
                if tok == LEGACY_HTTP_PORT.to_string() {
                    out.push(format!("{indent}port = 80"));
                    changed_port = true;
                } else {
                    out.push(raw.to_string());
                }
                continue;
            }
            if active_value(trimmed, "https_port").is_some() {
                if !wrote_https {
                    out.push(raw.to_string());
                    wrote_https = true;
                }
                continue;
            }
        }
        out.push(raw.to_string());
    }
    if in_dash && !wrote_https {
        out.push("https_port = 443".to_string());
        wrote_https = true;
    }
    // Only inject https_port when there is a live [dashboard] section.
    let _ = (saw_dash, changed_port, wrote_https);
    // Preserve final newline style of typical configs.
    let mut s = out.join("\n");
    if original.ends_with('\n') && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn active_value<'a>(trimmed: &'a str, key: &str) -> Option<&'a str> {
    if trimmed.starts_with('#') {
        return None;
    }
    let prefix = format!("{key}");
    let rest = trimmed.strip_prefix(&prefix)?;
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
    fn migrates_8080_and_adds_https_port() {
        let src = "[dashboard]\nport = 8080\nusername = \"admin\"\n";
        let out = compute_toml(src);
        assert!(out.contains("port = 80"));
        assert!(!out.contains("port = 8080"));
        assert!(out.contains("https_port = 443"));
        assert!(out.contains("username = \"admin\""));
    }

    #[test]
    fn leaves_custom_http_port() {
        let src = "[dashboard]\nport = 9080\n";
        let out = compute_toml(src);
        assert!(out.contains("port = 9080"));
        assert!(out.contains("https_port = 443"));
    }

    #[test]
    fn keeps_existing_https_port() {
        let src = "[dashboard]\nport = 8080\nhttps_port = 9443\n";
        let out = compute_toml(src);
        assert!(out.contains("port = 80"));
        assert!(out.contains("https_port = 9443"));
        assert_eq!(out.matches("https_port").count(), 1);
    }
}
