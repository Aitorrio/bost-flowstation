//! Visual configurator profiles (Cell × Brew), inspired by BTS Mia `bts-web`.
//!
//! Cell profiles store RF + network/cell identity (`phy_io`, `net_info`, `cell_info`).
//! Brew profiles store the optional `[brew]` backhaul. Apply merges a Cell × Brew
//! selection into the live `config.toml`, preserving Station/Integration sections,
//! then validates with the same parse+`validate()` path as the raw editor.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue};
use std::fs;
use std::path::{Path, PathBuf};
use toml::Value as TomlValue;

const MASKED_PASSWORD: &str = "••••••••";

/// Active profile selection on disk (`profiles/active.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveProfiles {
    pub cell: String,
    /// `None` / JSON null → offline (no `[brew]` section).
    pub brew: Option<String>,
}

impl Default for ActiveProfiles {
    fn default() -> Self {
        Self {
            cell: "Default".to_string(),
            brew: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileListItem {
    pub name: String,
    pub active: bool,
}

fn profiles_root(config_path: &str) -> PathBuf {
    Path::new(config_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("profiles")
}

fn cell_dir(config_path: &str) -> PathBuf {
    profiles_root(config_path).join("cell")
}

fn brew_dir(config_path: &str) -> PathBuf {
    profiles_root(config_path).join("brew")
}

fn active_path(config_path: &str) -> PathBuf {
    profiles_root(config_path).join("active.json")
}

fn sanitize_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("profile name is empty".into());
    }
    if name.len() > 64 {
        return Err("profile name too long (max 64)".into());
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err("profile name contains illegal characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.')
    {
        return Err("profile name may only contain letters, digits, space, ._-".into());
    }
    Ok(name.to_string())
}

fn profile_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

/// Ensure profile directories exist and seed from the live TOML on first run.
pub fn ensure_seeded(config_path: &str) -> Result<(), String> {
    let cell = cell_dir(config_path);
    let brew = brew_dir(config_path);
    fs::create_dir_all(&cell).map_err(|e| e.to_string())?;
    fs::create_dir_all(&brew).map_err(|e| e.to_string())?;

    let cell_empty = fs::read_dir(&cell)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true);
    if !cell_empty {
        return Ok(());
    }

    let toml_txt = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    let table: toml::Table = toml::from_str(&toml_txt).map_err(|e| format!("parse config: {e}"))?;

    let mut cell_json = Map::new();
    for key in ["phy_io", "net_info", "cell_info"] {
        if let Some(v) = table.get(key) {
            cell_json.insert(key.to_string(), toml_to_json(v));
        }
    }
    // Seed access control into the Default Cell so existing installs keep their whitelist
    // when they start using Cell-bound access (missing key = open network).
    if let Some(v) = table.get("security") {
        cell_json.insert("security".into(), toml_to_json(v));
    }
    // Keep stack_mode / config_version for completeness when re-materialising.
    if let Some(v) = table.get("config_version") {
        cell_json.insert("config_version".into(), toml_to_json(v));
    }
    if let Some(v) = table.get("stack_mode") {
        cell_json.insert("stack_mode".into(), toml_to_json(v));
    }

    save_json_profile(&cell, "Default", &JsonValue::Object(cell_json))?;

    let mut active = ActiveProfiles::default();
    if let Some(brew_val) = table.get("brew") {
        let mut brew_obj = match toml_to_json(brew_val) {
            JsonValue::Object(m) => m,
            other => {
                let mut m = Map::new();
                m.insert("value".into(), other);
                m
            }
        };
        // Prefer a friendly default name from host when present.
        let name = brew_obj
            .get("host")
            .and_then(|h| h.as_str())
            .map(|h| format!("Brew {h}"))
            .unwrap_or_else(|| "Default Brew".to_string());
        let name = sanitize_name(&name).unwrap_or_else(|_| "Default Brew".into());
        if let Some(JsonValue::String(p)) = brew_obj.get("password").cloned() {
            if p.is_empty() {
                brew_obj.insert("password".into(), JsonValue::String(String::new()));
            }
        }
        save_json_profile(&brew, &name, &JsonValue::Object(brew_obj))?;
        active.brew = Some(name);
    }
    write_active(config_path, &active)?;
    Ok(())
}

fn save_json_profile(dir: &Path, name: &str, value: &JsonValue) -> Result<(), String> {
    let name = sanitize_name(name)?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = profile_file(dir, &name);
    let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    atomic_write_str(&path, &body)
}

fn atomic_write_str(path: &Path, body: &str) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn read_active(config_path: &str) -> ActiveProfiles {
    let _ = ensure_seeded(config_path);
    fs::read_to_string(active_path(config_path))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn write_active(config_path: &str, active: &ActiveProfiles) -> Result<(), String> {
    fs::create_dir_all(profiles_root(config_path)).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(active).map_err(|e| e.to_string())?;
    atomic_write_str(&active_path(config_path), &body)
}

fn list_names(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return names;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            names.push(stem.to_string());
        }
    }
    names.sort();
    names
}

pub fn list_cell_profiles(config_path: &str) -> Result<Vec<ProfileListItem>, String> {
    let _ = ensure_seeded(config_path);
    let active = read_active(config_path);
    Ok(list_names(&cell_dir(config_path))
        .into_iter()
        .map(|name| ProfileListItem {
            active: name == active.cell,
            name,
        })
        .collect())
}

pub fn list_brew_profiles(config_path: &str) -> Result<Vec<ProfileListItem>, String> {
    let _ = ensure_seeded(config_path);
    let active = read_active(config_path);
    Ok(list_names(&brew_dir(config_path))
        .into_iter()
        .map(|name| ProfileListItem {
            active: active.brew.as_deref() == Some(name.as_str()),
            name,
        })
        .collect())
}

pub fn get_cell_profile(config_path: &str, name: &str) -> Result<JsonValue, String> {
    let name = sanitize_name(name)?;
    let path = profile_file(&cell_dir(config_path), &name);
    let txt = fs::read_to_string(&path).map_err(|e| format!("cell profile '{name}': {e}"))?;
    serde_json::from_str(&txt).map_err(|e| e.to_string())
}

pub fn get_brew_profile(config_path: &str, name: &str, mask_secrets: bool) -> Result<JsonValue, String> {
    let name = sanitize_name(name)?;
    let path = profile_file(&brew_dir(config_path), &name);
    let txt = fs::read_to_string(&path).map_err(|e| format!("brew profile '{name}': {e}"))?;
    let mut v: JsonValue = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    if mask_secrets {
        if let Some(obj) = v.as_object_mut() {
            if let Some(JsonValue::String(p)) = obj.get("password") {
                if !p.is_empty() {
                    obj.insert("password".into(), JsonValue::String(MASKED_PASSWORD.into()));
                    obj.insert("password_set".into(), JsonValue::Bool(true));
                } else {
                    obj.insert("password_set".into(), JsonValue::Bool(false));
                }
            }
        }
    }
    Ok(v)
}

pub fn put_cell_profile(config_path: &str, name: &str, body: &JsonValue) -> Result<(), String> {
    let name = sanitize_name(name)?;
    if !body.is_object() {
        return Err("cell profile body must be a JSON object".into());
    }
    // Light shape check — full ETSI validation happens on apply against a merged TOML.
    let obj = body.as_object().unwrap();
    if !obj.contains_key("phy_io") && !obj.contains_key("net_info") && !obj.contains_key("cell_info") {
        return Err("cell profile must include phy_io, net_info and/or cell_info".into());
    }
    let mut cleaned = body.clone();
    if let Some(ci) = cleaned.get_mut("cell_info") {
        strip_sds_command_control_json(ci);
    }
    save_json_profile(&cell_dir(config_path), &name, &cleaned)
}

pub fn put_brew_profile(config_path: &str, name: &str, body: &JsonValue) -> Result<(), String> {
    let name = sanitize_name(name)?;
    let mut obj = body
        .as_object()
        .cloned()
        .ok_or_else(|| "brew profile body must be a JSON object".to_string())?;
    let host = obj
        .get("host")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if host.is_empty() {
        return Err("brew.host is required".into());
    }
    // Rehydrate masked password from existing profile when the client did not retype it.
    if matches!(obj.get("password"), Some(JsonValue::String(p)) if p == MASKED_PASSWORD || p.chars().all(|c| c == '•'))
    {
        if let Ok(existing) = get_brew_profile_raw(config_path, &name) {
            if let Some(JsonValue::String(real)) = existing.get("password") {
                obj.insert("password".into(), JsonValue::String(real.clone()));
            }
        } else {
            return Err("password is masked; re-enter the password when creating a new brew profile".into());
        }
    }
    obj.remove("password_set");
    save_json_profile(&brew_dir(config_path), &name, &JsonValue::Object(obj))
}

fn get_brew_profile_raw(config_path: &str, name: &str) -> Result<JsonValue, String> {
    let path = profile_file(&brew_dir(config_path), name);
    let txt = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&txt).map_err(|e| e.to_string())
}

pub fn delete_cell_profile(config_path: &str, name: &str) -> Result<(), String> {
    let name = sanitize_name(name)?;
    let active = read_active(config_path);
    if active.cell == name {
        return Err("cannot delete the active cell profile".into());
    }
    let names = list_names(&cell_dir(config_path));
    if names.len() <= 1 {
        return Err("cannot delete the last cell profile".into());
    }
    let path = profile_file(&cell_dir(config_path), &name);
    fs::remove_file(&path).map_err(|e| e.to_string())
}

pub fn delete_brew_profile(config_path: &str, name: &str) -> Result<(), String> {
    let name = sanitize_name(name)?;
    let active = read_active(config_path);
    if active.brew.as_deref() == Some(name.as_str()) {
        return Err("cannot delete the active brew profile".into());
    }
    let path = profile_file(&brew_dir(config_path), &name);
    fs::remove_file(&path).map_err(|e| e.to_string())
}

/// Rename a cell profile file and update `active.json` when needed.
pub fn rename_cell_profile(config_path: &str, old_name: &str, new_name: &str) -> Result<(), String> {
    let old = sanitize_name(old_name)?;
    let new = sanitize_name(new_name)?;
    if old == new {
        return Ok(());
    }
    let dir = cell_dir(config_path);
    let from = profile_file(&dir, &old);
    let to = profile_file(&dir, &new);
    if !from.exists() {
        return Err(format!("cell profile '{old}' not found"));
    }
    if to.exists() {
        return Err(format!("cell profile '{new}' already exists"));
    }
    fs::rename(&from, &to).map_err(|e| e.to_string())?;
    let mut active = read_active(config_path);
    if active.cell == old {
        active.cell = new;
        write_active(config_path, &active)?;
    }
    Ok(())
}

/// Rename a brew profile file and update `active.json` when needed.
pub fn rename_brew_profile(config_path: &str, old_name: &str, new_name: &str) -> Result<(), String> {
    let old = sanitize_name(old_name)?;
    let new = sanitize_name(new_name)?;
    if old == new {
        return Ok(());
    }
    let dir = brew_dir(config_path);
    let from = profile_file(&dir, &old);
    let to = profile_file(&dir, &new);
    if !from.exists() {
        return Err(format!("brew profile '{old}' not found"));
    }
    if to.exists() {
        return Err(format!("brew profile '{new}' already exists"));
    }
    fs::rename(&from, &to).map_err(|e| e.to_string())?;
    let mut active = read_active(config_path);
    if active.brew.as_deref() == Some(old.as_str()) {
        active.brew = Some(new);
        write_active(config_path, &active)?;
    }
    Ok(())
}

pub fn duplicate_cell_profile(config_path: &str, name: &str, new_name: &str) -> Result<(), String> {
    let src = get_cell_profile(config_path, name)?;
    put_cell_profile(config_path, new_name, &src)
}

pub fn duplicate_brew_profile(config_path: &str, name: &str, new_name: &str) -> Result<(), String> {
    let src = get_brew_profile_raw(config_path, &sanitize_name(name)?)?;
    put_brew_profile(config_path, new_name, &src)
}

/// Extract a visual-config JSON view from the live TOML (secrets masked).
pub fn visual_config_from_toml(config_path: &str) -> Result<JsonValue, String> {
    let txt = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    let table: toml::Table = toml::from_str(&txt).map_err(|e| format!("parse config: {e}"))?;
    let mut out = Map::new();
    for key in ["config_version", "stack_mode", "phy_io", "net_info", "cell_info"] {
        if let Some(v) = table.get(key) {
            out.insert(key.to_string(), toml_to_json(v));
        }
    }
    // Live access control for the visual Config form (MHz UI + Cell-bound whitelist).
    if let Some(v) = table.get("security") {
        out.insert("security".into(), toml_to_json(v));
    } else {
        let mut sec = Map::new();
        sec.insert("issi_whitelist".into(), JsonValue::Array(vec![]));
        out.insert("security".into(), JsonValue::Object(sec));
    }
    if let Some(brew) = table.get("brew") {
        let mut brew_json = toml_to_json(brew);
        if let Some(obj) = brew_json.as_object_mut() {
            let set = obj
                .get("password")
                .and_then(|p| p.as_str())
                .map(|p| !p.is_empty())
                .unwrap_or(false);
            if set {
                obj.insert("password".into(), JsonValue::String(MASKED_PASSWORD.into()));
            }
            obj.insert("password_set".into(), JsonValue::Bool(set));
            obj.insert("enabled".into(), JsonValue::Bool(true));
        }
        out.insert("brew".into(), brew_json);
    } else {
        let mut brew = Map::new();
        brew.insert("enabled".into(), JsonValue::Bool(false));
        brew.insert("host".into(), JsonValue::String(String::new()));
        brew.insert("port".into(), JsonValue::Number(3003.into()));
        brew.insert("tls".into(), JsonValue::Bool(true));
        brew.insert("username".into(), JsonValue::Number(0.into()));
        brew.insert("password".into(), JsonValue::String(String::new()));
        brew.insert("password_set".into(), JsonValue::Bool(false));
        brew.insert("reconnect_delay_secs".into(), JsonValue::Number(15.into()));
        brew.insert("feature_sds_enabled".into(), JsonValue::Bool(true));
        brew.insert("feature_rssi_export".into(), JsonValue::Bool(false));
        out.insert("brew".into(), JsonValue::Object(brew));
    }
    out.insert(
        "active".into(),
        serde_json::to_value(read_active(config_path)).unwrap_or(JsonValue::Null),
    );
    Ok(JsonValue::Object(out))
}

/// Merge a visual-config POST body into the live TOML and validate.
pub fn write_visual_config(config_path: &str, body: &JsonValue) -> Result<(), String> {
    let obj = body
        .as_object()
        .ok_or_else(|| "body must be a JSON object".to_string())?;
    let current = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    let mut table: toml::Table = toml::from_str(&current).map_err(|e| format!("parse config: {e}"))?;

    for key in ["phy_io", "net_info", "cell_info"] {
        if let Some(v) = obj.get(key) {
            let mut cleaned = v.clone();
            if key == "cell_info" {
                strip_sds_command_control_json(&mut cleaned);
            }
            deep_merge_toml(&mut table, key, json_to_toml(&cleaned)?);
        }
    }
    // Optional: Save live may also write whitelist when the form includes security.
    if let Some(v) = obj.get("security") {
        deep_merge_toml(&mut table, "security", json_to_toml(v)?);
    }

    if let Some(brew) = obj.get("brew") {
        let brew_obj = brew
            .as_object()
            .ok_or_else(|| "brew must be an object".to_string())?;
        let enabled = brew_obj.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
        if enabled {
            let mut brew_toml = json_to_toml(brew)?.as_table().cloned().unwrap_or_default();
            brew_toml.remove("enabled");
            brew_toml.remove("password_set");
            rehydrate_brew_password(&mut brew_toml, table.get("brew"));
            table.insert("brew".into(), TomlValue::Table(brew_toml));
        } else {
            table.remove("brew");
        }
    }

    let rendered = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
    validate_toml_str(&rendered)?;
    backup_and_write(config_path, &rendered)
}

/// Apply named Cell × Brew profiles into the live config.toml.
pub fn apply_profiles(config_path: &str, cell_name: &str, brew_name: Option<&str>) -> Result<(), String> {
    let cell = get_cell_profile(config_path, cell_name)?;
    let current = fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    let mut table: toml::Table = toml::from_str(&current).map_err(|e| format!("parse config: {e}"))?;

    let cell_obj = cell
        .as_object()
        .ok_or_else(|| "cell profile is not an object".to_string())?;

    // Preserve live SDS command control — station-wide, not part of Cell profiles.
    let preserved_sds_cmd = table
        .get("cell_info")
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("sds_command_control"))
        .cloned();

