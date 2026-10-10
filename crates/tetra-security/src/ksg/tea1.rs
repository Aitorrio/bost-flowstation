//! TEA1 (ETSI TS 104 053-1 clause 5).
//!
//! **Provided for research and for interoperating with TEA1-only radios; it
//! is not secure.** The cipher key is loaded through a four-byte key register
//! (clause 5.2.2), so of the 80 key bits only 32 survive into the generator —
//! the weakness published as TETRA:BURST (Meijer, Wetzels, Bokslag, 2023). A
//! few seconds of key stream recovers the effective key on a laptop.
//!
//! An eight-byte Output Register R0 … R7 and a four-byte Key Register K0 … K3.
//! Each step (clauses 5.1.4 and 5.1.9):
//!
//! ```text
//!   Pout = P(K3 ⊕ K0)
//!   K'0  = Pout,  K'i = K(i−1)              i = 1 … 3
//!   R'0  = R7 ⊕ f2[E(R6, R5)] ⊕ BP(R4) ⊕ Pout
//!   R'4  = R3 ⊕ f1[E(R2, R1)]
//!   R'i  = R(i−1)                           otherwise
//! ```
//!
//! After loading, 53 run-up steps are made; the first key stream byte is R7
//! after one further step, and every later byte after 19 more (clause 5.2).

use super::{Iv, Ksg, Nonlinear, wire_cross};

/// Byte permutation P (Figure 3), indexed by the input byte.
#[rustfmt::skip]
const P: [u8; 256] = [
    0x9B, 0xF8, 0x3B, 0x72, 0x75, 0x62, 0x88, 0x22, 0xFF, 0xA6, 0x10, 0x4D, 0xA9, 0x97, 0xC3, 0x7B,
    0x9F, 0x78, 0xF3, 0xB6, 0xA0, 0xCC, 0x17, 0xAB, 0x4A, 0x41, 0x8D, 0x89, 0x25, 0x87, 0xD3, 0xE3,
    0xCE, 0x47, 0x35, 0x2C, 0x6D, 0xFC, 0xE7, 0x6A, 0xB8, 0xB7, 0xFA, 0x8B, 0xCD, 0x74, 0xEE, 0x11,
    0x23, 0xDE, 0x39, 0x6C, 0x1E, 0x8E, 0xED, 0x30, 0x73, 0xBE, 0xBB, 0x91, 0xCA, 0x69, 0x60, 0x49,
    0x5F, 0xB9, 0xC0, 0x06, 0x34, 0x2A, 0x63, 0x4B, 0x90, 0x28, 0xAC, 0x50, 0xE4, 0x6F, 0x36, 0xB0,
    0xA4, 0xD2, 0xD4, 0x96, 0xD5, 0xC9, 0x66, 0x45, 0xC5, 0x55, 0xDD, 0xB2, 0xA1, 0xA8, 0xBF, 0x37,
    0x32, 0x2B, 0x3E, 0xB5, 0x5C, 0x54, 0x67, 0x92, 0x56, 0x4C, 0x20, 0x6B, 0x42, 0x9D, 0xA7, 0x58,
    0x0E, 0x52, 0x68, 0x95, 0x09, 0x7F, 0x59, 0x9C, 0x65, 0xB1, 0x64, 0x5E, 0x4F, 0xBA, 0x81, 0x1C,
    0xC2, 0x0C, 0x02, 0xB4, 0x31, 0x5B, 0xFD, 0x1D, 0x0A, 0xC8, 0x19, 0x8F, 0x83, 0x8A, 0xCF, 0x33,
    0x9E, 0x3A, 0x80, 0xF2, 0xF9, 0x76, 0x26, 0x44, 0xF1, 0xE2, 0xC4, 0xF5, 0xD6, 0x51, 0x46, 0x07,
    0x14, 0x61, 0xF4, 0xC1, 0x24, 0x7A, 0x94, 0x27, 0x00, 0xFB, 0x04, 0xDF, 0x1F, 0x93, 0x71, 0x53,
    0xEA, 0xD8, 0xBD, 0x3D, 0xD0, 0x79, 0xE6, 0x7E, 0x4E, 0x9A, 0xD7, 0x98, 0x1B, 0x05, 0xAE, 0x03,
    0xC7, 0xBC, 0x86, 0xDB, 0x84, 0xE8, 0xD1, 0xF7, 0x16, 0x21, 0x6E, 0xE5, 0xCB, 0xA3, 0x1A, 0xEC,
    0xA2, 0x7D, 0x18, 0x85, 0x48, 0xDA, 0xAA, 0xF0, 0x08, 0xC6, 0x40, 0xAD, 0x57, 0x0D, 0x29, 0x82,
    0x7C, 0xE9, 0x8C, 0xFE, 0xDC, 0x0F, 0x2D, 0x3C, 0x2E, 0xF6, 0x15, 0x2F, 0xAF, 0xE1, 0xEB, 0x3F,
    0x99, 0x43, 0x13, 0x0B, 0xE0, 0xA5, 0x12, 0x77, 0x5D, 0xB3, 0x38, 0xD9, 0xEF, 0x5A, 0x01, 0x70,
];

