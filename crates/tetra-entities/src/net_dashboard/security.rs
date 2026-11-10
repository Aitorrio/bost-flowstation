//! Dashboard-editable air-interface security: authentication posture, subscriber keys and
//! class 2 / class 3 encryption (EN 300 392-7).
//!
//! Unlike the whitelist, these settings are read by the MM and MAC layers when the stack
//! starts, so a change is written to the TOML and takes effect on the next restart. The
//! page therefore shows both what is *running* and what is *saved*, and offers a restart.
//!
//! The TOML writer is surgical: it rewrites only the `authentication` /
//! `mutual_authentication` keys inside `[security]`, and regenerates the `[security.aie]`
//! table and the `[[security.subscribers]]` array, preserving everything else the operator
//! wrote by hand. A `.security.bak` backup is made first.

use tetra_config::bluestation::sec_security::{AuthenticationMode, CfgSecurity, parse_hex};

/// What the operator asked for, already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityEdit {
    pub authentication: AuthenticationMode,
    pub mutual_authentication: bool,
    pub subscribers: Vec<(u32, [u8; 16])>,
    pub aie: Option<AieEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CckEdit {
    pub key: [u8; 10],
    pub id: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieEdit {
    pub class3: Option<CckEdit>,
    /// Encryption enabled; false stages an SCK in class 1 for OTAR only.
    pub enabled: bool,
    /// 1 = TEA1, 2 = TEA2, 3 = TEA3
    pub ksg: u8,
    pub sck: [u8; 10],
    pub sckn: u8,
    pub sck_vn: u16,
    /// Talkgroups left in clear for radios without AIE (the fork's mixed-cell policy).
    pub clear_groups: Vec<u32>,
}

fn current_aie(a: &tetra_config::bluestation::CfgAie) -> AieEdit {
    AieEdit {
        class3: a.class3.as_ref().map(|c| CckEdit { key: c.cck.0, id: c.cck_id }),
        enabled: a.enabled, ksg: a.ksg, sck: a.sck.0,
        sckn: a.sckn, sck_vn: a.sck_vn, clear_groups: a.clear_groups.clone(),
    }
}

// Compare actual keys internally, never masked strings or public fingerprints.
fn settings_equal(a: &CfgSecurity, b: &CfgSecurity) -> bool {
    a.authentication == b.authentication && a.mutual_authentication == b.mutual_authentication
        && a.aie.as_ref().map(current_aie) == b.aie.as_ref().map(current_aie)
        && a.subscribers.iter().map(|s| (s.issi, s.k)).collect::<Vec<_>>()
            == b.subscribers.iter().map(|s| (s.issi, s.k)).collect::<Vec<_>>()
        && a.invalid_subscriber_keys == b.invalid_subscriber_keys
}

fn ksg_name(n: u8) -> String {
    format!("tea{n}")
}

/// Accepts `3`, `"3"`, `"tea3"` or `"TEA3"`.
fn parse_ksg(v: &serde_json::Value) -> Option<u8> {
    if let Some(n) = v.as_u64() {
        return u8::try_from(n).ok();
    }
    let s = v.as_str()?.trim().to_ascii_lowercase();
    s.trim_start_matches("tea").parse().ok()
}

/// Sentinel the browser sends for a key it did not change (it only ever sees a masked copy).
pub const KEEP: &str = "keep";

/// Prefix of the comment lines this writer emits, so a later save removes them with the block.
const AIE_COMMENT_MARK: &str = "# Air-interface encryption (written by the dashboard):";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Show just enough of a key to tell two apart: `0011…eeff`.
pub fn mask_key(bytes: &[u8]) -> String {
    let h = hex(bytes);
    format!("{}…{}", &h[..4], &h[h.len() - 4..])
}

fn auth_str(m: AuthenticationMode) -> &'static str {
    match m {
        AuthenticationMode::Off => "off",
        AuthenticationMode::Optional => "optional",
        AuthenticationMode::Required => "required",
    }
}

fn parse_auth(s: &str) -> Result<AuthenticationMode, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" => Ok(AuthenticationMode::Off),
        "optional" => Ok(AuthenticationMode::Optional),
        "required" => Ok(AuthenticationMode::Required),
        other => Err(format!("authentication must be off, optional or required (got {other:?})")),
    }
}

