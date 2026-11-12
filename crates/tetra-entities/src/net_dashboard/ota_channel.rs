//! Persist and read where the OTA update comes from: `[dashboard] ota_channel` plus, for a
//! custom source, `ota_repo` and `ota_branch`.
//!
//! Channels:
//! - `installed` (default when absent): the repository (`origin`) and branch of the checkout the
//!   station was installed from, so a station installed from a fork keeps updating from it.
//! - `stable` / `beta`: the product repository, branch `main` / `beta`.
//! - `custom`: `ota_repo` (git URL) and `ota_branch` as entered.
//!
//! Line-oriented edit of the active `config.toml` (same style as dual_carrier): preserves
//! comments and unrelated keys.

use tetra_core::ota_branch_for_channel;

/// Normalize a configured channel. Missing or unknown values mean `installed`.
pub fn normalize_channel(channel: &str) -> &'static str {
    match channel.trim().to_ascii_lowercase().as_str() {
        "stable" => "stable",
        "beta" => "beta",
        "custom" => "custom",
        _ => "installed",
    }
}

/// Read the configured OTA channel. Missing/invalid → `"installed"`.
pub fn read_ota_channel(config_path: &str) -> String {
    let txt = std::fs::read_to_string(config_path).unwrap_or_default();
    normalize_channel(&dashboard_value(&txt, "ota_channel").unwrap_or_default()).to_string()
}

/// Where an OTA update is fetched from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtaSource {
    /// `installed`, `stable`, `beta` or `custom`.
    pub channel: String,
    /// URL given to git (may carry credentials for a private installed origin).
    pub git_url: String,
    /// URL for display: no credentials, no `.git`.
    pub repo_display: String,
    /// `owner/repo` when the source is on GitHub (needed for the update check and notes).
    pub github_slug: Option<String>,
    pub branch: String,
}

impl OtaSource {
    fn new(channel: &str, git_url: &str, branch: &str) -> Self {
        OtaSource {
            channel: channel.to_string(),
            git_url: git_url.trim().to_string(),
            repo_display: strip_url_credentials(git_url),
            github_slug: github_slug(git_url),
            branch: branch.to_string(),
        }
    }
}

/// Resolve the OTA source from the config and the installed checkout (`origin` URL, branch).
pub fn resolve_ota_source(config_path: &str) -> OtaSource {
    let txt = std::fs::read_to_string(config_path).unwrap_or_default();
    let source_dir = dashboard_value(&txt, "source_dir");
    resolve_from(&txt, || installed_checkout(source_dir.as_deref()))
}

/// Testable resolution: `installed` is only called for the `installed` channel.
pub fn resolve_from(config_txt: &str, installed: impl FnOnce() -> (Option<String>, Option<String>)) -> OtaSource {
    let channel = normalize_channel(&dashboard_value(config_txt, "ota_channel").unwrap_or_default());
    let product = || OtaSource::new("stable", tetra_core::PRODUCT_REPO_GIT, ota_branch_for_channel("stable"));
    match channel {
        "stable" | "beta" => OtaSource::new(channel, tetra_core::PRODUCT_REPO_GIT, ota_branch_for_channel(channel)),
        "custom" => {
            let repo = dashboard_value(config_txt, "ota_repo").unwrap_or_default();
            let branch = dashboard_value(config_txt, "ota_branch").unwrap_or_default();
            if validate_repo_url(&repo).is_ok() && validate_branch(&branch).is_ok() {
                OtaSource::new("custom", &repo, &branch)
            } else {
                // A broken custom entry must not leave the station without updates.
                OtaSource { channel: "custom".into(), ..product() }
            }
        }
        _ => match installed() {
            (Some(url), branch) if validate_repo_url(&url).is_ok() => {
                let branch = branch
                    .filter(|b| validate_branch(b).is_ok())
                    .unwrap_or_else(|| ota_branch_for_channel("stable").to_string());
                OtaSource::new("installed", &url, &branch)
            }
            _ => OtaSource { channel: "installed".into(), ..product() },
        },
    }
}

