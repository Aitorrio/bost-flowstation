//! TEA2 (ETSI TS 104 053-1 clause 6).
//!
//! An eight-byte Output Register R0 … R7 and a ten-byte Cipher Key Register
//! K0 … K9. Each step (clauses 6.1.4 and 6.1.9):
//!
//! ```text
//!   K'0 = P(K9 ⊕ K2)                       (= POut)
//!   K'i = K(i−1)                            i = 1 … 9
//!   R'0 = R7 ⊕ BP(R5) ⊕ R2 ⊕ f1[E(R1, R0)] ⊕ POut
//!   R'3 = R2 ⊕ f2[E(R4, R3)]
//!   R'i = R(i−1)                            otherwise
//! ```
//!
//! After loading, 50 run-up steps are made; the first key stream byte is R7
//! after one further step, and every later byte after 19 more (clause 6.2).

use super::{Iv, Ksg, Nonlinear, wire_cross};

/// Byte permutation P (Figure 9), indexed by the input byte.
#[rustfmt::skip]
const P: [u8; 256] = [
    0x62, 0xDA, 0xFD, 0xB6, 0xBB, 0x9C, 0xD8, 0x2A, 0xAB, 0x28, 0x6E, 0x42, 0xE7, 0x1C, 0x78, 0x9E,
    0xFC, 0xCA, 0x81, 0x8E, 0x32, 0x3B, 0xB4, 0xEF, 0x9F, 0x8B, 0xDB, 0x94, 0x0F, 0x9A, 0xA2, 0x96,
    0x1B, 0x7A, 0xFF, 0xAA, 0xC5, 0xD6, 0xBC, 0x24, 0xDF, 0x44, 0x03, 0x09, 0x0B, 0x57, 0x90, 0xBA,
    0x7F, 0x1F, 0xCF, 0x71, 0x98, 0x07, 0xF8, 0xA1, 0x60, 0xF7, 0x52, 0x8D, 0xE5, 0xD7, 0x69, 0x87,
    0x14, 0xED, 0x92, 0xEB, 0xB3, 0x2F, 0xE9, 0x3D, 0xC6, 0x50, 0x5A, 0xA7, 0x45, 0x18, 0x11, 0xC4,
    0xCE, 0xAC, 0xF4, 0x1D, 0x82, 0x54, 0x3E, 0x49, 0xD5, 0xEE, 0x84, 0x35, 0x41, 0x3A, 0xEC, 0x34,
    0x17, 0xE0, 0xC9, 0xFE, 0xE8, 0xCB, 0xE6, 0xAE, 0x68, 0xE2, 0x6B, 0x46, 0xC8, 0x47, 0xB2, 0xE3,
    0x97, 0x10, 0x0E, 0xB8, 0x76, 0x5B, 0xBE, 0xF5, 0xA6, 0x3C, 0x8F, 0xF6, 0xD1, 0xAF, 0xC0, 0x5E,
    0x7E, 0xCD, 0x7C, 0x51, 0x6D, 0x74, 0x2C, 0x16, 0xF2, 0xA5, 0x65, 0x64, 0x58, 0x72, 0x1E, 0xF1,
    0x04, 0xA8, 0x13, 0x53, 0x31, 0xB1, 0x20, 0xD3, 0x75, 0x5F, 0xA4, 0x56, 0x06, 0x8A, 0x8C, 0xD9,
    0x70, 0x12, 0x29, 0x61, 0x4F, 0x4C, 0x15, 0x05, 0xD2, 0xBD, 0x7D, 0x9B, 0x99, 0x83, 0x2B, 0x25,
    0xD0, 0x23, 0x48, 0x3F, 0xB0, 0x2E, 0x0D, 0x0C, 0xC7, 0xCC, 0xB7, 0x5C, 0xF0, 0xBF, 0x2D, 0x4E,
    0x40, 0x39, 0x9D, 0x21, 0x37, 0x77, 0x73, 0x4B, 0x4D, 0x5D, 0xFA, 0xDE, 0x00, 0x80, 0x85, 0x6F,
    0x22, 0x91, 0xDC, 0x26, 0x38, 0xE4, 0x4A, 0x79, 0x6A, 0x67, 0x93, 0xF3, 0xFB, 0x19, 0xA0, 0x7B,
    0xF9, 0x95, 0x89, 0x66, 0xB9, 0xD4, 0xC1, 0xDD, 0x63, 0x33, 0xE1, 0xC3, 0xB5, 0xA3, 0xC2, 0x27,
    0x0A, 0x88, 0xA9, 0x1A, 0x6C, 0x43, 0xEA, 0xAD, 0x30, 0x86, 0x36, 0x59, 0x08, 0x55, 0x01, 0x02,
];

/// Expander E (Figure 10): the bits each box takes from the 16-bit word.
const TAPS: [[u8; 4]; 8] = [
    [1, 2, 15, 16],
    [2, 3, 16, 9],
    [3, 4, 9, 10],
    [4, 5, 10, 11],
    [5, 6, 11, 12],
    [6, 7, 12, 13],
    [7, 8, 13, 14],
    [8, 1, 14, 15],
];

