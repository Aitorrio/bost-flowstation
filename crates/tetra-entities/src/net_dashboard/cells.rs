//! Dashboard multi-cell support: per-cell status, and adding/removing `[[cells]]` entries.
//!
//! Adding or removing a cell is a config-file edit applied via a controlled restart (like the
//! Dual-Carrier switch). The prospective config is parsed and validated before it is written, so
//! a bad request never leaves an unbootable config behind.

use serde_json::{Value as JsonValue, json};
use tetra_config::bluestation::{CfgCellInfo, CfgSoapySdr, SharedConfig, StackConfig, parsing};
use tetra_core::CellId;

use crate::net_telemetry::{
    TelemetryEvent,
    channel::TelemetrySink,
    events::{CellCarrierInfo, CellInfo},
};

/// One cell's settings and live state, for the dashboard "Cells" card and telemetry.
fn cell_info(id: CellId, cfg: &SharedConfig, rf: Option<&crate::rf_status::RfStatus>) -> CellInfo {
    let c = cfg.config();
    let carriers = StackConfig::cell_phase_mod_carriers(&c.cell)
        .unwrap_or_default()
        .into_iter()
        .map(|(carrier_num, tx_freq_hz, rx_freq_hz)| CellCarrierInfo { carrier_num, tx_freq_hz, rx_freq_hz })
        .collect();
    let mut issis: Vec<u32> = cfg.state_read().subscribers.all_registered_issis().collect();
    issis.sort_unstable();
    CellInfo {
        id: id.0,
        primary: id.is_primary(),
        main_carrier: c.cell.main_carrier,
        secondary_carrier: c.cell.secondary_carrier,
        carriers,
        mcc: c.net.mcc,
        mnc: c.net.mnc,
        network: c.network.clone(),
        link_up: c.brew.as_ref().map(|_| cfg.state_read().brew_link_up),
        colour_code: c.cell.colour_code,
        location_area: c.cell.location_area,
        neighbours: c.cell.neighbor_cells_ca.len() as u16,
        device: c.phy_io.soapysdr.as_ref().and_then(|s| s.device.clone()),
        sdr_shared_with: Vec::new(),
        registered_radios: issis.len() as u32,
        registered_issis: issis,
        rf_state: rf.and_then(|r| serde_json::to_value(r.state).ok()?.as_str().map(str::to_string)),
        rf_detail: rf.map(|r| r.detail.clone()),
    }
}

/// Ids of the other cells whose device string equals cell `id`'s (empty without a device).
fn shared_with(devices: &[(u8, Option<String>)], id: u8) -> Vec<u8> {
    let Some(Some(mine)) = devices.iter().find(|(i, _)| *i == id).map(|(_, d)| d) else {
        return Vec::new();
    };
    devices
        .iter()
        .filter(|(i, d)| *i != id && d.as_deref().is_some_and(|d| !d.is_empty() && d == mine))
        .map(|(i, _)| *i)
        .collect()
}

/// Every running cell, primary first.
pub fn cell_infos(primary: &SharedConfig) -> Vec<CellInfo> {
    let rf = crate::rf_status::get_all();
    let rf_of = |id: CellId| rf.iter().find(|(c, _)| *c == id).map(|(_, s)| s);
    let mut cells = vec![cell_info(CellId::PRIMARY, primary, rf_of(CellId::PRIMARY))];
    for (id, cfg) in &crate::net_site::extra_cells() {
        cells.push(cell_info(*id, cfg, rf_of(*id)));
    }
    // Cells naming the same SDR device share it.
    let devices: Vec<(u8, Option<String>)> = cells.iter().map(|c| (c.id, c.device.as_ref().map(|d| d.trim().to_string()))).collect();
    for c in &mut cells {
        c.sdr_shared_with = shared_with(&devices, c.id);
    }
    cells
}

/// A cell's air-interface security from its config (EN 300 392-7 clause 6.2): `{"class": 1}` for a
/// clear cell, else the class and the key stream generator (`ksg` 1-4 = TEA1-TEA4).
fn security_summary(cfg: &SharedConfig) -> JsonValue {
    match cfg.config().security.aie.as_ref() {
        Some(a) if a.security_class() > 1 => json!({"class": a.security_class(), "ksg": a.ksg}),
        _ => json!({"class": 1}),
    }
}

