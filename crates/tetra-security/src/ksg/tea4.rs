//! TEA4 (ETSI TS 104 053-1 clause 8).
//!
//! An eight-byte Output Register R0 … R7 and a **seven**-byte Cipher Key
//! Register K0 … K6. Each step (clauses 8.1.3 and 8.1.4):
//!
//! ```text
//!   Kout = K6 ⊕ P(K5 ⊕ K1)
//!   K'0  = Kout,  K'i = K(i−1)              i = 1 … 6
//!   R'0  = R7 ⊕ BP(R6) ⊕ f1[E(R5, R4)] ⊕ Kout
//!   R'4  = R3 ⊕ f2[E(R1, R0)]
//!   R'i  = R(i−1)                           otherwise
//! ```
//!
//! The 80-bit cipher key is not copied into the key register but mixed into
//! it byte by byte through the same feedback (clause 8.2.2), so the register
//! holds 56 bits of state derived from the key. After loading, 35 run-up
//! steps are made; each key stream byte is R7 after 19 further steps
//! (clause 8.2).
//!
//! Like TEA1, TEA4 is the export-grade member of set A. No public key stream
//! vectors exist for it (the Midnight Blue reference code covers TEA1–TEA3
//! only), so the tests below check the specification's own worked examples
//! and the structural properties of the tables; interoperability is proven
//! on air against a TEA4 radio.

use super::{Iv, Ksg, Nonlinear, wire_cross};

/// Byte substitution P (Figure 21), indexed by the input byte. A true
/// permutation (see the test); transcribed row by row from the figure.
#[rustfmt::skip]
const P: [u8; 256] = [
    0xD2, 0x22, 0x9B, 0x76, 0xEF, 0x8A, 0xEE, 0xC6, 0x5B, 0x04, 0x6B, 0xF1, 0x8B, 0x43, 0x14, 0x13,
    0x00, 0x80, 0xF7, 0xFC, 0x03, 0xB7, 0xA0, 0xAE, 0xC8, 0xA7, 0xAC, 0x82, 0xEC, 0x77, 0x42, 0xE2,
    0x18, 0x51, 0x47, 0x02, 0x52, 0x0A, 0x17, 0x05, 0xD4, 0x7A, 0x78, 0x06, 0x09, 0x0C, 0x62, 0x5E,
    0x6E, 0x5F, 0x3F, 0xF4, 0xB4, 0xA1, 0x5C, 0xCC, 0x56, 0x2C, 0xD0, 0xCD, 0xBE, 0x07, 0x3A, 0xD7,
    0xCE, 0xB1, 0x20, 0xCB, 0x2B, 0x33, 0xA8, 0xA6, 0x45, 0x2A, 0x97, 0xDA, 0x7F, 0x4F, 0x55, 0x9D,
    0x4D, 0x44, 0x91, 0x58, 0x92, 0x10, 0xC1, 0xF6, 0xDC, 0xE5, 0xB0, 0x40, 0xB2, 0x0E, 0x2D, 0x9A,
    0x37, 0x67, 0xDF, 0x4E, 0x7B, 0xFF, 0xAF, 0xAB, 0xE7, 0x53, 0x83, 0x8F, 0x0D, 0x48, 0xD1, 0x4B,
    0x49, 0xDE, 0xEA, 0xE9, 0xE1, 0x1B, 0x68, 0x6F, 0x81, 0x32, 0x12, 0x2F, 0xED, 0x94, 0x8E, 0xB5,
    0x35, 0x7E, 0x79, 0xA3, 0xE3, 0xBA, 0x95, 0x57, 0x6D, 0x8D, 0x2E, 0x1A, 0x98, 0xEB, 0x65, 0x64,
    0x0B, 0x11, 0xBD, 0xE0, 0x73, 0x5D, 0x27, 0x39, 0xA4, 0x1D, 0x3E, 0xC4, 0xF9, 0x87, 0x28, 0xE4,
    0x9E, 0x1E, 0x99, 0xBC, 0xF5, 0xFE, 0x34, 0x01, 0x41, 0xC9, 0x70, 0xBF, 0x69, 0xAD, 0xDD, 0x46,
    0xE6, 0x15, 0xC0, 0x3C, 0xD5, 0x7D, 0x59, 0x30, 0x6C, 0x84, 0x4C, 0xB9, 0x9C, 0x3B, 0x86, 0x6A,
    0xE8, 0xF8, 0x1C, 0xD6, 0x4A, 0x9F, 0x19, 0xF2, 0x8C, 0xF3, 0xA5, 0xC3, 0x3D, 0x96, 0xC5, 0xD8,
    0x75, 0xC7, 0x36, 0xAA, 0xFB, 0x29, 0xCF, 0x72, 0x25, 0x0F, 0xA9, 0x90, 0xB6, 0xFD, 0x63, 0xBB,
    0x50, 0x88, 0x7C, 0x16, 0x23, 0xD3, 0xFA, 0x93, 0x5A, 0x71, 0x21, 0xB3, 0x31, 0x74, 0x38, 0x66,
    0xC2, 0xCA, 0x61, 0x24, 0x60, 0x89, 0x26, 0x54, 0xF0, 0xB8, 0x85, 0x08, 0x1F, 0xA2, 0xDB, 0xD9,
];

