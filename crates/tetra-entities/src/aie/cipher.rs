//! Per-cell air interface encryption state for security class 2: the SCK modified per carrier
//! (TB5), the ESI mapping (TA61) and the key stream segment of each timeslot. The key stream
//! generators (TEA1/2/3) all live in the `tetra-security` crate; this module only drives them.

use std::collections::HashMap;

use tetra_config::bluestation::CfgAie;
use tetra_core::{BitBuffer, SsiType, TdmaTime, TetraAddress};
use tetra_security::keys::CipherKey as SecCipherKey;
use tetra_security::ksg::{Iv, KsgId, new_ksg};

use super::{iv, ta61::Ta61, tb5};

/// Bits of key stream per timeslot on a phase modulation channel (TS 100 392-7 clause 6.3): enough
/// for a TCH/7,2 unprotected traffic channel. The second half slot starts at bit 216.
pub const KSS_BITS: usize = 432;
pub const SECOND_HALF_SLOT_KSS_OFFSET: usize = 216;

/// One timeslot's key stream segment. KSS(0) is the most significant bit of the first TEA1 key
/// byte.
pub struct Kss([u8; KSS_BITS / 8]);

impl Kss {
    pub fn bit(&self, i: usize) -> u8 {
        (self.0[i / 8] >> (7 - i % 8)) & 1
    }

    /// XORs `len` bits of `buf`, starting at bit position `start`, with KSS(`kss_offset`) onwards.
    /// The buffer position is restored afterwards.
    pub fn apply(&self, buf: &mut BitBuffer, start: usize, len: usize, kss_offset: usize) {
        assert!(
            kss_offset + len <= KSS_BITS,
            "key stream segment too short: {} + {}",
            kss_offset,
            len
        );
        let saved = buf.get_pos();
        buf.seek(start);
        for i in 0..len {
            buf.xor_bit(self.bit(kss_offset + i));
        }
        buf.seek(saved);
    }
}

/// TCH/S type-1 speech is two consecutive 137-bit codec frames. Apply AIE
/// BEFORE the sensitivity-class reordering of channel coding (EN 300 395-2
/// 5.2, EN 300 392-7 6.1/6.4). Permuting KSS here corrupts interoperable voice.
pub fn tch_s_kss_codec_order(kss: &Kss) -> [u8; 274] {
    std::array::from_fn(|i| kss.bit(i))
}

/// Class 2 encryption context of one cell.
pub struct CellCipher {
    /// 1 = TEA1 (this crate's generator), 2 / 3 = TEA2 / TEA3 from `tetra-security`.
    ksg: u8,
    class3: bool,
    config: Option<tetra_config::bluestation::SharedConfig>,
    location_area: u16,
    colour_code: u8,
    sck_vn: u16,
    ta61: Ta61,
    /// ECK per carrier number.
    eck: HashMap<u16, [u8; 10]>,
    /// Main carrier, used when a carrier is not known (never expected).
    main_carrier: u16,
}

impl CellCipher {
    /// `carriers` lists every carrier of the cell, main carrier first.
    pub fn new(aie: &CfgAie, carriers: &[u16], location_area: u16, colour_code: u8) -> Self {
        let sck = aie.class3.as_ref().map_or(aie.sck.0, |c| c.cck.0);
        CellCipher {
            ksg: aie.ksg,
            sck_vn: aie.class3.as_ref().map_or(aie.sck_vn, |c| c.cck_id),
            class3: aie.class3.is_some(),
            config: None,
            location_area,
            colour_code,
            ta61: Ta61::new(&sck),
            eck: carriers.iter().map(|&cn| (cn, tb5(&sck, cn, location_area, colour_code))).collect(),
            main_carrier: carriers[0],
        }
    }

    /// The downlink MAC-RESOURCE encryption mode of an encrypted PDU (TS 100 392-7 Table 6.6):
    /// 10 with an even SCK-VN, 11 with an odd one.
    pub fn encryption_mode(&self) -> u8 {
        0b10 | (self.sck_vn & 1) as u8
    }