/// One security posture as the browser sees it (keys masked).
pub fn posture_json(sec: &CfgSecurity) -> serde_json::Value {
    serde_json::json!({
        "class": sec.aie.as_ref().map_or(1, |a| a.security_class()),
        "authentication": auth_str(sec.authentication),
        "mutual_authentication": sec.mutual_authentication,
        "subscribers": sec.subscribers.iter().map(|s| serde_json::json!({
            "issi": s.issi, "k_masked": mask_key(&s.k),
        })).collect::<Vec<_>>(),
        "invalid_subscriber_keys": sec.invalid_subscriber_keys,
        "aie": sec.aie.as_ref().map(|a| serde_json::json!({
            "class": a.security_class(), "cck_id": a.class3.as_ref().map(|c| c.cck_id),
            "cck_masked": a.class3.as_ref().map(|_| "[configured]"),
            "enabled": a.enabled, "ksg": ksg_name(a.ksg), "sck_masked": mask_key(&a.sck.0), "sckn": a.sckn, "sck_vn": a.sck_vn,
            "clear_groups": a.clear_groups, "weak": a.ksg == 1,
        })),
    })
}

/// The key stream generators this build offers, for the picker.
pub fn ksg_catalogue() -> serde_json::Value {
    serde_json::json!([
        {"id": "tea1", "name": "TEA1", "available": true, "weak": true,
         "note": "Commercial/export algorithm. Keeps only 32 of its 80 key bits (TETRA:BURST, 2023): a few seconds of traffic recovers the key on a laptop. Research and interoperability only."},
        {"id": "tea2", "name": "TEA2", "available": true, "weak": false,
         "note": "European public-safety algorithm (Schengen). No practical weakness published."},
        {"id": "tea3", "name": "TEA3", "available": true, "weak": false,
         "note": "Export public-safety algorithm. A non-bijective S-box is a theoretical weakness only."},
        {"id": "tea4", "name": "TEA4", "available": false, "weak": false,
         "note": "Not implemented in this build."},
    ])
}

/// Everything the Security page needs: the running posture, the saved one, whether they
/// differ, and the algorithm catalogue.
pub fn page_json(running: &CfgSecurity, saved: Result<CfgSecurity, String>, runtime: serde_json::Value) -> String {
    let (saved_json, parse_error, restart_required) = match saved {
        Ok(s) => {
            let sj = posture_json(&s);
            let differs = !settings_equal(&s, running);
            (sj, serde_json::Value::Null, differs)
        }
        Err(e) => (serde_json::Value::Null, serde_json::Value::String(e), false),
    };
    serde_json::json!({
        "running": posture_json(running),
        "saved": saved_json,
        "parse_error": parse_error,
        "restart_required": restart_required,
        "ksgs": ksg_catalogue(),
        "runtime": runtime,
    })
    .to_string()
}

/// Non-secret live status. Never serialize StackState or a key-containing type.
pub fn runtime_json(shared: &Option<tetra_config::bluestation::SharedConfig>) -> serde_json::Value {
    let Some(shared) = shared else { return serde_json::json!({"class": 1, "radios": []}); };
    let cfg = shared.config();
    let sc3 = cfg.security.aie.as_ref().and_then(|a| a.class3.as_ref());
    let state = shared.state_read();
    let radios: Vec<_> = if sc3.is_some() { cfg.security.subscribers.iter().map(|s| {
        let progress = state.sc3_status.get(&s.issi);
        let available = state.sc3_dcks.contains_key(&s.issi);
        serde_json::json!({
            "issi": s.issi, "registered": state.subscribers.is_registered(s.issi),
            "dck_available": available,
            "dck_active": available && state.subscribers.encrypted(s.issi) == Some(true),
            "dck_retained": available && progress.is_some_and(|p| matches!(p.phase, "timeout" | "failed")),
            "phase": progress.map_or("not_authenticated", |p| p.phase),
            "cck": progress.map_or("not_sent", |p| p.cck),
        })
    }).collect() } else { vec![] };
    serde_json::json!({"class": cfg.security.aie.as_ref().map_or(1, |a| a.security_class()),
        "require_sc3": state.security_class3_required, "revision": tetra_core::SC3_REVISION,
        "cck_id": sc3.map(|c| c.cck_id), "radios": radios})
}

pub fn validate_startup_guard(edit: &SecurityEdit, shared: &Option<tetra_config::bluestation::SharedConfig>) -> Result<(), &'static str> {
    if shared.as_ref().is_some_and(|s| s.state_read().security_class3_required)
        && !edit.aie.as_ref().is_some_and(|a| a.class3.is_some()) {
        return Err("Station started with --require-sc3: remove that service option and restart before selecting SC1/SC2");
    }
    Ok(())
}

