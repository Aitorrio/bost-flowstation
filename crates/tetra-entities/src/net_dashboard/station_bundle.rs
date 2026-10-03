//! Station backup (`.bptbs`) and profile pack (`.ptbs`) as ZIP containers.
//!
//! - `bptbs-station`: live config + profiles + optional fallback/setup + WiFi PSKs
//! - `ptbs-profiles`: Cell/Brew profile tree only

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::net_dashboard::profiles;
use crate::wifi::{self, WifiNetworkExport};

pub const FORMAT_STATION: &str = "bptbs-station";
pub const FORMAT_PROFILES: &str = "ptbs-profiles";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub format: String,
    pub format_version: u32,
    pub product: String,
    pub created_utc: String,
    #[serde(default)]
    pub includes: Vec<String>,
}

#[derive(Debug)]
pub struct ImportResult {
    pub warnings: Vec<String>,
    /// True when the live config.toml was replaced (station import).
    pub restart_required: bool,
}

fn config_dir(config_path: &str) -> PathBuf {
    Path::new(config_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn zip_options() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)
}

fn add_file(zip: &mut ZipWriter<Cursor<Vec<u8>>>, name: &str, data: &[u8]) -> Result<(), String> {
    zip.start_file(name, zip_options())
        .map_err(|e| format!("zip start {name}: {e}"))?;
    zip.write_all(data)
        .map_err(|e| format!("zip write {name}: {e}"))?;
    Ok(())
}

fn add_dir_tree(
    zip: &mut ZipWriter<Cursor<Vec<u8>>>,
    prefix: &str,
    dir: &Path,
) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    fn walk(
        zip: &mut ZipWriter<Cursor<Vec<u8>>>,
        prefix: &str,
        base: &Path,
        cur: &Path,
    ) -> Result<(), String> {
        let entries = fs::read_dir(cur).map_err(|e| format!("read {}: {e}", cur.display()))?;
        for ent in entries {
            let ent = ent.map_err(|e| format!("readdir: {e}"))?;
            let path = ent.path();
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let rel = path
                .strip_prefix(base)
                .map_err(|_| "path strip".to_string())?;
            let zip_name = format!("{prefix}/{}", rel.to_string_lossy().replace('\\', "/"));
            if path.is_dir() {
                walk(zip, prefix, base, &path)?;
            } else if path.is_file() {
                let data = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
                add_file(zip, &zip_name, &data)?;
            }
        }
        Ok(())
    }
    walk(zip, prefix, dir, dir)
}

fn list_sibling_tomls(config_path: &str) -> Vec<PathBuf> {
    let dir = config_dir(config_path);
    let live_name = Path::new(config_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "config.toml".into());
    let Ok(rd) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ent in rd.flatten() {
        let path = ent.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".toml") {
            continue;
        }
        if name == live_name || name.ends_with(".bak") || name.contains(".preimport") {
            continue;
        }
        if name == "config.toml.fallback" || name.ends_with(".fallback") {
            continue;
        }
        out.push(path);
    }
    out.sort();
    out
}

/// Build a full station backup (`.bptbs`).
pub fn build_station_bptbs(config_path: &str) -> Result<Vec<u8>, String> {
    let _ = profiles::ensure_seeded(config_path);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let mut includes = vec!["config".into(), "profiles".into()];

    let config = fs::read_to_string(config_path)
        .map_err(|e| format!("read config: {e}"))?;
    add_file(&mut zip, "config.toml", config.as_bytes())?;

    let fallback = format!("{config_path}.fallback");
    if Path::new(&fallback).is_file() {
        let data = fs::read(&fallback).map_err(|e| format!("read fallback: {e}"))?;
        add_file(&mut zip, "config.toml.fallback", &data)?;
        includes.push("fallback".into());
    }

    let setup = config_dir(config_path).join("setup.json");
    if setup.is_file() {
        let data = fs::read(&setup).map_err(|e| format!("read setup.json: {e}"))?;
        add_file(&mut zip, "setup.json", &data)?;
        includes.push("setup".into());
    }

    let prof = profiles::profiles_dir(config_path);
    add_dir_tree(&mut zip, "profiles", &prof)?;

    for sib in list_sibling_tomls(config_path) {
        let Some(name) = sib.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let data = fs::read(&sib).map_err(|e| format!("read {}: {e}", sib.display()))?;
        add_file(
            &mut zip,
            &format!("optional/sibling-profiles/{name}"),
            &data,
        )?;
        if !includes.iter().any(|i| i == "sibling-profiles") {
            includes.push("sibling-profiles".into());
        }
    }

    let wifi_nets = match wifi::export_saved_networks() {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!("station export: WiFi secrets unavailable: {e}");
            Vec::new()
        }
    };
    let wifi_json = serde_json::to_vec_pretty(&wifi_nets)
        .map_err(|e| format!("serialize wifi: {e}"))?;
    add_file(&mut zip, "wifi/networks.json", &wifi_json)?;
    includes.push("wifi".into());

    let manifest = BundleManifest {
        format: FORMAT_STATION.to_string(),
        format_version: FORMAT_VERSION,
        product: tetra_core::PRODUCT_NAME.to_string(),
        created_utc: now_utc(),
        includes,
    };
    let man = serde_json::to_vec_pretty(&manifest).map_err(|e| format!("manifest: {e}"))?;
    add_file(&mut zip, "manifest.json", &man)?;

    let cursor = zip.finish().map_err(|e| format!("zip finish: {e}"))?;
    Ok(cursor.into_inner())
}