    pub fn with_config(mut self, config: tetra_config::bluestation::SharedConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn is_class3(&self) -> bool {
        self.class3
    }

    /// UL and individual DL use the terminal DCK; group DL uses the CCK.
    /// Missing individual key is a hard failure, never a fallback to CCK.
    pub fn kss_for(&self, carrier: u16, t: TdmaTime, uplink: bool, addr: TetraAddress) -> Option<Kss> {
        if !self.class3 || (!uplink && addr.ssi_type == SsiType::Gssi) {
            return Some(self.kss(carrier, t, uplink));
        }
        let dck = self.dck_for_address(addr)?;
        Some(self.kss_for_dck(carrier, t, uplink, &dck))
    }

    pub(crate) fn dck_for_address(&self, addr: TetraAddress) -> Option<tetra_config::bluestation::CipherKey> {
        if !matches!(addr.ssi_type, SsiType::Issi | SsiType::Ssi) { return None; }
        self.config.as_ref()?.state_read().sc3_dcks.get(&addr.ssi).cloned()
    }

    pub(crate) fn kss_for_dck(&self, carrier: u16, t: TdmaTime, uplink: bool, dck: &tetra_config::bluestation::CipherKey) -> Kss {
        let eck = SecCipherKey(tb5(&dck.0, carrier, self.location_area, self.colour_code));
        self.generate(&eck, t, uplink)
    }

    pub fn sck_vn(&self) -> u16 {
        self.sck_vn
    }

    /// SSI → ESI (TS 100 392-7 clause 4.2.6).
    pub fn esi(&self, ssi: u32) -> u32 {
        self.ta61.encrypt(ssi)
    }

    /// ESI → SSI, for addresses received in encrypted uplink MAC PDUs.
    pub fn ssi(&self, esi: u32) -> u32 {
        self.ta61.decrypt(esi)
    }

    /// The key stream segment of timeslot `t` on `carrier`.
    pub fn kss(&self, carrier: u16, t: TdmaTime, uplink: bool) -> Kss {
        let eck = self
            .eck
            .get(&carrier)
            .or_else(|| self.eck.get(&self.main_carrier))
            .expect("main carrier has an ECK");
        self.generate(&SecCipherKey(*eck), t, uplink)
    }

    fn generate(&self, eck: &SecCipherKey, t: TdmaTime, uplink: bool) -> Kss {
        let mut out = [0u8; KSS_BITS / 8];
        let id = KsgId::from_number(self.ksg - 1).expect("ksg checked by ksg_available");
        let mut g =
            new_ksg(id, eck, Iv::from_raw(iv(t.t, t.f, t.m, t.h, uplink))).expect("ksg_available guarantees a generator for this KSG");
        g.fill(&mut out);
        Kss(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_config::bluestation::CipherKey;

    fn cipher() -> CellCipher {
        let aie = CfgAie {
            class3: None,
            enabled: true,
            ksg: 1,
            sckn: 1,
            sck: CipherKey([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23]),
            sck_vn: 3,
            clear_groups: vec![],
        };
        CellCipher::new(&aie, &[3681, 3684], 2, 1)
    }

    #[test]
    fn kss_is_tea1_of_the_carrier_eck() {
        let c = cipher();
        let t = TdmaTime { t: 2, f: 5, m: 17, h: 300 };
        let kss = c.kss(3681, t, false);
        let mut expected = [0u8; 54];
        let eck = tb5(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23], 3681, 2, 1);
        new_ksg(KsgId::Tea1, &SecCipherKey(eck), Iv::from_raw(iv(2, 5, 17, 300, false)))
            .unwrap()
            .fill(&mut expected);
        assert_eq!(kss.0, expected);
        assert_ne!(kss.0, c.kss(3684, t, false).0, "each carrier has its own ECK");
        assert_ne!(kss.0, c.kss(3681, t, true).0, "uplink and downlink differ");
    }

    #[test]
    fn apply_twice_restores_the_bits() {
        let c = cipher();
        let kss = c.kss(3681, TdmaTime { t: 1, f: 1, m: 1, h: 0 }, false);
        let mut buf = BitBuffer::from_bytes(&[0xA5; 30]);
        let original = buf.dump_bin();
        kss.apply(&mut buf, 13, 150, 0);
        assert_ne!(buf.dump_bin(), original);
        kss.apply(&mut buf, 13, 150, 0);
        assert_eq!(buf.dump_bin(), original);
    }

    /// Radio-side reference: encrypt source frames, then channel-code them.
    /// Decode through the real LMAC path; a permuted KSS fails this test even
    /// though a local encrypt/decrypt round trip would hide that bug.
    #[test]
    fn r4_speech_cipher_matches_type1_before_channel_coding() {
        use crate::lmac::components::errorcontrol::{encode_tp, decode_tp};
        use tetra_saps::tmv::TmvUnitdataReq;
        use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
        let c = cipher();
        let speech: [u8; 274] = std::array::from_fn(|i| ((i * 17 + i / 7) % 3 == 0) as u8);
        for uplink in [false, true] {
            let kss = c.kss(3681, TdmaTime { t: 2, f: 4, m: 9, h: 1 }, uplink);
            let radio: [u8; 274] = std::array::from_fn(|i| speech[i] ^ kss.bit(i));
            let encoded = encode_tp(TmvUnitdataReq { logical_channel: LogicalChannel::TchS,
                mac_block: BitBuffer::from_bitarr(&radio), scrambling_code: 0x1234567 }, 1);
            let (decoded, crc) = decode_tp(LogicalChannel::TchS, encoded, 0x1234567);
            assert!(crc);
            let mut decoded = decoded.unwrap(); decoded.seek(0);
            let mut bits = [0; 274]; decoded.to_bitarr(&mut bits);
            let stack_kss = tch_s_kss_codec_order(&kss);
            for i in 0..274 { bits[i] ^= stack_kss[i]; }
            assert_eq!(bits, speech, "UL/DL must use source bit order, not sensitivity order");
        }
    }

    #[test]
    fn encryption_mode_follows_sck_vn_parity() {
        assert_eq!(cipher().encryption_mode(), 0b11);
    }

    /// Each KSG number selects a distinct generator: TEA1, TEA2 and TEA3 produce different key
    /// streams from the same ECK, so the dispatch in `kss()` is wired to the right one.
    #[test]
    fn each_ksg_produces_a_distinct_stream() {
        let t = TdmaTime { t: 2, f: 5, m: 17, h: 300 };
        let sck = [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23];
        let streams: Vec<[u8; KSS_BITS / 8]> = [1u8, 2, 3]
            .iter()
            .map(|&ksg| {
                let aie = CfgAie {
                    class3: None,
                    enabled: true,
                    ksg,
                    sckn: 1,
                    sck: CipherKey(sck),
                    sck_vn: 3,
                    clear_groups: vec![],
                };
                CellCipher::new(&aie, &[3681], 2, 1).kss(3681, t, false).0
            })
            .collect();
        assert_ne!(streams[0], streams[1], "TEA1 and TEA2 differ");
        assert_ne!(streams[0], streams[2], "TEA1 and TEA3 differ");
        assert_ne!(streams[1], streams[2], "TEA2 and TEA3 differ");
    }
}

#[cfg(test)]
mod sc3_tests {
    use super::*;
    use tetra_config::bluestation::sec_security::CfgClass3;
    use tetra_config::bluestation::{CipherKey, SharedConfig};
    #[test]
    fn sc3_dck_selection_and_group_cck_are_distinct_and_cell_bound() {
        let aie = CfgAie {
            enabled: true,
            ksg: 2,
            sck: CipherKey([0; 10]),
            sckn: 1,
            sck_vn: 0,
            class3: Some(CfgClass3 {
                cck: CipherKey([0x33; 10]),
                cck_id: 43,
            }),
            clear_groups: vec![],
        };
        let cfg = tetra_config::bluestation::from_toml_str(include_str!("../../../../example_config/config.toml")).unwrap();
        let shared = SharedConfig::from_parts(cfg, None);
        shared.state_write().sc3_dcks.insert(100, CipherKey([0x11; 10]));
        shared.state_write().sc3_dcks.insert(101, CipherKey([0x22; 10]));
        let c = CellCipher::new(&aie, &[1521, 1522], 7, 3).with_config(shared.clone());
        let t = TdmaTime { t: 2, f: 3, m: 4, h: 5 };
        assert!(c.kss_for(1521, t, true, TetraAddress::issi(102)).is_none());
        assert!(c.kss_for(1521, t, true, TetraAddress::new(900, SsiType::Gssi)).is_none());
        let d1 = c.kss_for(1521, t, true, TetraAddress::issi(100)).unwrap();
        let d2 = c.kss_for(1521, t, true, TetraAddress::issi(101)).unwrap();
        assert_ne!(d1.0, d2.0);
        let expected = c.generate(&SecCipherKey(tb5(&[0x11; 10], 1521, 7, 3)), t, true);
        assert_eq!(d1.0, expected.0);
        assert_ne!(d1.0, c.kss_for(1522, t, true, TetraAddress::issi(100)).unwrap().0);
        assert_ne!(d1.0, c.kss_for(1521, t, false, TetraAddress::issi(100)).unwrap().0);
        assert_eq!(
            c.kss_for(1521, t, false, TetraAddress::new(900, SsiType::Gssi)).unwrap().0,
            c.kss(1521, t, false).0
        );
        assert_eq!(c.esi(100), Ta61::new(&[0x33; 10]).encrypt(100));
        assert_eq!(c.encryption_mode(), 0b11);
        shared.state_write().sc3_dcks.remove(&100);
        assert!(c.kss_for(1521, t, true, TetraAddress::issi(100)).is_none());
        let (_, ext) = super::super::sysinfo_security(Some(&aie));
        assert!(ext.class3_supported && ext.auth_required && !ext.class2_supported && !ext.class1_supported);
        assert!(!ext.gck_supported);
        use super::super::{CipherDecision, registration_decision};
        use tetra_pdus::mm::fields::ciphering_parameters::CipheringParameters;
        assert!(matches!(registration_decision(Some(&aie), false, None), CipherDecision::Reject(_)));
        assert!(matches!(
            registration_decision(Some(&aie), true, Some(CipheringParameters::class2(2, 1).to_bits())),
            CipherDecision::Reject(_)
        ));
        assert!(matches!(
            registration_decision(Some(&aie), true, Some(CipheringParameters::class3(2).to_bits())),
            CipherDecision::Encrypted(_)
        ));
        // The MS can advertise capabilities in these five bits; the BS must
        // clear them in its downlink ciphering parameters (EN 300 392-7 A.8.12).
        assert_eq!(
            registration_decision(Some(&aie), true, Some(CipheringParameters::class3(2).to_bits() | 0b11111)),
            CipherDecision::Encrypted(CipheringParameters::class3(2))
        );
    }
}
