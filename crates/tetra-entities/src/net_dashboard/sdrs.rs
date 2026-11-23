//! Dashboard Cells page: every SDR of the station with the cells on it, and per cell its carriers,
//! network, location area and encryption.
//!
//! Each SDR runs in one of three modes:
//! - `single`: one cell on one carrier (the classic setup);
//! - `multi_carrier`: one cell on two carriers (`secondary_carrier`, one cell identity);
//! - `multi_cell`: several cells on the one SDR (multi-cell phase 7, `SdrHub`), each with its own
//!   colour code, location area and network; a cell may still have a second carrier.
//!
//! The page writes the existing config format, so every other editor (Config page, Dual Carrier,
//! profiles, calibration) keeps working: cells naming the same `soapysdr.device` share that SDR,
//! and every member of a group gets the same hardware keys. The centre frequencies are computed
//! here (midway between the group's carriers) instead of being typed in. The primary cell's
//! encryption is the station's `[security]` (only the per-cell keys, see `cells::CELL_SECURITY_KEYS`);
//! an additional cell's is `[cells.security]` or, without it, the station's.

use serde_json::{Value as JsonValue, json};
use tetra_config::bluestation::{StackConfig, parsing};
use tetra_core::CellId;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::net_dashboard::cells::{carrier_freqs, validated};
use crate::net_dashboard::dual_carrier::device_default_sample_rate_hz;

/// Hardware keys the page owns in a cell's `soapysdr` table (plus `rx_gain_*` / `tx_gain_*`).
const HW_KEYS: &[&str] = &["device", "sample_rate", "ppm_err", "rx_antenna", "tx_antenna", "tx_center_freq", "rx_center_freq"];

/// The `soapysdr` table of cell `id` in the document (`[phy_io.soapysdr]` for the primary).
fn soapy_table<'a>(doc: &'a DocumentMut, id: u8) -> Option<&'a dyn toml_edit::TableLike> {
    if id == 0 {
        return doc.get("phy_io")?.get("soapysdr")?.as_table_like();
    }
    doc.get("cells")?
        .as_array_of_tables()?
        .iter()
        .find(|t| t.get("id").and_then(|v| v.as_integer()) == Some(id as i64))?
        .get("soapysdr")?
        .as_table_like()
}

/// `rx_gain_*` / `tx_gain_*` keys of a soapysdr table, with the gain names as written.
fn gains_of(t: Option<&dyn toml_edit::TableLike>, prefix: &str) -> serde_json::Map<String, JsonValue> {
    let mut m = serde_json::Map::new();
    if let Some(t) = t {
        for (k, v) in t.iter() {
            if let Some(name) = k.strip_prefix(prefix) {
                if let Some(n) = v.as_float().or_else(|| v.as_integer().map(|i| i as f64)) {
                    m.insert(name.to_string(), json!(n));
                }
            }
        }
    }
    m
}

