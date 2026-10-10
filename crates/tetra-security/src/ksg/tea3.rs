//! TEA3 (ETSI TS 104 053-1 clause 7).
//!
//! An eight-byte Output Register R0 … R7 and a ten-byte Cipher Key Register
//! K0 … K9. Each step (clauses 7.1.3 and 7.1.4):
//!
//! ```text
//!   Kout = K9 ⊕ P(K7 ⊕ K2)
//!   K'0  = Kout,  K'i = K(i−1)              i = 1 … 9
//!   R'0  = R7 ⊕ BP(R4) ⊕ f2[E(R2, R1)] ⊕ Kout
//!   R'5  = R4 ⊕ f1[E(R6, R5)]
//!   R'i  = R(i−1)                           otherwise
//! ```
//!
//! After loading, 32 run-up steps are made; each key stream byte is R7 after
//! 19 further steps (clause 7.2).

use super::{Iv, Ksg, Nonlinear, wire_cross};

/// Byte substitution P (Figure 15), indexed by the input byte. Transcribed
/// as printed — see the test for why it is not quite a permutation.
#[rustfmt::skip]
const P: [u8; 256] = [
    0x7D, 0xBF, 0x7B, 0x92, 0xAE, 0x7C, 0xF2, 0x10, 0x5A, 0x0F, 0x61, 0x7A, 0x98, 0x76, 0x07, 0x64,
    0xEE, 0x89, 0xF7, 0xBA, 0xC2, 0x02, 0x0D, 0xE8, 0x56, 0x2E, 0xCA, 0x58, 0xC0, 0xFA, 0x2A, 0x01,
    0x57, 0x6E, 0x3F, 0x4B, 0x9C, 0xDA, 0xA6, 0x5B, 0x41, 0x26, 0x50, 0x24, 0x3E, 0xF8, 0x0A, 0x86,
    0xB6, 0x5C, 0x34, 0xE9, 0x06, 0x88, 0x1F, 0x39, 0x33, 0xDF, 0xD9, 0x78, 0xD8, 0xA8, 0x51, 0xB2,
    0x09, 0xCD, 0xA1, 0xDD, 0x8E, 0x62, 0x69, 0x4D, 0x23, 0x2B, 0xA9, 0xE1, 0x53, 0x94, 0x90, 0x1E,
    0xB4, 0x3B, 0xF9, 0x4E, 0x36, 0xFE, 0xB5, 0xD1, 0xA2, 0x8D, 0x66, 0xCE, 0xB7, 0xC4, 0x60, 0xED,
    0x96, 0x4F, 0x31, 0x79, 0x35, 0xEB, 0x8F, 0xBB, 0x54, 0x14, 0xCB, 0xDE, 0x6B, 0x2D, 0x19, 0x82,
    0x80, 0xAC, 0x17, 0x05, 0xFF, 0xA4, 0xCF, 0xC6, 0x6F, 0x65, 0xE6, 0x74, 0xC8, 0x93, 0xF4, 0x7E,
    0xF3, 0x43, 0x9F, 0x71, 0xAB, 0x9A, 0x0B, 0x87, 0x55, 0x70, 0x0C, 0xAD, 0xCC, 0xA5, 0x44, 0xE7,
    0x46, 0x45, 0x03, 0x30, 0x1A, 0xEA, 0x67, 0x99, 0xDB, 0x4A, 0x42, 0xD7, 0xAA, 0xE4, 0xC2, 0xD5,
    0xF0, 0x77, 0x20, 0xC3, 0x3C, 0x16, 0xB9, 0xE2, 0xEF, 0x6C, 0x3D, 0x1B, 0x22, 0x84, 0x2F, 0x81,
    0x1D, 0xB1, 0x3A, 0xE5, 0x73, 0x40, 0xD0, 0x18, 0xC7, 0x6A, 0x9E, 0x91, 0x48, 0x27, 0x95, 0x72,
    0x68, 0x0E, 0x00, 0xFC, 0xC5, 0x5F, 0xF1, 0xF5, 0x38, 0x11, 0x7F, 0xE3, 0x5E, 0x13, 0xAF, 0x37,
    0xE0, 0x8A, 0x49, 0x1C, 0x21, 0x47, 0xD4, 0xDC, 0xB0, 0xEC, 0x83, 0x28, 0xB8, 0xF6, 0xA7, 0xC9,
    0x63, 0x59, 0xBD, 0x32, 0x85, 0x08, 0xBE, 0xD3, 0xFD, 0x4C, 0x2C, 0xFB, 0xA0, 0xC1, 0x9D, 0xB3,
    0x52, 0x8C, 0x5D, 0x29, 0x6D, 0x04, 0xBC, 0x25, 0x15, 0x8B, 0x12, 0x9B, 0xD6, 0x75, 0xA3, 0x97,
];

/// Expander E (Figure 16): the bits each box takes from the 16-bit word.
const TAPS: [[u8; 4]; 8] = [
    [3, 4, 11, 12],
    [4, 5, 12, 13],
    [5, 6, 13, 14],
    [6, 7, 14, 15],
    [7, 8, 15, 16],
    [8, 1, 16, 9],
    [1, 2, 9, 10],
    [2, 3, 10, 11],
];

/// f1 (Figure 17).
#[rustfmt::skip]
const F1: Nonlinear = Nonlinear { taps: TAPS, table: [
    [1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0],
    [1, 1, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 0, 0, 1, 0],
    [1, 0, 0, 1, 0, 0, 1, 1, 0, 1, 0, 0, 1, 1, 0, 1],
    [1, 1, 0, 1, 0, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1],
    [0, 0, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0],
    [0, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 0, 1, 0, 0, 0],
    [1, 0, 1, 1, 0, 1, 1, 0, 0, 0, 1, 0, 0, 1, 0, 1],
    [0, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 1, 1, 0, 0, 1],
]};

