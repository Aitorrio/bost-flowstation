//! Driver-aware SoapySDR gain/antenna key checks (dashboard + parse path).
//!
//! Does **not** invent dB min/max (probe-dependent). It only rejects stage names
//! that belong to another SDR family — the failure mode that bricks RF open
//! (`Unsupported TX gains for SXceiver: {"pad": …}`).

use crate::bluestation::CfgSoapySdr;

/// Coarse SDR family inferred from `phy_io.soapysdr.device`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoapyDriverFamily {
    Sxceiver,
    Lime,
    Pluto,
    Usrp,
    /// Unknown / empty device — do not invent stage rules.
    Unknown,
}

impl SoapyDriverFamily {
    pub fn from_device_arg(device: Option<&str>) -> Self {
        let d = device.unwrap_or("").to_ascii_lowercase();
        if d.is_empty() {
            return Self::Unknown;
        }
        if d.contains("sx") || d.contains("sxceiver") {
            return Self::Sxceiver;
        }
        if d.contains("lime") {
            return Self::Lime;
        }
        if d.contains("pluto") {
            return Self::Pluto;
        }
        if d.contains("usrp") || d.contains("b200") || d.contains("b210") || d.contains("uhd") {
            return Self::Usrp;
        }
        Self::Unknown
    }

    pub fn allowed_tx_stages(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Sxceiver => Some(&["dac", "mixer"]),
            Self::Lime => Some(&["pad", "iamp"]),
            Self::Pluto | Self::Usrp => Some(&["pga"]),
            Self::Unknown => None,
        }
    }

    pub fn allowed_rx_stages(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Sxceiver => Some(&["lna", "pga"]),
            Self::Lime => Some(&["lna", "tia", "pga"]),
            Self::Pluto | Self::Usrp => Some(&["pga"]),
            Self::Unknown => None,
        }
    }

    pub fn allowed_rx_antennas(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Sxceiver => Some(&["RX"]),
            Self::Lime => Some(&["LNAL", "LNAH", "LNAW"]),
            Self::Pluto => Some(&["A_BALANCED", "A_N", "A_P"]),
            Self::Usrp => Some(&["TX/RX", "RX2"]),
            Self::Unknown => None,
        }
    }

    pub fn allowed_tx_antennas(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Sxceiver => Some(&["TX"]),
            Self::Lime => Some(&["BAND1", "BAND2"]),
            Self::Pluto => Some(&["A"]),
            Self::Usrp => Some(&["TX/RX"]),
            Self::Unknown => None,
        }
    }
}

fn stage_allowed(allowed: &[&str], name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    allowed.iter().any(|a| a.eq_ignore_ascii_case(&n))
}

fn antenna_allowed(allowed: &[&str], name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() {
        return true;
    }
    allowed.iter().any(|a| a.eq_ignore_ascii_case(n))
}

/// Reject gain/antenna keys that this device family cannot apply.
pub fn validate_soapysdr_driver_keys(cfg: &CfgSoapySdr) -> Result<(), String> {
    let family = SoapyDriverFamily::from_device_arg(cfg.device.as_deref());
    let label = cfg.device.as_deref().unwrap_or("(no device=)");

    if let Some(allowed) = family.allowed_tx_stages() {
        for stage in cfg.tx_gains.keys() {
            if !stage_allowed(allowed, stage) {
                return Err(format!(
                    "TX gain stage '{stage}' is not valid for device '{label}' (allowed: {}). \
                     Clear the field or use Setup to pick the matching SDR.",
                    allowed.join(", ")
                ));
            }
        }
    }
    if let Some(allowed) = family.allowed_rx_stages() {
        for stage in cfg.rx_gains.keys() {
            if !stage_allowed(allowed, stage) {
                return Err(format!(
                    "RX gain stage '{stage}' is not valid for device '{label}' (allowed: {}). \
                     Clear the field or use Setup to pick the matching SDR.",
                    allowed.join(", ")
                ));
            }
        }
    }
    if let Some(allowed) = family.allowed_rx_antennas() {
        if let Some(ant) = cfg.rx_ant.as_deref() {
            if !antenna_allowed(allowed, ant) {
                return Err(format!(
                    "RX antenna '{ant}' is not valid for device '{label}' (allowed: {}).",
                    allowed.join(", ")
                ));
            }
        }
    }
    if let Some(allowed) = family.allowed_tx_antennas() {
        if let Some(ant) = cfg.tx_ant.as_deref() {
            if !antenna_allowed(allowed, ant) {
                return Err(format!(
                    "TX antenna '{ant}' is not valid for device '{label}' (allowed: {}).",
                    allowed.join(", ")
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn soapy(device: &str, tx: &[(&str, f64)], rx: &[(&str, f64)]) -> CfgSoapySdr {
        CfgSoapySdr {
            ul_freq: 433e6,
            dl_freq: 438e6,
            rx_center_freq: None,
            tx_center_freq: None,
            ppm_err: 0.0,
            device: Some(device.into()),
            rx_ant: None,
            tx_ant: None,
            rx_gains: rx.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            tx_gains: tx.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            fs: None,
            rx_ch: None,
            tx_ch: None,
        }
    }

    #[test]
    fn sx_rejects_pad() {
        let cfg = soapy("driver=sx", &[("pad", 24.0)], &[]);
        let err = validate_soapysdr_driver_keys(&cfg).unwrap_err();
        assert!(err.contains("pad"), "{err}");
    }

    #[test]
    fn sx_accepts_dac() {
        let cfg = soapy("driver=sx", &[("dac", 9.0)], &[("lna", 42.0)]);
        validate_soapysdr_driver_keys(&cfg).unwrap();
    }

    #[test]
    fn lime_accepts_pad() {
        let cfg = soapy("driver=lime", &[("pad", 22.0)], &[("tia", 6.0)]);
        validate_soapysdr_driver_keys(&cfg).unwrap();
    }

    #[test]
    fn unknown_device_skips_stage_rules() {
        let cfg = soapy("driver=custom", &[("pad", 1.0)], &[]);
        validate_soapysdr_driver_keys(&cfg).unwrap();
    }

    #[test]
    fn sx_rejects_lime_antenna() {
        let mut cfg = soapy("driver=sx", &[], &[]);
        cfg.rx_ant = Some("LNAL".into());
        let err = validate_soapysdr_driver_keys(&cfg).unwrap_err();
        assert!(err.contains("LNAL"), "{err}");
    }

    #[test]
    fn empty_maps_ok() {
        let cfg = soapy("driver=sx", &[], &[]);
        validate_soapysdr_driver_keys(&cfg).unwrap();
        let _ = HashMap::<String, f64>::new();
    }
}
