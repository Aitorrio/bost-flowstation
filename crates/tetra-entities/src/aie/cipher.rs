//! Per-cell air interface encryption state for security class 2: the SCK modified per carrier
//! (TB5), the ESI mapping (TA61) and the key stream segment of each timeslot (TEA1).

use std::collections::HashMap;

use tetra_config::bluestation::CfgAie;
use tetra_core::{BitBuffer, TdmaTime};

use super::{iv, ta61::Ta61, tb5, tea1::Tea1};

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
        assert!(kss_offset + len <= KSS_BITS, "key stream segment too short: {} + {}", kss_offset, len);
        let saved = buf.get_pos();
        buf.seek(start);
        for i in 0..len {
            buf.xor_bit(self.bit(kss_offset + i));
        }
        buf.seek(saved);
    }
}

/// TCH/S speech: KSS(0 to 273) applies to the 274 type-1 bits in channel order (TS 100 392-7
/// Table 6.4). The stack carries speech in codec order and LMAC sorts it into channel order
/// (EN 300 395-2 Table 4), so the key stream is returned permuted into codec order: XORing it onto
/// codec-order speech is the same as XORing KSS onto the channel-order bits. One bit per byte.
pub fn tch_s_kss_codec_order(kss: &Kss) -> [u8; 274] {
    let channel: [u8; 274] = std::array::from_fn(|i| kss.bit(i));
    crate::lmac::components::tch_reorder::channel_to_codec(&channel)
}

/// Class 2 encryption context of one cell.
pub struct CellCipher {
    /// 1 = TEA1 (this crate's generator), 2 / 3 = TEA2 / TEA3 from `tetra-security`.
    ksg: u8,
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
        let sck = aie.sck.0;
        CellCipher {
            ksg: aie.ksg,
            sck_vn: aie.sck_vn,
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
        let eck = self.eck.get(&carrier).or_else(|| self.eck.get(&self.main_carrier)).expect("main carrier has an ECK");
        let mut out = [0u8; KSS_BITS / 8];
        let iv = iv(t.t, t.f, t.m, t.h, uplink);
        match self.ksg {
            1 => Tea1::new(eck, iv).key_bytes(&mut out),
            n => {
                use tetra_security::ksg::{Iv, KsgId, new_ksg};
                let id = KsgId::from_number(n - 1).expect("ksg checked by ksg_available");
                let mut g = new_ksg(id, &tetra_security::keys::CipherKey(*eck), Iv::from_raw(iv)).expect("TEA2/TEA3 are provided");
                g.fill(&mut out);
            }
        }
        Kss(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_config::bluestation::CipherKey;

    fn cipher() -> CellCipher {
        let aie = CfgAie {
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
        Tea1::new(&eck, iv(2, 5, 17, 300, false)).key_bytes(&mut expected);
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

    /// XOR in codec order with the permuted key stream equals XOR in channel order with KSS.
    #[test]
    fn tch_key_stream_in_codec_order_matches_channel_order() {
        use crate::lmac::components::tch_reorder::codec_to_channel;
        let c = cipher();
        let kss = c.kss(3681, TdmaTime { t: 2, f: 4, m: 9, h: 1 }, false);
        let codec_kss = tch_s_kss_codec_order(&kss);
        let channel = codec_to_channel(&codec_kss);
        for (i, &b) in channel.iter().enumerate() {
            assert_eq!(b, kss.bit(i), "channel bit {i}");
        }

        let speech: [u8; 274] = std::array::from_fn(|i| (i % 3 == 0) as u8);
        let encrypted_codec: [u8; 274] = std::array::from_fn(|i| speech[i] ^ codec_kss[i]);
        let via_channel = codec_to_channel(&speech);
        let encrypted_channel = codec_to_channel(&encrypted_codec);
        for i in 0..274 {
            assert_eq!(encrypted_channel[i], via_channel[i] ^ kss.bit(i));
        }
    }

    #[test]
    fn encryption_mode_follows_sck_vn_parity() {
        assert_eq!(cipher().encryption_mode(), 0b11);
    }

    /// The in-tree TEA1 and the `tetra-security` generators agree, and TEA2/TEA3 produce a
    /// different stream from the same key.
    #[test]
    fn tea_generators_agree_and_differ() {
        use tetra_security::ksg::{Iv, KsgId, new_ksg};
        let eck = tb5(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23], 3681, 2, 1);
        let t = TdmaTime { t: 2, f: 5, m: 17, h: 300 };
        let ivv = iv(t.t, t.f, t.m, t.h, false);
        let mut ours = [0u8; 54];
        Tea1::new(&eck, ivv).key_bytes(&mut ours);
        let mut theirs = [0u8; 54];
        new_ksg(KsgId::Tea1, &tetra_security::keys::CipherKey(eck), Iv::from_raw(ivv)).unwrap().fill(&mut theirs);
        assert_eq!(ours, theirs, "two independent TEA1 implementations agree");
        for ksg in [2u8, 3] {
            let aie = CfgAie { enabled: true, ksg, sckn: 1, sck: CipherKey([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23]), sck_vn: 3, clear_groups: vec![] };
            let c = CellCipher::new(&aie, &[3681], 2, 1);
            assert_ne!(c.kss(3681, t, false).0, ours, "TEA{ksg} differs from TEA1");
        }
    }
}