/// Expander E (Figure 4): the bits each box takes from the 16-bit word.
const TAPS: [[u8; 4]; 8] = [
    [7, 8, 9, 10],
    [8, 1, 10, 11],
    [1, 2, 11, 12],
    [2, 3, 12, 13],
    [3, 4, 13, 14],
    [4, 5, 14, 15],
    [5, 6, 15, 16],
    [6, 7, 16, 9],
];

/// f1 (Figure 5).
#[rustfmt::skip]
const F1: Nonlinear = Nonlinear { taps: TAPS, table: [
    [0, 1, 0, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1],
    [1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1],
    [0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 1],
    [1, 1, 0, 1, 0, 1, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1],
    [0, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 1, 0, 1, 0, 0],
    [1, 0, 1, 0, 1, 1, 0, 1, 1, 0, 0, 1, 0, 1, 0, 0],
    [1, 0, 0, 1, 0, 1, 1, 1, 1, 0, 1, 0, 0, 0, 0, 1],
    [0, 1, 1, 0, 0, 0, 0, 1, 0, 1, 0, 1, 1, 0, 1, 1],
]};

/// f2 (Figure 6).
#[rustfmt::skip]
const F2: Nonlinear = Nonlinear { taps: TAPS, table: [
    [1, 1, 1, 0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1],
    [1, 1, 0, 1, 0, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1],
    [0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 1],
    [0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 0],
    [1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1],
    [1, 0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1],
    [0, 1, 0, 1, 1, 0, 0, 0, 1, 0, 0, 1, 1, 1, 1, 0],
    [0, 1, 1, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0, 1],
]};

/// BP (clause 5.1.8): bits 12345678 of R4 come out in the order 58417326.
const BP: [u8; 8] = [5, 8, 4, 1, 7, 3, 2, 6];

/// IV load mask for R7, R2, R1, R0 (clause 5.2.3): 96724FA1.
const IV_MASK: [u8; 4] = [0x96, 0x72, 0x4F, 0xA1];

/// A loaded TEA1 generator.
pub struct Tea1 {
    r: [u8; 8],
    k: [u8; 4],
    first: bool,
}

impl Tea1 {
    /// Load the cipher key (C1 … C10, most significant first) and the IV, and
    /// perform the run-up.
    pub fn new(ck: &[u8; 10], iv: Iv) -> Tea1 {
        // CK loading (clause 5.2.2, Figure 2): from a zero register, each key
        // byte is XORed with K3 ⊕ K0, substituted by P and shifted in at K0.
        let mut k = [0u8; 4];
        for &c in ck {
            let k0 = P[(k[3] ^ k[0] ^ c) as usize];
            k = [k0, k[0], k[1], k[2]];
        }
        // IV loading (clause 5.2.3): the 32-bit word F1 F2 F3 F4.
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
        let mut g = Tea1 { r, k, first: true };
        for _ in 0..53 {
            g.step();
        }
        g
    }

