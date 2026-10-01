//! Air interface encryption, security class 2 (EN 300 392-7 clause 6). Design and phases:
//! `Docs/aie-class2-plan.md`.
//!
//! Built so far: the policy and signalling around AIE (phase 1), and the TEA1 key stream
//! generator, TB5 and the IV (phase 2). The MAC layer does not encrypt yet, so the cell keeps
//! advertising class 1 and a configured `[security.aie]` is reported and otherwise ignored.

pub mod cipher;
pub mod ta61;
pub mod tea1;

use tetra_config::bluestation::{CfgAie, StackConfig};
use tetra_pdus::mm::enums::reject_cause::RejectCause;
use tetra_pdus::mm::fields::ciphering_parameters::CipheringParameters;
use tetra_pdus::umac::fields::sysinfo_ext_services::SysinfoExtendedServices;

/// True when this build carries the key stream generator for TEA`tea`.
pub fn ksg_available(tea: u8) -> bool {
    tea == 1
}

/// Whether the MAC layer encrypts and decrypts yet. Until it does (phase 2, with TA61 for the
/// addresses), the cell must not advertise class 2, whatever key stream generators exist.
const MAC_ENCRYPTION_READY: bool = false;

/// TB5 (TS 104 053-3 clause 5.23): the encryption key of one carrier, ECK = CK XOR
/// [LA:14 CN:12 CC:6 CN:12 CC:6 CN:12 CC:6 CN:12] (TS 100 392-7 clause 6.3.2.2).
pub fn tb5(ck: &[u8; 10], carrier: u16, location_area: u16, colour_code: u8) -> [u8; 10] {
    let (cn, la, cc) = (carrier as u128 & 0xFFF, location_area as u128 & 0x3FFF, colour_code as u128 & 0x3F);
    let mut mask = la;
    for _ in 0..3 {
        mask = (mask << 12 | cn) << 6 | cc;
    }
    mask = mask << 12 | cn;
    let mut ck_bits = 0u128;
    for &b in ck {
        ck_bits = ck_bits << 8 | b as u128;
    }
    let eck = ck_bits ^ mask;
    std::array::from_fn(|i| (eck >> (8 * (9 - i))) as u8)
}

/// The 29-bit IV of TEA set A (TS 100 392-7 clause 6.3.2.1): IV(0) is the lsb. `tn` is the
/// timeslot 1..=4, `fn_` the frame 1..=18, `mn` the multiframe 1..=60, `hn` the hyperframe.
pub fn iv(tn: u8, fn_: u8, mn: u8, hn: u16, uplink: bool) -> u32 {
    (tn as u32 - 1) | (fn_ as u32) << 2 | (mn as u32) << 7 | (hn as u32 & 0x7FFF) << 13 | (uplink as u32) << 28
}

/// The AIE settings this cell actually runs with: the configured class 2 settings when the KSG
/// they name is available and the MAC can use it, else `None` (class 1, clear).
pub fn effective(cfg: &StackConfig) -> Option<&CfgAie> {
    cfg.security.aie.as_ref().filter(|a| MAC_ENCRYPTION_READY && ksg_available(a.ksg))
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
        (Some(a), None) if !ksg_available(a.ksg) => format!(
            "class 1 (clear) — [security.aie] asks for class 2 with TEA{}, but this build has no TEA{} key stream generator",
            a.ksg, a.ksg
        ),
        (Some(a), None) => format!(
            "class 1 (clear) — [security.aie] asks for class 2 with TEA{}, but MAC-layer encryption is not finished yet",
            a.ksg
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
    // SCK number field: 0 = SCK 1 … 31 = SCK 32 (EN 300 392-7 Table A.96).
    ext.sck_n = Some(aie.sckn - 1);
    (true, ext)
}

/// The ciphering parameters this cell prefers, sent back in a D-LOCATION UPDATE REJECT that
/// refuses a radio's proposal (EN 300 392-7 clause 6.6.2.1.2).
pub fn preferred_parameters(aie: &CfgAie) -> CipheringParameters {
    CipheringParameters::class2(aie.ksg, aie.sckn)
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

/// Decide how a registering radio is ciphered from what it asked for (EN 300 392-7 clause
/// 6.6.2.1: in a class 2 cell the radio proposes KSG and SCKN; unacceptable ones are rejected).
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
            sck_vn: 0,
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

    /// Known answers from MidnightBlueLabs/TETRA_crypto `tb5()`.
    #[test]
    fn tb5_known_answers() {
        let ck = [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23];
        assert_eq!(tb5(&ck, 3681, 2, 1), [0x01, 0x28, 0xDD, 0x26, 0x6F, 0xBB, 0xB4, 0x6B, 0x1F, 0x42]);
        assert_eq!(tb5(&ck, 1521, 1, 1), [0x01, 0x26, 0x39, 0x26, 0xD6, 0xBB, 0x9A, 0x2B, 0x14, 0xD2]);
        assert_eq!(tb5(&ck, 4095, 16383, 63), [0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10, 0xFE, 0xDC]);
    }

    /// Operator-supplied TB5 vectors.
    #[test]
    fn tb5_supplied_vectors() {
        let cases: [(u16, u16, u8, [u8; 10], [u8; 10]); 4] = [
            (0x02BC, 0x1DCC, 0x05, [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xAA, 0xBB], [0x76, 0x13, 0xEA, 0x62, 0xA2, 0x6A, 0x87, 0x1F, 0xF8, 0x07]),
            (0x0DE8, 0x3AF0, 0x16, [0xBD, 0xF8, 0xE8, 0xD4, 0x7C, 0xA2, 0xED, 0xAE, 0x0C, 0xFB], [0x56, 0x3B, 0x92, 0xC2, 0xA2, 0x27, 0x5A, 0x0F, 0x61, 0x13]),
            (0x0DF7, 0x29E2, 0x22, [0x8A, 0x41, 0xC5, 0x61, 0x75, 0xBF, 0xBE, 0x35, 0x68, 0x91], [0x2D, 0xCA, 0xB8, 0x83, 0xAA, 0xC7, 0x09, 0xEB, 0x45, 0x66]),
            (0x0757, 0x082E, 0x3F, [0xBA, 0x3E, 0x06, 0x96, 0xE8, 0x3D, 0x16, 0x60, 0x89, 0x89], [0x9A, 0x87, 0xD3, 0x69, 0x9D, 0x42, 0xCB, 0x3F, 0x7E, 0xDE]),
        ];
        for (cn, la, cc, ck, eck) in cases {
            assert_eq!(tb5(&ck, cn, la, cc), eck, "CN {cn:03X} LA {la:04X} CC {cc:02X}");
        }
    }

    #[test]
    fn iv_layout() {
        assert_eq!(iv(1, 1, 1, 0, false), 1 << 2 | 1 << 7);
        assert_eq!(iv(4, 18, 60, 0x7FFF, true), 3 | 18 << 2 | 60 << 7 | 0x7FFF << 13 | 1 << 28);
        assert_eq!(iv(1, 1, 1, 0x8001, false) >> 13, 1, "only the 15 lsbs of the hyperframe");
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
        assert!(posture(&cfg).contains("not finished"));
    }
}