/// GET /api/cells payload: every running cell, primary first, each with its `security`.
pub fn cells_json(primary: &SharedConfig) -> JsonValue {
    let mut security = vec![(CellId::PRIMARY.0, security_summary(primary))];
    for (id, cfg) in &crate::net_site::extra_cells() {
        security.push((id.0, security_summary(cfg)));
    }
    let cells: Vec<JsonValue> = cell_infos(primary)
        .into_iter()
        .map(|c| {
            let sec = security.iter().find(|(id, _)| *id == c.id).map(|(_, s)| s.clone());
            let mut v = serde_json::to_value(c).unwrap_or(JsonValue::Null);
            if let (Some(obj), Some(sec)) = (v.as_object_mut(), sec) {
                obj.insert("security".into(), sec);
            }
            v
        })
        .collect();
    json!({
        "cells": cells,
        "site_linked": primary.config().is_site_linked(),
        "max_cells": CellId::MAX as usize + 1,
    })
}

/// Background worker: sends a [`TelemetryEvent::CellsSnapshot`] every `CELLS_SNAPSHOT_INTERVAL`.
pub fn spawn_cells_telemetry(sink: TelemetrySink, primary: SharedConfig) {
    let spawned = std::thread::Builder::new().name("cells-telemetry".into()).spawn(move || {
        let mut last_site: Option<((Option<String>, f64, f64), std::time::Instant)> = None;
        loop {
            sink.send(TelemetryEvent::CellsSnapshot {
                site_linked: primary.config().is_site_linked(),
                cells: cell_infos(&primary),
            });
            // Config is read each round so an edit is picked up; unset/(0,0) sends nothing. Sent
            // when it changes and re-sent every SITE_LOCATION_RESEND in case the server missed it.
            let site = primary.config().telemetry.as_ref().and_then(|t| t.site_location.map(|(lat, lon)| (t.site_name.clone(), lat, lon)));
            match site {
                Some(cur) => {
                    let due = last_site.as_ref().is_none_or(|(sent, at)| *sent != cur || at.elapsed() >= SITE_LOCATION_RESEND);
                    if due {
                        sink.send(TelemetryEvent::SiteLocation { name: cur.0.clone(), lat: cur.1, lon: cur.2 });
                        last_site = Some((cur, std::time::Instant::now()));
                    }
                }
                None => last_site = None,
            }
            std::thread::sleep(CELLS_SNAPSHOT_INTERVAL);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("cells telemetry thread not started: {e}");
    }
}

const CELLS_SNAPSHOT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
/// How often the station position is re-sent when unchanged.
const SITE_LOCATION_RESEND: std::time::Duration = std::time::Duration::from_secs(60);

/// Ids used by `[[cells]]` entries in the file, including disabled ones.
fn used_ids(toml_text: &str) -> Result<Vec<u8>, String> {
    let table: toml::Table = toml::from_str(toml_text).map_err(|e| format!("config does not parse: {e}"))?;
    Ok(table
        .get("cells")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|c| c.get("id").and_then(|v| v.as_integer()))
                .filter_map(|i| u8::try_from(i).ok())
                .collect()
        })
        .unwrap_or_default())
}

/// DL/UL frequency of `main_carrier` with the primary cell's band plan.
pub(crate) fn carrier_freqs(primary: &CfgCellInfo, main_carrier: u16) -> Result<(u32, u32), String> {
    let mut cell = primary.clone();
    cell.main_carrier = main_carrier;
    cell.secondary_carrier = None;
    let carriers = StackConfig::cell_phase_mod_carriers(&cell)?;
    carriers
        .first()
        .map(|(_, dl, ul)| (*dl, *ul))
        .ok_or_else(|| "carrier has no frequency".to_string())
}

pub(crate) fn validated(toml_text: &str) -> Result<(), String> {
    let cfg = parsing::from_toml_str(toml_text).map_err(|e| format!("resulting config does not parse: {e}"))?;
    cfg.validate().map_err(|e| format!("resulting config is invalid: {e}"))?;
    let soapies = std::iter::once(cfg.phy_io.soapysdr.as_ref()).chain(cfg.extra_cells.iter().map(|c| c.soapysdr.as_ref()));
    for soapy in soapies.flatten() {
        validate_driver(soapy)?;
    }
    Ok(())
}

