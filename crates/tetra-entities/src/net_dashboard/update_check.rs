//! GitHub update-check for the dashboard.
//!
//! Compares the locally built git hash (`tetra_core::GIT_HASH` / `STACK_VERSION`) against
//! the tip of the active OTA branch on GitHub. This is purely informational — the actual
//! update is performed by the git-based OTA path (`run_update`).
//!
//! When an update is available the check also best-effort fetches human-readable notes:
//! 1. `CHANGELOG.md` sections between the local and remote Bost versions
//! 2. GitHub Release body for `v{remote_version}` (if tagged)
//! 3. Commit subjects via the Compare API (fallback)
//!
//! The check is best-effort: any network/parse failure yields `UpdateCheck::unknown()`
//! rather than an error, so a flaky connection never breaks the dashboard.

use std::time::Duration;

const USER_AGENT: &str = "Bost-FlowStation-Dashboard";
const CHANGELOG_LIMIT: usize = 10;

/// A parsed semantic version (major.minor.patch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SemVer {
    major: u32,
    minor: u32,
    patch: u32,
}

impl SemVer {
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
        let core = s.split(['-', '+']).next().unwrap_or(s);
        // Also accept Keep-a-Changelog headers like "[0.1.54]".
        let core = core.trim_start_matches('[').trim_end_matches(']');
        let mut it = core.split('.');
        let major = it.next()?.trim().parse().ok()?;
        let minor = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
        let patch = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
        Some(SemVer { major, minor, patch })
    }

    fn display(self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// One commit line for the technical fallback list.
#[derive(Debug, Clone)]
pub struct ChangelogEntry {
    pub sha: String,
    pub title: String,
}

/// Where `release_notes` came from (for UI hints).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesSource {
    None,
    Changelog,
    Release,
    Commits,
}

impl NotesSource {
    fn as_str(self) -> &'static str {
        match self {
            NotesSource::None => "none",
            NotesSource::Changelog => "changelog",
            NotesSource::Release => "release",
            NotesSource::Commits => "commits",
        }
    }
}

/// Result of an update check, serialised to JSON for the dashboard.
#[derive(Debug, Clone)]
pub struct UpdateCheck {
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub release_url: Option<String>,
    pub check_failed: bool,
    pub channel: String,
    pub branch: String,
    pub remote_version: Option<String>,
    pub changelog: Vec<ChangelogEntry>,
    pub changelog_truncated: bool,
    /// Human-readable notes (CHANGELOG / Release body), plain text with `- ` bullets.
    pub release_notes: Option<String>,
    pub notes_source: NotesSource,
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
            release_notes: None,
            notes_source: NotesSource::None,
        }
    }

    pub fn to_json(&self) -> String {
        let latest = opt_str_json(self.latest.as_deref());
        let url = opt_str_json(self.release_url.as_deref());
        let remote_version = opt_str_json(self.remote_version.as_deref());
        let release_notes = opt_str_json(self.release_notes.as_deref());
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
            "{{\"current\":\"{}\",\"latest\":{},\"update_available\":{},\"release_url\":{},\"check_failed\":{},\"channel\":\"{}\",\"branch\":\"{}\",\"remote_version\":{},\"changelog\":{},\"changelog_truncated\":{},\"release_notes\":{},\"notes_source\":\"{}\"}}",
            json_escape(&self.current),
            latest,
            self.update_available,
            url,
            self.check_failed,
            json_escape(&self.channel),
            json_escape(&self.branch),
            remote_version,
            changelog,
            self.changelog_truncated,
            release_notes,
            self.notes_source.as_str()
        )
    }
}

fn opt_str_json(s: Option<&str>) -> String {
    s.map(|s| format!("\"{}\"", json_escape(s)))
        .unwrap_or_else(|| "null".to_string())
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

/// Parse `## v0.1.54` / `## [0.1.54] - date` headings and collect body until next `## `.
fn parse_changelog_sections(md: &str) -> Vec<(SemVer, String)> {
    let mut sections: Vec<(SemVer, String)> = Vec::new();
    let mut current_ver: Option<SemVer> = None;
    let mut body = String::new();

    let flush = |ver: &mut Option<SemVer>, body: &mut String, out: &mut Vec<(SemVer, String)>| {
        if let Some(v) = ver.take() {
            let text = body.trim().to_string();
            if !text.is_empty() {
                out.push((v, text));
            }
        }
        body.clear();
    };

    for line in md.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("## ") {
            flush(&mut current_ver, &mut body, &mut sections);
            // Take first token that looks like a version.
            let token = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| c == '[' || c == ']' || c == ',' || c == ':');
            current_ver = SemVer::parse(token);
            continue;
        }
        if current_ver.is_some() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        }
    }
    flush(&mut current_ver, &mut body, &mut sections);
    sections
}

/// Sections with version `local < v <= remote`, newest first.
fn notes_from_changelog(md: &str, local: Option<SemVer>, remote: Option<SemVer>) -> Option<String> {
    let remote = remote?;
    let sections = parse_changelog_sections(md);
    let mut selected: Vec<(SemVer, String)> = sections
        .into_iter()
        .filter(|(v, _)| {
            if *v > remote {
                return false;
            }
            match local {
                Some(l) => *v > l,
                None => *v == remote,
            }
        })
        .collect();
    if selected.is_empty() {
        return None;
    }
    selected.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = String::new();
    for (i, (ver, text)) in selected.into_iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&format!("## v{}\n{}", ver.display(), text.trim()));
    }
    Some(out)
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

