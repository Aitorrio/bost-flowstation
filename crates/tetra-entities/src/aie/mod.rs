//! Air interface encryption, security class 2 (EN 300 392-7 clause 6). Design and phases:
//! `Docs/aie-class2-plan.md`.
//!
//! Phase 1 (this module today): the policy and signalling around AIE, with no cipher. Nothing
//! is encrypted and the cell keeps advertising class 1 until a key stream generator is built in
//! ([`ksg_available`]), so a configured `[security.aie]` is reported and otherwise ignored.

use tetra_config::bluestation::{CfgAie, StackConfig};
use tetra_pdus::mm::enums::reject_cause::RejectCause;
use tetra_pdus::mm::fields::ciphering_parameters::CipheringParameters;
use tetra_pdus::umac::fields::sysinfo_ext_services::SysinfoExtendedServices;

/// True when this build carries the key stream generator for TEA`tea`. None does yet; the TB5 /
/// TEA primitives arrive in phase 2 as a separate, replaceable module.
pub fn ksg_available(_tea: u8) -> bool {
    false
}

/// The AIE settings this cell actually runs with: the configured class 2 settings when the KSG
/// they name is available, else `None` (class 1, clear).
pub fn effective(cfg: &StackConfig) -> Option<&CfgAie> {
    cfg.security.aie.as_ref().filter(|a| ksg_available(a.ksg))
}

/// One startup line describing the AIE posture, so an operator sees why a configured key is not
/// in use yet.
pub fn posture(cfg: &StackConfig) -> String {
    match (&cfg.security.aie, effective(cfg)) {
        (None, _) => "class 1 (clear) — no [security.aie] configured".to_string(),
        (Some(a), Some(_)) => format!(
            "class 2 — TEA{} SCKN {}, clear radios allowed on {} clear group(s)",
            a.ksg,
            a.sckn,
            a.clear_groups.len()
        ),
        (Some(a), None) => format!(
            "class 1 (clear) — [security.aie] asks for class 2 with TEA{}, but this build has no TEA{} key stream generator",
            a.ksg, a.ksg
        ),
    }
}

/// The security part of the broadcast: the BS service details "AIE service" bit (D-MLE-SYSINFO)
/// and the SYSINFO extended services element. Without AIE these are exactly what the cell has
/// always sent.
pub fn sysinfo_security(aie: Option<&CfgAie>) -> (bool, SysinfoExtendedServices) {
    let mut ext = SysinfoExtendedServices {
        auth_required: false,
        class1_supported: true,
        class2_supported: true,
        class3_supported: false,
        sck_n: Some(0),
        dck_retrieval_during_cell_select: None,
        dck_retrieval_during_cell_reselect: None,
        linked_gck_crypto_periods: None,
        short_gck_vn: None,
        sdstl_addressing_method: 2,
        gck_supported: false,
        section: 0,
        section_data: 0,
    };
    let Some(aie) = aie else {
        return (false, ext);
    };
    // Mixed cell: class 1 stays advertised so radios without AIE still register in clear.
    // SCK number field: 0 = SCK 1 … 31 = SCK 32. [verify]
    ext.sck_n = Some(aie.sckn - 1);
    (true, ext)
}

/// Outcome of the ciphering part of a U-LOCATION UPDATE DEMAND.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherDecision {
    /// Register the radio in clear (class 1).
    Clear,
    /// Register the radio encrypted with these parameters.
    Encrypted(CipheringParameters),
    /// Refuse the registration with this cause.
    Reject(RejectCause),
    /// The cell does not run AIE but the radio asked for ciphering: handled as before (dropped).
    Unsupported,
}

/// Decide how a registering radio is ciphered from what it asked for (EN 300 392-7 clause 6.5).
/// `aie` is the cell's effective setting ([`effective`]).
pub fn registration_decision(aie: Option<&CfgAie>, cipher_control: bool, params: Option<u64>) -> CipherDecision {
    if !cipher_control {
        return CipherDecision::Clear;
    }
    let Some(aie) = aie else {
        return CipherDecision::Unsupported;
    };
    let Some(params) = params.map(CipheringParameters::from_bits) else {
        return CipherDecision::Reject(RejectCause::MandatoryElementError);
    };
    if params.class3 {
        return CipherDecision::Reject(RejectCause::RequestedCipherKeyTypeNotAvailable);
    }
    if params.tea() != Some(aie.ksg) {
        return CipherDecision::Reject(RejectCause::IdentifiedCipherKsgNotSupported);
    }
    if params.sckn() != Some(aie.sckn) {
        return CipherDecision::Reject(RejectCause::IdentifiedCipherKeyNotAvailable);
    }
    CipherDecision::Encrypted(params)
}

