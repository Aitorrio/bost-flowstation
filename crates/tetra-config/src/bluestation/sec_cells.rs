//! Additional cells (`[[cells]]`) for multi-cell operation: one extra SDR per entry.
//!
//! The primary cell (id 0) is still described by the legacy `[cell_info]` + `[phy_io.soapysdr]`
//! sections, so single-cell configs are unchanged. Each `[[cells]]` entry adds one more cell:
//!
//! ```toml
//! [[cells]]
//! id = 1
//! enabled = true            # optional, default true
//!
//! [cells.cell_info]         # overrides merged on top of the primary [cell_info]
//! main_carrier = 1590
//! colour_code = 2
//!
//! [cells.soapysdr]          # full SDR section for this cell (same keys as [phy_io.soapysdr])
//! device = "driver=plutosdr,uri=ip:192.168.2.2"
//! tx_freq = 438150000
//! rx_freq = 433150000
//! ```

use serde::Deserialize;
use tetra_core::CellId;
use toml::Value;

use crate::bluestation::{CfgSecurity, CfgSecurityDto, apply_security_patch, CellInfoDto, CfgCellInfo, CfgSoapySdr, SoapySdrDto, cell_dto_to_cfg, soapy_dto_to_cfg, soapy_dto_unknown_keys};

/// `[cell_info]` keys that may not be overridden per cell: SDS remote control is a station-wide
/// function owned by the primary cell.
const NON_OVERRIDABLE_CELL_KEYS: &[&str] = &["sds_command_control"];

/// `[security]` keys that may not be overridden per cell: the key database, E2EE
/// and the registry / rate limits belong to the station, not to a cell.
const NON_OVERRIDABLE_SECURITY_KEYS: &[&str] =
    &["subscribers", "e2ee", "max_registered_clients", "registration_rate_limit_per_min", "honour_unauthenticated_detach"];

/// One additional cell (SDR) of a multi-cell station.
#[derive(Debug, Clone)]
pub struct CfgExtraCell {
    pub id: CellId,
    /// Effective cell parameters: primary `[cell_info]` with this cell's overrides applied.
    pub cell: CfgCellInfo,
    /// SDR settings for this cell. Required when the PHY backend is SoapySdr.
    pub soapysdr: Option<CfgSoapySdr>,
    /// The `[networks]` entry this cell serves; `None` = the primary cell's network.
    pub network: Option<String>,
    /// This cell's own `[cells.security]` (air-interface encryption, whitelist, authentication
    /// posture) merged over the station's `[security]`; `None` = the station's settings.
    pub security: Option<CfgSecurity>,
}

#[derive(Deserialize)]
struct ExtraCellDto {
    id: u8,
    enabled: Option<bool>,
    network: Option<String>,
    security: Option<toml::Table>,
    cell_info: Option<toml::Table>,
    soapysdr: Option<SoapySdrDto>,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, Value>,
}