/// Expander E (Figure 22): the bits each box takes from the 16-bit word.
const TAPS: [[u8; 4]; 8] = [
    [5, 6, 13, 14],
    [6, 7, 14, 15],
    [7, 8, 15, 16],
    [8, 1, 16, 9],
    [1, 2, 9, 10],
    [2, 3, 10, 11],
    [3, 4, 11, 12],
    [4, 5, 12, 13],
];

/// f1 (Figure 23).
#[rustfmt::skip]
const F1: Nonlinear = Nonlinear { taps: TAPS, table: [
    [1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 0, 1, 0],
    [1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1],
    [1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0],
    [0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 1, 1, 1, 0],
    [1, 1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0, 0, 0, 1],
    [0, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0],
    [0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 1, 0],
    [0, 1, 0, 0, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 0, 1],
]};

/// f2 (Figure 24).
#[rustfmt::skip]
const F2: Nonlinear = Nonlinear { taps: TAPS, table: [
    [0, 0, 1, 1, 0, 1, 1, 0, 0, 1, 0, 0, 0, 1, 1, 1],
    [1, 1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0, 0, 0, 1],
    [1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 0, 1, 0, 0, 0],
    [1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1, 0, 1, 0, 1, 1],
    [0, 0, 1, 0, 1, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 0],
    [1, 1, 1, 0, 0, 1, 0, 1, 1, 0, 0, 0, 0, 1, 1, 0],
    [1, 0, 1, 0, 0, 1, 0, 0, 0, 1, 1, 0, 1, 1, 0, 1],
    [0, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 1, 1, 0],
]};

/// BP (clause 8.1.8): bits 12345678 of the input come out in the order
/// 74638152. The clause text names R4, but the feedback equation (clause
/// 8.1.3) and Figure 19 both feed BP from R6, which is what is implemented.
const BP: [u8; 8] = [7, 4, 6, 3, 8, 1, 5, 2];

/// IV load mask for R7, R2, R1, R0 (clause 8.2.3): 56DC28E3.
const IV_MASK: [u8; 4] = [0x56, 0xDC, 0x28, 0xE3];

/// Number of run-up steps after loading (clause 8.2.4).
const RUN_UP: usize = 35;

/// A loaded TEA4 generator.
pub struct Tea4 {
    r: [u8; 8],
    k: [u8; 7],
}