fn validate_driver(soapy: &CfgSoapySdr) -> Result<(), String> {
    tetra_config::bluestation::validate_soapysdr_driver_keys(soapy)
}

/// Append a new `[[cells]]` entry for SDR `device` on `main_carrier`. Returns the new config text
/// and the id given to the cell. The SDR's TX/RX frequencies follow from the carrier with the
/// primary cell's band plan; everything else is inherited from the primary cell.
pub fn add_cell_toml(
    original: &str,
    device: &str,
    main_carrier: u16,
    colour_code: Option<u8>,
    location_area: Option<u16>,
) -> Result<(String, u8), String> {
    let device = device.trim();
    if device.is_empty() {
        return Err("device is required (e.g. driver=plutosdr,uri=ip:192.168.3.1)".into());
    }
    let cfg = parsing::from_toml_str(original).map_err(|e| format!("current config does not parse: {e}"))?;
    let used = used_ids(original)?;
    let id = (1..=CellId::MAX)
        .find(|i| !used.contains(i))
        .ok_or_else(|| format!("at most {} cells", CellId::MAX as usize + 1))?;
    let (dl, ul) = carrier_freqs(&cfg.cell, main_carrier)?;

    let mut block = format!("\n[[cells]]\nid = {id}\n\n[cells.cell_info]\nmain_carrier = {main_carrier}\n");
    if let Some(cc) = colour_code {
        block.push_str(&format!("colour_code = {cc}\n"));
    }
    // Without it the cell inherits the primary's location area.
    if let Some(la) = location_area {
        if la > 16383 {
            return Err("location_area must be 0-16383".into());
        }
        block.push_str(&format!("location_area = {la}\n"));
    }
    block.push_str(&format!(
        "\n[cells.soapysdr]\ndevice = {}\ntx_freq = {dl}\nrx_freq = {ul}\n",
        toml::Value::String(device.to_string())
    ));

    let mut text = original.trim_end().to_string();
    text.push('\n');
    text.push_str(&block);
    validated(&text)?;
    Ok((text, id))
}

/// Remove the `[[cells]]` entry with `id` (with its `[cells.*]` sub-tables), keeping every other
/// line — comments included — untouched.
pub fn remove_cell_toml(original: &str, id: u8) -> Result<String, String> {
    if id == 0 {
        return Err("cell 0 is the primary [cell_info]; it cannot be removed".into());
    }
    let is_header = |l: &str| l.trim_start().starts_with('[');
    let is_cells_part = |l: &str| {
        let t = l.trim_start();
        t.starts_with("[[cells]]") || t.starts_with("[cells.")
    };

    let lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut i = 0;
    let mut removed = false;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with("[[cells]]") {
            out.push(lines[i]);
            i += 1;
            continue;
        }
        // One entry: its header, keys, and [cells.*] sub-tables up to the next other header.
        let start = i;
        i += 1;
        while i < lines.len() && !(is_header(lines[i]) && !is_cells_part(lines[i]) || lines[i].trim_start().starts_with("[[cells]]")) {
            i += 1;
        }
        let block = &lines[start..i];
        let block_id = block
            .iter()
            .take_while(|l| !l.trim_start().starts_with("[cells."))
            .filter_map(|l| {
                let (k, v) = l.split_once('=')?;
                (k.trim() == "id").then(|| v.split('#').next().unwrap_or("").trim().parse::<u8>().ok())?
            })
            .next();
        if block_id == Some(id) {
            removed = true;
            while out.last().is_some_and(|l| l.trim().is_empty()) {
                out.pop();
            }
        } else {
            out.extend_from_slice(block);
        }
    }
    if !removed {
        return Err(format!("no [[cells]] entry with id {id}"));
    }
    let mut text = out.join("\n");
    text.push('\n');
    validated(&text)?;
    Ok(text)
}

