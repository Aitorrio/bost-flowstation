//! TEA1 key stream generator, ETSI TS 104 053-1 V1.2.1 clause 5.
//!
//! The tables below were transcribed from the specification's figures 3, 5 and 6. The byte
//! permutation is checked to be a permutation of all 256 bytes (and P(0x27) = 0x6A, the example in
//! clause 5.1.5), and every S-box of f1 and f2 is checked to be balanced. The specification carries
//! no test vectors; the known-answer tests below come from Midnight Blue's independent
//! implementation (github.com/MidnightBlueLabs/TETRA_crypto, Apache-2.0).
//!
//! Note (clause 5.2.2): TEA1 reduces the 80-bit cipher key to 32 bits of state, so it protects
//! against casual listening only.

/// Byte permutation P (Figure 3): `P[x]`, row = high nibble, column = low nibble.
const P: [u8; 256] = [
    0x9B, 0xF8, 0x3B, 0x72, 0x75, 0x62, 0x88, 0x22, 0xFF, 0xA6, 0x10, 0x4D, 0xA9, 0x97, 0xC3, 0x7B, //
    0x9F, 0x78, 0xF3, 0xB6, 0xA0, 0xCC, 0x17, 0xAB, 0x4A, 0x41, 0x8D, 0x89, 0x25, 0x87, 0xD3, 0xE3, //
    0xCE, 0x47, 0x35, 0x2C, 0x6D, 0xFC, 0xE7, 0x6A, 0xB8, 0xB7, 0xFA, 0x8B, 0xCD, 0x74, 0xEE, 0x11, //
    0x23, 0xDE, 0x39, 0x6C, 0x1E, 0x8E, 0xED, 0x30, 0x73, 0xBE, 0xBB, 0x91, 0xCA, 0x69, 0x60, 0x49, //
    0x5F, 0xB9, 0xC0, 0x06, 0x34, 0x2A, 0x63, 0x4B, 0x90, 0x28, 0xAC, 0x50, 0xE4, 0x6F, 0x36, 0xB0, //
    0xA4, 0xD2, 0xD4, 0x96, 0xD5, 0xC9, 0x66, 0x45, 0xC5, 0x55, 0xDD, 0xB2, 0xA1, 0xA8, 0xBF, 0x37, //
    0x32, 0x2B, 0x3E, 0xB5, 0x5C, 0x54, 0x67, 0x92, 0x56, 0x4C, 0x20, 0x6B, 0x42, 0x9D, 0xA7, 0x58, //
    0x0E, 0x52, 0x68, 0x95, 0x09, 0x7F, 0x59, 0x9C, 0x65, 0xB1, 0x64, 0x5E, 0x4F, 0xBA, 0x81, 0x1C, //
    0xC2, 0x0C, 0x02, 0xB4, 0x31, 0x5B, 0xFD, 0x1D, 0x0A, 0xC8, 0x19, 0x8F, 0x83, 0x8A, 0xCF, 0x33, //
    0x9E, 0x3A, 0x80, 0xF2, 0xF9, 0x76, 0x26, 0x44, 0xF1, 0xE2, 0xC4, 0xF5, 0xD6, 0x51, 0x46, 0x07, //
    0x14, 0x61, 0xF4, 0xC1, 0x24, 0x7A, 0x94, 0x27, 0x00, 0xFB, 0x04, 0xDF, 0x1F, 0x93, 0x71, 0x53, //
    0xEA, 0xD8, 0xBD, 0x3D, 0xD0, 0x79, 0xE6, 0x7E, 0x4E, 0x9A, 0xD7, 0x98, 0x1B, 0x05, 0xAE, 0x03, //
    0xC7, 0xBC, 0x86, 0xDB, 0x84, 0xE8, 0xD1, 0xF7, 0x16, 0x21, 0x6E, 0xE5, 0xCB, 0xA3, 0x1A, 0xEC, //
    0xA2, 0x7D, 0x18, 0x85, 0x48, 0xDA, 0xAA, 0xF0, 0x08, 0xC6, 0x40, 0xAD, 0x57, 0x0D, 0x29, 0x82, //
    0x7C, 0xE9, 0x8C, 0xFE, 0xDC, 0x0F, 0x2D, 0x3C, 0x2E, 0xF6, 0x15, 0x2F, 0xAF, 0xE1, 0xEB, 0x3F, //
    0x99, 0x43, 0x13, 0x0B, 0xE0, 0xA5, 0x12, 0x77, 0x5D, 0xB3, 0x38, 0xD9, 0xEF, 0x5A, 0x01, 0x70, //
];

