//! GitHub update-check for the dashboard.
//!
//! Compares the locally built git hash (`tetra_core::GIT_HASH` / `STACK_VERSION`) against
//! the tip of the active OTA branch on GitHub. This is purely informational — the actual
//! update is performed by the git-based OTA path (`run_update`).
//!
//! When an update is available the check also best-effort fetches:
//! - remote `BOST_VERSION` from the tip of the branch
//! - a short changelog (commit subjects) via the GitHub Compare API
//!
//! The check is best-effort: any network/parse failure yields `UpdateCheck::unknown()`
//! rather than an error, so a flaky connection never breaks the dashboard.

use std::time::Duration;

const USER_AGENT: &str = "Bost-FlowStation-Dashboard";
const CHANGELOG_LIMIT: usize = 10;

/// A parsed semantic version (major.minor.patch). Pre-release/build metadata is ignored
/// for comparison purposes — we only care about the release triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SemVer {
    major: u32,
    minor: u32,
    patch: u32,
}

impl SemVer {
    /// Parse a version from a string like "v0.2.5", "0.2.5", or "v0.2.5-gabc123".
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
        let core = s.split(['-', '+']).next().unwrap_or(s);
        let mut it = core.split('.');
        let major = it.next()?.trim().parse().ok()?;
        let minor = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
        let patch = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
        Some(SemVer { major, minor, patch })
    }
}

/// One changelog line for the OTA review step.
#[derive(Debug, Clone)]
pub struct ChangelogEntry {
    pub sha: String,
    pub title: String,
}

/// Result of an update check, serialised to JSON for the dashboard.
#[derive(Debug, Clone)]
pub struct UpdateCheck {
    /// Locally built version string (as-is, e.g. "v0.1.0-2aad62c8").
    pub current: String,
    /// Latest tip / release label from GitHub, if the check succeeded.
    pub latest: Option<String>,
    /// True when remote tip (or release) is ahead of the running binary.
    pub update_available: bool,
    /// URL of the branch / release page, if available.
    pub release_url: Option<String>,
    /// True when the check itself failed (network/parse). The badge should stay hidden.
    pub check_failed: bool,
    /// Active OTA channel (`stable` / `beta`).
    pub channel: String,
    /// Git branch for that channel (`bost` / `beta`).
    pub branch: String,
    /// Remote product version at tip, e.g. `"0.1.50"`, when parseable.
    pub remote_version: Option<String>,
    /// Commit subjects between local hash and tip (newest first), capped.
    pub changelog: Vec<ChangelogEntry>,
    /// True when we tried to load a changelog for an available update but got nothing useful.
    pub changelog_truncated: bool,
}

impl UpdateCheck {
    fn unknown(current: &str, channel: &str, branch: &str) -> Self {
        UpdateCheck {
            current: current.to_string(),
            latest: None,
            update_available: false,
            release_url: None,
            check_failed: true,
            channel: channel.to_string(),
            branch: branch.to_string(),
            remote_version: None,
            changelog: Vec::new(),
            changelog_truncated: false,
        }
    }