    // Deep-merge so advanced cell keys (neighbors, HMD, …) survive a partial Cell profile.
    // SDS command control is stripped from the profile payload and restored after merge.
    for key in ["phy_io", "net_info", "cell_info"] {
        if let Some(v) = cell_obj.get(key) {
            let mut cleaned = v.clone();
            if key == "cell_info" {
                strip_sds_command_control_json(&mut cleaned);
            }
            deep_merge_toml(&mut table, key, json_to_toml(&cleaned)?);
        }
    }
    // Access control is part of the Cell profile. Missing key = open network for this
    // Cell (empty whitelist), so Apply switches access when changing profiles.
    let security_val = if let Some(v) = cell_obj.get("security") {
        v.clone()
    } else {
        let mut sec = Map::new();
        sec.insert("issi_whitelist".into(), JsonValue::Array(vec![]));
        JsonValue::Object(sec)
    };
    deep_merge_toml(&mut table, "security", json_to_toml(&security_val)?);
    if let Some(v) = cell_obj.get("stack_mode") {
        table.insert("stack_mode".into(), json_to_toml(v)?);
    }
    // Always keep schema version required by current parser.
    table.insert(
        "config_version".into(),
        TomlValue::String(
            cell_obj
                .get("config_version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.6")
                .to_string(),
        ),
    );

