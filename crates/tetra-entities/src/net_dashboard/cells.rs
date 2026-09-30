//! Dashboard multi-cell support: per-cell status, and adding/removing `[[cells]]` entries.
//!
//! Adding or removing a cell is a config-file edit applied via a controlled restart (like the
//! Dual-Carrier switch). The prospective config is parsed and validated before it is written, so
//! a bad request never leaves an unbootable config behind.

use serde_json::{Value as JsonValue, json};
use tetra_config::bluestation::{CfgCellInfo, CfgSoapySdr, SharedConfig, StackConfig, parsing};
use tetra_core::CellId;

/// One cell's settings and live state for the dashboard "Cells" card.
fn cell_json(id: CellId, cfg: &SharedConfig, rf: Option<&crate::rf_status::RfStatus>) -> JsonValue {
    let c = cfg.config();
    let carriers: Vec<JsonValue> = StackConfig::cell_phase_mod_carriers(&c.cell)
        .unwrap_or_default()
        .into_iter()
        .map(|(n, dl, ul)| json!({ "carrier_num": n, "tx_freq_hz": dl, "rx_freq_hz": ul }))
        .collect();
    let radios = cfg.state_read().subscribers.all_registered_issis().count();
    json!({
        "id": id.0,
        "primary": id.is_primary(),
        "main_carrier": c.cell.main_carrier,
        "secondary_carrier": c.cell.secondary_carrier,
        "carriers": carriers,
        "colour_code": c.cell.colour_code,
        "location_area": c.cell.location_area,
        "neighbours": c.cell.neighbor_cells_ca.len(),
        "device": c.phy_io.soapysdr.as_ref().and_then(|s| s.device.clone()),
        "registered_radios": radios,
        "rf_state": rf.map(|r| r.state),
        "rf_detail": rf.map(|r| r.detail.clone()),
    })
}

/// GET /api/cells payload: every running cell, primary first.
pub fn cells_json(primary: &SharedConfig) -> JsonValue {
    let rf = crate::rf_status::get_all();
    let rf_of = |id: CellId| rf.iter().find(|(c, _)| *c == id).map(|(_, s)| s);
    let mut cells = vec![cell_json(CellId::PRIMARY, primary, rf_of(CellId::PRIMARY))];
    for (id, cfg) in crate::net_site::extra_cells() {
        cells.push(cell_json(*id, cfg, rf_of(*id)));
    }
    json!({
        "cells": cells,
        "site_linked": primary.config().is_site_linked(),
        "max_cells": CellId::MAX as usize + 1,
    })
}

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
fn carrier_freqs(primary: &CfgCellInfo, main_carrier: u16) -> Result<(u32, u32), String> {
    let mut cell = primary.clone();
    cell.main_carrier = main_carrier;
    cell.secondary_carrier = None;
    let carriers = StackConfig::cell_phase_mod_carriers(&cell)?;
    carriers
        .first()
        .map(|(_, dl, ul)| (*dl, *ul))
        .ok_or_else(|| "carrier has no frequency".to_string())
}

fn validated(toml_text: &str) -> Result<(), String> {
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
pub fn add_cell_toml(original: &str, device: &str, main_carrier: u16, colour_code: Option<u8>) -> Result<(String, u8), String> {
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

#[cfg(test)]
mod tests {
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
    fn add_then_remove_cell_round_trips() {
        let (with_one, id) = add_cell_toml(BASE, "driver=plutosdr,uri=ip:192.168.3.1", 1525, Some(2)).unwrap();
        assert_eq!(id, 1);
        let cfg = parsing::from_toml_str(&with_one).unwrap();
        let cell = &cfg.extra_cells[0];
        assert_eq!(cell.cell.main_carrier, 1525);
        assert_eq!(cell.cell.colour_code, 2);
        let soapy = cell.soapysdr.as_ref().unwrap();
        assert_eq!(soapy.device.as_deref(), Some("driver=plutosdr,uri=ip:192.168.3.1"));
        let (dl, ul) = carrier_freqs(&cfg.cell, 1525).unwrap();
        assert_eq!((soapy.dl_freq as u32, soapy.ul_freq as u32), (dl, ul));

        let (with_two, id2) = add_cell_toml(&with_one, "driver=plutosdr,uri=ip:192.168.4.1", 1529, None).unwrap();
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
        let err = add_cell_toml(BASE, "driver=plutosdr", 1521, None).unwrap_err();
        assert!(err.contains("carrier 1521"), "{err}");
    }

    #[test]
    fn remove_keeps_following_sections_and_rejects_unknown_ids() {
        let (text, _) = add_cell_toml(BASE, "driver=plutosdr", 1525, None).unwrap();
        let text = format!("{text}\n[security]\n# keep me\n");
        let out = remove_cell_toml(&text, 1).unwrap();
        assert!(out.contains("[security]") && out.contains("# keep me"));
        assert!(!out.contains("[[cells]]"));
        assert!(remove_cell_toml(&out, 1).is_err());
        assert!(remove_cell_toml(&out, 0).is_err());
    }
}