/// Build a profiles-only pack (`.ptbs`).
pub fn build_profiles_ptbs(config_path: &str) -> Result<Vec<u8>, String> {
    let _ = profiles::ensure_seeded(config_path);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let prof = profiles::profiles_dir(config_path);
    add_dir_tree(&mut zip, "profiles", &prof)?;
    let manifest = BundleManifest {
        format: FORMAT_PROFILES.to_string(),
        format_version: FORMAT_VERSION,
        product: tetra_core::PRODUCT_NAME.to_string(),
        created_utc: now_utc(),
        includes: vec!["profiles".into()],
    };
    let man = serde_json::to_vec_pretty(&manifest).map_err(|e| format!("manifest: {e}"))?;
    add_file(&mut zip, "manifest.json", &man)?;
    let cursor = zip.finish().map_err(|e| format!("zip finish: {e}"))?;
    Ok(cursor.into_inner())
}

fn read_zip_file(archive: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>, String> {
    let mut f = archive
        .by_name(name)
        .map_err(|_| format!("missing {name} in archive"))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)
        .map_err(|e| format!("read {name}: {e}"))?;
    Ok(buf)
}

fn read_manifest(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Result<BundleManifest, String> {
    let data = read_zip_file(archive, "manifest.json")?;
    serde_json::from_slice(&data).map_err(|e| format!("invalid manifest.json: {e}"))
}

fn extract_prefix_to_dir(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    prefix: &str,
    dest: &Path,
) -> Result<(), String> {
    if dest.exists() {
        fs::remove_dir_all(dest).map_err(|e| format!("remove {}: {e}", dest.display()))?;
    }
    fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    let prefix_slash = format!("{prefix}/");
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("zip index {i}: {e}"))?;
        let name = file.name().replace('\\', "/");
        if !name.starts_with(&prefix_slash) || name.ends_with('/') {
            continue;
        }
        let rel = &name[prefix_slash.len()..];
        if rel.is_empty() || rel.contains("..") {
            continue;
        }
        let out = dest.join(rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| format!("read {name}: {e}"))?;
        fs::write(&out, &data).map_err(|e| format!("write {}: {e}", out.display()))?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    for ent in fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let ent = ent.map_err(|e| format!("readdir: {e}"))?;
        let from = ent.path();
        let to = dst.join(ent.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

fn preserve_ota_channel(imported: &str, current: &str) -> String {
    let dest_channel = extract_dashboard_key(current, "ota_channel");
    let Some(ch) = dest_channel else {
        return imported.to_string();
    };
    replace_or_insert_dashboard_key(imported, "ota_channel", &ch)
}

fn extract_dashboard_key(toml: &str, key: &str) -> Option<String> {
    let mut in_dash = false;
    for line in toml.lines() {
        let t = line.trim_start();
        if t.starts_with('[') && t.contains(']') {
            in_dash = t.starts_with("[dashboard]");
            continue;
        }
        if !in_dash || t.starts_with('#') {
            continue;
        }
        let Some(rest) = t.strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        return Some(rest.trim().to_string());
    }
    None
}

fn replace_or_insert_dashboard_key(toml: &str, key: &str, value_token: &str) -> String {
    let line_out = format!("{key} = {value_token}");
    let mut out = Vec::new();
    let mut in_dash = false;
    let mut wrote = false;
    let mut saw_dash = false;
    for line in toml.lines() {
        let t = line.trim_start();
        if t.starts_with('[') && t.contains(']') {
            if in_dash && !wrote {
                out.push(line_out.clone());
                wrote = true;
            }
            in_dash = t.starts_with("[dashboard]");
            if in_dash {
                saw_dash = true;
            }
            out.push(line.to_string());
            continue;
        }
        if in_dash {
            let is_key = !t.starts_with('#')
                && t.strip_prefix(key)
                    .map(|r| r.trim_start().starts_with('='))
                    .unwrap_or(false);
            if is_key {
                out.push(line_out.clone());
                wrote = true;
                continue;
            }
        }
        out.push(line.to_string());
    }
    if in_dash && !wrote {
        out.push(line_out);
    } else if !saw_dash {
        out.push(String::new());
        out.push("[dashboard]".into());
        out.push(line_out);
    }
    let mut s = out.join("\n");
    if toml.ends_with('\n') && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn scrub_source_dir(toml: &str) -> String {
    let mut out = Vec::new();
    let mut in_dash = false;
    for line in toml.lines() {
        let t = line.trim_start();
        if t.starts_with('[') && t.contains(']') {
            in_dash = t.starts_with("[dashboard]");
            out.push(line.to_string());
            continue;
        }
        if in_dash && !t.starts_with('#') {
            if let Some(rest) = t.strip_prefix("source_dir") {
                if rest.trim_start().starts_with('=') {
                    let val = rest
                        .trim_start()
                        .trim_start_matches('=')
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'');
                    if !val.is_empty() && !Path::new(val).is_dir() {
                        // Drop invalid host path — auto-detect on boot.
                        continue;
                    }
                }
            }
        }
        out.push(line.to_string());
    }
    let mut s = out.join("\n");
    if toml.ends_with('\n') && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn validate_config_toml(body: &str) -> Result<(), String> {
    match tetra_config::bluestation::parsing::from_toml_str(body) {
        Ok(cfg) => cfg.validate().map_err(|e| format!("config invalid: {e}")),
        Err(e) => Err(format!("config does not parse: {e}")),
    }
}

fn backup_profiles(config_path: &str) -> Result<(), String> {
    let src = profiles::profiles_dir(config_path);
    if !src.is_dir() {
        return Ok(());
    }
    let dst = config_dir(config_path).join("profiles.preimport");
    if dst.exists() {
        fs::remove_dir_all(&dst).map_err(|e| format!("remove profiles.preimport: {e}"))?;
    }
    copy_dir_recursive(&src, &dst)
}

/// Import a `.bptbs` station backup.
pub fn import_station_bptbs(config_path: &str, bytes: &[u8]) -> Result<ImportResult, String> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a valid zip/.bptbs: {e}"))?;
    let manifest = read_manifest(&mut archive)?;
    if manifest.format != FORMAT_STATION {
        return Err(format!(
            "expected {FORMAT_STATION} (got {}); use System → Import station for .bptbs",
            manifest.format
        ));
    }
    if manifest.format_version != FORMAT_VERSION {
        return Err(format!(
            "unsupported format_version {} (need {FORMAT_VERSION})",
            manifest.format_version
        ));
    }

    let mut config_body = String::from_utf8(read_zip_file(&mut archive, "config.toml")?)
        .map_err(|e| format!("config.toml utf8: {e}"))?;
    let current = fs::read_to_string(config_path).unwrap_or_default();
    config_body = preserve_ota_channel(&config_body, &current);
    config_body = scrub_source_dir(&config_body);
    validate_config_toml(&config_body)?;

    // Backup live config + profiles before replacing.
    let pre = format!("{config_path}.preimport");
    if Path::new(config_path).is_file() {
        fs::copy(config_path, &pre).map_err(|e| format!("backup config: {e}"))?;
    }
    backup_profiles(config_path)?;

    // Write config
    crate::net_dashboard::server::atomic_write(config_path, &config_body)
        .map_err(|e| format!("write config: {e}"))?;

    // Profiles
    let prof_dest = profiles::profiles_dir(config_path);
    extract_prefix_to_dir(&mut archive, "profiles", &prof_dest)?;

    // Fallback
    if let Ok(data) = read_zip_file(&mut archive, "config.toml.fallback") {
        let fb = format!("{config_path}.fallback");
        fs::write(&fb, data).map_err(|e| format!("write fallback: {e}"))?;
    }

    // setup.json
    if let Ok(data) = read_zip_file(&mut archive, "setup.json") {
        let setup = config_dir(config_path).join("setup.json");
        fs::write(&setup, data).map_err(|e| format!("write setup.json: {e}"))?;
    }

    // Sibling TOML profiles
    let sib_prefix = "optional/sibling-profiles/";
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("zip index: {e}"))?;
        let name = file.name().replace('\\', "/");
        if !name.starts_with(sib_prefix) || name.ends_with('/') {
            continue;
        }
        let base = &name[sib_prefix.len()..];
        if base.is_empty() || base.contains("..") || base.contains('/') {
            continue;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| format!("read {name}: {e}"))?;
        let dest = config_dir(config_path).join(base);
        fs::write(&dest, &data).map_err(|e| format!("write {}: {e}", dest.display()))?;
    }

    let mut warnings = Vec::new();
    match read_zip_file(&mut archive, "wifi/networks.json") {
        Ok(data) => match serde_json::from_slice::<Vec<WifiNetworkExport>>(&data) {
            Ok(nets) => {
                warnings.extend(wifi::import_saved_networks(&nets));
            }
            Err(e) => warnings.push(format!("wifi/networks.json invalid: {e}")),
        },
        Err(_) => {}
    }

    let _ = profiles::ensure_seeded(config_path);
    Ok(ImportResult {
        warnings,
        restart_required: true,
    })
}

