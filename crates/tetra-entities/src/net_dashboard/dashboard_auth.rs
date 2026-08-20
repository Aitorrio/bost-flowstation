//! Dashboard-editable `[dashboard]` username / password.
//!
//! Single-account model matching upstream: both keys must be set together (or neither).
//! We patch the TOML surgically so comments and unrelated keys stay intact.

/// Escape a string for a TOML double-quoted literal.
fn escape_toml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Rewrite (or insert) `username` / `password` under `[dashboard]` in `original`.
///
/// - If `[dashboard]` exists, replace active `username` / `password` lines or add them
///   just after the section header when missing.
/// - If no `[dashboard]` section exists, append a minimal one with only those two keys.
pub fn patch_dashboard_credentials(original: &str, username: &str, password: &str) -> String {
    let user_line = format!("username = \"{}\"", escape_toml(username));
    let pass_line = format!("password = \"{}\"", escape_toml(password));

    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 4);

    let mut in_dashboard = false;
    let mut wrote_user = false;
    let mut wrote_pass = false;
    let mut dashboard_seen = false;

    for &line in &lines {
        let trimmed = line.trim_start();

        if trimmed.starts_with('[') && trimmed.contains(']') {
            // Leaving [dashboard] — emit any keys we still owe.
            if in_dashboard {
                if !wrote_user {
                    out.push(user_line.clone());
                    wrote_user = true;
                }
                if !wrote_pass {
                    out.push(pass_line.clone());
                    wrote_pass = true;
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
            // Replace active assignments; leave commented examples alone.
            let is_user = trimmed.trim_start_matches('#').trim_start().starts_with("username");
            let is_pass = trimmed.trim_start_matches('#').trim_start().starts_with("password");

            if is_user && !trimmed.starts_with('#') {
                out.push(user_line.clone());
                wrote_user = true;
                continue;
            }
            if is_pass && !trimmed.starts_with('#') {
                out.push(pass_line.clone());
                wrote_pass = true;
                continue;
            }
        }

        out.push(line.to_string());
    }

    if in_dashboard {
        if !wrote_user {
            out.push(user_line.clone());
            wrote_user = true;
        }
        if !wrote_pass {
            out.push(pass_line.clone());
            wrote_pass = true;
        }
    }

    if !dashboard_seen {
        if !out.is_empty() && !out.last().map(|l| l.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("[dashboard]".to_string());
        out.push(user_line);
        out.push(pass_line);
    } else {
        let _ = (wrote_user, wrote_pass);
    }

    let mut new_content = out.join("\n");
    if original.ends_with('\n') {
        new_content.push('\n');
    }
    new_content
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_credentials() {
        let src = "\
[dashboard]
port = 8080
username = \"old\"
password = \"oldpass\"
";
        let out = patch_dashboard_credentials(src, "admin", "newpass");
        assert!(out.contains("username = \"admin\""));
        assert!(out.contains("password = \"newpass\""));
        assert!(!out.contains("oldpass"));
        assert!(out.contains("port = 8080"));
    }

    #[test]
    fn inserts_into_dashboard_without_auth() {
        let src = "\
[dashboard]
port = 8080
bind = \"0.0.0.0\"
";
        let out = patch_dashboard_credentials(src, "admin", "1234");
        assert!(out.contains("[dashboard]"));
        assert!(out.contains("username = \"admin\""));
        assert!(out.contains("password = \"1234\""));
        assert!(out.contains("port = 8080"));
    }

    #[test]
    fn appends_dashboard_when_missing() {
        let src = "config_version = \"0.6\"\n";
        let out = patch_dashboard_credentials(src, "u", "p");
        assert!(out.contains("[dashboard]"));
        assert!(out.contains("username = \"u\""));
        assert!(out.contains("password = \"p\""));
    }

    #[test]
    fn escapes_quotes() {
        let src = "[dashboard]\n";
        let out = patch_dashboard_credentials(src, "a\"b", "c\\d");
        assert!(out.contains("username = \"a\\\"b\""));
        assert!(out.contains("password = \"c\\\\d\""));
    }

    #[test]
    fn leaves_commented_examples() {
        let src = "\
[dashboard]
# username = \"example\"
# password = \"example\"
username = \"live\"
password = \"live\"
";
        let out = patch_dashboard_credentials(src, "admin", "x");
        assert!(out.contains("# username = \"example\""));
        assert!(out.contains("username = \"admin\""));
        assert_eq!(out.matches("username = \"admin\"").count(), 1);
    }
}
