//! Dashboard-editable SDS command control (U-STATUS → ISSI 9999).
//!
//! Lives under `[cell_info.sds_command_control]` in the TOML. A runtime override on
//! `StackState` applies edits immediately; this module rewrites the nested TOML section
//! surgically so the rest of the file (and Cell/Brew profiles) stay untouched.

use tetra_config::bluestation::{CfgSdsCommandControl, CfgSdsCommandEntry, SdsCommandRuntimeOverride};

/// Actions accepted by `handle_sds_command_status` in `sds_bs`.
pub const ALLOWED_ACTIONS: &[&str] = &["ip", "temp", "info", "restart", "shutdown", "kick_all"];

/// Parse POST body into a runtime override.
///
/// Expected JSON:
/// ```json
/// {
///   "enabled": true,
///   "authorized_issis": [2144485],
///   "commands": [{"status_code": 61000, "action": "ip"}]
/// }
/// ```
pub fn parse_body(body: &str) -> Result<SdsCommandRuntimeOverride, String> {
    let v: serde_json::Value =
        serde_json::from_str(body.trim()).map_err(|e| format!("invalid JSON: {e}"))?;
    let enabled = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);

    let mut authorized_issis: Vec<u32> = Vec::new();
    if let Some(arr) = v.get("authorized_issis").and_then(|x| x.as_array()) {
        for item in arr {
            let n = if let Some(u) = item.as_u64() {
                u
            } else if let Some(s) = item.as_str() {
                s.trim()
                    .parse::<u64>()
                    .map_err(|_| format!("invalid ISSI '{s}'"))?
            } else {
                return Err(format!("invalid ISSI entry: {item}"));
            };
            if n == 0 || n > 0xFF_FFFF {
                return Err(format!("ISSI {n} out of range (1..=16777215)"));
            }
            authorized_issis.push(n as u32);
        }
    }
    authorized_issis.sort_unstable();
    authorized_issis.dedup();

    let mut commands: Vec<CfgSdsCommandEntry> = Vec::new();
    if let Some(arr) = v.get("commands").and_then(|x| x.as_array()) {
        for item in arr {
            let status_code = item
                .get("status_code")
                .and_then(|x| x.as_u64())
                .ok_or_else(|| "command missing status_code".to_string())?;
            if status_code > u16::MAX as u64 {
                return Err(format!("status_code {status_code} out of range (0..=65535)"));
            }
            let action = item
                .get("action")
                .and_then(|x| x.as_str())
                .ok_or_else(|| "command missing action".to_string())?
                .trim()
                .to_ascii_lowercase();
            if !ALLOWED_ACTIONS.contains(&action.as_str()) {
                return Err(format!(
                    "unknown action '{action}' (allowed: {})",
                    ALLOWED_ACTIONS.join(", ")
                ));
            }
            commands.push(CfgSdsCommandEntry {
                status_code: status_code as u16,
                action,
            });
        }
    }

    // Duplicate status codes are ambiguous — reject.
    let mut codes: Vec<u16> = commands.iter().map(|c| c.status_code).collect();
    codes.sort_unstable();
    for w in codes.windows(2) {
        if w[0] == w[1] {
            return Err(format!("duplicate status_code {}", w[0]));
        }
    }

    Ok(SdsCommandRuntimeOverride {
        enabled,
        authorized_issis,
        commands,
    })
}

pub fn override_to_json(ov: &SdsCommandRuntimeOverride) -> String {
    let issis: Vec<String> = ov.authorized_issis.iter().map(|n| n.to_string()).collect();
    let cmds: Vec<String> = ov
        .commands
        .iter()
        .map(|c| {
            format!(
                "{{\"status_code\":{},\"action\":\"{}\"}}",
                c.status_code,
                json_escape(&c.action)
            )
        })
        .collect();
    format!(
        "{{\"enabled\":{},\"authorized_issis\":[{}],\"commands\":[{}],\"control_issi\":9999}}",
        ov.enabled,
        issis.join(","),
        cmds.join(",")
    )
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn render_section(ov: &SdsCommandRuntimeOverride) -> String {
    let mut out = String::new();
    out.push_str("[cell_info.sds_command_control]\n");
    out.push_str("authorized_issis = [");
    out.push_str(
        &ov.authorized_issis
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", "),
    );
    out.push_str("]\n");
    for c in &ov.commands {
        out.push('\n');
        out.push_str("[[cell_info.sds_command_control.commands]]\n");
        out.push_str(&format!("status_code = {}\n", c.status_code));
        out.push_str(&format!("action = \"{}\"\n", c.action));
    }
    out
}