/// Effective settings of cell `id` (an additional cell) for the dashboard editor: what the cell
/// runs with, and whether its security is its own (`security_own`) or the station's. Key material
/// is never returned, only whether a key is set.
pub fn cell_settings_json(text: &str, id: u8) -> Result<JsonValue, String> {
    let cfg = parsing::from_toml_str(text).map_err(|e| format!("config does not parse: {e}"))?;
    let extra = cfg.extra_cells.iter().find(|c| c.id.0 == id).ok_or_else(|| format!("no cell {id} in the config"))?;
    let eff = cfg.for_extra_cell(extra.id).ok_or_else(|| format!("no cell {id} in the config"))?;
    Ok(json!({
        "id": id,
        "network": cfg.network_of_cell(extra.id).name,
        "network_explicit": extra.network.is_some(),
        "device": extra.soapysdr.as_ref().and_then(|s| s.device.clone()),
        "main_carrier": extra.cell.main_carrier,
        "colour_code": extra.cell.colour_code,
        "location_area": extra.cell.location_area,
        "security_own": extra.security.is_some(),
        "security": security_json(&eff.security),
    }))
}

/// The air-interface encryption and access settings a cell runs with, for the dashboard. Key
/// material is never returned, only whether a key is set.
pub(crate) fn security_json(sec: &tetra_config::bluestation::CfgSecurity) -> JsonValue {
    let aie = sec.aie.as_ref();
    let low = |d: String| d.to_lowercase();
    json!({
        "class": aie.map_or(1, |a| a.security_class()),
        "ksg": aie.map(|a| a.ksg),
        "sckn": aie.map(|a| a.sckn),
        "sck_vn": aie.map(|a| a.sck_vn),
        "has_sck": aie.is_some_and(|a| a.class3.is_none()),
        "cck_id": aie.and_then(|a| a.class3.as_ref().map(|c| c.cck_id)),
        "has_cck": aie.is_some_and(|a| a.class3.is_some()),
        "clear_groups": aie.map(|a| a.clear_groups.clone()).unwrap_or_default(),
        "issi_whitelist": sec.issi_whitelist,
        "whitelist_mode": low(format!("{:?}", sec.whitelist_mode)),
        "authentication": low(format!("{:?}", sec.authentication)),
        "mutual_authentication": sec.mutual_authentication,
    })
}

/// `[security]` keys the per-cell editor owns. Everything else in the table (key source,
/// subscriber keys, E2EE, limits) is station-wide and left alone.
pub(crate) const CELL_SECURITY_KEYS: &[&str] = &["aie", "issi_whitelist", "whitelist_mode", "authentication", "mutual_authentication"];

/// Build the security table for an editor request `{aie?: {...}, issi_whitelist?, whitelist_mode?,
/// authentication?, mutual_authentication?}`. `old_aie` is the `aie` table in the file: an empty
/// `sck`/`cck` keeps the key already there.
pub(crate) fn security_table(sec: &JsonValue, old_aie: Option<&toml_edit::Item>) -> Result<toml_edit::Table, String> {
    use toml_edit::{Array, Item, Table, value};
    let old_key = |k: &str| old_aie.and_then(|a| a.get(k)).and_then(|v| v.as_str()).map(str::to_string);
    let mut st = Table::new();
    if let Some(a) = sec.get("aie").filter(|a| a.is_object()) {
        let class = ti_u(a, "class", 3)?.unwrap_or(1);
        let mut at = Table::new();
        at["class"] = value(class);
        if class >= 2 {
            if let Some(k) = ti_u(a, "ksg", 4)? {
                at["ksg"] = value(k);
            }
        }
        let hex = |k: &str| a.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).or_else(|| old_key(k));
        if class == 2 {
            for k in ["sckn", "sck_vn"] {
                if let Some(n) = ti_u(a, k, 65535)? {
                    at[k] = value(n);
                }
            }
            if let Some(k) = hex("sck") {
                at["sck"] = value(k);
            }
            let groups: Vec<i64> = a.get("clear_groups").and_then(|g| g.as_array()).map(|g| g.iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default();
            if !groups.is_empty() {
                at["clear_groups"] = value(groups.into_iter().collect::<Array>());
            }
        }
        if class == 3 {
            if let Some(k) = hex("cck") {
                at["cck"] = value(k);
            }
            if let Some(n) = ti_u(a, "cck_id", 65535)? {
                at["cck_id"] = value(n);
            }
        }
        st["aie"] = Item::Table(at);
    }
    if let Some(list) = sec.get("issi_whitelist").and_then(|l| l.as_array()) {
        st["issi_whitelist"] = value(list.iter().filter_map(|x| x.as_i64()).collect::<Array>());
    }
    for k in ["whitelist_mode", "authentication"] {
        if let Some(v) = sec.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
            st[k] = value(v);
        }
    }
    if let Some(b) = sec.get("mutual_authentication").and_then(|v| v.as_bool()) {
        st["mutual_authentication"] = value(b);
    }
    Ok(st)
}