/// `origin` URL (as configured, credentials included) and branch of the station's checkout.
/// `None` where there is no checkout or git fails.
pub fn installed_checkout(source_dir: Option<&str>) -> (Option<String>, Option<String>) {
    let Ok(dir) = crate::net_dashboard::server::resolve_source_dir(source_dir) else {
        return (None, None);
    };
    let dir_str = dir.to_string_lossy().to_string();
    // The checkout belongs to the service user while the station may run as root: allow it
    // for this call only, so a missing safe.directory entry does not hide the source.
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .args(["-c", &format!("safe.directory={dir_str}"), "-C", &dir_str])
            .args(args)
            .output()
            .ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !v.is_empty()).then_some(v)
    };
    let repo = git(&["remote", "get-url", "origin"]);
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD");
    (repo, branch)
}

/// Installed repository (without credentials) and branch, for the System page.
pub fn installed_source(config_path: &str) -> (Option<String>, Option<String>) {
    let txt = std::fs::read_to_string(config_path).unwrap_or_default();
    let (repo, branch) = installed_checkout(dashboard_value(&txt, "source_dir").as_deref());
    (repo.map(|u| strip_url_credentials(&u)), branch)
}

/// `https://user:token@host/path.git` -> `https://host/path`.
pub fn strip_url_credentials(url: &str) -> String {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let rest = match rest.split_once('/') {
                Some((host, path)) => format!("{}/{}", host.rsplit('@').next().unwrap_or(host), path),
                None => rest.rsplit('@').next().unwrap_or(rest).to_string(),
            };
            format!("{scheme}://{rest}")
        }
        None => url.to_string(),
    }
}

/// `owner/repo` for a GitHub URL (`https://github.com/o/r(.git)`, `ssh://git@github.com/o/r`,
/// `git@github.com:o/r.git`).
pub fn github_slug(url: &str) -> Option<String> {
    let u = strip_url_credentials(url);
    let path = if let Some(rest) = u.strip_prefix("git@github.com:") {
        rest.to_string()
    } else {
        let (_, rest) = u.split_once("://")?;
        let (host, path) = rest.split_once('/')?;
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
            return None;
        }
        path.to_string()
    };
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    let (owner, repo) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let ok = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (ok(owner) && ok(repo)).then(|| format!("{owner}/{repo}"))
}

/// Accept only plain https / ssh git URLs: git is run with this as an argument as root, so no
/// option-like values, no other transports (`ext::`, `file://`, local paths), no spaces or quotes.
pub fn validate_repo_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    let scheme_ok = u.starts_with("https://") || u.starts_with("ssh://") || u.starts_with("git@");
    let chars_ok = !u.is_empty()
        && u.len() <= 300
        && u.chars().all(|c| c.is_ascii_graphic() && !matches!(c, '"' | '\'' | '\\' | '`' | '<' | '>'));
    if scheme_ok && chars_ok {
        Ok(())
    } else {
        Err("repository must be an https:// or ssh git URL".into())
    }
}

/// Branch names as git accepts them for our purposes: letters, digits, `. _ - /`, no leading `-`
/// or `/`, no `..`.
pub fn validate_branch(branch: &str) -> Result<(), String> {
    let b = branch.trim();
    let ok = !b.is_empty()
        && b.len() <= 100
        && !b.starts_with(['-', '/'])
        && !b.ends_with(['/', '.'])
        && !b.contains("..")
        && b.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
    if ok { Ok(()) } else { Err("invalid branch name".into()) }
}