fn is_sds_command_header(trimmed: &str) -> bool {
    trimmed.starts_with("[cell_info.sds_command_control]")
        || trimmed.starts_with("[[cell_info.sds_command_control.commands]]")
}

fn is_other_section_header(trimmed: &str) -> bool {
    if !(trimmed.starts_with('[') && trimmed.contains(']')) {
        return false;
    }
    !is_sds_command_header(trimmed)
}

/// Rewrite or remove `[cell_info.sds_command_control]` (+ command array tables).
/// When `enabled` is false, the section is removed entirely (control off).
pub fn write_to_toml(config_path: &str, ov: &SdsCommandRuntimeOverride) -> std::io::Result<()> {
    let original = std::fs::read_to_string(config_path)?;
    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 16);
    let mut i = 0;
    let mut removed = false;

    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if is_sds_command_header(trimmed) {
            removed = true;
            i += 1;
            while i < lines.len() {
                let t = lines[i].trim_start();
                if is_other_section_header(t) {
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(lines[i].to_string());
        i += 1;
    }

    // Collapse trailing blank lines before appending.
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }

    if ov.enabled {
        out.push(String::new());
        for line in render_section(ov).lines() {
            out.push(line.to_string());
        }
    } else if removed {
        // leave section absent
    }

    let mut new_content = out.join("\n");
    if !new_content.ends_with('\n') {
        new_content.push('\n');
    }

    let backup = format!("{config_path}.sds-cmd.bak");
    let _ = std::fs::copy(config_path, &backup);
    std::fs::write(config_path, new_content)?;
    let _ = removed;
    Ok(())
}

/// Build override snapshot from effective config (for GET when no runtime override yet).
pub fn from_cfg(ctrl: Option<&CfgSdsCommandControl>) -> SdsCommandRuntimeOverride {
    match ctrl {
        Some(c) => SdsCommandRuntimeOverride {
            enabled: true,
            authorized_issis: c.authorized_issis.clone(),
            commands: c.commands.clone(),
        },
        None => SdsCommandRuntimeOverride {
            enabled: false,
            authorized_issis: Vec::new(),
            commands: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok() {
        let ov = parse_body(
            r#"{"enabled":true,"authorized_issis":[2144485],"commands":[{"status_code":61000,"action":"ip"}]}"#,
        )
        .unwrap();
        assert!(ov.enabled);
        assert_eq!(ov.authorized_issis, vec![2144485]);
        assert_eq!(ov.commands[0].status_code, 61000);
        assert_eq!(ov.commands[0].action, "ip");
    }

    #[test]
    fn reject_bad_action() {
        assert!(parse_body(
            r#"{"enabled":true,"authorized_issis":[1],"commands":[{"status_code":1,"action":"boom"}]}"#
        )
        .is_err());
    }

    #[test]
    fn write_roundtrip_insert() {
        let dir = std::env::temp_dir().join(format!("fs_sds_cmd_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "config_version = \"0.6\"\n\n[cell_info]\ncolour_code = 1\nlocation_area = 2\n",
        )
        .unwrap();
        let ov = SdsCommandRuntimeOverride {
            enabled: true,
            authorized_issis: vec![2144485],
            commands: vec![CfgSdsCommandEntry {
                status_code: 61000,
                action: "ip".into(),
            }],
        };
        write_to_toml(path.to_str().unwrap(), &ov).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("[cell_info.sds_command_control]"));
        assert!(out.contains("authorized_issis = [2144485]"));
        assert!(out.contains("status_code = 61000"));
        assert!(out.contains("action = \"ip\""));

        let off = SdsCommandRuntimeOverride {
            enabled: false,
            ..Default::default()
        };
        write_to_toml(path.to_str().unwrap(), &off).unwrap();
        let out2 = std::fs::read_to_string(&path).unwrap();
        assert!(!out2.contains("sds_command_control"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