/// Parse the raw `cells` array. `primary_cell_info` is the primary `[cell_info]` table after the
/// separately-parsed sub-tables were removed; each cell's overrides are merged on top of it.
/// Disabled entries are dropped (their settings stay in the TOML for when they are re-enabled).
pub fn parse_extra_cells(raw_cells: Option<Value>, primary_cell_info: &toml::Table, primary_security: &toml::Table) -> Result<Vec<CfgExtraCell>, String> {
    let Some(raw_cells) = raw_cells else {
        return Ok(Vec::new());
    };
    let Value::Array(entries) = raw_cells else {
        return Err("cells must be an array of tables ([[cells]])".into());
    };

    let mut cells = Vec::with_capacity(entries.len());
    for (i, entry) in entries.into_iter().enumerate() {
        let dto: ExtraCellDto = entry.try_into().map_err(|e| format!("cells[{i}]: {e}"))?;
        if !dto.extra.is_empty() {
            let mut keys: Vec<&str> = dto.extra.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            return Err(format!("Unrecognized fields in cells[{i}]: {keys:?}"));
        }
        if !dto.enabled.unwrap_or(true) {
            continue;
        }

        let mut merged = primary_cell_info.clone();
        let mut overrides = dto.cell_info.unwrap_or_default();
        // A cell's own neighbour list (`[[cells.cell_info.neighbor_cells_ca]]`); without one it
        // advertises only its sibling cells (added by `add_sibling_neighbours`).
        let neighbours = match overrides.remove("neighbor_cells_ca") {
            Some(v) => crate::bluestation::parsing::parse_neighbour_list(v, &format!("cells[{i}].cell_info.neighbor_cells_ca"))?,
            None => Vec::new(),
        };
        for (key, value) in overrides {
            if NON_OVERRIDABLE_CELL_KEYS.contains(&key.as_str()) {
                return Err(format!("cells[{i}].cell_info.{key} cannot be set per cell"));
            }
            merged.insert(key, value);
        }
        let cell_dto: CellInfoDto = Value::Table(merged).try_into().map_err(|e| format!("cells[{i}].cell_info: {e}"))?;
        if !cell_dto.extra.is_empty() {
            let mut keys: Vec<&str> = cell_dto.extra.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            return Err(format!("Unrecognized fields in cells[{i}].cell_info: {keys:?}"));
        }

        let soapysdr = match dto.soapysdr {
            Some(soapy) => {
                let unknown = soapy_dto_unknown_keys(&soapy);
                if !unknown.is_empty() {
                    return Err(format!("Unrecognized fields: cells[{i}].soapysdr::{unknown:?}"));
                }
                Some(soapy_dto_to_cfg(soapy))
            }
            None => None,
        };

        let security = match dto.security {
            Some(over) => Some(parse_cell_security(over, primary_security).map_err(|e| format!("cells[{i}].security: {e}"))?),
            None => None,
        };

        let mut cell = cell_dto_to_cfg(cell_dto);
        cell.neighbor_cells_ca = neighbours;
        cells.push(CfgExtraCell {
            id: CellId(dto.id),
            cell,
            soapysdr,
            network: dto.network,
            security,
        });
    }
    Ok(cells)
}

/// A cell's `[cells.security]` on top of the station's `[security]`. Keys override one by one,
/// except `aie`, which replaces the station's whole `[security.aie]` (class, keys and clear
/// groups only make sense together: a class 2 cell next to a class 3 station must not inherit
/// the CCK). `aie = { class = 1 }` makes the cell clear.
fn parse_cell_security(over: toml::Table, primary: &toml::Table) -> Result<CfgSecurity, String> {
    let mut merged = primary.clone();
    for (key, value) in over {
        if NON_OVERRIDABLE_SECURITY_KEYS.contains(&key.as_str()) {
            return Err(format!("{key} cannot be set per cell"));
        }
        merged.insert(key, value);
    }
    let dto: CfgSecurityDto = Value::Table(merged).try_into().map_err(|e| e.to_string())?;
    apply_security_patch(dto)
}

#[cfg(test)]
mod tests {
    use crate::bluestation::parsing::from_toml_str;
    use tetra_core::CellId;

    const BASE: &str = r#"
config_version = "0.6"
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
colour_code = 1
"#;

    fn parse(extra: &str) -> Result<crate::bluestation::StackConfig, String> {
        from_toml_str(&format!("{BASE}{extra}")).map_err(|e| e.to_string())
    }

    #[test]
    fn single_cell_config_has_no_extra_cells() {
        let cfg = parse("").unwrap();
        assert!(cfg.extra_cells.is_empty());
        assert_eq!(cfg.cell_count(), 1);
        cfg.validate().unwrap();
    }