/// f2 (Figure 18).
#[rustfmt::skip]
const F2: Nonlinear = Nonlinear { taps: TAPS, table: [
    [1, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0, 1, 1, 1, 0],
    [0, 0, 1, 0, 1, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0],
    [0, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 0, 1, 0, 0, 0],
    [0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 0, 1, 0, 0],
    [0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 1, 0, 1, 1, 0],
    [0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 1],
    [1, 0, 0, 0, 0, 1, 1, 0, 1, 1, 1, 0, 0, 1, 0, 1],
    [1, 1, 1, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 0, 0, 1],
]};

/// BP (clause 7.1.8): bits 12345678 of R4 come out in the order 38467215.
const BP: [u8; 8] = [3, 8, 4, 6, 7, 2, 1, 5];

/// IV load mask for R7, R2, R1, R0 (clause 7.2.3): C43A7D51.
const IV_MASK: [u8; 4] = [0xC4, 0x3A, 0x7D, 0x51];

/// A loaded TEA3 generator.
pub struct Tea3 {
    r: [u8; 8],
    k: [u8; 10],
}

impl Tea3 {
    /// Load the cipher key (C1 … C10, most significant first) and the IV, and
    /// perform the run-up.
    pub fn new(ck: &[u8; 10], iv: Iv) -> Tea3 {
        // CK loading (clause 7.2.2): K(10−i) = Ci.
        let mut k = [0u8; 10];
        for (i, &c) in ck.iter().enumerate() {
            k[9 - i] = c;
        }
        // IV loading (clause 7.2.3): the 32-bit word IV1 IV2 IV3 IV4.
        let v = iv.bytes();
        let r = [
            v[3] ^ IV_MASK[3],
            v[2] ^ IV_MASK[2],
            v[1] ^ IV_MASK[1],
            v[3],
            v[2],
            v[1],
            v[0],
            v[0] ^ IV_MASK[0],
        ];
        let mut g = Tea3 { r, k };
        for _ in 0..32 {
            g.step();
        }
        g
    }

    /// One step of both registers (clauses 7.1.3 and 7.1.4).
    fn step(&mut self) {
        let r = &self.r;
        let k = &self.k;
        let k_out = k[9] ^ P[(k[7] ^ k[2]) as usize];
        let r0 = r[7] ^ wire_cross(r[4], BP) ^ F2.apply(r[2], r[1]) ^ k_out;
        let r5 = r[4] ^ F1.apply(r[6], r[5]);
        self.r = [r0, r[0], r[1], r[2], r[3], r5, r[5], r[6]];
        self.k = [k_out, k[0], k[1], k[2], k[3], k[4], k[5], k[6], k[7], k[8]];
    }
}

impl Ksg for Tea3 {
    fn next_byte(&mut self) -> u8 {
        for _ in 0..19 {
            self.step();
        }
        self.r[7]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p_matches_the_documented_example_and_known_collision() {
        assert_eq!(P[0x27], 0x5B); // "P(27) = 5B", clause 7.1.5
        // The specification calls P a permutation, but the published table is
        // not one: C2 appears at both 14 and 9E and D2 never — the property
        // analysed in "Observations on TETRA Encryption Algorithm TEA-3"
        // (IACR ePrint 2024/2045). Real radios use the table as printed, and
        // the reference key stream vectors above only pass with it.
        assert_eq!((P[0x14], P[0x9E]), (0xC2, 0xC2));
        assert!(!P.contains(&0xD2));
        let mut seen = [0u8; 256];
        for &v in &P {
            seen[v as usize] += 1;
        }
        assert_eq!(seen.iter().filter(|&&n| n == 2).count(), 1);
        assert_eq!(seen.iter().filter(|&&n| n == 0).count(), 1);
    }

    #[test]
    fn iv_load_matches_the_worked_example() {
        // Clause 7.2.3: IV 11010 00011010 11100010 00000110 gives R7 … R0 =
        // 11011110 00011010 00011010 11100010 00000110 00100000 10011111 01010111.
        let v = Iv::from_raw(0b11010_00011010_11100010_00000110).bytes();
        let r7_to_r0 = [v[0] ^ 0xC4, v[0], v[1], v[2], v[3], v[1] ^ 0x3A, v[2] ^ 0x7D, v[3] ^ 0x51];
        assert_eq!(
            r7_to_r0,
            [
                0b11011110, 0b00011010, 0b00011010, 0b11100010, 0b00000110, 0b00100000, 0b10011111, 0b01010111
            ]
        );
    }

    #[test]
    fn key_stream_matches_reference_vectors() {
        // Independent implementation (Midnight Blue, TETRA_crypto, Apache-2.0).
        let cases: [(u32, [u8; 10], [u8; 10]); 2] = [
            (0x11111111, [0; 10], [0x06, 0xA6, 0x58, 0x8C, 0x5D, 0x9A, 0x99, 0x6D, 0xD2, 0x5E]),
            (
                0x01234567,
                [0xA7, 0x98, 0x39, 0xE4, 0xBA, 0x88, 0xEE, 0x54, 0xA0, 0x29],
                [0x02, 0x49, 0x1e, 0xf5, 0x57, 0xc5, 0x1c, 0x17, 0x73, 0x0c],
            ),
        ];
        for (iv, key, want) in cases {
            let mut g = Tea3::new(&key, Iv::from_raw(iv));
            let mut got = [0u8; 10];
            g.fill(&mut got);
            assert_eq!(got, want, "iv {iv:08x}");
        }
    }
}