/// Persist the OTA source. `custom` needs a valid repository and branch, which are stored as
/// `ota_repo` / `ota_branch`; for the other channels those keys are left as they are, so
/// switching back to `custom` keeps them.
pub fn write_ota_source(config_path: &str, channel: &str, repo: Option<&str>, branch: Option<&str>) -> Result<(), String> {
    let channel = normalize_channel(channel);
    let mut txt = std::fs::read_to_string(config_path).unwrap_or_default();
    if channel == "custom" {
        let repo = repo.map(str::trim).unwrap_or_default();
        let branch = branch.map(str::trim).unwrap_or_default();
        validate_repo_url(repo)?;
        validate_branch(branch)?;
        txt = set_dashboard_key(&txt, "ota_repo", &format!("\"{repo}\""));
        txt = set_dashboard_key(&txt, "ota_branch", &format!("\"{branch}\""));
    }
    txt = set_dashboard_key(&txt, "ota_channel", &format!("\"{channel}\""));
    // Same style as dual_carrier: direct write. Config path is owned by the service.
    std::fs::write(config_path, txt).map_err(|e| format!("failed to save OTA source: {e}"))
}

/// Testable TOML rewrite for `ota_channel` (kept for callers and tests).
pub fn compute_toml(original: &str, channel: &str) -> String {
    set_dashboard_key(original, "ota_channel", &format!("\"{}\"", normalize_channel(channel)))
}