    #[test]
    fn extra_cell_inherits_and_overrides_primary() {
        let cfg = parse(
            r#"
[[cells]]
id = 1
[cells.cell_info]
main_carrier = 1525
colour_code = 2
"#,
        )
        .unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.cell_count(), 2);
        let c = &cfg.extra_cells[0];
        assert_eq!(c.id, CellId(1));
        assert_eq!(c.cell.main_carrier, 1525);
        assert_eq!(c.cell.colour_code, 2);
        assert_eq!(c.cell.location_area, 1, "inherited from primary");
        let ids: Vec<_> = cfg.cells().iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ids, vec![CellId(0), CellId(1)]);
    }

    fn soapy(device: &str, fs: Option<f64>, center: f64) -> crate::bluestation::CfgSoapySdr {
        crate::bluestation::CfgSoapySdr {
            ul_freq: center,
            dl_freq: center,
            rx_center_freq: Some(center),
            tx_center_freq: Some(center),
            ppm_err: 0.0,
            device: Some(device.to_string()),
            rx_ant: None,
            tx_ant: None,
            rx_gains: Default::default(),
            tx_gains: Default::default(),
            fs,
            rx_ch: None,
            tx_ch: None,
        }
    }

    #[test]
    fn cells_may_share_one_sdr_when_carriers_fit_the_passband() {
        use crate::bluestation::StackConfig;
        let cfg = parse("[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1522\n").unwrap();
        let cells = cfg.cells();
        let (dl, ul): (Vec<u32>, Vec<u32>) = cells
            .iter()
            .flat_map(|(_, c, _)| StackConfig::cell_phase_mod_carriers(c).unwrap())
            .map(|(_, d, u)| (d, u))
            .unzip();
        let center = |v: &[u32]| (v[0] as f64 + v[1] as f64) / 2.0;
        let (tx_c, rx_c) = (center(&dl), center(&ul));
        let mk = |dev: &str, fs: Option<f64>| {
            let mut s = soapy(dev, fs, tx_c);
            s.rx_center_freq = Some(rx_c);
            s
        };
        let run = |a: &crate::bluestation::CfgSoapySdr, b: &crate::bluestation::CfgSoapySdr| {
            let v = vec![(cells[0].0, cells[0].1, Some(a)), (cells[1].0, cells[1].1, Some(b))];
            StackConfig::validate_shared_sdrs(&v)
        };
        assert!(run(&mk("d", Some(600e3)), &mk("d", Some(600e3))).is_ok());
        assert!(run(&mk("d", Some(600e3)), &mk("e", Some(600e3))).is_ok(), "different devices are independent");
        assert!(run(&mk("d", Some(600e3)), &mk("d", Some(1.0e6))).unwrap_err().contains("differ"));
        assert!(run(&mk("d", None), &mk("d", None)).unwrap_err().contains("sample_rate"));
        assert!(run(&mk("d", Some(20e3)), &mk("d", Some(20e3))).unwrap_err().contains("do not cover"));
    }

    const KEY_A: &str = "00112233445566778899";
    const KEY_B: &str = "aabbccddeeff00112233";

    /// A two-network station: primary on "a" (901/9999), cell 1 on "b" (901/1).
    fn two_networks(security: &str, cell_extra: &str) -> Result<crate::bluestation::StackConfig, String> {
        let body = BASE.replace("config_version = \"0.6\"\n", "config_version = \"0.6\"\nnetwork = \"a\"\n");
        from_toml_str(&format!(
            "{body}\n[networks.a]\n[networks.b]\nmcc = 901\nmnc = 1\n{security}\n[[cells]]\nid = 1\nnetwork = \"b\"\n[cells.cell_info]\nmain_carrier = 1525\n{cell_extra}"
        ))
        .map_err(|e| e.to_string())
    }

    #[test]
    fn a_cell_can_have_its_own_security() {
        // Station is class 2; cell 1 (other network) is clear, then class 2 with its own key.
        let sec = format!("[security.aie]\nclass = 2\nsck = \"{KEY_A}\"\nsckn = 1\n");
        let cfg = two_networks(&sec, "[cells.security.aie]\nclass = 1\n").unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.security.aie.as_ref().unwrap().security_class(), 2);
        assert!(cfg.for_extra_cell(CellId(1)).unwrap().security.aie.is_none(), "cell 1 is clear");

        let own = format!("[cells.security.aie]\nclass = 2\nsck = \"{KEY_B}\"\nsckn = 2\nksg = 2\n");
        let cfg = two_networks(&sec, &own).unwrap();
        cfg.validate().unwrap();
        let aie1 = cfg.for_extra_cell(CellId(1)).unwrap().security.aie.clone().unwrap();
        assert_eq!((aie1.sckn, aie1.ksg), (2, 2));
        assert_eq!(cfg.for_extra_cell(CellId(0)).map(|_| ()), None, "primary is not an extra cell");
        assert_eq!(cfg.security.aie.as_ref().unwrap().sckn, 1, "primary keeps its own");

        // Whitelist per cell; everything else inherited.
        let cfg = two_networks(&sec, "[cells.security]\nissi_whitelist = [1001]\nwhitelist_mode = \"enforce\"\n").unwrap();
        let c1 = cfg.for_extra_cell(CellId(1)).unwrap();
        assert_eq!(c1.security.issi_whitelist, vec![1001]);
        assert_eq!(c1.security.aie.as_ref().unwrap().sckn, 1, "aie inherited");
        assert!(cfg.security.issi_whitelist.is_empty());
    }

    #[test]
    fn cells_of_one_network_must_share_the_security_class() {
        let sec = format!("[security.aie]\nclass = 2\nsck = \"{KEY_A}\"\n");
        // Cell 1 on the primary's network but clear: refused.
        let body = format!("{BASE}\n{sec}\n[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\n[cells.security.aie]\nclass = 1\n");
        let cfg = from_toml_str(&body).unwrap();
        assert!(cfg.validate().unwrap_err().contains("security class"), "same network, different class");
        // Same class, different key, same network: fine.
        let body = format!("{BASE}\n{sec}\n[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\n[cells.security.aie]\nclass = 2\nsck = \"{KEY_B}\"\n");
        from_toml_str(&body).unwrap().validate().unwrap();
    }

    #[test]
    fn station_wide_security_keys_cannot_be_set_per_cell() {
        for key in ["max_registered_clients = 5", "honour_unauthenticated_detach = false"] {
            let err = parse(&format!("[[cells]]\nid = 1\n[cells.security]\n{key}\n")).unwrap_err();
            assert!(err.contains("cannot be set per cell"), "{key}: {err}");
        }
        assert!(parse("[[cells]]\nid = 1\n[cells.security]\nauthentication = \"bogus\"\n").unwrap_err().contains("cells[0].security"));
    }

    #[test]
    fn cells_without_network_link_are_not_site_linked() {
        let cfg = parse("[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\n").unwrap();
        assert!(!cfg.is_site_linked());
        assert!(!cfg.for_extra_cell(CellId(1)).unwrap().is_site_linked());
        assert!(!parse("").unwrap().is_site_linked(), "single cell is never site-linked");
    }

    #[test]
    fn disabled_cell_is_dropped() {
        let cfg = parse("[[cells]]\nid = 1\nenabled = false\n[cells.cell_info]\nmain_carrier = 1525\n").unwrap();
        assert!(cfg.extra_cells.is_empty());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse("[[cells]]\nid = 1\nbogus = 1\n").unwrap_err().contains("bogus"));
        assert!(
            parse("[[cells]]\nid = 1\n[cells.cell_info]\nbogus = 1\n")
                .unwrap_err()
                .contains("bogus")
        );
    }

    #[test]
    fn rejects_non_overridable_keys() {
        let err = parse("[[cells]]\nid = 1\n[cells.cell_info]\nsds_command_control = {}\n").unwrap_err();
        assert!(err.contains("cannot be set per cell"), "{err}");
    }

    #[test]
    fn validate_rejects_bad_cell_sets() {
        let dup_carrier = parse("[[cells]]\nid = 1\n").unwrap();
        assert!(dup_carrier.validate().unwrap_err().contains("carrier 1521"));

        let id_zero = parse("[[cells]]\nid = 0\n[cells.cell_info]\nmain_carrier = 1525\n").unwrap();
        assert!(id_zero.validate().unwrap_err().contains("reserved"));

        let dup_id =
            parse("[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\n[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1530\n")
                .unwrap();
        assert!(dup_id.validate().unwrap_err().contains("duplicate id"));

        let out_of_range = parse("[[cells]]\nid = 8\n[cells.cell_info]\nmain_carrier = 1525\n").unwrap();
        assert!(out_of_range.validate().unwrap_err().contains("out of range"));

        let band = parse("[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\nfreq_band = 3\n").unwrap();
        assert!(band.validate().unwrap_err().contains("freq_band"));
    }

    /// SC3 and several cells are either/or: the DCK lives only in the cell that authenticated the
    /// radio. SC3 on one cell and SC2 on several stay valid.
    #[test]
    fn validate_refuses_security_class_3_with_extra_cells() {
        const SC3: &str = "[security]\nauthentication = \"required\"\n[[security.subscribers]]\nissi = 100\nk = \"11111111111111111111111111111111\"\n[security.aie]\nclass = 3\nksg = 2\ncck = \"22222222222222222222\"\ncck_id = 42\n";
        const CELL: &str = "[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\n";
        let single = parse(SC3).unwrap();
        assert!(single.security_class3());
        single.validate().unwrap();

        let multi = parse(&format!("{SC3}{CELL}")).unwrap();
        assert_eq!(multi.validate().unwrap_err(), crate::bluestation::SC3_SINGLE_CELL);

        const SC2: &str = "[security.aie]\nclass = 2\nksg = 2\nsck = \"22222222222222222222\"\n";
        // Class 3 on the primary with the cell on another network (cells never share radios): fine.
        let two_nets = two_networks(SC3, "").unwrap();
        two_nets.validate().unwrap();
        let sc2_multi = parse(&format!("{SC2}{CELL}")).unwrap();
        assert!(!sc2_multi.security_class3());
        sc2_multi.validate().unwrap();
    }

    #[test]
    fn for_extra_cell_isolates_radio_settings() {
        let cfg = parse(
            r#"
[brew]
host = "brew.example"
port = 3000
tls = false
username = 1
password = "x"

[[cells]]
id = 1
[cells.cell_info]
main_carrier = 1525
colour_code = 2
"#,
        )
        .unwrap();
        assert!(cfg.brew.is_some());

        let cell = cfg.for_extra_cell(CellId(1)).unwrap();
        assert_eq!(cell.cell.main_carrier, 1525);
        assert_eq!(cell.cell.colour_code, 2);
        assert!(cell.extra_cells.is_empty());
        assert!(cell.brew.is_some(), "kept so the cell's CMCE routes to the site switch");
        assert!(cfg.is_site_linked() && cell.is_site_linked(), "Brew + [[cells]] links the cells");
        assert!(cell.dashboard.is_none());
        assert!(!cell.recovery.enabled);
        assert_eq!(cell.cell_id, CellId(1));
        assert!(cell.is_multi_cell() && cfg.is_multi_cell());
        assert_eq!(cell.net.mcc, cfg.net.mcc);
        cell.validate().unwrap();
        let primary = cfg.cell.main_carrier;
        assert!(cfg.is_station_carrier(primary) && cfg.is_station_carrier(1525), "the primary knows its siblings");
        assert!(cell.is_station_carrier(primary) && cell.is_station_carrier(1525), "an extra cell knows its siblings");
        assert!(!cell.is_station_carrier(1600) && !cfg.is_station_carrier(1600), "another station's carrier");

        assert!(cfg.for_extra_cell(CellId(2)).is_none());
    }

    #[test]
    fn sibling_cells_become_neighbours() {
        let mut cfg = parse(
            r#"
[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 0
cell_reselection_types_supported = 1
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1525

[[cells]]
id = 1
[cells.cell_info]
main_carrier = 1525
[[cells]]
id = 2
[cells.cell_info]
main_carrier = 1529
location_area = 2
"#,
        )
        .unwrap();
        cfg.add_sibling_neighbours();
        cfg.validate().unwrap();

        let carriers = |c: &crate::bluestation::CfgCellInfo| c.neighbor_cells_ca.iter().map(|n| n.main_carrier_number).collect::<Vec<_>>();
        assert_eq!(carriers(&cfg.cell), vec![1525, 1529], "configured 1525 kept, 1529 added");
        assert_eq!(cfg.cell.neighbor_cells_ca[1].cell_identifier_ca, 1, "first free id");
        assert_eq!(cfg.cell.neighbor_cells_ca[1].location_area, Some(2), "differing LA is advertised");
        assert_eq!(carriers(&cfg.extra_cells[0].cell), vec![1521, 1529]);
        assert_eq!(carriers(&cfg.extra_cells[1].cell), vec![1521, 1525]);
        assert!(cfg.cells().iter().all(|(_, c, _)| c.neighbor_cell_broadcast & 0b10 != 0));
        let added = &cfg.cell.neighbor_cells_ca[1];
        assert_eq!(added.cell_reselection_types_supported, 3, "announced and unannounced");
        assert_eq!(added.main_carrier_number_extension, None, "same band plan");
        assert_eq!(added.maximum_ms_transmit_power, None, "same max power");
        assert_eq!(added.minimum_rx_access_level, None, "same RX access level");
        assert_eq!(cfg.cell.neighbor_cells_ca[0].cell_reselection_types_supported, 1, "configured entry untouched");
        assert_eq!(cfg.for_extra_cell(CellId(2)).unwrap().cell.neighbor_cells_ca.len(), 2);
    }

    #[test]
    fn an_extra_cell_may_have_its_own_neighbour_list() {
        let mut cfg = parse(
            r#"
[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 0
cell_reselection_types_supported = 2
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1530

[[cells]]
id = 1
[cells.cell_info]
main_carrier = 1525
[[cells.cell_info.neighbor_cells_ca]]
cell_identifier_ca = 4
cell_reselection_types_supported = 1
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1540
location_area = 7
[cells.cell_info.neighbor_cells_ca.bs_service_details]
registration = true
voice_service = true
"#,
        )
        .unwrap();
        cfg.add_sibling_neighbours();
        cfg.validate().unwrap();
        let carriers = |c: &crate::bluestation::CfgCellInfo| c.neighbor_cells_ca.iter().map(|n| n.main_carrier_number).collect::<Vec<_>>();
        assert_eq!(carriers(&cfg.cell), vec![1530, 1525], "primary: its own list plus the sibling");
        assert_eq!(carriers(&cfg.extra_cells[0].cell), vec![1540, 1521], "cell 1: its own list plus the sibling");
        let own = &cfg.extra_cells[0].cell.neighbor_cells_ca[0];
        assert_eq!((own.cell_identifier_ca, own.location_area), (4, Some(7)));
        assert!(own.bs_service_details.as_ref().is_some_and(|d| d.registration && d.voice_service && !d.migration));
    }

    #[test]
    fn a_bad_neighbour_list_of_an_extra_cell_is_rejected() {
        let entry = |id: u8, carrier: u16| {
            format!("[[cells.cell_info.neighbor_cells_ca]]\ncell_identifier_ca = {id}\ncell_reselection_types_supported = 1\nneighbor_cell_synchronized = false\ncell_load_ca = 0\nmain_carrier_number = {carrier}\n")
        };
        let base = "[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\n";
        let dup = format!("{base}{}{}", entry(1, 1540), entry(1, 1541));
        assert!(parse(&dup).unwrap().validate().unwrap_err().contains("duplicate cell_identifier_ca"));
        let own = format!("{base}{}", entry(1, 1525));
        assert!(parse(&own).unwrap().validate().unwrap_err().contains("own main_carrier"));
    }

    #[test]
    fn single_cell_neighbours_are_untouched() {
        let mut cfg = parse("").unwrap();
        cfg.add_sibling_neighbours();
        assert!(cfg.cell.neighbor_cells_ca.is_empty());
        assert_eq!(cfg.cell.neighbor_cell_broadcast, 0);
    }

    #[test]
    fn sibling_with_another_band_plan_or_power_is_described_to_radios() {
        let mut cfg = parse(
            "[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\nfreq_offset = 12500\nms_txpwr_max_cell = 6\n",
        )
        .unwrap();
        cfg.add_sibling_neighbours();
        let n = &cfg.cell.neighbor_cells_ca[0];
        // band 4, offset +12.5 kHz (id 3), duplex spacing 4, not reversed.
        assert_eq!(n.main_carrier_number_extension, Some((4 << 6) | (3 << 4) | (4 << 1)));
        assert_eq!(n.maximum_ms_transmit_power, Some(6));
        let back = &cfg.extra_cells[0].cell.neighbor_cells_ca[0];
        assert_eq!(back.main_carrier_number, 1521);
        assert_eq!(back.main_carrier_number_extension, Some((4 << 6) | (0 << 4) | (4 << 1)), "primary's plan, offset 0");
        cfg.validate().unwrap();
    }

    #[test]
    fn sibling_with_another_rx_access_level_is_described_to_radios() {
        // Primary -75 dBm (code 10, table 21.65), the extra cell keeps the -110 dBm default
        // (code 3) by overriding the inherited value.
        let mut cfg = parse(
            "rxlev_access_min_dbm = -75\n[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\nrxlev_access_min_dbm = -110\n",
        )
        .unwrap();
        cfg.add_sibling_neighbours();
        cfg.validate().unwrap();
        assert_eq!(cfg.cell.neighbor_cells_ca[0].minimum_rx_access_level, Some(3), "18.5.12");
        assert_eq!(cfg.extra_cells[0].cell.neighbor_cells_ca[0].minimum_rx_access_level, Some(10));
    }

    #[test]
    fn extra_cell_inherits_rx_access_level_and_bad_override_is_rejected() {
        let cfg = parse("rxlev_access_min_dbm = -80\n[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\n").unwrap();
        assert_eq!(cfg.extra_cells[0].cell.rxlev_access_min(), 9, "inherited from [cell_info]");
        let cfg = parse("[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\nrxlev_access_min_dbm = -77\n").unwrap();
        assert!(cfg.validate().unwrap_err().contains("rxlev_access_min_dbm"));
    }

    #[test]
    fn cells_must_share_a_custom_duplex_spacing() {
        let cfg = parse("[[cells]]\nid = 1\n[cells.cell_info]\nmain_carrier = 1525\ncustom_duplex_spacing = 10000000\n").unwrap();
        assert!(cfg.validate().unwrap_err().contains("custom_duplex_spacing"));
    }
}