/// Read the `[security]` settings as they are on disk now (they may be newer than the
/// running stack's copy).
pub fn load_saved(config_path: &str) -> Result<CfgSecurity, String> {
    let text = std::fs::read_to_string(config_path).map_err(|e| format!("cannot read config: {e}"))?;
    tetra_config::bluestation::from_toml_str(&text)
        .map(|c| c.security)
        .map_err(|e| format!("config does not parse: {e}"))
}

/// Validate a POST body against the current (saved) settings, resolving `keep` sentinels.
pub fn parse_edit(body: &str, current: &CfgSecurity) -> Result<SecurityEdit, String> {
    let json: serde_json::Value = serde_json::from_str(body.trim()).map_err(|e| format!("invalid JSON: {e}"))?;
    if !json.is_object() { return Err("security settings must be an object".into()); }
    // Cached SC2 forms must not silently downgrade an existing SC3 configuration.
    if current.aie.as_ref().is_some_and(|a| a.class3.is_some())
        && json.get("aie").is_some_and(|a| a.get("class").is_none()) {
        return Err("SC3: select an explicit security class with the updated dashboard".into());
    }
    let authentication = match json.get("authentication").and_then(|v| v.as_str()) {
        Some(s) => parse_auth(s)?,
        None => current.authentication,
    };
    let mutual_authentication = json
        .get("mutual_authentication")
        .and_then(|v| v.as_bool())
        .unwrap_or(current.mutual_authentication);

    let mut subscribers = Vec::new();
    if let Some(arr) = json.get("subscribers").and_then(|v| v.as_array()) {
        for item in arr {
            let issi = item.get("issi").and_then(|v| v.as_u64()).ok_or("subscriber without ISSI")?;
            if issi == 0 || issi > 0xFF_FFFF {
                return Err(format!("ISSI {issi} out of range (1..=16777215)"));
            }
            let issi = issi as u32;
            if subscribers.iter().any(|(i, _)| *i == issi) {
                return Err(format!("ISSI {issi} listed twice"));
            }
            let k_str = item.get("k").and_then(|v| v.as_str()).unwrap_or(KEEP);
            let k = if k_str == KEEP {
                current
                    .subscribers
                    .iter()
                    .find(|s| s.issi == issi)
                    .map(|s| s.k)
                    .ok_or_else(|| format!("ISSI {issi}: no key on file, enter one"))?
            } else {
                parse_hex::<16>(k_str).ok_or_else(|| format!("ISSI {issi}: K must be 32 hex digits (128 bits)"))?
            };
            subscribers.push((issi, k));
        }
    } else {
        subscribers = current.subscribers.iter().map(|s| (s.issi, s.k)).collect();
    }

    let aie = match json.get("aie") {
        None => current.aie.as_ref().map(current_aie),
        Some(serde_json::Value::Null) => None,
        Some(a) => {
            if !a.is_object() { return Err("aie must be an object".into()); }
            let class = match a.get("class") {
                Some(v) => v.as_u64().filter(|c| (1..=3).contains(c)).ok_or("class must be 1, 2 or 3")?,
                None => if a.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true) { 2 } else { 1 },
            };
            let ksg = match a.get("ksg") {
                Some(v) => parse_ksg(v).ok_or("invalid KSG")?,
                None => current.aie.as_ref().map_or(2, |c| c.ksg),
            };
            if !(1..=3).contains(&ksg) { return Err(format!("TEA{ksg} is not available in this build")); }
            if class == 3 {
                if a.get("enabled").is_some_and(|v| v.as_bool() != Some(true)) {
                    return Err("SC3 requires encryption enabled".into());
                }
                if ["sck", "sckn", "sck_vn"].iter().any(|field| a.get(field).is_some())
                    || a.get("clear_groups").is_some_and(|g| g.as_array().is_none_or(|g| !g.is_empty())) {
                    return Err("SC3 uses CCK/DCK: remove SCK fields and clear_groups".into());
                }
                let old = current.aie.as_ref().and_then(|c| c.class3.as_ref());
                let cck_value = a.get("cck").map(|v| v.as_str().ok_or("CCK must be a string")).transpose()?.unwrap_or(KEEP);
                let key = match cck_value {
                    KEEP => old.map(|c| c.cck.0).ok_or("no CCK on file, enter or generate one")?,
                    value => parse_hex::<10>(value).ok_or("CCK must be 20 hex digits (80 bits)")?,
                };
                let id = match a.get("cck_id") {
                    Some(v) => v.as_u64().filter(|n| *n <= u16::MAX as u64).ok_or("CCK ID must be 0-65535")? as u16,
                    None => old.map(|c| c.cck_id).ok_or("CCK ID is required")?,
                };
                if old.is_some_and(|c| c.cck.0 != key && c.cck_id == id) {
                    return Err("A changed CCK needs a new CCK ID; restart and register the radios again".into());
                }
                Some(AieEdit { class3: Some(CckEdit { key, id }), enabled: true, ksg,
                    sck: [0; 10], sckn: 1, sck_vn: 0, clear_groups: vec![] })
            } else {
                if a.get("cck").is_some() || a.get("cck_id").is_some() {
                    return Err("CCK fields require class 3".into());
                }
                let old = current.aie.as_ref().filter(|c| c.class3.is_none());
                let sck_given = a.get("sck").and_then(|v| v.as_str()).is_some_and(|s| s != KEEP && !s.trim().is_empty());
                if class == 1 && !sck_given && old.is_none() {
                    None
                } else {
                    let sck = match a.get("sck").and_then(|v| v.as_str()).unwrap_or(KEEP) {
                        KEEP => old.map(|c| c.sck.0).ok_or("no SCK on file, enter or generate one")?,
                        value => parse_hex::<10>(value).ok_or("SCK must be 20 hex digits (80 bits)")?,
                    };
                    let number = |name: &str, default: u64, low: u64, high: u64| -> Result<u64, String> {
                        match a.get(name) {
                            None => Ok(default),
                            Some(v) => v.as_u64().filter(|n| (low..=high).contains(n))
                                .ok_or_else(|| format!("{name} must be {low}-{high}")),
                        }
                    };
                    let sckn = number("sckn", old.map_or(1, |c| c.sckn as u64), 1, 32)? as u8;
                    let sck_vn = number("sck_vn", old.map_or(1, |c| c.sck_vn as u64), 0, 65535)? as u16;
                    let clear_groups = match a.get("clear_groups") {
                        Some(v) => v.as_array().ok_or("clear_groups must be an array")?.iter().map(|g| {
                            g.as_u64().filter(|n| (1..=0xFF_FFFF).contains(n)).map(|n| n as u32)
                                .ok_or_else(|| "clear_groups must contain GSSI numbers 1-16777215".to_string())
                        }).collect::<Result<Vec<_>, _>>()?,
                        None => old.map(|c| c.clear_groups.clone()).unwrap_or_default(),
                    };
                    Some(AieEdit { class3: None, enabled: class == 2, ksg, sck, sckn, sck_vn, clear_groups })
                }
            }
        }
    };
    if aie.as_ref().is_some_and(|a| a.class3.is_some()) {
        if authentication != AuthenticationMode::Required { return Err("SC3 requires authentication = required".into()); }
        if subscribers.is_empty() { return Err("SC3 requires at least one subscriber with a matching K".into()); }
    }

    Ok(SecurityEdit { authentication, mutual_authentication, subscribers, aie })
}