/// Set `key = value` (value already in TOML form) under `[dashboard]`, creating it if missing.
fn set_dashboard_key(original: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
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
        if in_dash && active_value(trimmed, key).is_some() {
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

/// Value of `key` under `[dashboard]` (quotes removed), if set.
fn dashboard_value(txt: &str, key: &str) -> Option<String> {
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
        if let Some(v) = active_value(trimmed, key) {
            let raw = value_token(v).trim_matches('"').trim_matches('\'');
            return Some(raw.to_string());
        }
    }
    None
}

pub fn channel_json(config_path: &str) -> String {
    let txt = std::fs::read_to_string(config_path).unwrap_or_default();
    let src = resolve_ota_source(config_path);
    let (inst_repo, inst_branch) = installed_source(config_path);
    serde_json::json!({
        "channel": src.channel,
        "branch": src.branch,
        "repo": src.repo_display,
        "github": src.github_slug.is_some(),
        "custom_repo": dashboard_value(&txt, "ota_repo").unwrap_or_default(),
        "custom_branch": dashboard_value(&txt, "ota_branch").unwrap_or_default(),
        "installed_repo": inst_repo,
        "installed_branch": inst_branch,
        "channels": [
            {"id": "installed", "label": "Installed"},
            {"id": "stable", "branch": "main", "label": "Estable (main)"},
            {"id": "beta", "branch": "beta", "label": "Beta"},
            {"id": "custom", "label": "Custom"},
        ],
    })
    .to_string()
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
    fn default_installed() {
        assert_eq!(read_ota_channel("/no/such/file.toml"), "installed");
    }

    fn inst(url: &str, br: &str) -> impl FnOnce() -> (Option<String>, Option<String>) {
        let (u, b) = (url.to_string(), br.to_string());
        move || (Some(u), Some(b))
    }

    #[test]
    fn installed_is_the_default_and_follows_the_fork() {
        let s = resolve_from("[dashboard]\nport = 1\n", inst("https://github.com/someone/bost-flowstation.git", "handover"));
        assert_eq!(s.channel, "installed");
        assert_eq!(s.branch, "handover");
        assert_eq!(s.github_slug.as_deref(), Some("someone/bost-flowstation"));
        assert_eq!(s.repo_display, "https://github.com/someone/bost-flowstation");
    }

    #[test]
    fn installed_without_checkout_falls_back_to_product_main() {
        let s = resolve_from("", || (None, None));
        assert_eq!((s.channel.as_str(), s.branch.as_str()), ("installed", "main"));
        assert_eq!(s.git_url, tetra_core::PRODUCT_REPO_GIT);
        let s = resolve_from("", inst("https://github.com/x/y.git", "HEAD~1"));
        assert_eq!(s.branch, "main");
    }

    #[test]
    fn stable_and_beta_ignore_the_checkout() {
        let s = resolve_from("[dashboard]\nota_channel = \"beta\"\n", || panic!("not asked"));
        assert_eq!((s.git_url.as_str(), s.branch.as_str()), (tetra_core::PRODUCT_REPO_GIT, "beta"));
    }

    #[test]
    fn custom_source_is_used_and_bad_values_fall_back() {
        let ok = "[dashboard]\nota_channel = \"custom\"\nota_repo = \"https://github.com/a/b.git\"\nota_branch = \"feature/x\"\n";
        let s = resolve_from(ok, || panic!("not asked"));
        assert_eq!((s.branch.as_str(), s.github_slug.as_deref()), ("feature/x", Some("a/b")));
        let bad = "[dashboard]\nota_channel = \"custom\"\nota_repo = \"ext::sh -c id\"\nota_branch = \"main\"\n";
        let s = resolve_from(bad, || panic!("not asked"));
        assert_eq!(s.git_url, tetra_core::PRODUCT_REPO_GIT);
    }

    #[test]
    fn repo_and_branch_validation() {
        for good in ["https://github.com/a/b.git", "ssh://git@host/a/b", "git@github.com:a/b.git"] {
            assert!(validate_repo_url(good).is_ok(), "{good}");
        }
        for bad in ["", "-uhttps://x", "ext::sh -c id", "file:///etc", "/opt/x", "https://a b", "https://x\"y"] {
            assert!(validate_repo_url(bad).is_err(), "{bad}");
        }
        for good in ["main", "handover", "feature/x-1", "v0.5.8"] {
            assert!(validate_branch(good).is_ok(), "{good}");
        }
        for bad in ["", "-x", "/x", "a..b", "a b", "x/", "a;b"] {
            assert!(validate_branch(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn github_slug_and_credentials() {
        assert_eq!(github_slug("https://user:tok@github.com/o/r.git").as_deref(), Some("o/r"));
        assert_eq!(github_slug("git@github.com:o/r.git").as_deref(), Some("o/r"));
        assert_eq!(github_slug("https://gitlab.com/o/r"), None);
        assert_eq!(strip_url_credentials("https://u:t@github.com/o/r.git"), "https://github.com/o/r");
    }

    #[test]
    fn every_ota_source_is_accepted_by_the_config_parser() {
        let base = include_str!("../../../../example_config/config.toml");
        for channel in ["installed", "stable", "beta", "custom"] {
            let mut txt = set_dashboard_key(base, "ota_channel", &format!("\"{channel}\""));
            if channel == "custom" {
                txt = set_dashboard_key(&txt, "ota_repo", "\"https://github.com/a/b.git\"");
                txt = set_dashboard_key(&txt, "ota_branch", "\"dev\"");
            }
            let cfg = tetra_config::bluestation::from_toml_str(&txt)
                .unwrap_or_else(|e| panic!("ota_channel = {channel}: {e}"));
            assert_eq!(cfg.dashboard.as_ref().map(|d| d.ota_channel.as_str()), Some(channel));
        }
        // No key at all: still a valid config, default "installed".
        let cfg = tetra_config::bluestation::from_toml_str(&set_dashboard_key(base, "port", "8080")).unwrap();
        assert_eq!(cfg.dashboard.unwrap().ota_channel, "installed");
    }

    #[test]
    fn write_custom_source_round_trips() {
        let dir = std::env::temp_dir().join(format!("ota-src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, "[dashboard]\nport = 8080\n").unwrap();
        let path = p.to_str().unwrap();
        assert!(write_ota_source(path, "custom", Some("ext::x"), Some("main")).is_err());
        write_ota_source(path, "custom", Some("https://github.com/a/b.git"), Some("dev")).unwrap();
        let s = resolve_ota_source(path);
        assert_eq!((s.channel.as_str(), s.branch.as_str()), ("custom", "dev"));
        write_ota_source(path, "beta", None, None).unwrap();
        let txt = std::fs::read_to_string(&p).unwrap();
        assert!(txt.contains("ota_channel = \"beta\"") && txt.contains("ota_repo = \"https://github.com/a/b.git\""));
        assert!(txt.contains("port = 8080"));
        let _ = std::fs::remove_dir_all(&dir);
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