/// SDRs of the config with their cells, for the page. Cells without a device (no SoapySDR)
/// are each shown as an SDR of their own.
pub fn sdrs_json(text: &str) -> Result<JsonValue, String> {
    let cfg = parsing::from_toml_str(text).map_err(|e| format!("config does not parse: {e}"))?;
    let doc: DocumentMut = text.parse().map_err(|e| format!("config does not parse: {e}"))?;
    let mut groups: Vec<(String, Vec<CellId>)> = Vec::new();
    for (id, _, soapy) in cfg.cells() {
        let dev = soapy.and_then(|s| s.device.as_deref()).map(str::trim).unwrap_or("").to_string();
        match groups.iter_mut().find(|(d, _)| !dev.is_empty() && *d == dev) {
            Some((_, m)) => m.push(id),
            None => groups.push((dev, vec![id])),
        }
    }
    let cells = cfg.cells();
    let mut out = Vec::new();
    for (device, ids) in groups {
        let first = ids[0];
        let (_, _, soapy) = cells.iter().find(|(i, _, _)| *i == first).copied().expect("cell of the group");
        let raw = soapy_table(&doc, first.0);
        let mut members = Vec::new();
        let mut carriers = 0;
        for id in &ids {
            let (_, cell, _) = cells.iter().find(|(i, _, _)| i == id).copied().expect("cell of the group");
            let extra = cfg.extra_cells.iter().find(|c| c.id == *id);
            carriers += 1 + usize::from(cell.secondary_carrier.is_some());
            let sec = if id.0 == 0 { cfg.security.clone() } else { cfg.for_extra_cell(*id).map(|e| e.security).unwrap_or_else(|| cfg.security.clone()) };
            let freqs = |c: Option<u16>| c.and_then(|c| carrier_freqs(&cfg.cell, c).ok()).map(|(dl, ul)| json!({"dl_hz": dl, "ul_hz": ul}));
            members.push(json!({
                "id": id.0,
                "primary": id.0 == 0,
                "main_freqs": freqs(Some(cell.main_carrier)),
                "secondary_freqs": freqs(cell.secondary_carrier),
                "location_area_own": id.0 == 0 || own_location_area(&doc, id.0),
                "security_own": id.0 == 0 || extra.is_some_and(|c| c.security.is_some()),
                "security": crate::net_dashboard::cells::security_json(&sec),
                "main_carrier": cell.main_carrier,
                "secondary_carrier": cell.secondary_carrier,
                "colour_code": cell.colour_code,
                "location_area": cell.location_area,
                "network": cfg.network_of_cell(*id).name,
                "network_explicit": extra.is_some_and(|c| c.network.is_some()),
            }));
        }
        let mode = match (ids.len(), carriers) {
            (1, 1) => "single",
            (1, _) => "multi_carrier",
            _ => "multi_cell",
        };
        out.push(json!({
            "device": device,
            "mode": mode,
            // Without `sample_rate` the SDR runs at its driver's default; show that value.
            "sample_rate": soapy.and_then(|s| s.fs).unwrap_or_else(|| device_default_sample_rate_hz(Some(device.as_str()))),
            "sample_rate_default": soapy.and_then(|s| s.fs).is_none(),
            "ppm_err": soapy.map_or(0.0, |s| s.ppm_err),
            "rx_antenna": soapy.and_then(|s| s.rx_ant.clone()),
            "tx_antenna": soapy.and_then(|s| s.tx_ant.clone()),
            "rx_gains": gains_of(raw, "rx_gain_"),
            "tx_gains": gains_of(raw, "tx_gain_"),
            "cells": members,
        }));
    }
    let networks: Vec<JsonValue> = if cfg.networks_explicit {
        cfg.networks
            .iter()
            .map(|n| json!({"name": n.name, "mcc": n.net.mcc, "mnc": n.net.mnc, "cell_count": cfg.network_cells(&n.name).len()}))
            .collect()
    } else {
        vec![json!({"name": cfg.network, "mcc": cfg.net.mcc, "mnc": cfg.net.mnc, "cell_count": cfg.extra_cells.len() + 1})]
    };
    Ok(json!({
        "networks_explicit": cfg.networks_explicit,
        "networks": networks,
        "max_cells": CellId::MAX as usize + 1,
        "soapysdr": cfg.phy_io.backend == tetra_config::bluestation::PhyBackend::SoapySdr,
        "sdrs": out,
    }))
}

/// True when `[[cells]]` entry `id` sets its own location area (else it inherits cell 0's).
fn own_location_area(doc: &DocumentMut, id: u8) -> bool {
    doc.get("cells")
        .and_then(|c| c.as_array_of_tables())
        .and_then(|a| a.iter().find(|t| t.get("id").and_then(|v| v.as_integer()) == Some(id as i64)))
        .and_then(|t| t.get("cell_info"))
        .and_then(|c| c.get("location_area"))
        .is_some()
}

/// One SDR of a request, checked.
struct SdrPlan {
    device: String,
    sample_rate: Option<f64>,
    ppm_err: f64,
    rx_antenna: Option<String>,
    tx_antenna: Option<String>,
    rx_gains: Vec<(String, f64)>,
    tx_gains: Vec<(String, f64)>,
    /// Centre frequencies (Hz); `None` = single carrier, the SDR tunes to the carrier itself.
    tx_center: Option<f64>,
    rx_center: Option<f64>,
    cells: Vec<CellPlan>,
    /// (cell index in `cells`, carrier, DL Hz, UL Hz) of every carrier on this SDR.
    carriers: Vec<(usize, u16, u32, u32)>,
}

struct CellPlan {
    /// Existing cell id, `None` = a new cell.
    id: Option<u8>,
    main_carrier: u16,
    secondary_carrier: Option<u16>,
    colour_code: Option<u8>,
    /// `None` = inherit the primary's (additional cells only).
    location_area: Option<u16>,
    /// `None` = leave as it is; `Some("")` = the primary's network (additional cells).
    network: Option<String>,
    /// Encryption and access request (`cells::security_table` format, `{"inherit": true}` for an
    /// additional cell on the station's `[security]`); `None` = leave as it is.
    security: Option<JsonValue>,
}