    if let Some(sds) = preserved_sds_cmd {
        let cell_tbl = table
            .entry("cell_info".to_string())
            .or_insert_with(|| TomlValue::Table(toml::Table::new()));
        if let TomlValue::Table(t) = cell_tbl {
            t.insert("sds_command_control".into(), sds);
        }
    } else if let Some(TomlValue::Table(t)) = table.get_mut("cell_info") {
        t.remove("sds_command_control");
    }

    match brew_name {
        Some(name) => {
            let brew = get_brew_profile_raw(config_path, &sanitize_name(name)?)?;
            let mut brew_toml = json_to_toml(&brew)?.as_table().cloned().unwrap_or_default();
            brew_toml.remove("enabled");
            brew_toml.remove("password_set");
            table.insert("brew".into(), TomlValue::Table(brew_toml));
        }
        None => {
            table.remove("brew");
        }
    }

    let rendered = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
    validate_toml_str(&rendered)?;
    backup_and_write(config_path, &rendered)?;
    write_active(
        config_path,
        &ActiveProfiles {
            cell: sanitize_name(cell_name)?,
            brew: brew_name.map(|n| sanitize_name(n)).transpose()?,
        },
    )?;
    Ok(())
}

/// Capture current visual form sections into a new Cell profile.
pub fn save_cell_from_visual(config_path: &str, name: &str, visual: &JsonValue) -> Result<(), String> {
    let obj = visual
        .as_object()
        .ok_or_else(|| "body must be a JSON object".to_string())?;
    let mut cell = Map::new();
    for key in ["config_version", "stack_mode", "phy_io", "net_info", "cell_info"] {
        if let Some(v) = obj.get(key) {
            let mut cleaned = v.clone();
            if key == "cell_info" {
                strip_sds_command_control_json(&mut cleaned);
            }
            cell.insert(key.to_string(), cleaned);
        }
    }
    // Always persist Access Control from the visual form (empty = open for this Cell).
    if let Some(v) = obj.get("security") {
        cell.insert("security".to_string(), v.clone());
    } else {
        let mut sec = Map::new();
        sec.insert("issi_whitelist".into(), JsonValue::Array(vec![]));
        cell.insert("security".into(), JsonValue::Object(sec));
    }
    put_cell_profile(config_path, name, &JsonValue::Object(cell))
}