/// Truth table of f1 (Figure 5): bit `n` of `F1[i]` is the output of S(i+1) for input nibble `n`.
const F1: [u16; 8] = [
    sbox([0, 1, 0, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1]),
    sbox([1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1]),
    sbox([0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 1]),
    sbox([1, 1, 0, 1, 0, 1, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1]),
    sbox([0, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 1, 0, 1, 0, 0]),
    sbox([1, 0, 1, 0, 1, 1, 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]),
    sbox([1, 0, 0, 1, 0, 1, 1, 1, 1, 0, 1, 0, 0, 0, 0, 1]),
    sbox([0, 1, 1, 0, 0, 0, 0, 1, 0, 1, 0, 1, 1, 0, 1, 1]),
];

/// Truth table of f2 (Figure 6), same layout as [`F1`].
const F2: [u16; 8] = [
    sbox([1, 1, 1, 0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1]),
    sbox([1, 1, 0, 1, 0, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1]),
    sbox([0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 1]),
    sbox([0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 0]),
    sbox([1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1]),
    sbox([1, 0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1]),
    sbox([0, 1, 0, 1, 1, 0, 0, 0, 1, 0, 0, 1, 1, 1, 1, 0]),
    sbox([0, 1, 1, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0, 1]),
];

const fn sbox(bits: [u8; 16]) -> u16 {
    let mut v = 0u16;
    let mut n = 0;
    while n < 16 {
        v |= (bits[n] as u16) << n;
        n += 1;
    }
    v
}

/// Expander E inputs (Figure 4): the four bits feeding S1..S8, msb first. Bits 1-8 are byte 1
/// (bit 1 = msb), bits 9-16 are byte 2.
const E: [[u8; 4]; 8] = [
    [7, 8, 9, 10],
    [8, 1, 10, 11],
    [1, 2, 11, 12],
    [2, 3, 12, 13],
    [3, 4, 13, 14],
    [4, 5, 14, 15],
    [5, 6, 15, 16],
    [6, 7, 16, 9],
];

/// Nonlinear function f1 or f2 of two bytes (clauses 5.1.6, 5.1.7): S1 gives the msb of the result.
fn f(table: &[u16; 8], byte1: u8, byte2: u8) -> u8 {
    let word = ((byte1 as u16) << 8) | byte2 as u16;
    let bit = |k: u8| ((word >> (16 - k)) & 1) as usize;
    let mut out = 0u8;
    for (i, inputs) in E.iter().enumerate() {
        let nibble = (bit(inputs[0]) << 3) | (bit(inputs[1]) << 2) | (bit(inputs[2]) << 1) | bit(inputs[3]);
        out |= (((table[i] >> nibble) & 1) as u8) << (7 - i);
    }
    out
}

/// Wire crossing BP (clause 5.1.8): bits 12345678 (1 = msb) become 58417326.
fn bp(x: u8) -> u8 {
    const ORDER: [u8; 8] = [5, 8, 4, 1, 7, 3, 2, 6];
    ORDER
        .iter()
        .enumerate()
        .fold(0, |acc, (i, &src)| acc | (((x >> (8 - src)) & 1) << (7 - i)))
}

/// TEA1 state after CK and IV loading.
pub struct Tea1 {
    r: [u8; 8],
    k: [u8; 4],
}

impl Tea1 {
    /// Loads the (TB5-modified) cipher key and the 29-bit IV and performs the run-up
    /// (clauses 5.2.2 to 5.2.4).
    pub fn new(eck: &[u8; 10], iv: u32) -> Self {
        let mut k = [0u8; 4];
        // CK loading (Figure 2): from 0000, each CK byte is XORed into the K0/K3 feedback.
        for &ck in eck {
            let next = P[(k[3] ^ k[0] ^ ck) as usize];
            k = [next, k[0], k[1], k[2]];
        }
        let [f1, f2, f3, f4] = (iv & 0x1FFF_FFFF).to_be_bytes();
        let r = [f4 ^ 0xA1, f3 ^ 0x4F, f2 ^ 0x72, f4, f3, f2, f1, f1 ^ 0x96];
        let mut s = Tea1 { r, k };
        for _ in 0..53 {
            s.step();
        }
        s
    }