/// Write an additional cell's security request into its `[[cells]]` entry: `{"inherit": true}`
/// removes `[cells.security]` (the station's applies), anything else replaces it.
pub(crate) fn write_cell_security(entry: &mut toml_edit::Table, sec: &JsonValue) -> Result<(), String> {
    if sec.get("inherit").and_then(|v| v.as_bool()) == Some(true) {
        entry.remove("security");
        return Ok(());
    }
    let old_aie = entry.get("security").and_then(|s| s.get("aie")).cloned();
    let st = security_table(sec, old_aie.as_ref())?;
    entry["security"] = toml_edit::Item::Table(st);
    Ok(())
}

/// Write the primary cell's security request into the station's `[security]`: only the keys the
/// cell editor owns ([`CELL_SECURITY_KEYS`]) are replaced, the station-wide ones stay.
pub(crate) fn write_primary_security(doc: &mut toml_edit::DocumentMut, sec: &JsonValue) -> Result<(), String> {
    use toml_edit::{Item, Table};
    let old_aie = doc.get("security").and_then(|s| s.get("aie")).cloned();
    let st = security_table(sec, old_aie.as_ref())?;
    let table = doc.entry("security").or_insert(Item::Table(Table::new())).as_table_mut().ok_or("[security] is not a table")?;
    for k in CELL_SECURITY_KEYS {
        table.remove(k);
    }
    for (k, v) in st.iter() {
        table.insert(k, v.clone());
    }
    Ok(())
}

fn ti_u(v: &JsonValue, key: &str, max: u64) -> Result<Option<i64>, String> {
    match v.get(key).filter(|x| !x.is_null()) {
        None => Ok(None),
        Some(x) => x.as_u64().filter(|n| *n <= max).map(|n| Some(n as i64)).ok_or_else(|| format!("{key} must be 0-{max}")),
    }
}