fn strip_sds_command_control_json(v: &mut JsonValue) {
    if let Some(obj) = v.as_object_mut() {
        obj.remove("sds_command_control");
    }
}

/// Read `security.issi_whitelist` from a Cell profile JSON. `None` = key absent (legacy).
pub fn extract_issi_whitelist(cell: &JsonValue) -> Option<Vec<u32>> {
    let sec = cell.get("security")?;
    let arr = sec.get("issi_whitelist")?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|v| v.as_u64().map(|n| n as u32))
            .collect(),
    )
}

/// Patch only the Cell profile's ISSI whitelist. Returns `true` if that Cell is the active one.
pub fn set_cell_whitelist(config_path: &str, name: &str, list: &[u32]) -> Result<bool, String> {
    let name = sanitize_name(name)?;
    let path = profile_file(&cell_dir(config_path), &name);
    let txt = fs::read_to_string(&path).map_err(|e| format!("cell profile: {e}"))?;
    let mut v: JsonValue = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    let obj = v
        .as_object_mut()
        .ok_or_else(|| "cell profile is not an object".to_string())?;
    let mut sec = Map::new();
    sec.insert(
        "issi_whitelist".into(),
        JsonValue::Array(
            list.iter()
                .map(|i| JsonValue::Number((*i).into()))
                .collect(),
        ),
    );
    obj.insert("security".into(), JsonValue::Object(sec));
    save_json_profile(&cell_dir(config_path), &name, &v)?;
    Ok(read_active(config_path).cell == name)
}