fn num(v: &JsonValue, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

fn uint(v: &JsonValue, key: &str, max: u64, what: &str) -> Result<Option<u64>, String> {
    match v.get(key).filter(|x| !x.is_null()) {
        None => Ok(None),
        Some(x) => x.as_u64().filter(|n| *n <= max).map(Some).ok_or_else(|| format!("{what}: {key} must be 0-{max}")),
    }
}

fn gains(v: &JsonValue, key: &str, what: &str) -> Result<Vec<(String, f64)>, String> {
    let Some(map) = v.get(key).and_then(|g| g.as_object()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (name, n) in map {
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("{what}: gain name '{name}' must be letters, digits or _"));
        }
        let n = n.as_f64().ok_or_else(|| format!("{what}: gain {name} must be a number"))?;
        out.push((name.to_string(), n));
    }
    Ok(out)
}

/// Read and check a request (`{"sdrs": [...]}`, the shape `sdrs_json` returns) against the
/// config: modes, cell counts, carriers, and the passband of every SDR.
fn plan(cfg: &StackConfig, req: &JsonValue) -> Result<Vec<SdrPlan>, String> {
    let list = req.get("sdrs").and_then(|s| s.as_array()).ok_or("sdrs is required")?;
    if list.is_empty() {
        return Err("at least one SDR is required".into());
    }
    let mut plans = Vec::new();
    let mut seen_ids = Vec::new();
    let mut seen_devices: Vec<String> = Vec::new();
    let mut total_cells = 0;
    for (n, s) in list.iter().enumerate() {
        let what = format!("SDR {}", n + 1);
        let device = s.get("device").and_then(|d| d.as_str()).map(str::trim).unwrap_or("").to_string();
        if !device.is_empty() {
            if seen_devices.contains(&device) {
                return Err(format!("{what}: device '{device}' is listed twice; put its cells under one SDR"));
            }
            seen_devices.push(device.clone());
        } else if cfg.phy_io.backend == tetra_config::bluestation::PhyBackend::SoapySdr && list.len() > 1 {
            return Err(format!("{what}: device is required when the station has more than one SDR"));
        }
        let mode = s.get("mode").and_then(|m| m.as_str()).unwrap_or("single");
        let raw_cells = s.get("cells").and_then(|c| c.as_array()).ok_or_else(|| format!("{what}: cells is required"))?;
        let mut cells = Vec::new();
        for c in raw_cells {
            let id = uint(c, "id", CellId::MAX as u64, &what)?.map(|i| i as u8);
            if let Some(id) = id {
                if seen_ids.contains(&id) {
                    return Err(format!("cell {id} is listed twice"));
                }
                if id != 0 && !cfg.extra_cells.iter().any(|e| e.id.0 == id) {
                    return Err(format!("cell {id} is not in the config"));
                }
                seen_ids.push(id);
            }
            let main_carrier = uint(c, "main_carrier", 4095, &what)?.ok_or_else(|| format!("{what}: every cell needs a main carrier"))? as u16;
            let secondary_carrier = uint(c, "secondary_carrier", 4095, &what)?.map(|v| v as u16);
            if secondary_carrier == Some(main_carrier) {
                return Err(format!("{what}: the second carrier must differ from the main carrier"));
            }
            cells.push(CellPlan {
                id,
                main_carrier,
                secondary_carrier,
                colour_code: uint(c, "colour_code", 63, &what)?.map(|v| v as u8),
                location_area: uint(c, "location_area", 16383, &what)?.map(|v| v as u16),
                network: c.get("network").and_then(|v| v.as_str()).map(|v| v.trim().to_string()),
                security: c.get("security").filter(|v| v.is_object()).cloned(),
            });
        }
        match mode {
            "single" if cells.len() != 1 || cells[0].secondary_carrier.is_some() => {
                return Err(format!("{what}: mode single runs one cell on one carrier"));
            }
            "multi_carrier" if cells.len() != 1 || cells[0].secondary_carrier.is_none() => {
                return Err(format!("{what}: mode multi-carrier runs one cell on two carriers"));
            }
            "multi_cell" if cells.len() < 2 => return Err(format!("{what}: mode multi-cell needs at least two cells")),
            "single" | "multi_carrier" | "multi_cell" => {}
            other => return Err(format!("{what}: unknown mode '{other}'")),
        }
        total_cells += cells.len();

        let mut carriers = Vec::new();
        for (i, c) in cells.iter().enumerate() {
            for carrier in [Some(c.main_carrier), c.secondary_carrier].into_iter().flatten() {
                let (dl, ul) = carrier_freqs(&cfg.cell, carrier).map_err(|e| format!("{what}: carrier {carrier}: {e}"))?;
                carriers.push((i, carrier, dl, ul));
            }
        }
        let sample_rate = num(s, "sample_rate").filter(|f| *f > 0.0).or_else(|| (!device.is_empty()).then(|| device_default_sample_rate_hz(Some(device.as_str()))));
        let (tx_center, rx_center) = if carriers.len() > 1 {
            let fs = sample_rate.ok_or_else(|| format!("{what}: several carriers on one SDR need a sample rate"))?;
            let mid = |f: &dyn Fn(&(usize, u16, u32, u32)) -> u32| {
                let lo = carriers.iter().map(f).min().unwrap_or(0) as f64;
                let hi = carriers.iter().map(f).max().unwrap_or(0) as f64;
                (lo, hi, (lo + hi) / 2.0)
            };
            let (dlo, dhi, tx) = mid(&|c| c.2);
            let (ulo, uhi, rx) = mid(&|c| c.3);
            if dhi - dlo > fs || uhi - ulo > fs {
                return Err(format!(
                    "{what}: the carriers span {:.3} MHz, more than the sample rate ({:.3} MHz) covers; raise the sample rate or move the carriers closer",
                    (dhi - dlo).max(uhi - ulo) / 1e6,
                    fs / 1e6
                ));
            }
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let text_opt = |k: &str| s.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        plans.push(SdrPlan {
            device,
            sample_rate,
            ppm_err: num(s, "ppm_err").unwrap_or(0.0),
            rx_antenna: text_opt("rx_antenna"),
            tx_antenna: text_opt("tx_antenna"),
            rx_gains: gains(s, "rx_gains", &what)?,
            tx_gains: gains(s, "tx_gains", &what)?,
            tx_center,
            rx_center,
            cells,
            carriers,
        });
    }
    if !seen_ids.contains(&0) {
        return Err("the primary cell (cell 0) must be on one of the SDRs".into());
    }
    if total_cells > CellId::MAX as usize + 1 {
        return Err(format!("at most {} cells", CellId::MAX as usize + 1));
    }
    Ok(plans)
}

/// POST /api/sdrs/check: what the request would give (centres, carriers' frequencies) or why it
/// is refused, without writing anything. The whole resulting config is validated too.
pub fn check_json(text: &str, req: &JsonValue) -> JsonValue {
    let cfg = match parsing::from_toml_str(text) {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": format!("config does not parse: {e}")}),
    };
    let plans = match plan(&cfg, req) {
        Ok(p) => p,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let sdrs: Vec<JsonValue> = plans
        .iter()
        .map(|p| {
            json!({
                "tx_center_freq": p.tx_center,
                "rx_center_freq": p.rx_center,
                "carriers": p.carriers.iter().map(|(i, c, dl, ul)| json!({"cell": i, "carrier": c, "dl_hz": dl, "ul_hz": ul})).collect::<Vec<_>>(),
            })
        })
        .collect();
    match apply_sdrs(text, req) {
        Ok(_) => json!({"ok": true, "sdrs": sdrs}),
        Err(e) => json!({"ok": false, "error": e, "sdrs": sdrs}),
    }
}

/// Write one SDR's hardware keys and this cell's carrier into a `soapysdr` table.
fn write_soapy(t: &mut Table, p: &SdrPlan, dl: u32, ul: u32) {
    let mut want: Vec<(String, Item)> = Vec::new();
    if !p.device.is_empty() {
        want.push(("device".into(), value(p.device.as_str())));
    }
    want.push(("tx_freq".into(), value(dl as i64)));
    want.push(("rx_freq".into(), value(ul as i64)));
    if let Some(fs) = p.sample_rate {
        want.push(("sample_rate".into(), value(fs as i64)));
    }
    if let Some(c) = p.tx_center {
        want.push(("tx_center_freq".into(), value(c.round() as i64)));
    }
    if let Some(c) = p.rx_center {
        want.push(("rx_center_freq".into(), value(c.round() as i64)));
    }
    if p.ppm_err != 0.0 {
        want.push(("ppm_err".into(), value(p.ppm_err)));
    }
    if let Some(a) = &p.rx_antenna {
        want.push(("rx_antenna".into(), value(a.as_str())));
    }
    if let Some(a) = &p.tx_antenna {
        want.push(("tx_antenna".into(), value(a.as_str())));
    }
    // Whole dB stay integers in the file (as people write them).
    let gain = |g: f64| if g.fract() == 0.0 && g.abs() < 1e9 { value(g as i64) } else { value(g) };
    for (name, g) in &p.rx_gains {
        want.push((format!("rx_gain_{name}"), gain(*g)));
    }
    for (name, g) in &p.tx_gains {
        want.push((format!("tx_gain_{name}"), gain(*g)));
    }
    // Drop the page's keys that are no longer wanted; keys that stay keep their place and comments.
    let stale: Vec<String> = t
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| (HW_KEYS.contains(&k.as_str()) || k.starts_with("rx_gain_") || k.starts_with("tx_gain_")) && !want.iter().any(|(w, _)| w == k))
        .collect();
    for k in stale {
        t.remove(&k);
    }
    for (k, v) in want {
        match t.get_mut(&k) {
            Some(old) if old.is_value() => {
                // Keep the old value's decoration (inline comment) and number style.
                let decor = old.as_value().map(|o| o.decor().clone());
                *old = v;
                if let (Some(d), Some(nv)) = (decor, old.as_value_mut()) {
                    *nv.decor_mut() = d;
                }
            }
            _ => {
                t.insert(&k, v);
            }
        }
    }
}

/// Apply the page's request to the config text: SDR hardware, which cells run on which SDR,
/// their carriers, colour codes and location areas. Cells left out are removed, cells without an
/// id are added. Comments and every key the page does not own are kept; the result is validated.
pub fn apply_sdrs(original: &str, req: &JsonValue) -> Result<String, String> {
    let cfg = parsing::from_toml_str(original).map_err(|e| format!("current config does not parse: {e}"))?;
    let plans = plan(&cfg, req)?;
    let mut doc: DocumentMut = original.parse().map_err(|e| format!("config does not parse: {e}"))?;
    let primary_secondary = cfg.cell.secondary_carrier.is_some();

    // Cells kept on some SDR; enabled entries not listed are removed (disabled ones stay).
    let kept: Vec<u8> = plans.iter().flat_map(|p| p.cells.iter().filter_map(|c| c.id)).collect();
    let enabled_ids: Vec<u8> = cfg.extra_cells.iter().map(|c| c.id.0).collect();
    if let Some(arr) = doc.get_mut("cells").and_then(|c| c.as_array_of_tables_mut()) {
        arr.retain(|t| {
            let id = t.get("id").and_then(|v| v.as_integer()).and_then(|i| u8::try_from(i).ok());
            id.is_none_or(|id| !enabled_ids.contains(&id) || kept.contains(&id))
        });
    }
    let mut used: Vec<u8> = doc
        .get("cells")
        .and_then(|c| c.as_array_of_tables())
        .map(|a| a.iter().filter_map(|t| t.get("id").and_then(|v| v.as_integer()).and_then(|i| u8::try_from(i).ok())).collect())
        .unwrap_or_default();

    for p in &plans {
        // Channels of the SDR's first existing cell carry over to new cells on it.
        let channels: Vec<(String, Item)> = p
            .cells
            .iter()
            .find_map(|c| c.id)
            .and_then(|id| soapy_table(&doc, id))
            .map(|t| t.iter().filter(|(k, _)| *k == "rx_channel" || *k == "tx_channel").map(|(k, v)| (k.to_string(), v.clone())).collect())
            .unwrap_or_default();
        for (i, c) in p.cells.iter().enumerate() {
            let (dl, ul) = p.carriers.iter().find(|(ci, car, _, _)| *ci == i && *car == c.main_carrier).map(|(_, _, d, u)| (*d, *u)).expect("planned");
            if c.id == Some(0) {
                let ci = doc.entry("cell_info").or_insert(Item::Table(Table::new())).as_table_mut().ok_or("[cell_info] is not a table")?;
                ci["main_carrier"] = value(c.main_carrier as i64);
                match c.secondary_carrier {
                    Some(s) => {
                        ci["secondary_carrier"] = value(s as i64);
                        ci["dual_carrier_enabled"] = value(true);
                    }
                    None if ci.contains_key("secondary_carrier") => ci["dual_carrier_enabled"] = value(false),
                    None => {
                        ci.remove("dual_carrier_enabled");
                    }
                }
                if let Some(cc) = c.colour_code {
                    ci["colour_code"] = value(cc as i64);
                }
                if let Some(la) = c.location_area {
                    ci["location_area"] = value(la as i64);
                }
                if let Some(net) = c.network.as_deref().filter(|n| !n.is_empty() && cfg.networks_explicit) {
                    doc["network"] = value(net);
                }
                if let Some(sec) = &c.security {
                    crate::net_dashboard::cells::write_primary_security(&mut doc, sec)?;
                }
                let phy = doc.entry("phy_io").or_insert(Item::Table(Table::new())).as_table_mut().ok_or("[phy_io] is not a table")?;
                let soapy = phy.entry("soapysdr").or_insert(Item::Table(Table::new())).as_table_mut().ok_or("[phy_io.soapysdr] is not a table")?;
                write_soapy(soapy, p, dl, ul);
                continue;
            }
            let arr = doc
                .entry("cells")
                .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
                .as_array_of_tables_mut()
                .ok_or("cells is not an array of tables")?;
            let entry = match c.id {
                Some(id) => arr.iter_mut().find(|t| t.get("id").and_then(|v| v.as_integer()) == Some(id as i64)).ok_or_else(|| format!("no [[cells]] entry with id {id}"))?,
                None => {
                    let id = (1..=CellId::MAX).find(|i| !used.contains(i)).ok_or_else(|| format!("at most {} cells", CellId::MAX as usize + 1))?;
                    used.push(id);
                    let mut t = Table::new();
                    t["id"] = value(id as i64);
                    arr.push(t);
                    arr.iter_mut().last().expect("just pushed")
                }
            };
            let ci = entry.entry("cell_info").or_insert(Item::Table(Table::new())).as_table_mut().ok_or("cell_info is not a table")?;
            ci["main_carrier"] = value(c.main_carrier as i64);
            // A cell inherits the primary's [cell_info], its second carrier included: say so explicitly.
            match c.secondary_carrier {
                Some(s) => {
                    ci["secondary_carrier"] = value(s as i64);
                    ci["dual_carrier_enabled"] = value(true);
                }
                None => {
                    ci.remove("secondary_carrier");
                    if primary_secondary {
                        ci["dual_carrier_enabled"] = value(false);
                    } else {
                        ci.remove("dual_carrier_enabled");
                    }
                }
            }
            if let Some(cc) = c.colour_code {
                ci["colour_code"] = value(cc as i64);
            }
            match c.location_area {
                Some(la) => ci["location_area"] = value(la as i64),
                None => {
                    ci.remove("location_area");
                }
            }
            match c.network.as_deref() {
                Some("") => {
                    entry.remove("network");
                }
                Some(net) => entry["network"] = value(net),
                None => {}
            }
            if let Some(sec) = &c.security {
                crate::net_dashboard::cells::write_cell_security(entry, sec)?;
            }
            let new_cell = !entry.contains_key("soapysdr");
            let soapy = entry.entry("soapysdr").or_insert(Item::Table(Table::new())).as_table_mut().ok_or("soapysdr is not a table")?;
            if new_cell {
                for (k, v) in &channels {
                    soapy.insert(k, v.clone());
                }
            }
            write_soapy(soapy, p, dl, ul);
        }
    }

    let text = doc.to_string();
    validated(&text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "SoapySdr"

[phy_io.soapysdr]
# keep me
device = "driver=plutosdr"
tx_freq = 438200000
rx_freq = 430600000
sample_rate = 1000000
rx_gain_PGA = 30

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1528
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 2
colour_code = 1
"#;

    fn pluto(mode: &str, cells: JsonValue) -> JsonValue {
        json!({"sdrs": [{"device": "driver=plutosdr", "mode": mode, "sample_rate": 1000000, "rx_gains": {"PGA": 30}, "cells": cells}]})
    }

    #[test]
    fn reads_the_classic_single_cell_setup() {
        let v = sdrs_json(BASE).unwrap();
        let s = &v["sdrs"][0];
        assert_eq!((s["device"].clone(), s["mode"].clone()), (json!("driver=plutosdr"), json!("single")));
        assert_eq!(s["rx_gains"]["PGA"], 30.0, "gain names keep their case");
        assert_eq!(s["cells"][0]["main_carrier"], 1528);
    }

    #[test]
    fn missing_sample_rate_is_the_drivers_default() {
        let base = BASE.replace("sample_rate = 1000000\n", "");
        let v = sdrs_json(&base).unwrap();
        assert_eq!((v["sdrs"][0]["sample_rate"].clone(), v["sdrs"][0]["sample_rate_default"].clone()), (json!(1000000.0), json!(true)));
        // Multi-carrier without a typed sample rate uses the Pluto's 1 MHz.
        let req = json!({"sdrs": [{"device": "driver=plutosdr", "mode": "multi_carrier", "cells": [{"id": 0, "main_carrier": 1528, "secondary_carrier": 1530}]}]});
        let t = apply_sdrs(&base, &req).unwrap();
        assert_eq!(parsing::from_toml_str(&t).unwrap().phy_io.soapysdr.unwrap().fs, Some(1_000_000.0));
    }

    #[test]
    fn multi_carrier_sets_the_second_carrier_and_the_centre() {
        let req = pluto("multi_carrier", json!([{"id": 0, "main_carrier": 1528, "secondary_carrier": 1530}]));
        let t = apply_sdrs(BASE, &req).unwrap();
        assert!(t.contains("# keep me"));
        let cfg = parsing::from_toml_str(&t).unwrap();
        assert_eq!(cfg.cell.secondary_carrier, Some(1530));
        let s = cfg.phy_io.soapysdr.as_ref().unwrap();
        let (dl0, _) = carrier_freqs(&cfg.cell, 1528).unwrap();
        let (dl1, _) = carrier_freqs(&cfg.cell, 1530).unwrap();
        assert_eq!(s.tx_center_freq, Some(((dl0 + dl1) / 2) as f64));
        assert_eq!(sdrs_json(&t).unwrap()["sdrs"][0]["mode"], "multi_carrier");
        // Back to one carrier: the stack sees no second carrier any more.
        let back = apply_sdrs(&t, &pluto("single", json!([{"id": 0, "main_carrier": 1528}]))).unwrap();
        assert_eq!(parsing::from_toml_str(&back).unwrap().cell.secondary_carrier, None);
    }

    #[test]
    fn multi_cell_adds_a_cell_on_the_same_sdr_with_the_same_hardware() {
        let req = pluto("multi_cell", json!([{"id": 0, "main_carrier": 1528}, {"main_carrier": 1530, "colour_code": 2, "location_area": 3}]));
        let t = apply_sdrs(BASE, &req).unwrap();
        let cfg = parsing::from_toml_str(&t).unwrap();
        assert_eq!(cfg.extra_cells.len(), 1);
        let c = &cfg.extra_cells[0];
        assert_eq!((c.id, c.cell.main_carrier, c.cell.colour_code, c.cell.location_area), (CellId(1), 1530, 2, 3));
        let (p, e) = (cfg.phy_io.soapysdr.as_ref().unwrap(), c.soapysdr.as_ref().unwrap());
        assert_eq!(p.device, e.device);
        assert_eq!((p.fs, p.tx_center_freq, p.rx_center_freq), (e.fs, e.tx_center_freq, e.rx_center_freq));
        assert_eq!(e.rx_gains.get("pga"), Some(&30.0));
        assert!(t.contains("rx_gain_PGA = 30\n"), "whole dB stay integers");
        let v = sdrs_json(&t).unwrap();
        assert_eq!(v["sdrs"].as_array().unwrap().len(), 1);
        assert_eq!(v["sdrs"][0]["mode"], "multi_cell");
        // Removing the cell from the SDR removes its [[cells]] entry.
        let back = apply_sdrs(&t, &pluto("single", json!([{"id": 0, "main_carrier": 1528}]))).unwrap();
        assert!(parsing::from_toml_str(&back).unwrap().extra_cells.is_empty());
    }

    #[test]
    fn sdrs_in_different_modes_combine() {
        let req = json!({"sdrs": [
            {"device": "driver=plutosdr", "mode": "multi_cell", "sample_rate": 1000000,
             "cells": [{"id": 0, "main_carrier": 1528}, {"main_carrier": 1530, "colour_code": 2}]},
            {"device": "driver=sx1255", "mode": "multi_carrier", "sample_rate": 1000000,
             "cells": [{"main_carrier": 1540, "secondary_carrier": 1542, "colour_code": 3}]}
        ]});
        let t = apply_sdrs(BASE, &req).unwrap();
        let cfg = parsing::from_toml_str(&t).unwrap();
        assert_eq!(cfg.extra_cells.len(), 2);
        let sx = cfg.extra_cells.iter().find(|c| c.cell.main_carrier == 1540).unwrap();
        assert_eq!(sx.cell.secondary_carrier, Some(1542));
        assert_eq!(sx.soapysdr.as_ref().unwrap().device.as_deref(), Some("driver=sx1255"));
        let modes: Vec<String> = sdrs_json(&t).unwrap()["sdrs"].as_array().unwrap().iter().map(|s| s["mode"].as_str().unwrap().to_string()).collect();
        assert_eq!(modes, vec!["multi_cell", "multi_carrier"]);
    }

    #[test]
    fn refuses_what_does_not_fit() {
        // Carriers too far apart for the sample rate.
        let far = pluto("multi_cell", json!([{"id": 0, "main_carrier": 1528}, {"main_carrier": 1600}]));
        assert!(apply_sdrs(BASE, &far).unwrap_err().contains("sample rate"));
        // Mode and cell count must agree.
        assert!(apply_sdrs(BASE, &pluto("multi_cell", json!([{"id": 0, "main_carrier": 1528}]))).is_err());
        assert!(apply_sdrs(BASE, &pluto("single", json!([{"id": 0, "main_carrier": 1528, "secondary_carrier": 1530}]))).is_err());
        // The primary must stay, carriers stay unique.
        assert!(apply_sdrs(BASE, &pluto("single", json!([{"main_carrier": 1528}]))).unwrap_err().contains("primary"));
        assert!(apply_sdrs(BASE, &pluto("multi_cell", json!([{"id": 0, "main_carrier": 1528}, {"main_carrier": 1528}]))).is_err());
        // Check gives the reason without writing.
        let v = check_json(BASE, &far);
        assert_eq!(v["ok"], false);
        let ok = check_json(BASE, &pluto("multi_carrier", json!([{"id": 0, "main_carrier": 1528, "secondary_carrier": 1530}])));
        assert_eq!(ok["ok"], true, "{ok}");
        assert!(ok["sdrs"][0]["tx_center_freq"].is_number());
    }
    #[test]
    fn primary_cell_security_replaces_only_the_cell_keys() {
        let base = format!("{BASE}\n[security]\nmax_registered_clients = 5\n[security.aie]\nclass = 2\nksg = 1\nsckn = 1\nsck = \"aabbccddeeff00112233\"\n");
        let v = sdrs_json(&base).unwrap();
        let c0 = &v["sdrs"][0]["cells"][0];
        assert_eq!((c0["primary"].clone(), c0["security"]["class"].clone(), c0["security"]["has_sck"].clone()), (json!(true), json!(2), json!(true)));
        assert!(!v.to_string().contains("aabbccddeeff00112233"), "key material stays out of the API");
        let req = pluto("single", json!([{"id": 0, "main_carrier": 1528,
            "security": {"aie": {"class": 2, "ksg": 2, "sckn": 3, "sck": ""}, "issi_whitelist": [1001], "whitelist_mode": "enforce"}}]));
        let t = apply_sdrs(&base, &req).unwrap();
        let cfg = parsing::from_toml_str(&t).unwrap();
        let aie = cfg.security.aie.as_ref().unwrap();
        assert_eq!((aie.ksg, aie.sckn), (2, 3));
        assert!(t.contains("aabbccddeeff00112233"), "an empty key keeps the one in the file");
        assert!(t.contains("max_registered_clients = 5"), "station-wide keys stay");
        assert_eq!(cfg.security.issi_whitelist, vec![1001]);
    }

    #[test]
    fn additional_cell_gets_its_network_and_own_security() {
        let base = format!("network = \"home\"\n{}\n[networks.home]\n[networks.club]\nmcc = 901\nmnc = 1\n", BASE);
        // club has no cell yet: kept, listed as unused.
        let v = sdrs_json(&base).unwrap();
        assert_eq!(v["networks"].as_array().unwrap().iter().find(|n| n["name"] == "club").unwrap()["cell_count"], 0);
        let req = pluto("multi_cell", json!([
            {"id": 0, "main_carrier": 1528},
            {"main_carrier": 1530, "colour_code": 2, "network": "club", "security": {"aie": {"class": 2, "ksg": 1, "sckn": 2, "sck": "00112233445566778899"}}},
        ]));
        let t = apply_sdrs(&base, &req).unwrap();
        let cfg = parsing::from_toml_str(&t).unwrap();
        let cell = &cfg.extra_cells[0];
        assert_eq!(cell.network.as_deref(), Some("club"));
        assert_eq!(cfg.for_extra_cell(cell.id).unwrap().security.aie.as_ref().unwrap().sckn, 2);
        let v = sdrs_json(&t).unwrap();
        let c1 = &v["sdrs"][0]["cells"][1];
        assert_eq!((c1["network"].clone(), c1["security_own"].clone(), c1["security"]["class"].clone()), (json!("club"), json!(true), json!(2)));
        // Back to the primary's network and the station's security.
        let id = c1["id"].as_u64().unwrap();
        let req = pluto("multi_cell", json!([{"id": 0, "main_carrier": 1528}, {"id": id, "main_carrier": 1530, "network": "", "security": {"inherit": true}}]));
        let t = apply_sdrs(&t, &req).unwrap();
        let cfg = parsing::from_toml_str(&t).unwrap();
        assert!(cfg.extra_cells[0].network.is_none() && cfg.extra_cells[0].security.is_none());
    }

}