/// f1 (Figure 11).
#[rustfmt::skip]
const F1: Nonlinear = Nonlinear { taps: TAPS, table: [
    [1, 1, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0, 0, 0, 1, 1],
    [0, 1, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 1, 1, 0],
    [1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1],
    [0, 0, 1, 0, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0],
    [0, 1, 1, 0, 1, 0, 1, 1, 1, 0, 0, 0, 1, 1, 0, 0],
    [0, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1],
    [1, 0, 1, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0, 0, 0, 1],
    [1, 0, 0, 1, 1, 1, 1, 0, 1, 0, 1, 0, 0, 1, 0, 0],
]};

/// f2 (Figure 12).
#[rustfmt::skip]
const F2: Nonlinear = Nonlinear { taps: TAPS, table: [
    [1, 0, 0, 0, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 0],
    [0, 1, 0, 0, 1, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 1],
    [0, 0, 0, 1, 0, 1, 1, 1, 0, 1, 1, 0, 1, 1, 0, 0],
    [1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1],
    [0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 0, 1, 0, 0],
    [1, 0, 0, 1, 0, 0, 1, 1, 0, 1, 0, 0, 1, 1, 0, 1],
    [1, 0, 0, 0, 0, 1, 0, 1, 1, 1, 1, 0, 1, 0, 0, 1],
    [0, 1, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0, 1, 0, 1, 1],
]};

/// BP (clause 6.1.8): bits 12345678 of R5 come out in the order 48572136.
const BP: [u8; 8] = [4, 8, 5, 7, 2, 1, 3, 6];

/// IV load mask for R7, R2, R1, R0 (clause 6.2.3).
const IV_MASK: [u8; 4] = [0x5A, 0x6E, 0x32, 0x78];

/// A loaded TEA2 generator.
pub struct Tea2 {
    r: [u8; 8],
    k: [u8; 10],
    first: bool,
}

impl Tea2 {
    /// Load the cipher key (C1 … C10, most significant first) and the IV, and
    /// perform the run-up.
    pub fn new(ck: &[u8; 10], iv: Iv) -> Tea2 {
        // CK loading (Figure 8): C1 ends up in K9 … C10 in K0.
        let mut k = [0u8; 10];
        for (i, &c) in ck.iter().enumerate() {
            k[9 - i] = c;
        }
        // IV loading (clause 6.2.3): the 32-bit word F1 F2 F3 F4.
        let f = iv.bytes();
        let r = [
            f[3] ^ IV_MASK[3],
            f[2] ^ IV_MASK[2],
            f[1] ^ IV_MASK[1],
            f[3],
            f[2],
            f[1],
            f[0],
            f[0] ^ IV_MASK[0],
        ];
        let mut g = Tea2 { r, k, first: true };
        for _ in 0..50 {
            g.step();
        }
        g
    }

    /// One step of both registers (clauses 6.1.4 and 6.1.9).
    fn step(&mut self) {
        let r = &self.r;
        let k = &self.k;
        let p_out = P[(k[9] ^ k[2]) as usize];
        let r0 = r[7] ^ wire_cross(r[5], BP) ^ r[2] ^ F1.apply(r[1], r[0]) ^ p_out;
        let r3 = r[2] ^ F2.apply(r[4], r[3]);
        self.r = [r0, r[0], r[1], r3, r[3], r[4], r[5], r[6]];
        self.k = [p_out, k[0], k[1], k[2], k[3], k[4], k[5], k[6], k[7], k[8]];
    }
}

impl Ksg for Tea2 {
    fn next_byte(&mut self) -> u8 {
        let steps = if std::mem::take(&mut self.first) { 1 } else { 19 };
        for _ in 0..steps {
            self.step();
        }
        self.r[7]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p_is_a_permutation_with_the_documented_example() {
        let mut seen = [false; 256];
        for &v in &P {
            assert!(!seen[v as usize]);
            seen[v as usize] = true;
        }
        assert_eq!(P[0x37], 0xA1); // "P(37) = A1", clause 6.1.5
    }

    #[test]
    fn iv_load_matches_the_worked_example() {
        // Clause 6.2.3: IV 11010 00011010 11100010 00000110 gives
        // R7 … R4 = 01000000 00011010 00011010 11100010 and
        // R3 … R0 = 00000110 01110100 11010000 01111110.
        let iv = Iv::from_raw(0b11010_00011010_11100010_00000110);
        let f = iv.bytes();
        let r7_to_r0 = [f[0] ^ 0x5A, f[0], f[1], f[2], f[3], f[1] ^ 0x6E, f[2] ^ 0x32, f[3] ^ 0x78];
        assert_eq!(
            r7_to_r0,
            [
                0b01000000, 0b00011010, 0b00011010, 0b11100010, 0b00000110, 0b01110100, 0b11010000, 0b01111110
            ]
        );
    }

    #[test]
    fn key_stream_matches_reference_vectors() {
        // Independent implementation (Midnight Blue, TETRA_crypto, Apache-2.0).
        let cases: [(u32, [u8; 10], [u8; 10]); 2] = [
            (0x12345678, [0; 10], [0xA7, 0x98, 0x39, 0xE4, 0xBA, 0x88, 0xEE, 0x54, 0xA0, 0x29]),
            (
                0x12345678,
                [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA],
                [0x64, 0x70, 0x4E, 0xA9, 0xD7, 0xDC, 0x25, 0x60, 0x81, 0x39],
            ),
        ];
        for (iv, key, want) in cases {
            let mut g = Tea2::new(&key, Iv::from_raw(iv));
            let mut got = [0u8; 10];
            g.fill(&mut got);
            assert_eq!(got, want, "iv {iv:08x}");
        }
    }
}