/// Capture brew form into a Brew profile.
pub fn save_brew_from_visual(config_path: &str, name: &str, visual: &JsonValue) -> Result<(), String> {
    let brew = visual
        .get("brew")
        .cloned()
        .ok_or_else(|| "missing brew object".to_string())?;
    put_brew_profile(config_path, name, &brew)
}

fn rehydrate_brew_password(brew_toml: &mut toml::Table, existing: Option<&TomlValue>) {
    let incoming = brew_toml.get("password").and_then(|v| v.as_str()).unwrap_or("");
    let masked = incoming == MASKED_PASSWORD || incoming.chars().all(|c| c == '•');
    if masked {
        if let Some(TomlValue::Table(prev)) = existing {
            if let Some(TomlValue::String(real)) = prev.get("password") {
                brew_toml.insert("password".into(), TomlValue::String(real.clone()));
            }
        }
    }
}

fn validate_toml_str(toml_str: &str) -> Result<(), String> {
    match tetra_config::bluestation::parsing::from_toml_str(toml_str) {
        Ok(cfg) => cfg.validate().map_err(|e| format!("config is invalid: {e}")),
        Err(e) => Err(format!("config does not parse: {e}")),
    }
}

fn backup_and_write(config_path: &str, body: &str) -> Result<(), String> {
    let backup = format!("{config_path}.bak");
    let _ = fs::copy(config_path, &backup);
    let tmp = format!("{config_path}.tmp");
    fs::write(&tmp, body).map_err(|e| e.to_string())?;
    fs::rename(&tmp, config_path).map_err(|e| e.to_string())
}

