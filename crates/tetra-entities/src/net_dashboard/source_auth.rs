//! Optional read token for the update source, given by the Brew server with one Fleet Control
//! `CheckUpdate` or `StartUpdate` (field `auth`) for a repository that needs one.
//!
//! The token only lives in memory while that check or update runs ([`scope`]): git gets it as an
//! HTTP header through the `GIT_CONFIG_PARAMETERS` environment variable (what `git -c` uses, also
//! understood by older git such as 2.30 on Debian bullseye) ([`git_env`]), never in a URL,
//! `.git/config` or any other file, and the GitHub requests of the check get it as an
//! `Authorization` header ([`http_headers`]). It is never logged.

use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// The token as it comes in a command; `Debug` never prints it.
#[derive(Clone, PartialEq, Eq, Encode, Decode, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceToken(String);

impl std::fmt::Debug for SourceToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SourceToken(***)")
    }
}

impl SourceToken {
    #[cfg(test)]
    pub fn new(t: &str) -> Self {
        Self(t.to_owned())
    }

    /// GitHub token characters only (no header injection).
    pub fn valid(&self) -> bool {
        (20..=255).contains(&self.0.len()) && self.0.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }
}

/// Token in use and how many checks / updates use it.
static ACTIVE: Mutex<Option<(String, usize)>> = Mutex::new(None);

/// Holds the token for as long as it lives (one check or update).
#[must_use]
pub struct Scope(bool);

/// Use `token` (if any and valid) until the returned scope is dropped.
pub fn scope(token: Option<&SourceToken>) -> Scope {
    let Some(t) = token.filter(|t| t.valid()) else { return Scope(false) };
    let mut a = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    let n = a.as_ref().map(|(_, n)| *n).unwrap_or(0);
    *a = Some((t.0.clone(), n + 1));
    Scope(true)
}

impl Drop for Scope {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let mut a = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, n)) = a.as_mut() {
            *n -= 1;
            if *n == 0 {
                *a = None;
            }
        }
    }
}

fn active() -> Option<String> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(t, _)| t.clone())
}

/// Give a git command (or a script that runs git) the token for GitHub over HTTPS.
pub fn git_env(cmd: &mut std::process::Command) {
    use base64::Engine;
    let Some(t) = active() else { return };
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{t}"));
    // Base64 has no quote characters, so the single-quoted form is safe.
    cmd.env("GIT_CONFIG_PARAMETERS", format!("'http.https://github.com/.extraheader=Authorization: Basic {basic}'"))
        .env("GIT_TERMINAL_PROMPT", "0");
}

/// Default headers for the GitHub requests of the update check.
pub fn http_headers() -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    if let Some(t) = active() {
        if let Ok(mut v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {t}")) {
            v.set_sensitive(true);
            h.insert(reqwest::header::AUTHORIZATION, v);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz";

    fn env_of(cmd: &std::process::Command) -> Vec<(String, String)> {
        cmd.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()).unwrap_or_default())).collect()
    }

    // One test: the token is process-wide.
    #[test]
    fn token_only_inside_its_scope_and_never_printed() {
        let mut none = std::process::Command::new("git");
        git_env(&mut none);
        assert!(env_of(&none).is_empty(), "no token, no environment");
        assert!(http_headers().is_empty());

        let tok = SourceToken::new(TOKEN);
        assert!(!format!("{tok:?}").contains("abcdefghijklmnop"));
        assert!(!SourceToken::new("ghp_0123456789abcdef\r\nX-Evil: 1").valid(), "no header injection");
        {
            let _bad = scope(Some(&SourceToken::new("short")));
            assert!(http_headers().is_empty(), "invalid token is not used");
        }
        {
            let _a = scope(Some(&tok));
            let _b = scope(Some(&tok));
            let mut git = std::process::Command::new("git");
            git_env(&mut git);
            let env = env_of(&git);
            let value = &env.iter().find(|(k, _)| k == "GIT_CONFIG_PARAMETERS").unwrap().1;
            assert!(value.starts_with("'http.https://github.com/.extraheader=Authorization: Basic ") && value.ends_with('\'') && !value.contains(TOKEN), "{value}");
            // git really reads it (the header goes to GitHub only).
            if let Ok(out) = git.args(["config", "--get-urlmatch", "http.extraheader", "https://github.com/owner/repo.git"]).output() {
                if out.status.success() {
                    assert!(String::from_utf8_lossy(&out.stdout).starts_with("Authorization: Basic "));
                }
            }
            let h = http_headers();
            assert!(h[reqwest::header::AUTHORIZATION].is_sensitive());
            drop(_a);
            assert!(!http_headers().is_empty(), "still used by the other scope");
        }
        assert!(http_headers().is_empty(), "gone after the last scope");
    }

    #[test]
    fn command_field_is_optional_and_hidden() {
        use crate::net_control::commands::ControlCommand;
        let c: ControlCommand = serde_json::from_str(&format!(r#"{{"StartUpdate":{{"handle":1,"auth":"{TOKEN}"}}}}"#)).unwrap();
        assert!(!format!("{c:?}").contains("abcdefghijklmnop"));
        assert!(matches!(c, ControlCommand::StartUpdate { auth: Some(ref t), .. } if *t == SourceToken::new(TOKEN)));
        let c: ControlCommand = serde_json::from_str(r#"{"CheckUpdate":{"handle":2}}"#).unwrap();
        assert!(matches!(c, ControlCommand::CheckUpdate { handle: 2, auth: None }));
    }
}
