//! Key stream generators (KSG) for TETRA air-interface encryption.
//!
//! A KSG is loaded with an 80-bit cipher key and a 29-bit initialisation
//! vector derived from the frame numbering, and then produces one key stream
//! byte at a time; the MAC XORs the key stream over the bits to protect
//! (ETSI EN 300 392-7 clause 6). The generators themselves are specified
//! register by register in ETSI TS 104 053-1 (set A) and implemented here
//! exactly that way, so the code can be checked against the figures.

mod tea1;
mod tea2;
mod tea3;

pub use tea1::Tea1;
pub use tea2::Tea2;
pub use tea3::Tea3;

use crate::keys::CipherKey;

/// Identifies a key stream generator, as carried in the air interface
/// "KSG number" element (EN 300 392-7, table 6.3 / clause A.8.37): value 0
/// = TEA1, 1 = TEA2, 2 = TEA3, 3 = TEA4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum KsgId {
    Tea1 = 0,
    Tea2 = 1,
    Tea3 = 2,
    Tea4 = 3,
}

impl KsgId {
    pub fn from_number(n: u8) -> Option<KsgId> {
        match n {
            0 => Some(KsgId::Tea1),
            1 => Some(KsgId::Tea2),
            2 => Some(KsgId::Tea3),
            3 => Some(KsgId::Tea4),
            _ => None,
        }
    }

    /// Whether this crate can run the generator (TEA4 is not yet implemented).
    pub fn supported(self) -> bool {
        matches!(self, KsgId::Tea1 | KsgId::Tea2 | KsgId::Tea3)
    }

    /// TEA1 keeps only 32 bits of its 80-bit key (TETRA:BURST, 2023): usable for
    /// research and for talking to TEA1-only radios, but not for protection.
    pub fn is_weak(self) -> bool {
        self == KsgId::Tea1
    }
}

/// A loaded key stream generator.
pub trait Ksg {
    /// Produce the next key stream byte, most significant bit first on air.
    fn next_byte(&mut self) -> u8;

    /// Fill `out` with key stream.
    fn fill(&mut self, out: &mut [u8]) {
        for b in out {
            *b = self.next_byte();
        }
    }
}

/// Builds a loaded generator for a KSG number, or `None` if unsupported.
pub fn new_ksg(id: KsgId, key: &CipherKey, iv: Iv) -> Option<Box<dyn Ksg>> {
    match id {
        KsgId::Tea1 => Some(Box::new(Tea1::new(&key.0, iv))),
        KsgId::Tea2 => Some(Box::new(Tea2::new(&key.0, iv))),
        KsgId::Tea3 => Some(Box::new(Tea3::new(&key.0, iv))),
        KsgId::Tea4 => None,
    }
}

/// The 29-bit Initial Value (EN 300 392-7 clause 6.3.2.1): the frame
/// numbering of the burst being protected plus the link direction, so every
/// timeslot of every hyperframe gets its own key stream.
///
/// Layout from bit 0 upwards: IV(0..1) timeslot − 1, IV(2..6) frame number
/// 1–18, IV(7..12) multiframe number 1–60, IV(13..27) the 15 low bits of
/// the hyperframe number, IV(28) direction (0 = downlink, 1 = uplink).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Iv(u32);

impl Iv {
    /// Mask of the 29 significant bits.
    pub const MASK: u32 = 0x1FFF_FFFF;

    /// From raw bits (the top three bits are ignored).
    pub const fn from_raw(bits: u32) -> Iv {
        Iv(bits & Iv::MASK)
    }

    /// From the TETRA frame numbering: `timeslot` 1–4, `frame` 1–18,
    /// `multiframe` 1–60, `hyperframe` 0–65535 (only 15 bits are used),
    /// `uplink` false for the downlink.
    pub fn from_frame(timeslot: u8, frame: u8, multiframe: u8, hyperframe: u16, uplink: bool) -> Iv {
        debug_assert!((1..=4).contains(&timeslot) && (1..=18).contains(&frame) && (1..=60).contains(&multiframe));
        let bits = u32::from(timeslot - 1)
            | u32::from(frame) << 2
            | u32::from(multiframe) << 7
            | u32::from(hyperframe & 0x7FFF) << 13
            | u32::from(uplink) << 28;
        Iv(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    /// The IV as the 32-bit word the generators load (three zero bits on top),
    /// most significant byte first.
    pub(crate) fn bytes(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }
}

/// Bit `n` (1 = most significant) of a 16-bit word.
#[inline]
fn bit16(w: u16, n: u8) -> u8 {
    ((w >> (16 - n)) & 1) as u8
}

/// The expander-plus-nonlinear-function block shared by the TEA generators.
///
/// Two register bytes form a 16-bit word (byte 1 is the more significant);
/// the expander picks, for each of the eight boxes S1 … S8, four bits of
/// that word (positions numbered 1–16, most significant first) to make a
/// nibble, and each box's truth table turns the nibble into one output bit,
/// S1 being the most significant bit of the result.
pub(crate) struct Nonlinear {
    /// Bit positions feeding each box, most significant nibble bit first.
    pub taps: [[u8; 4]; 8],
    /// Truth table of each box, indexed by the nibble value.
    pub table: [[u8; 16]; 8],
}

impl Nonlinear {
    pub(crate) fn apply(&self, byte1: u8, byte2: u8) -> u8 {
        let w = u16::from(byte1) << 8 | u16::from(byte2);
        let mut out = 0u8;
        for (i, (taps, table)) in self.taps.iter().zip(self.table.iter()).enumerate() {
            let nibble = bit16(w, taps[0]) << 3 | bit16(w, taps[1]) << 2 | bit16(w, taps[2]) << 1 | bit16(w, taps[3]);
            out |= table[nibble as usize] << (7 - i);
        }
        out
    }
}

/// A fixed reordering of the bits of a byte ("wire crossing"): `order[i]` is
/// the input bit (1 = most significant) that becomes output bit i + 1.
#[inline]
pub(crate) fn wire_cross(byte: u8, order: [u8; 8]) -> u8 {
    let mut out = 0u8;
    for (i, &src) in order.iter().enumerate() {
        out |= ((byte >> (8 - src)) & 1) << (7 - i);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iv_layout_matches_the_standard() {
        // timeslot 1, frame 6, multiframe 30, hyperframe 110, downlink
        let iv = Iv::from_frame(1, 6, 30, 110, false);
        assert_eq!(iv.bits(), 6 << 2 | 30 << 7 | 110 << 13);
        assert_eq!(Iv::from_frame(4, 18, 60, 0xFFFF, true).bits() >> 28, 1);
        assert_eq!(Iv::from_raw(0xFFFF_FFFF).bits(), Iv::MASK);
    }

    #[test]
    fn wire_cross_identity_and_reverse() {
        assert_eq!(wire_cross(0b1011_0010, [1, 2, 3, 4, 5, 6, 7, 8]), 0b1011_0010);
        assert_eq!(wire_cross(0b1011_0010, [8, 7, 6, 5, 4, 3, 2, 1]), 0b0100_1101);
    }
}