fn fetch_changelog_md(client: &reqwest::blocking::Client, branch: &str) -> Option<String> {
    let url = format!(
        "https://raw.githubusercontent.com/Aitorrio/bost-flowstation/{}/CHANGELOG.md",
        branch
    );
    client
        .get(&url)
        .send()
        .and_then(|r| r.error_for_status())
        .ok()?
        .text()
        .ok()
}

fn fetch_release_body(client: &reqwest::blocking::Client, version: &str) -> Option<String> {
    let tag = if version.starts_with('v') || version.starts_with('V') {
        version.to_string()
    } else {
        format!("v{version}")
    };
    let url = format!(
        "https://api.github.com/repos/Aitorrio/bost-flowstation/releases/tags/{tag}"
    );
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
        .ok()?;
    let json: serde_json::Value = resp.json().ok()?;
    let body = json.get("body").and_then(|v| v.as_str())?.trim();
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

fn fetch_commit_changelog(
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

fn resolve_notes(
    client: &reqwest::blocking::Client,
    branch: &str,
    current_version: &str,
    remote_version: Option<&str>,
    commits: &[ChangelogEntry],
) -> (Option<String>, NotesSource) {
    let local = SemVer::parse(current_version);
    let remote = remote_version.and_then(SemVer::parse);

    if let Some(md) = fetch_changelog_md(client, branch) {
        if let Some(notes) = notes_from_changelog(&md, local, remote) {
            return (Some(notes), NotesSource::Changelog);
        }
    }
    if let Some(ver) = remote_version {
        if let Some(body) = fetch_release_body(client, ver) {
            return (Some(body), NotesSource::Release);
        }
    }
    if !commits.is_empty() {
        let mut lines = String::new();
        for e in commits {
            if !lines.is_empty() {
                lines.push('\n');
            }
            lines.push_str(&format!("- {} ({})", e.title, e.sha));
        }
        return (Some(lines), NotesSource::Commits);
    }
    (None, NotesSource::None)
}

/// Query GitHub for the given OTA branch tip and compare against `current_version`.
pub fn check_for_update(current_version: &str, channel: &str) -> UpdateCheck {
    let channel = tetra_core::normalize_ota_channel(channel).to_string();
    let branch = tetra_core::ota_branch_for_channel(&channel).to_string();

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
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
                let (changelog, mut changelog_truncated) = if update_available {
                    match &local {
                        Some(base) => fetch_commit_changelog(&client, base, sha),
                        None => (Vec::new(), true),
                    }
                } else {
                    (Vec::new(), false)
                };

                let (release_notes, notes_source) = if update_available {
                    resolve_notes(
                        &client,
                        &branch,
                        current_version,
                        remote_version.as_deref(),
                        &changelog,
                    )
                } else {
                    (None, NotesSource::None)
                };
                if update_available
                    && release_notes.is_none()
                    && changelog.is_empty()
                {
                    changelog_truncated = true;
                }

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
                    release_notes,
                    notes_source,
                };
            }
        }
    }

    // Fallback: GitHub Releases SemVer.
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
    let body = json
        .get("body")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let Some(tag) = tag else {
        return UpdateCheck::unknown(current_version, &channel, &branch);
    };

    let update_available = match (SemVer::parse(current_version), SemVer::parse(tag)) {
        (Some(cur), Some(latest)) => latest > cur,
        _ => false,
    };
    let remote_version =
        SemVer::parse(tag).map(|v| format!("{}.{}.{}", v.major, v.minor, v.patch));
    let (release_notes, notes_source) = if update_available {
        if let Some(b) = body {
            (Some(b), NotesSource::Release)
        } else {
            (None, NotesSource::None)
        }
    } else {
        (None, NotesSource::None)
    };

    UpdateCheck {
        current: current_version.to_string(),
        latest: Some(tag.to_string()),
        update_available,
        release_url: html_url,
        check_failed: false,
        channel,
        branch,
        remote_version,
        changelog: Vec::new(),
        changelog_truncated: update_available && release_notes.is_none(),
        release_notes,
        notes_source,
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
    fn parse_bracket_header() {
        assert_eq!(
            SemVer::parse("[0.1.54]"),
            Some(SemVer {
                major: 0,
                minor: 1,
                patch: 54
            })
        );
    }

    #[test]
    fn notes_range_newest_first() {
        let md = r#"
# Changelog

## v0.1.54

- Feature A

## v0.1.53

- Fix B

## v0.1.52

- Fix C
"#;
        let local = SemVer::parse("0.1.52");
        let remote = SemVer::parse("0.1.54");
        let notes = notes_from_changelog(md, local, remote).unwrap();
        assert!(notes.contains("v0.1.54"));
        assert!(notes.contains("Feature A"));
        assert!(notes.contains("v0.1.53"));
        assert!(!notes.contains("Fix C"));
        assert!(notes.find("v0.1.54").unwrap() < notes.find("v0.1.53").unwrap());
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
            remote_version: Some("0.1.54".into()),
            changelog: vec![ChangelogEntry {
                sha: "ef012345".into(),
                title: "Add OTA channels".into(),
            }],
            changelog_truncated: false,
            release_notes: Some("## v0.1.54\n- Hello".into()),
            notes_source: NotesSource::Changelog,
        };
        let j = u.to_json();
        assert!(j.contains("\"notes_source\":\"changelog\""));
        assert!(j.contains("release_notes"));
        assert!(j.contains("Hello"));
    }
}