/// Change cell `id`'s settings in the config text, keeping comments and everything not named.
/// `req`: `{"id": N, "network"?: "name"|"", "cell": {main_carrier?, colour_code?, location_area?
/// (null = inherit)}, "security": {"inherit": true} | {aie?: {...}, issi_whitelist?, whitelist_mode?,
/// authentication?, mutual_authentication?}}`. An empty `sck`/`cck` keeps the key already in the
/// file. The result is validated like add/remove.
pub fn update_cell_toml(original: &str, req: &JsonValue) -> Result<String, String> {
    use toml_edit::{DocumentMut, Item, Table, value};
    let id = req.get("id").and_then(|v| v.as_u64()).and_then(|v| u8::try_from(v).ok()).filter(|i| *i != 0).ok_or("id (1-7) is required")?;
    let cfg = parsing::from_toml_str(original).map_err(|e| format!("current config does not parse: {e}"))?;
    let mut doc: DocumentMut = original.parse().map_err(|e| format!("config does not parse: {e}"))?;
    let entry = doc
        .get_mut("cells")
        .and_then(|c| c.as_array_of_tables_mut())
        .and_then(|a| a.iter_mut().find(|t| t.get("id").and_then(|v| v.as_integer()) == Some(id as i64)))
        .ok_or_else(|| format!("no [[cells]] entry with id {id}"))?;

    if let Some(net) = req.get("network") {
        match net.as_str().map(str::trim) {
            Some(n) if !n.is_empty() => entry["network"] = value(n),
            _ => {
                entry.remove("network");
            }
        }
    }

    if let Some(cell) = req.get("cell").filter(|c| c.is_object()) {
        let ci = entry.entry("cell_info").or_insert(Item::Table(Table::new())).as_table_mut().ok_or("cell_info is not a table")?;
        if let Some(c) = ti_u(cell, "main_carrier", 4095)? {
            ci["main_carrier"] = value(c);
            if let Some(sdr) = entry.get_mut("soapysdr").and_then(|s| s.as_table_mut()) {
                let (dl, ul) = carrier_freqs(&cfg.cell, c as u16)?;
                sdr["tx_freq"] = value(dl as i64);
                sdr["rx_freq"] = value(ul as i64);
            }
        }
        let ci = entry.get_mut("cell_info").and_then(|c| c.as_table_mut()).ok_or("cell_info is not a table")?;
        if let Some(c) = ti_u(cell, "colour_code", 63)? {
            ci["colour_code"] = value(c);
        }
        if cell.get("location_area").is_some() {
            match ti_u(cell, "location_area", 16383)? {
                Some(la) => ci["location_area"] = value(la),
                None => {
                    ci.remove("location_area");
                }
            }
        }
    }

    if let Some(sec) = req.get("security").filter(|s| s.is_object()) {
        write_cell_security(entry, sec)?;
    }

    let text = doc.to_string();
    validated(&text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    #[test]
    fn cells_on_one_device_list_each_other() {
        let d = |i: u8, x: &str| (i, Some(x.to_string()));
        let devs = vec![d(0, "driver=lime"), d(1, "driver=lime"), d(2, "driver=pluto"), (3, None)];
        assert_eq!(super::shared_with(&devs, 0), vec![1]);
        assert_eq!(super::shared_with(&devs, 1), vec![0]);
        assert!(super::shared_with(&devs, 2).is_empty());
        assert!(super::shared_with(&devs, 3).is_empty());
    }

    use super::*;

    const BASE: &str = r#"config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1521
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1
"#;

    #[test]
    fn security_summary_names_class_and_tea() {
        let shared = |t: &str| SharedConfig::try_from_parts(parsing::from_toml_str(t).unwrap(), None).unwrap();
        assert_eq!(security_summary(&shared(BASE)), json!({"class": 1}));
        let sc2 = format!("{BASE}[security.aie]\nclass = 2\nksg = 4\nsckn = 1\nsck = \"00112233445566778899\"\n");
        assert_eq!(security_summary(&shared(&sc2)), json!({"class": 2, "ksg": 4}));
        let staged = format!("{BASE}[security.aie]\nclass = 1\nksg = 2\nsckn = 1\nsck = \"00112233445566778899\"\n");
        assert_eq!(security_summary(&shared(&staged)), json!({"class": 1}), "a staged key keeps the cell clear");
        assert!(cells_json(&shared(&sc2))["cells"][0]["security"] == json!({"class": 2, "ksg": 4}));
    }

    #[test]
    fn add_then_remove_cell_round_trips() {
        let (with_one, id) = add_cell_toml(BASE, "driver=plutosdr,uri=ip:192.168.3.1", 1525, Some(2), None).unwrap();
        assert_eq!(id, 1);
        let cfg = parsing::from_toml_str(&with_one).unwrap();
        let cell = &cfg.extra_cells[0];
        assert_eq!(cell.cell.main_carrier, 1525);
        assert_eq!(cell.cell.colour_code, 2);
        assert_eq!(cell.cell.location_area, 1, "inherited from the primary");
        let soapy = cell.soapysdr.as_ref().unwrap();
        assert_eq!(soapy.device.as_deref(), Some("driver=plutosdr,uri=ip:192.168.3.1"));
        let (dl, ul) = carrier_freqs(&cfg.cell, 1525).unwrap();
        assert_eq!((soapy.dl_freq as u32, soapy.ul_freq as u32), (dl, ul));

        let (with_two, id2) = add_cell_toml(&with_one, "driver=plutosdr,uri=ip:192.168.4.1", 1529, None, None).unwrap();
        assert_eq!(id2, 2);

        let back = remove_cell_toml(&with_two, 1).unwrap();
        let cfg = parsing::from_toml_str(&back).unwrap();
        assert_eq!(cfg.extra_cells.len(), 1);
        assert_eq!(cfg.extra_cells[0].id, CellId(2));

        let empty = remove_cell_toml(&back, 2).unwrap();
        assert_eq!(empty.trim_end(), BASE.trim_end(), "the original file comes back unchanged");
    }

    #[test]
    fn add_rejects_a_carrier_already_in_use() {
        let err = add_cell_toml(BASE, "driver=plutosdr", 1521, None, None).unwrap_err();
        assert!(err.contains("carrier 1521"), "{err}");
    }

    #[test]
    fn remove_keeps_following_sections_and_rejects_unknown_ids() {
        let (text, _) = add_cell_toml(BASE, "driver=plutosdr", 1525, None, None).unwrap();
        let text = format!("{text}\n[security]\n# keep me\n");
        let out = remove_cell_toml(&text, 1).unwrap();
        assert!(out.contains("[security]") && out.contains("# keep me"));
        assert!(!out.contains("[[cells]]"));
        assert!(remove_cell_toml(&out, 1).is_err());
        assert!(remove_cell_toml(&out, 0).is_err());
    }

    #[test]
    fn add_sets_the_cells_own_location_area() {
        let (text, _) = add_cell_toml(BASE, "driver=plutosdr", 1525, None, Some(7)).unwrap();
        let cfg = parsing::from_toml_str(&text).unwrap();
        assert_eq!((cfg.cell.location_area, cfg.extra_cells[0].cell.location_area), (1, 7));
        assert!(add_cell_toml(BASE, "driver=plutosdr", 1525, None, Some(16384)).is_err());
    }
    #[test]
    fn update_changes_cell_settings_and_security_without_touching_the_rest() {
        // A class 2 station: its cells of one network stay class 2, each with its own key.
        let base = format!("{BASE}# keep me\n[security.aie]\nclass = 2\nksg = 1\nsck = \"aabbccddeeff00112233\"\n");
        let (with_one, _) = add_cell_toml(&base, "driver=plutosdr,uri=ip:192.168.3.1", 1525, Some(2), None).unwrap();
        // Cell settings.
        let req = json!({"id": 1, "cell": {"colour_code": 7, "location_area": 5, "main_carrier": 1530}});
        let t = update_cell_toml(&with_one, &req).unwrap();
        assert!(t.contains("# keep me"));
        let cfg = parsing::from_toml_str(&t).unwrap();
        let c = &cfg.extra_cells[0];
        assert_eq!((c.cell.colour_code, c.cell.location_area, c.cell.main_carrier), (7, 5, 1530));
        assert_eq!(c.soapysdr.as_ref().unwrap().dl_freq as u32, carrier_freqs(&cfg.cell, 1530).unwrap().0, "SDR frequencies follow the carrier");
        // Back to the inherited location area.
        let t = update_cell_toml(&t, &json!({"id": 1, "cell": {"location_area": null}})).unwrap();
        assert_eq!(parsing::from_toml_str(&t).unwrap().extra_cells[0].cell.location_area, 1);

        // Own security: class 2 with a key; reading never returns the key.
        let sec = json!({"id": 1, "security": {"aie": {"class": 2, "ksg": 2, "sckn": 3, "sck": "00112233445566778899"}, "issi_whitelist": [1001], "whitelist_mode": "enforce"}});
        let t = update_cell_toml(&t, &sec).unwrap();
        let s = cell_settings_json(&t, 1).unwrap();
        assert_eq!(s["security_own"], true);
        assert_eq!((s["security"]["class"].clone(), s["security"]["sckn"].clone(), s["security"]["has_sck"].clone()), (json!(2), json!(3), json!(true)));
        assert_eq!(s["security"]["issi_whitelist"], json!([1001]));
        assert!(!s.to_string().contains("00112233445566778899"), "key material stays out of the API");
        // An empty key keeps the one in the file; a new class-2 setting with a changed SCKN works.
        let t2 = update_cell_toml(&t, &json!({"id": 1, "security": {"aie": {"class": 2, "ksg": 2, "sckn": 4, "sck": ""}}})).unwrap();
        assert!(t2.contains("00112233445566778899"));
        assert_eq!(cell_settings_json(&t2, 1).unwrap()["security"]["sckn"], 4);
        // Back to the station's security.
        let t3 = update_cell_toml(&t2, &json!({"id": 1, "security": {"inherit": true}})).unwrap();
        assert_eq!(cell_settings_json(&t3, 1).unwrap()["security_own"], false);
        // A clear cell next to the class 2 station is refused (same network).
        assert!(update_cell_toml(&t3, &json!({"id": 1, "security": {"aie": {"class": 1}}})).unwrap_err().contains("security class"));
        // Class 2 without any key is refused by validation; cell 0 and unknown ids are refused.
        assert!(update_cell_toml(&t3, &json!({"id": 1, "security": {"aie": {"class": 2}}})).is_err());
        assert!(update_cell_toml(&t3, &json!({"id": 0})).is_err());
        assert!(update_cell_toml(&t3, &json!({"id": 6})).is_err());
        assert!(update_cell_toml(&t3, &json!({"id": 1, "cell": {"colour_code": 64}})).is_err());
    }
}