/// Mixed-cell rule: clear and encrypted radios never talk to each other (individual calls, SDS,
/// status). `None` = the party's mode is unknown (not registered here, e.g. a network party),
/// which this rule does not block.
pub fn may_communicate(a_encrypted: Option<bool>, b_encrypted: Option<bool>) -> bool {
    match (a_encrypted, b_encrypted) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// Mixed-cell rule for groups: an encrypted radio uses only encrypted groups and a clear radio
/// only the configured clear groups. Without AIE every group is open to every radio.
pub fn may_use_group(aie: Option<&CfgAie>, radio_encrypted: bool, gssi: u32) -> bool {
    match aie {
        None => true,
        Some(aie) => aie.is_clear_group(gssi) != radio_encrypted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_config::bluestation::CipherKey;

    fn aie() -> CfgAie {
        CfgAie {
            ksg: 1,
            sckn: 3,
            sck: CipherKey([0; 10]),
            clear_groups: vec![100],
        }
    }

    #[test]
    fn no_aie_broadcast_is_unchanged() {
        let (aie_service, ext) = sysinfo_security(None);
        assert!(!aie_service);
        assert_eq!(ext.sck_n, Some(0));
        let (aie_service, ext) = sysinfo_security(Some(&aie()));
        assert!(aie_service);
        assert_eq!(ext.sck_n, Some(2));
        assert!(ext.class1_supported && ext.class2_supported && !ext.auth_required);
    }

    #[test]
    fn registration_follows_the_configured_key() {
        let a = aie();
        let ok = CipheringParameters::class2(1, 3);
        assert_eq!(registration_decision(Some(&a), false, None), CipherDecision::Clear);
        assert_eq!(registration_decision(None, true, Some(ok.to_bits())), CipherDecision::Unsupported);
        assert_eq!(registration_decision(Some(&a), true, Some(ok.to_bits())), CipherDecision::Encrypted(ok));
        assert_eq!(
            registration_decision(Some(&a), true, Some(CipheringParameters::class2(2, 3).to_bits())),
            CipherDecision::Reject(RejectCause::IdentifiedCipherKsgNotSupported)
        );
        assert_eq!(
            registration_decision(Some(&a), true, Some(CipheringParameters::class2(1, 4).to_bits())),
            CipherDecision::Reject(RejectCause::IdentifiedCipherKeyNotAvailable)
        );
        assert_eq!(
            registration_decision(Some(&a), true, Some(0b0000_1_00000)),
            CipherDecision::Reject(RejectCause::RequestedCipherKeyTypeNotAvailable)
        );
    }

    #[test]
    fn clear_and_encrypted_never_meet() {
        assert!(may_communicate(Some(true), Some(true)));
        assert!(may_communicate(Some(false), Some(false)));
        assert!(!may_communicate(Some(true), Some(false)));
        assert!(may_communicate(Some(true), None));

        let a = aie();
        assert!(may_use_group(Some(&a), false, 100));
        assert!(!may_use_group(Some(&a), true, 100));
        assert!(may_use_group(Some(&a), true, 200));
        assert!(!may_use_group(Some(&a), false, 200));
        assert!(may_use_group(None, false, 200));
    }

    #[test]
    fn configured_aie_stays_off_without_a_ksg() {
        let toml = r#"
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

[security.aie]
class = 2
sck = "0123456789ABCDEF0123"
clear_groups = [100]
"#;
        let cfg = tetra_config::bluestation::parsing::from_toml_str(toml).unwrap();
        assert_eq!(cfg.security.aie.as_ref().map(|a| a.clear_groups.clone()), Some(vec![100]));
        assert!(effective(&cfg).is_none());
        assert!(posture(&cfg).contains("no TEA1"));
    }
}