/// Validate a full TOML document then atomically replace `config_path` (with `.bak`).
pub fn validate_and_write_toml(config_path: &str, body: &str) -> Result<(), String> {
    validate_toml_str(body)?;
    backup_and_write(config_path, body)
}

fn deep_merge_toml(table: &mut toml::Table, key: &str, incoming: TomlValue) {
    match (table.get_mut(key), incoming) {
        (Some(TomlValue::Table(dst)), TomlValue::Table(src)) => {
            for (k, v) in src {
                if let (Some(TomlValue::Table(d)), TomlValue::Table(s)) = (dst.get_mut(&k), v.clone()) {
                    for (ik, iv) in s {
                        d.insert(ik, iv);
                    }
                } else {
                    dst.insert(k, v);
                }
            }
        }
        (_, incoming) => {
            table.insert(key.to_string(), incoming);
        }
    }
}

fn toml_to_json(v: &TomlValue) -> JsonValue {
    match v {
        TomlValue::String(s) => JsonValue::String(s.clone()),
        TomlValue::Integer(i) => JsonValue::Number((*i).into()),
        TomlValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        TomlValue::Boolean(b) => JsonValue::Bool(*b),
        TomlValue::Datetime(d) => JsonValue::String(d.to_string()),
        TomlValue::Array(a) => JsonValue::Array(a.iter().map(toml_to_json).collect()),
        TomlValue::Table(t) => {
            let mut m = Map::new();
            for (k, v) in t {
                m.insert(k.clone(), toml_to_json(v));
            }
            JsonValue::Object(m)
        }
    }
}