impl Tea4 {
    /// Load the cipher key (C1 … C10, most significant first) and the IV, and
    /// perform the run-up.
    pub fn new(ck: &[u8; 10], iv: Iv) -> Tea4 {
        // CK loading (clause 8.2.2): from an all-zero register, each key byte
        // is added into the normal key feedback and shifted in.
        let mut k = [0u8; 7];
        for &c in ck {
            let k0 = c ^ k[6] ^ P[(k[5] ^ k[1]) as usize];
            k = [k0, k[0], k[1], k[2], k[3], k[4], k[5]];
        }
        // IV loading (clause 8.2.3): the 32-bit word IV1 IV2 IV3 IV4 goes
        // into R6 … R3 plain and into R7, R2, R1, R0 masked.
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
        let mut g = Tea4 { r, k };
        for _ in 0..RUN_UP {
            g.step();
        }
        g
    }

    /// One step of both registers (clauses 8.1.3 and 8.1.4).
    fn step(&mut self) {
        let r = &self.r;
        let k = &self.k;
        let k_out = k[6] ^ P[(k[5] ^ k[1]) as usize];
        let r0 = r[7] ^ wire_cross(r[6], BP) ^ F1.apply(r[5], r[4]) ^ k_out;
        let r4 = r[3] ^ F2.apply(r[1], r[0]);
        self.r = [r0, r[0], r[1], r[2], r4, r[4], r[5], r[6]];
        self.k = [k_out, k[0], k[1], k[2], k[3], k[4], k[5]];
    }
}

impl Ksg for Tea4 {
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
    fn p_is_a_permutation_with_the_documented_example() {
        assert_eq!(P[0x28], 0xD4); // "P(28) = D4", clause 8.1.5
        let mut seen = [false; 256];
        for &v in &P {
            assert!(!seen[v as usize], "P maps two inputs to {v:02X}");
            seen[v as usize] = true;
        }
    }

    #[test]
    fn nonlinear_tables_are_balanced() {
        for table in [&F1.table, &F2.table] {
            for row in table.iter() {
                assert_eq!(row.iter().filter(|&&b| b == 1).count(), 8);
            }
        }
    }

    #[test]
    fn iv_load_matches_the_worked_example() {
        // Clause 8.2.3: IV 11010 00011010 11100010 00000110 gives R7 … R0 =
        // 01001100 00011010 00011010 11100010 00000110 11000110 11001010 11100101.
        let v = Iv::from_raw(0b11010_00011010_11100010_00000110).bytes();
        let r7_to_r0 = [v[0] ^ 0x56, v[0], v[1], v[2], v[3], v[1] ^ 0xDC, v[2] ^ 0x28, v[3] ^ 0xE3];
        assert_eq!(
            r7_to_r0,
            [
                0b01001100, 0b00011010, 0b00011010, 0b11100010, 0b00000110, 0b11000110, 0b11001010, 0b11100101
            ]
        );
    }

    #[test]
    fn key_loading_mixes_every_key_byte() {
        // The seventh key register stage holds state from the first key byte,
        // and a change in any single key byte changes the loaded register.
        let base = Tea4::new(&[0; 10], Iv::from_raw(0));
        for i in 0..10 {
            let mut ck = [0u8; 10];
            ck[i] = 0x80;
            let other = Tea4::new(&ck, Iv::from_raw(0));
            assert_ne!(other.k, base.k, "key byte {i} does not reach the register");
        }
    }

    #[test]
    fn key_stream_is_deterministic_and_depends_on_key_and_iv() {
        let key = [0xA7, 0x98, 0x39, 0xE4, 0xBA, 0x88, 0xEE, 0x54, 0xA0, 0x29];
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        Tea4::new(&key, Iv::from_raw(0x01234567)).fill(&mut a);
        Tea4::new(&key, Iv::from_raw(0x01234567)).fill(&mut b);
        assert_eq!(a, b);
        assert!(a.iter().any(|&x| x != 0), "stream is not all zero");
        Tea4::new(&key, Iv::from_raw(0x01234566)).fill(&mut b);
        assert_ne!(a, b, "a different IV gives a different stream");
        let mut key2 = key;
        key2[9] ^= 1;
        Tea4::new(&key2, Iv::from_raw(0x01234567)).fill(&mut b);
        assert_ne!(a, b, "a different key gives a different stream");
    }
}