    /// One step of the key register (clause 5.1.4) and the output register (clause 5.1.9).
    fn step(&mut self) {
        let k = &mut self.k;
        let pout = P[(k[3] ^ k[0]) as usize];
        *k = [pout, k[0], k[1], k[2]];

        let r = self.r;
        let r0 = r[7] ^ f(&F2, r[6], r[5]) ^ bp(r[4]) ^ pout;
        let r4 = r[3] ^ f(&F1, r[2], r[1]);
        self.r = [r0, r[0], r[1], r[2], r4, r[4], r[5], r[6]];
    }

    /// Fills `out` with key bytes (clause 5.2.5): one step for the first byte, then 19 steps per
    /// byte, each time taking R7.
    pub fn key_bytes(&mut self, out: &mut [u8]) {
        for (i, byte) in out.iter_mut().enumerate() {
            let steps = if i == 0 { 1 } else { 19 };
            for _ in 0..steps {
                self.step();
            }
            *byte = self.r[7];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_permutation_is_a_permutation() {
        let mut seen = [false; 256];
        for &v in &P {
            assert!(!seen[v as usize], "0x{v:02X} appears twice");
            seen[v as usize] = true;
        }
        assert_eq!(P[0x27], 0x6A, "example of clause 5.1.5");
    }

    #[test]
    fn sboxes_are_balanced() {
        for t in F1.iter().chain(F2.iter()) {
            assert_eq!(t.count_ones(), 8);
        }
    }

    #[test]
    fn expander_uses_every_input_bit_twice() {
        let mut uses = [0u8; 17];
        for inputs in &E {
            for &b in inputs {
                uses[b as usize] += 1;
            }
        }
        assert!(uses[1..].iter().all(|&n| n == 2));
    }

    #[test]
    fn bp_moves_single_bits() {
        // Output bit 1 (msb) comes from input bit 5, output bit 4 from input bit 1.
        assert_eq!(bp(0b0000_1000), 0b1000_0000);
        assert_eq!(bp(0b1000_0000), 0b0001_0000);
        assert_eq!(bp(0xFF), 0xFF);
    }

    #[test]
    fn iv_load_matches_clause_5_2_3_example() {
        // 29-bit counter 11010 00011010 11100010 00000110 → R7..R0 per the example.
        let iv = 0b11010_00011010_11100010_00000110;
        let [f1, f2, f3, f4] = (iv as u32).to_be_bytes();
        let r = [f4 ^ 0xA1, f3 ^ 0x4F, f2 ^ 0x72, f4, f3, f2, f1, f1 ^ 0x96];
        assert_eq!(r[7], 0b1000_1100);
        assert_eq!(r[6], 0b0001_1010);
        assert_eq!(r[3], 0b0000_0110);
        assert_eq!(r[2], 0b0110_1000);
        assert_eq!(r[0], 0b1010_0111);
    }

    /// Known answers from MidnightBlueLabs/TETRA_crypto: the two vectors of its `tests.c`, then
    /// six generated with its `tea1()` (24 key bytes each).
    #[test]
    fn known_answers() {
        let vectors: &[(u32, [u8; 10], &[u8])] = &[
            (0x11111111, [0; 10], &[0xd3, 0x3f, 0xd8, 0xa6, 0x05, 0xa0, 0xa1, 0xbb, 0x90, 0x23]),
            (
                0x01234567,
                [0xA7, 0x98, 0x39, 0xE4, 0xBA, 0x88, 0xEE, 0x54, 0xA0, 0x29],
                &[0x1d, 0xec, 0x9c, 0x7e, 0xc6, 0x22, 0x3d, 0x87, 0xc2, 0xcc],
            ),
            (
                0x0D359898,
                [0xDC, 0x04, 0x65, 0xAA, 0x1F, 0xAD, 0x1D, 0x5A, 0xDA, 0xE5],
                &[
                    0xC3, 0x38, 0x1D, 0xDD, 0xA1, 0x17, 0x31, 0x03, 0x48, 0x47, 0xDD, 0x9C, 0x60, 0xBC, 0xCB, 0x5D, 0x94, 0x55, 0xE2, 0x5F,
                    0x96, 0x18, 0xF5, 0xA6,
                ],
            ),
            (
                0x0B43201D,
                [0x1B, 0x1E, 0x5F, 0x13, 0x70, 0x79, 0x6C, 0xFD, 0x10, 0xFF],
                &[
                    0xC0, 0x91, 0x11, 0x22, 0x03, 0xCC, 0x4C, 0x1F, 0xCD, 0x00, 0x01, 0x58, 0xD8, 0x4E, 0xAA, 0xEF, 0x0D, 0x2C, 0xB9, 0x3C,
                    0xB6, 0x23, 0x29, 0xB1,
                ],
            ),
            (
                0x198F157B,
                [0xAF, 0x60, 0x1D, 0x04, 0xAC, 0xB4, 0x1D, 0x02, 0x2B, 0x46],
                &[
                    0x2A, 0x92, 0x77, 0x6A, 0xC7, 0x2D, 0x43, 0x9B, 0xCD, 0xCE, 0x34, 0x36, 0x0C, 0xBE, 0x76, 0xF2, 0xE0, 0xD4, 0xED, 0xFB,
                    0xCE, 0xC4, 0xDB, 0xE7,
                ],
            ),
            (
                0x163DD111,
                [0x73, 0x3A, 0xF2, 0xDF, 0x5F, 0xAE, 0xB7, 0x08, 0x59, 0xD1],
                &[
                    0xE5, 0x0C, 0xEE, 0xC4, 0x24, 0xE3, 0x6B, 0x02, 0xC9, 0xB0, 0xA9, 0xDD, 0x4F, 0x31, 0xA6, 0xC4, 0xC2, 0xB8, 0xA3, 0xBC,
                    0xFE, 0x61, 0xAF, 0x4E,
                ],
            ),
            (
                0x15FFEE15,
                [0x39, 0x10, 0xCB, 0x48, 0x95, 0xB5, 0xCC, 0x89, 0x29, 0x11],
                &[
                    0x18, 0x3E, 0x63, 0x25, 0x7A, 0x3E, 0x2E, 0xDE, 0x65, 0xCB, 0x32, 0x94, 0x68, 0x8B, 0xD8, 0xD7, 0x44, 0x5E, 0xCF, 0x79,
                    0x1A, 0x93, 0x59, 0xF9,
                ],
            ),
            (
                0x07D28973,
                [0x06, 0xB6, 0x62, 0x2E, 0xDF, 0x3C, 0xF9, 0x35, 0xFD, 0x4B],
                &[
                    0x98, 0x32, 0x56, 0x79, 0x9F, 0xF6, 0x69, 0x7C, 0xEB, 0xFF, 0x9E, 0x0D, 0x1A, 0x16, 0x20, 0xB3, 0x84, 0x2B, 0x03, 0x3A,
                    0x21, 0x76, 0x0C, 0xAF,
                ],
            ),
        ];
        for (iv, key, expected) in vectors {
            let mut ks = vec![0u8; expected.len()];
            Tea1::new(key, *iv).key_bytes(&mut ks);
            assert_eq!(&ks[..], *expected, "IV 0x{iv:08X}");
        }
    }

    #[test]
    fn key_stream_depends_on_key_and_iv() {
        let key = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut a = [0u8; 54];
        let mut b = [0u8; 54];
        let mut c = [0u8; 54];
        Tea1::new(&key, 1).key_bytes(&mut a);
        Tea1::new(&key, 2).key_bytes(&mut b);
        Tea1::new(&[0; 10], 1).key_bytes(&mut c);
        assert_ne!(a, b);
        assert_ne!(a, c);
        let mut again = [0u8; 54];
        Tea1::new(&key, 1).key_bytes(&mut again);
        assert_eq!(a, again);
    }
}