fn json_to_toml(v: &JsonValue) -> Result<TomlValue, String> {
    match v {
        JsonValue::Null => Err("null is not valid in TOML config values".into()),
        JsonValue::Bool(b) => Ok(TomlValue::Boolean(*b)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(TomlValue::Integer(i))
            } else if let Some(u) = n.as_u64() {
                Ok(TomlValue::Integer(u as i64))
            } else if let Some(f) = n.as_f64() {
                Ok(TomlValue::Float(f))
            } else {
                Err("unsupported number".into())
            }
        }
        JsonValue::String(s) => Ok(TomlValue::String(s.clone())),
        JsonValue::Array(a) => Ok(TomlValue::Array(
            a.iter().map(json_to_toml).collect::<Result<Vec<_>, _>>()?,
        )),
        JsonValue::Object(o) => {
            let mut t = toml::Table::new();
            for (k, v) in o {
                // Skip UI helper flags that must not land in TOML.
                if k == "password_set" || k == "enabled" {
                    continue;
                }
                t.insert(k.clone(), json_to_toml(v)?);
            }
            Ok(TomlValue::Table(t))
        }
    }
}

/// Band-4 / non-reverse helper used by the dashboard Auto button (and unit-tested here).
pub fn auto_main_carrier_rx(
    tx_freq_hz: f64,
    duplex_hz: f64,
    freq_offset_hz: f64,
    freq_band: u8,
    reverse_operation: bool,
) -> Result<(u16, f64), String> {
    if freq_band != 4 {
        return Err("Auto currently supports freq_band = 4".into());
    }
    if reverse_operation {
        return Err("Auto currently supports reverse_operation = false".into());
    }
    let main = (tx_freq_hz - 400_000_000.0 - freq_offset_hz) / 25_000.0;
    if (main - main.round()).abs() > 1e-9 || main < 0.0 || main > u16::MAX as f64 {
        return Err("main_carrier is not an integer with the current values".into());
    }
    let rx = tx_freq_hz - duplex_hz;
    Ok((main.round() as u16, rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_toml() -> String {
        r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "SoapySdr"

[phy_io.soapysdr]
tx_freq = 438025000.0
rx_freq = 433025000.0

[net_info]
mcc = 204
mnc = 1337

[cell_info]
freq_band = 4
main_carrier = 1521
duplex_spacing = 4
freq_offset = 0
reverse_operation = false
location_area = 2
colour_code = 1
system_wide_services = true
voice_service = true

[brew]
host = "core.example"
port = 3003
tls = true
username = 12345
password = "secret"
"#
        .to_string()
    }

    #[test]
    fn seed_split_cell_and_brew() {
        let dir = tempfile_dir();
        let cfg = dir.join("config.toml");
        fs::write(&cfg, minimal_toml()).unwrap();
        ensure_seeded(cfg.to_str().unwrap()).unwrap();
        let cells = list_cell_profiles(cfg.to_str().unwrap()).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].name, "Default");
        let brews = list_brew_profiles(cfg.to_str().unwrap()).unwrap();
        assert_eq!(brews.len(), 1);
        let active = read_active(cfg.to_str().unwrap());
        assert_eq!(active.cell, "Default");
        assert!(active.brew.is_some());
    }

    #[test]
    fn apply_offline_removes_brew() {
        let dir = tempfile_dir();
        let cfg = dir.join("config.toml");
        fs::write(&cfg, minimal_toml()).unwrap();
        ensure_seeded(cfg.to_str().unwrap()).unwrap();
        apply_profiles(cfg.to_str().unwrap(), "Default", None).unwrap();
        let txt = fs::read_to_string(&cfg).unwrap();
        assert!(!txt.contains("[brew]"), "brew section should be gone: {txt}");
        assert!(txt.contains("main_carrier"));
        let active = read_active(cfg.to_str().unwrap());
        assert!(active.brew.is_none());
    }

    #[test]
    fn auto_helper_band4() {
        let (carrier, rx) = auto_main_carrier_rx(438_025_000.0, 5_000_000.0, 0.0, 4, false).unwrap();
        assert_eq!(carrier, 1521);
        assert_eq!(rx, 433_025_000.0);
    }

    fn tempfile_dir() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("fs-profiles-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