/// A fresh random key as hex, from the kernel's generator.
pub fn random_hex(n_bytes: usize) -> std::io::Result<String> {
    use rand::TryRngCore;
    let mut buf = vec![0u8; n_bytes];
    rand::rngs::OsRng.try_fill_bytes(&mut buf).map_err(std::io::Error::other)?;
    Ok(hex(&buf))
}

fn is_header(trimmed: &str) -> bool {
    trimmed.starts_with('[') && trimmed.contains(']')
}

/// The header name without brackets/whitespace: `[[ security.subscribers ]]` → `security.subscribers`.
fn header_name(trimmed: &str) -> String {
    trimmed.trim_matches(|c| c == '[' || c == ']' || c == ' ' || c == '\t').to_string()
}

/// Rewrite the security settings into the TOML text (pure; see [`write_to_toml`]).
pub fn render_toml(original: &str, edit: &SecurityEdit) -> String {
    // Pass 1: drop the blocks we regenerate, and the comment lines we wrote with them.
    let mut kept: Vec<String> = Vec::new();
    let mut skipping = false;
    for line in original.lines() {
        let t = line.trim_start();
        if is_header(t) {
            let name = header_name(t);
            skipping = name == "security.aie" || name == "security.subscribers";
            if skipping {
                continue;
            }
        }
        if !skipping && !t.starts_with(AIE_COMMENT_MARK) {
            kept.push(line.to_string());
        }
    }

    let auth_line = format!("authentication = \"{}\"", auth_str(edit.authentication));
    let mutual_line = format!("mutual_authentication = {}", edit.mutual_authentication);

    // The regenerated sub-tables, appended at the end of the [security] block.
    let mut tail: Vec<String> = Vec::new();
    if let Some(a) = &edit.aie {
        tail.extend([
            String::new(),
            format!("{AIE_COMMENT_MARK} class 1 = clear, class 2 = SCK, class 3 = DCK/CCK."),
            format!("{AIE_COMMENT_MARK} Changes apply after station restart."),
            "[security.aie]".to_string(),
            format!("class = {}", if a.class3.is_some() { 3 } else if a.enabled { 2 } else { 1 }),
            format!("ksg = {}", a.ksg),
        ]);
        if let Some(c) = &a.class3 {
            tail.extend([format!("cck = \"{}\"", hex(&c.key)), format!("cck_id = {}", c.id)]);
        } else {
            tail.extend([
                format!("sck = \"{}\"", hex(&a.sck)), format!("sckn = {}", a.sckn),
                format!("sck_vn = {}", a.sck_vn),
                format!("clear_groups = [{}]", a.clear_groups.iter().map(|g| g.to_string()).collect::<Vec<_>>().join(", ")),
            ]);
        }
    }

    for (issi, k) in &edit.subscribers {
        tail.extend([
            String::new(),
            "[[security.subscribers]]".to_string(),
            format!("issi = {issi}"),
            format!("k = \"{}\"", hex(k)),
        ]);
    }

    // Pass 2: patch the [security] block.
    let mut out: Vec<String> = Vec::with_capacity(kept.len() + tail.len() + 4);
    let mut in_security = false;
    let mut security_seen = false;
    let mut wrote_auth = false;
    let mut wrote_mutual = false;
    let close_security = |out: &mut Vec<String>, wrote_auth: bool, wrote_mutual: bool| {
        if !wrote_auth {
            out.push(auth_line.clone());
        }
        if !wrote_mutual {
            out.push(mutual_line.clone());
        }
        // Trim blank lines so the tail sits snugly, then add it.
        while out.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            out.pop();
        }
        out.extend(tail.iter().cloned());
        out.push(String::new());
    };
    for line in kept {
        let t = line.trim_start();
        if is_header(t) {
            if in_security {
                close_security(&mut out, wrote_auth, wrote_mutual);
            }
            in_security = header_name(t) == "security";
            if in_security {
                security_seen = true;
            }
            out.push(line);
            continue;
        }
        if in_security && !t.starts_with('#') {
            let key = t.split('=').next().map(|k| k.trim()).unwrap_or("");
            if key == "authentication" {
                out.push(auth_line.clone());
                wrote_auth = true;
                continue;
            }
            if key == "mutual_authentication" {
                out.push(mutual_line.clone());
                wrote_mutual = true;
                continue;
            }
        }
        out.push(line);
    }
    if in_security {
        close_security(&mut out, wrote_auth, wrote_mutual);
    }
    if !security_seen {
        if !out.last().map(|l| l.is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("[security]".to_string());
        close_security(&mut out, false, false);
    }
    let mut text = out.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Write the settings into the config file, keeping a `.security.bak` copy of the original.
pub fn write_to_toml(config_path: &str, edit: &SecurityEdit) -> std::io::Result<()> {
    let original = std::fs::read_to_string(config_path)?;
    let new_text = render_toml(&original, edit);
    // The rendered file must parse, or we would brick the next start.
    match tetra_config::bluestation::from_toml_str(&new_text) {
        Err(e) => return Err(std::io::Error::other(format!("rendered config does not parse: {e}"))),
        // E.g. SC3 together with [[cells]]: refused here, not at the next start.
        Ok(cfg) => cfg.validate().map_err(|e| std::io::Error::other(format!("rendered config is invalid: {e}")))?,
    }
    // The backup holds keys too: give it the config file's own permissions (0600 on a
    // packaged install), never the umask default.
    let bak = format!("{config_path}.security.bak");
    std::fs::copy(config_path, &bak)?;
    std::fs::write(config_path, new_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "config_version = \"0.6\"\nstack_mode = \"Bs\"\n\n[phy_io]\nbackend = \"None\"\n\n[net_info]\nmcc = 901\nmnc = 16383\n\n[cell_info]\nmain_carrier = 1584\nfreq_band = 4\nfreq_offset = 0\nduplex_spacing = 4\nreverse_operation = false\nlocation_area = 1\n";
    const BASE: &str = "config_version = \"0.6\"\nstack_mode = \"Bs\"\n\n[phy_io]\nbackend = \"None\"\n\n[net_info]\nmcc = 901\nmnc = 16383\n\n[cell_info]\nmain_carrier = 1584\nfreq_band = 4\nfreq_offset = 0\nduplex_spacing = 4\nreverse_operation = false\nlocation_area = 1\n\n[security]\n# comment kept\nissi_whitelist = [1, 2]\nauthentication = \"off\"\n\n[security.aie]\nclass = 2\nksg = 2\nsck = \"00000000000000000000\"\nclear_groups = [900]\n\n[[security.subscribers]]\nissi = 7\nk = \"00000000000000000000000000000000\"\n\n[recovery]\nenabled = false\n";

    fn edit() -> SecurityEdit {
        SecurityEdit {
            authentication: AuthenticationMode::Required,
            mutual_authentication: true,
            subscribers: vec![(2358245, [0x11; 16])],
            aie: Some(AieEdit { class3: None, enabled: true, ksg: 3, sck: [0x22; 10], sckn: 3, sck_vn: 4, clear_groups: vec![900] }),
        }
    }

    #[test]
    fn rewrites_only_the_security_tables() {
        let out = render_toml(BASE, &edit());
        assert!(out.contains("[net_info]\nmcc = 901"), "other sections untouched");
        assert!(out.contains("# comment kept\nissi_whitelist = [1, 2]"), "security comments and keys kept");
        assert!(out.contains("authentication = \"required\""));
        assert!(out.contains("mutual_authentication = true"));
        assert_eq!(out.matches("[security.aie]").count(), 1);
        assert_eq!(out.matches("[[security.subscribers]]").count(), 1);
        assert!(!out.contains("issi = 7"), "old subscriber removed");
        assert!(out.contains("issi = 2358245"));
        assert!(out.contains("[recovery]\nenabled = false"));
        // The sub-tables must come after the plain [security] keys and before [brew].
        let sec = out.find("[security]").unwrap();
        let aie = out.find("[security.aie]").unwrap();
        let brew = out.find("[recovery]").unwrap();
        assert!(sec < aie && aie < brew);
        // And the result is a config the stack accepts.
        let cfg = tetra_config::bluestation::from_toml_str(&out).expect("parses");
        assert_eq!(cfg.security.authentication, AuthenticationMode::Required);
        assert_eq!(cfg.security.aie.as_ref().unwrap().sckn, 3);
        assert_eq!(cfg.security.aie.as_ref().unwrap().clear_groups, vec![900]);
        assert_eq!(cfg.security.subscribers.len(), 1);
    }

    #[test]
    fn creates_the_section_when_missing() {
        let out = render_toml(HEAD, &edit());
        let cfg = tetra_config::bluestation::from_toml_str(&out).expect("parses");
        assert_eq!(cfg.security.authentication, AuthenticationMode::Required);
        assert!(cfg.security.aie.is_some());
    }

    #[test]
    fn disabling_encryption_keeps_the_key_staged() {
        let mut e = edit();
        e.aie.as_mut().unwrap().enabled = false;
        let out = render_toml(BASE, &e);
        assert!(out.contains("[security.aie]\nclass = 1"));
        let cfg = tetra_config::bluestation::from_toml_str(&out).expect("parses");
        assert!(cfg.security.aie_staged().is_some(), "key kept for OTAR");
        assert!(!cfg.security.aie.as_ref().unwrap().enabled, "cell runs in clear");
        // No key at all: the table goes.
        e.aie = None;
        let out = render_toml(BASE, &e);
        assert!(!out.contains("[security.aie]"));
        assert!(tetra_config::bluestation::from_toml_str(&out).unwrap().security.aie.is_none());
    }

    #[test]
    fn keep_sentinel_resolves_against_the_file() {
        let current = tetra_config::bluestation::from_toml_str(BASE).unwrap().security;
        let body = r#"{"authentication":"optional","subscribers":[{"issi":7,"k":"keep"}],"aie":{"enabled":true,"ksg":"tea2","sck":"keep","sckn":2,"sck_vn":9}}"#;
        let e = parse_edit(body, &current).expect("valid");
        assert_eq!(e.authentication, AuthenticationMode::Optional);
        assert_eq!(e.subscribers, vec![(7, [0u8; 16])]);
        let a = e.aie.unwrap();
        assert_eq!(a.sck, [0u8; 10]);
        assert_eq!((a.sckn, a.sck_vn, a.ksg), (2, 9, 2));
        assert_eq!(a.clear_groups, vec![900], "clear groups carried over from the file");
        // Unknown ISSI with keep → error; bad hex → error; TEA4 → error.
        assert!(parse_edit(r#"{"subscribers":[{"issi":8,"k":"keep"}]}"#, &current).is_err());
        assert!(parse_edit(r#"{"subscribers":[{"issi":8,"k":"zz"}]}"#, &current).is_err());
        assert!(parse_edit(r#"{"aie":{"ksg":"tea4","sck":"keep"}}"#, &current).is_err());
        let off = parse_edit(r#"{"aie":{"enabled":false}}"#, &current).unwrap().aie.unwrap();
        assert!(!off.enabled && off.sck == [0u8; 10], "disabled but the file's key is kept");
        let bare = tetra_config::bluestation::from_toml_str(HEAD).unwrap().security;
        assert!(parse_edit(r#"{"aie":{"enabled":false}}"#, &bare).unwrap().aie.is_none());
    }

    #[test]
    fn masks_keys() {
        assert_eq!(mask_key(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99]), "0011…8899");
        let p = posture_json(&tetra_config::bluestation::from_toml_str(BASE).unwrap().security);
        assert!(p.to_string().contains("0000…0000"));
        assert!(!p.to_string().contains("00000000000000000000"));
    }

    #[test]
    fn repeated_saves_do_not_accumulate_comment_lines() {
        let once = render_toml(BASE, &edit());
        let twice = render_toml(&once, &edit());
        assert_eq!(once, twice, "a second save with the same settings is a no-op");
        assert_eq!(twice.matches(AIE_COMMENT_MARK).count(), 2);
    }

    fn sc3_edit() -> SecurityEdit {
        let current = tetra_config::bluestation::from_toml_str(BASE).unwrap().security;
        parse_edit(r#"{"authentication":"required","aie":{"class":3,"ksg":"tea2","cck":"1234567890abcdef1234","cck_id":42}}"#, &current).unwrap()
    }

    #[test]
    fn dashboard_sc2_to_sc3_keeps_subscriber_keys_and_other_settings() {
        let e = sc3_edit();
        assert_eq!(e.subscribers, vec![(7, [0; 16])]);
        let out = render_toml(BASE, &e);
        let cfg = tetra_config::bluestation::from_toml_str(&out).unwrap();
        let a = cfg.security.aie.as_ref().unwrap();
        assert_eq!(a.security_class(), 3);
        assert_eq!(a.class3.as_ref().unwrap().cck_id, 42);
        assert_eq!(cfg.security.authentication, AuthenticationMode::Required);
        assert!(!out.contains("sck =") && !out.contains("clear_groups ="));
        assert!(out.contains("issi_whitelist = [1, 2]"));
        assert!(out.contains("[recovery]\nenabled = false"));
        assert_eq!(render_toml(&out, &e), out);
    }

    #[test]
    fn dashboard_sc3_keep_rotation_and_explicit_downgrade() {
        let cfg = tetra_config::bluestation::from_toml_str(&render_toml(BASE, &sc3_edit())).unwrap().security;
        let e = parse_edit(r#"{"aie":{"class":3,"cck":"keep","cck_id":42}}"#, &cfg).unwrap();
        assert_eq!(e.aie, sc3_edit().aie);
        assert!(parse_edit(r#"{"aie":{"class":3,"cck":"22222222222222222222","cck_id":42}}"#, &cfg).is_err());
        let rotated = parse_edit(r#"{"aie":{"class":3,"cck":"22222222222222222222","cck_id":43}}"#, &cfg).unwrap();
        assert_eq!(rotated.aie.unwrap().class3.unwrap().id, 43);
        for legacy in [r#"{"aie":null}"#, r#"{"aie":{"enabled":true,"sck":"keep"}}"#] {
            assert!(parse_edit(legacy, &cfg).unwrap_err().contains("SC3"));
        }
        assert!(parse_edit(r#"{"aie":{"class":2,"sck":"keep"}}"#, &cfg).is_err(), "a CCK is never reused as SCK");
        let off = parse_edit(r#"{"aie":{"class":1,"enabled":false}}"#, &cfg).unwrap();
        assert!(off.aie.is_none());
        let sc2 = parse_edit(r#"{"aie":{"class":2,"ksg":2,"sck":"55555555555555555555"}}"#, &cfg).unwrap();
        let cfg = tetra_config::bluestation::from_toml_str(&render_toml(BASE, &sc2)).unwrap();
        assert_eq!(cfg.security.aie.unwrap().security_class(), 2);
    }

    #[test]
    fn dashboard_sc3_rejects_incomplete_or_invalid_security() {
        let cfg = tetra_config::bluestation::from_toml_str(&render_toml(BASE, &sc3_edit())).unwrap().security;
        for bad in [
            r#"{"authentication":"off"}"#, r#"{"authentication":"optional"}"#,
            r#"{"subscribers":[]}"#, r#"{"aie":{"class":3,"cck":"bad"}}"#,
            r#"{"aie":{"class":3,"cck":123}}"#, r#"{"aie":{"class":3,"cck_id":-1}}"#,
            r#"{"aie":{"class":3,"cck_id":65536}}"#, r#"{"aie":{"class":3,"cck_id":1.5}}"#,
            r#"{"aie":{"class":3,"ksg":"tea4"}}"#, r#"{"aie":{"class":3,"sck":"keep"}}"#,
            r#"{"aie":{"class":3,"clear_groups":[900]}}"#, r#"{"aie":{"class":3,"enabled":false}}"#,
            r#"{"aie":{"class":4}}"#, r#"{"subscribers":[{"issi":7},{"issi":7}]}"#,
        ] { assert!(parse_edit(bad, &cfg).is_err(), "accepted {bad}"); }
    }

    #[test]
    fn changed_cck_requires_restart_even_when_public_masks_are_identical() {
        let cfg = tetra_config::bluestation::from_toml_str(&render_toml(BASE, &sc3_edit())).unwrap().security;
        let mut changed = cfg.clone();
        changed.aie.as_mut().unwrap().class3.as_mut().unwrap().cck.0[4] ^= 1;
        assert_eq!(posture_json(&cfg), posture_json(&changed), "CCK never disclosed");
        let p: serde_json::Value = serde_json::from_str(&page_json(&cfg, Ok(changed), serde_json::Value::Null)).unwrap();
        assert_eq!(p["restart_required"], true);
        assert!(!p.to_string().contains("1234567890abcdef1234"));
    }

    #[test]
    fn sc3_runtime_distinguishes_derived_from_active_without_disclosing_keys() {
        use tetra_config::bluestation::{SharedConfig, CipherKey};
        let cfg = tetra_config::bluestation::from_toml_str(&render_toml(BASE, &sc3_edit())).unwrap();
        let shared = SharedConfig::from_parts(cfg, None);
        shared.state_write().sc3_dcks.insert(7, CipherKey([0xA3; 10]));
        let first = runtime_json(&Some(shared.clone()));
        assert_eq!(first["radios"][0]["dck_available"], true);
        assert_eq!(first["radios"][0]["dck_active"], false);
        shared.state_write().subscribers.register(7);
        shared.state_write().subscribers.set_encrypted(7, true);
        let second = runtime_json(&Some(shared.clone()));
        assert_eq!(second["radios"][0]["dck_active"], true);
        shared.state_write().sc3_status.entry(7).or_default().phase="timeout";
        let timeout=runtime_json(&Some(shared.clone()));
        assert_eq!(timeout["radios"][0]["dck_retained"],true);
        assert_eq!(timeout["revision"],"sc3-r4");
        assert!(!second.to_string().contains("a3a3"));
        assert!(!second.to_string().contains("1234567890"));
        shared.state_write().sc3_dcks.remove(&7);
        assert_eq!(runtime_json(&Some(shared))["radios"][0]["dck_active"], false);
    }

    #[test]
    fn cck_generator_uses_os_randomness_and_returns_valid_80_bit_keys() {
        let a = random_hex(10).unwrap();
        let b = random_hex(10).unwrap();
        assert!(parse_hex::<10>(&a).is_some());
        assert!(parse_hex::<10>(&b).is_some());
        assert_ne!(a, b);
    }

    #[test]
    fn dashboard_respects_require_sc3_startup_guard() {
        let cfg = tetra_config::bluestation::from_toml_str(&render_toml(BASE, &sc3_edit())).unwrap();
        let shared = tetra_config::bluestation::SharedConfig::from_parts(cfg, None);
        let clear = SecurityEdit { aie: None, ..sc3_edit() };
        assert!(validate_startup_guard(&clear, &Some(shared.clone())).is_ok());
        shared.state_write().security_class3_required = true;
        assert!(validate_startup_guard(&clear, &Some(shared.clone())).is_err());
        assert!(validate_startup_guard(&sc3_edit(), &Some(shared.clone())).is_ok());
        assert_eq!(runtime_json(&Some(shared))["require_sc3"], true);
    }
}