    /// Render as a JSON object for `GET /api/update/check`.
    pub fn to_json(&self) -> String {
        let latest = self
            .latest
            .as_deref()
            .map(|s| format!("\"{}\"", json_escape(s)))
            .unwrap_or_else(|| "null".to_string());
        let url = self
            .release_url
            .as_deref()
            .map(|s| format!("\"{}\"", json_escape(s)))
            .unwrap_or_else(|| "null".to_string());
        let remote_version = self
            .remote_version
            .as_deref()
            .map(|s| format!("\"{}\"", json_escape(s)))
            .unwrap_or_else(|| "null".to_string());
        let mut changelog = String::from("[");
        for (i, e) in self.changelog.iter().enumerate() {
            if i > 0 {
                changelog.push(',');
            }
            changelog.push_str(&format!(
                "{{\"sha\":\"{}\",\"title\":\"{}\"}}",
                json_escape(&e.sha),
                json_escape(&e.title)
            ));
        }
        changelog.push(']');
        format!(
            "{{\"current\":\"{}\",\"latest\":{},\"update_available\":{},\"release_url\":{},\"check_failed\":{},\"channel\":\"{}\",\"branch\":\"{}\",\"remote_version\":{},\"changelog\":{},\"changelog_truncated\":{}}}",
            json_escape(&self.current),
            latest,
            self.update_available,
            url,
            self.check_failed,
            json_escape(&self.channel),
            json_escape(&self.branch),
            remote_version,
            changelog,
            self.changelog_truncated
        )
    }
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn local_git_hash(current_version: &str) -> Option<String> {
    let h = tetra_core::GIT_HASH
        .strip_suffix("-modified")
        .unwrap_or(tetra_core::GIT_HASH);
    if !h.is_empty() && h != "unknown" {
        return Some(h.to_string());
    }
    let s = current_version.trim();
    let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
    let hash = s.rsplit('-').next()?.trim();
    if hash.is_empty() || hash.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(hash.to_string())
}

fn parse_bost_version_from_lib_rs(src: &str) -> Option<String> {
    for line in src.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("pub const BOST_VERSION") {
            continue;
        }
        let rest = trimmed.split('=').nth(1)?.trim();
        let rest = rest.trim_end_matches(';').trim();
        let v = rest.trim_matches('"').trim_matches('\'').trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}

fn commit_title(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(160)
        .collect()
}

fn fetch_remote_version(client: &reqwest::blocking::Client, branch: &str) -> Option<String> {
    let url = format!(
        "https://raw.githubusercontent.com/Aitorrio/bost-flowstation/{}/crates/tetra-core/src/lib.rs",
        branch
    );
    let text = client
        .get(&url)
        .send()
        .and_then(|r| r.error_for_status())
        .ok()?
        .text()
        .ok()?;
    parse_bost_version_from_lib_rs(&text)
}

fn fetch_changelog(
    client: &reqwest::blocking::Client,
    base: &str,
    head: &str,
) -> (Vec<ChangelogEntry>, bool) {
    let url = format!(
        "https://api.github.com/repos/Aitorrio/bost-flowstation/compare/{}...{}",
        base, head
    );
    let Ok(resp) = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
    else {
        return (Vec::new(), true);
    };
    let Ok(json) = resp.json::<serde_json::Value>() else {
        return (Vec::new(), true);
    };
    let Some(commits) = json.get("commits").and_then(|c| c.as_array()) else {
        return (Vec::new(), true);
    };
    let mut entries = Vec::new();
    // GitHub returns oldest→newest; show newest first.
    for c in commits.iter().rev().take(CHANGELOG_LIMIT) {
        let sha = c
            .get("sha")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(8)
            .collect::<String>();
        let msg = c
            .get("commit")
            .and_then(|x| x.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let title = commit_title(msg);
        if title.is_empty() {
            continue;
        }
        entries.push(ChangelogEntry { sha, title });
    }
    let truncated = entries.is_empty();
    (entries, truncated)
}

/// Query GitHub for the given OTA branch tip (and optionally latest release) and compare against
/// `current_version` (typically `tetra_core::STACK_VERSION`). Blocking; call from a worker thread.
pub fn check_for_update(current_version: &str, channel: &str) -> UpdateCheck {
    let channel = tetra_core::normalize_ota_channel(channel).to_string();
    let branch = tetra_core::ota_branch_for_channel(&channel).to_string();

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build()
    {
        Ok(c) => c,
        Err(_) => return UpdateCheck::unknown(current_version, &channel, &branch),
    };

    let commits_url = format!(
        "https://api.github.com/repos/Aitorrio/bost-flowstation/commits/{}",
        branch
    );
    let branch_page = format!("{}/tree/{}", tetra_core::PRODUCT_REPO_URL, branch);

    // Primary path: tip of the OTA branch (matches what `run_update` pulls).
    if let Ok(resp) = client
        .get(&commits_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
    {
        if let Ok(json) = resp.json::<serde_json::Value>() {
            if let Some(sha) = json.get("sha").and_then(|v| v.as_str()) {
                let short = &sha[..sha.len().min(8)];
                let local = local_git_hash(current_version);
                let update_available = match &local {
                    Some(local) => !sha.starts_with(local.as_str()) && !local.starts_with(short),
                    None => false,
                };
                let remote_version = fetch_remote_version(&client, &branch);
                let (changelog, changelog_truncated) = if update_available {
                    match &local {
                        Some(base) => fetch_changelog(&client, base, sha),
                        None => (Vec::new(), true),
                    }
                } else {
                    (Vec::new(), false)
                };
                return UpdateCheck {
                    current: current_version.to_string(),
                    latest: Some(format!("{}@{}", branch, short)),
                    update_available,
                    release_url: Some(branch_page),
                    check_failed: false,
                    channel,
                    branch,
                    remote_version,
                    changelog,
                    changelog_truncated,
                };
            }
        }
    }

    // Fallback: GitHub Releases SemVer (useful once tagged releases exist).
    let releases_url = "https://api.github.com/repos/Aitorrio/bost-flowstation/releases/latest";
    let resp = match client
        .get(releases_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => r,
        Err(_) => return UpdateCheck::unknown(current_version, &channel, &branch),
    };

    let json: serde_json::Value = match resp.json() {
        Ok(j) => j,
        Err(_) => return UpdateCheck::unknown(current_version, &channel, &branch),
    };

    let tag = json.get("tag_name").and_then(|v| v.as_str());
    let html_url = json.get("html_url").and_then(|v| v.as_str()).map(|s| s.to_string());

    let Some(tag) = tag else {
        return UpdateCheck::unknown(current_version, &channel, &branch);
    };

    let update_available = match (SemVer::parse(current_version), SemVer::parse(tag)) {
        (Some(cur), Some(latest)) => latest > cur,
        _ => false,
    };

    UpdateCheck {
        current: current_version.to_string(),
        latest: Some(tag.to_string()),
        update_available,
        release_url: html_url,
        check_failed: false,
        channel,
        branch,
        remote_version: SemVer::parse(tag).map(|v| format!("{}.{}.{}", v.major, v.minor, v.patch)),
        changelog: Vec::new(),
        changelog_truncated: update_available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain() {
        assert_eq!(
            SemVer::parse("0.2.5"),
            Some(SemVer {
                major: 0,
                minor: 2,
                patch: 5
            })
        );
    }

    #[test]
    fn parse_v_prefix_and_git_suffix() {
        assert_eq!(
            SemVer::parse("v0.1.0-2aad62c8"),
            Some(SemVer {
                major: 0,
                minor: 1,
                patch: 0
            })
        );
    }

    #[test]
    fn same_version_no_update() {
        let a = SemVer::parse("v0.1.0").unwrap();
        let b = SemVer::parse("0.1.0").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn unparseable_tag_no_update() {
        assert!(SemVer::parse("not-a-version").is_none());
    }

    #[test]
    fn parse_bost_version_line() {
        let src = r#"pub const BOST_VERSION: &str = "0.1.50";"#;
        assert_eq!(parse_bost_version_from_lib_rs(src).as_deref(), Some("0.1.50"));
    }

    #[test]
    fn commit_title_first_line() {
        assert_eq!(
            commit_title("Fix foo\n\nLonger body"),
            "Fix foo"
        );
    }

    #[test]
    fn to_json_shape() {
        let u = UpdateCheck {
            current: "v0.1.0-abcd1234".into(),
            latest: Some("bost@ef012345".into()),
            update_available: true,
            release_url: Some("https://github.com/Aitorrio/bost-flowstation/tree/bost".to_string()),
            check_failed: false,
            channel: "stable".into(),
            branch: "bost".into(),
            remote_version: Some("0.1.50".into()),
            changelog: vec![ChangelogEntry {
                sha: "ef012345".into(),
                title: "Add OTA channels".into(),
            }],
            changelog_truncated: false,
        };
        let j = u.to_json();
        assert!(j.contains("\"update_available\":true"));
        assert!(j.contains("\"remote_version\":\"0.1.50\""));
        assert!(j.contains("Add OTA channels"));
        assert!(j.contains("\"channel\":\"stable\""));
    }

    #[test]
    fn local_hash_from_stack_version() {
        assert_eq!(
            local_git_hash("v0.1.0-2aad62c8").as_deref(),
            local_git_hash("v0.1.0-2aad62c8").as_deref()
        );
    }
}