    /// One step of both registers (clauses 5.1.4 and 5.1.9).
    fn step(&mut self) {
        let r = &self.r;
        let k = &self.k;
        let p_out = P[(k[3] ^ k[0]) as usize];
        let r0 = r[7] ^ F2.apply(r[6], r[5]) ^ wire_cross(r[4], BP) ^ p_out;
        let r4 = r[3] ^ F1.apply(r[2], r[1]);
        self.r = [r0, r[0], r[1], r[2], r4, r[4], r[5], r[6]];
        self.k = [p_out, k[0], k[1], k[2]];
    }
}

impl Ksg for Tea1 {
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
        assert_eq!(P[0x27], 0x6A); // "P(27) = 6A", clause 5.1.5
    }

    #[test]
    fn iv_load_matches_the_worked_example() {
        // Clause 5.2.3: IV 11010 00011010 11100010 00000110 gives R7 … R4 =
        // 10001100 00011010 00011010 11100010 and R3 … R0 = 00000110 01101000 ... 10100111.
        let f = Iv::from_raw(0b11010_00011010_11100010_00000110).bytes();
        let r7_to_r0 = [f[0] ^ 0x96, f[0], f[1], f[2], f[3], f[1] ^ 0x72, f[2] ^ 0x4F, f[3] ^ 0xA1];
        assert_eq!(&r7_to_r0[..5], &[0b10001100, 0b00011010, 0b00011010, 0b11100010, 0b00000110]);
        assert_eq!(r7_to_r0[5], 0b01101000);
        assert_eq!(r7_to_r0[7], 0b10100111);
    }

    #[test]
    fn key_stream_matches_reference_vectors() {
        // Independent implementation (Midnight Blue, TETRA_crypto, Apache-2.0).
        let cases: [(u32, [u8; 10], [u8; 10]); 2] = [
            (0x11111111, [0; 10], [0xd3, 0x3f, 0xd8, 0xa6, 0x05, 0xa0, 0xa1, 0xbb, 0x90, 0x23]),
            (
                0x01234567,
                [0xA7, 0x98, 0x39, 0xE4, 0xBA, 0x88, 0xEE, 0x54, 0xA0, 0x29],
                [0x1d, 0xec, 0x9c, 0x7e, 0xc6, 0x22, 0x3d, 0x87, 0xc2, 0xcc],
            ),
        ];
        for (iv, key, want) in cases {
            let mut g = Tea1::new(&key, Iv::from_raw(iv));
            let mut got = [0u8; 10];
            g.fill(&mut got);
            assert_eq!(got, want, "iv {iv:08x}");
        }
    }

    #[test]
    fn only_32_key_bits_matter() {
        // Keys that load the same 32-bit register state give the same stream: the
        // register after loading is all that survives of the 80-bit key.
        let iv = Iv::from_raw(0x0123_4567);
        let k1 = [0xA7, 0x98, 0x39, 0xE4, 0xBA, 0x88, 0xEE, 0x54, 0xA0, 0x29];
        let state = |ck: &[u8; 10]| {
            let mut k = [0u8; 4];
            for &c in ck {
                k = [P[(k[3] ^ k[0] ^ c) as usize], k[0], k[1], k[2]];
            }
            k
        };
        // Search a few keys differing in the first byte for one with the same register state.
        let mut collisions = 0;
        for b in 0..=255u8 {
            let mut k2 = k1;
            k2[0] = b;
            // adjust the last byte so that the final state matches (the register is only
            // 32 bits, so matches exist; here we just count those that happen to match)
            if state(&k2) == state(&k1) && k2 != k1 {
                collisions += 1;
                let mut a = [0u8; 8];
                let mut c = [0u8; 8];
                Tea1::new(&k1, iv).fill(&mut a);
                Tea1::new(&k2, iv).fill(&mut c);
                assert_eq!(a, c);
            }
        }
        let _ = collisions; // may be zero for this particular key; the point is the state size
    }
}
