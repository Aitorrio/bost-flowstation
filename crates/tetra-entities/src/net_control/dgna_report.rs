//! `dgna-report`: the groups given to radios by DGNA (SS-DGNA ASSIGN / DEASSIGN, TS 100 392-12-22),
//! reported to the Brew server over the control channel, so the server knows them (also those
//! given on this station's dashboard) and can take them back later.
//!
//! Only to a server that lists `dgna-report` in `X-Brew-Extensions` on the control channel; an
//! older server never sees the messages. MM sends a [`ControlResponse::DgnaChanged`] for every
//! assign and remove that goes into the station's DGNA registry; the control worker sends the
//! whole registry as [`ControlResponse::DgnaGroups`] once after each connect. Nothing changes on
//! the air interface.

use tetra_config::bluestation::SharedConfig;

use super::commands::{ControlResponse, DgnaReportEntry};

/// Name in `X-Brew-Extensions` (control channel) with which a server asks for the reports.
pub const EXT_DGNA_REPORT: &str = "dgna-report";

/// Does this server ask for the DGNA reports (its control channel `X-Brew-Extensions`)?
pub fn offered(server_extensions: &[String]) -> bool {
    server_extensions.iter().any(|e| e == EXT_DGNA_REPORT)
}

/// One assign (`assigned`) or remove, as MM reports it.
pub fn changed(issi: u32, gssi: u32, mnemonic: Option<String>, attachment_mode: u8, assigned: bool) -> ControlResponse {
    ControlResponse::DgnaChanged {
        issi,
        gssi,
        mnemonic: if assigned { mnemonic } else { None },
        attachment_mode: if assigned { attachment_mode } else { 0 },
        assigned,
    }
}

/// Every DGNA group the station remembers.
pub fn snapshot(shared: &SharedConfig) -> ControlResponse {
    let groups = shared
        .state_read()
        .subscribers
        .all_dgna_groups()
        .into_iter()
        .map(|(issi, g)| DgnaReportEntry {
            issi,
            gssi: g.gssi,
            mnemonic: g.mnemonic,
            attachment_mode: g.attachment_mode,
        })
        .collect();
    ControlResponse::DgnaGroups { groups }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net_control::codec::ControlCodecJson;

    #[test]
    fn offered_only_with_the_extension() {
        assert!(!offered(&[]));
        assert!(!offered(&["fleet-control".to_string()]));
        assert!(offered(&["fleet-control".to_string(), EXT_DGNA_REPORT.to_string()]));
    }

    #[test]
    fn wire_shapes_match_brew_server() {
        // Externally tagged, as brew-server's `control.rs` parses it (test `dgna_reports_fill_the_register`).
        let codec = ControlCodecJson;
        let v: serde_json::Value =
            serde_json::from_slice(&codec.encode_response(&changed(2001, 91, Some("Ops".into()), 0, true))).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"DgnaChanged": {"issi": 2001, "gssi": 91, "mnemonic": "Ops", "attachment_mode": 0, "assigned": true}})
        );
        let v: serde_json::Value =
            serde_json::from_slice(&codec.encode_response(&changed(2001, 91, Some("Ops".into()), 3, false))).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"DgnaChanged": {"issi": 2001, "gssi": 91, "mnemonic": null, "attachment_mode": 0, "assigned": false}}),
            "a remove carries no name or mode"
        );
        let groups = ControlResponse::DgnaGroups {
            groups: vec![DgnaReportEntry { issi: 2002, gssi: 95, mnemonic: None, attachment_mode: 1 }],
        };
        let v: serde_json::Value = serde_json::from_slice(&codec.encode_response(&groups)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"DgnaGroups": {"groups": [{"issi": 2002, "gssi": 95, "mnemonic": null, "attachment_mode": 1}]}})
        );
    }

    #[test]
    fn snapshot_lists_the_registry_in_order() {
        let cfg = tetra_config::bluestation::from_toml_str(include_str!("../../../../example_config/config.toml")).unwrap();
        let shared = SharedConfig::from_parts(cfg, None);
        {
            let mut st = shared.state_write();
            st.subscribers.remember_dgna_group(2002, 95, None, 1);
            st.subscribers.remember_dgna_group(2001, 92, Some("B".into()), 0);
            st.subscribers.remember_dgna_group(2001, 91, Some("A".into()), 0);
        }
        let ControlResponse::DgnaGroups { groups } = snapshot(&shared) else { panic!("DgnaGroups expected") };
        let keys: Vec<(u32, u32)> = groups.iter().map(|g| (g.issi, g.gssi)).collect();
        assert_eq!(keys, vec![(2001, 91), (2001, 92), (2002, 95)]);
        assert_eq!(groups[0].mnemonic.as_deref(), Some("A"));
    }
}
