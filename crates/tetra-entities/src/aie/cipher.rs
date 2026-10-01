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

/// Class 2 encryption context of one cell.
pub struct CellCipher {
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
        Tea1::new(eck, iv(t.t, t.f, t.m, t.h, uplink)).key_bytes(&mut out);
        Kss(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_config::bluestation::CipherKey;

    fn cipher() -> CellCipher {
        let aie = CfgAie {
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

    #[test]
    fn encryption_mode_follows_sck_vn_parity() {
        assert_eq!(cipher().encryption_mode(), 0b11);
    }
}