/// Import a `.ptbs` profiles pack (no service restart).
pub fn import_profiles_ptbs(config_path: &str, bytes: &[u8]) -> Result<ImportResult, String> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a valid zip/.ptbs: {e}"))?;
    let manifest = read_manifest(&mut archive)?;
    if manifest.format != FORMAT_PROFILES {
        return Err(format!(
            "expected {FORMAT_PROFILES} (got {}); use Config → Import profiles for .ptbs, or System for .bptbs",
            manifest.format
        ));
    }
    if manifest.format_version != FORMAT_VERSION {
        return Err(format!(
            "unsupported format_version {} (need {FORMAT_VERSION})",
            manifest.format_version
        ));
    }
    backup_profiles(config_path)?;
    let prof_dest = profiles::profiles_dir(config_path);
    extract_prefix_to_dir(&mut archive, "profiles", &prof_dest)?;
    let _ = profiles::ensure_seeded(config_path);
    Ok(ImportResult {
        warnings: Vec::new(),
        restart_required: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::fs;

    fn tmp_cfg() -> (PathBuf, String) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("ptbs-bundle-test-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        let body = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[dashboard]
port = 80
https_port = 443
ota_channel = "beta"
"#;
        fs::write(&cfg, body).unwrap();
        (dir, cfg.to_string_lossy().to_string())
    }

    #[test]
    fn station_round_trip_preserves_dest_ota() {
        let (dir, cfg_path) = tmp_cfg();
        let _ = profiles::ensure_seeded(&cfg_path);
        let bytes = build_station_bptbs(&cfg_path).expect("export");
        let live = fs::read_to_string(&cfg_path).unwrap().replace(
            "ota_channel = \"beta\"",
            "ota_channel = \"stable\"",
        );
        fs::write(&cfg_path, &live).unwrap();
        let res = import_station_bptbs(&cfg_path, &bytes).expect("import");
        assert!(res.restart_required);
        let after = fs::read_to_string(&cfg_path).unwrap();
        assert!(
            after.contains("ota_channel = \"stable\""),
            "destination ota_channel must be preserved: {after}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn profiles_reject_station_format() {
        let (dir, cfg_path) = tmp_cfg();
        let _ = profiles::ensure_seeded(&cfg_path);
        let bytes = build_station_bptbs(&cfg_path).unwrap();
        let err = import_profiles_ptbs(&cfg_path, &bytes).unwrap_err();
        assert!(err.contains("ptbs-profiles") || err.contains("bptbs-station"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn profiles_round_trip() {
        let (dir, cfg_path) = tmp_cfg();
        let _ = profiles::ensure_seeded(&cfg_path);
        let bytes = build_profiles_ptbs(&cfg_path).unwrap();
        let res = import_profiles_ptbs(&cfg_path, &bytes).unwrap();
        assert!(!res.restart_required);
        assert!(profiles::profiles_dir(&cfg_path).join("active.json").is_file());
        let _ = fs::remove_dir_all(dir);
    }
}
