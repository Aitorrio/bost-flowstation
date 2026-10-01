//! TA61, the encrypted short identity (ESI) algorithm: ETSI TS 104 053-3 clause 5.12, used by
//! TS 100 392-7 clause 4.2.6 to replace the SSI in every encrypted MAC header.
//!
//! TA61 runs the TAA1 block cipher BC (a 16-round Feistel cipher on 64-bit blocks with a 128-bit
//! key) once per key, then mixes the 24-bit identity with three 24-bit strings taken from its
//! output. The block cipher and its key schedule follow Midnight Blue's independent
//! implementation (github.com/MidnightBlueLabs/TETRA_crypto, Apache-2.0), written here
//! byte-wise so the result does not depend on the CPU's byte order.

/// The 8-bit S-box of BC.
const SBOX: [u8; 256] = [
    0xF4, 0x65, 0x01, 0x00, 0xBA, 0x7A, 0xA7, 0x47, 0x98, 0xDD, 0x9D, 0xAD, 0x96, 0x5D, 0xAA, 0x3D, //
    0x58, 0xC0, 0x72, 0xD8, 0x66, 0x4C, 0x3E, 0xE0, 0x80, 0x55, 0xDE, 0x90, 0x2A, 0x4B, 0x83, 0xA0, //
    0x51, 0x39, 0xED, 0x6C, 0x8A, 0x2C, 0x56, 0x60, 0x4A, 0x1F, 0xD0, 0x70, 0x6E, 0x33, 0x8B, 0x26, //
    0x2E, 0x6F, 0x89, 0x48, 0x5E, 0x40, 0xC3, 0xA4, 0xA9, 0xCF, 0x22, 0x50, 0xE1, 0x15, 0x0C, 0xAB, //
    0xD5, 0xF8, 0x5F, 0x36, 0x04, 0xA6, 0x4E, 0x92, 0x1E, 0x2B, 0x88, 0x30, 0x93, 0x45, 0x67, 0x16, //
    0x8C, 0x68, 0x23, 0x38, 0x61, 0x25, 0x1A, 0x81, 0x63, 0xCB, 0xC1, 0x13, 0x41, 0x37, 0x0E, 0x97, //
    0x5B, 0xCA, 0x57, 0x24, 0x4D, 0x17, 0xC4, 0xB9, 0xB3, 0xEF, 0x8D, 0x52, 0x32, 0x2F, 0xEC, 0x20, //
    0xD9, 0x11, 0xD1, 0x28, 0x79, 0xDA, 0xFB, 0xE9, 0xBB, 0x06, 0x77, 0xDB, 0xFC, 0xFE, 0xCD, 0x84, //
    0x1D, 0xA1, 0x54, 0x1B, 0xB0, 0xE4, 0xCC, 0x7C, 0x2D, 0x27, 0x31, 0x49, 0xF5, 0x02, 0x69, 0x53, //
    0x4F, 0x44, 0xDF, 0x18, 0x5C, 0x0F, 0xBC, 0x9B, 0x94, 0xBD, 0xDC, 0x0B, 0xA2, 0xC7, 0x09, 0xAC, //
    0xC6, 0x9F, 0x82, 0x1C, 0x05, 0x46, 0xC2, 0x34, 0x3C, 0x0D, 0x3B, 0xCE, 0xB7, 0xBE, 0x08, 0x9C, //
    0x6B, 0xEE, 0xE5, 0x87, 0xAF, 0xBF, 0xF2, 0xEB, 0x7B, 0x07, 0x64, 0xC5, 0xB6, 0xAE, 0x9A, 0x95, //
    0x35, 0xA5, 0x59, 0x12, 0x9E, 0xA3, 0xB8, 0x8E, 0x5A, 0xF7, 0x62, 0xD2, 0x3A, 0xA8, 0x7D, 0x85, //
    0xF6, 0xC8, 0x71, 0x29, 0xD6, 0xD7, 0x43, 0xF9, 0x78, 0x76, 0x73, 0x10, 0x91, 0x19, 0x0A, 0x99, //
    0xF0, 0xE6, 0x3F, 0x14, 0xF1, 0xE2, 0xB1, 0x86, 0xB4, 0xF3, 0x74, 0xFA, 0x6A, 0xB2, 0x21, 0x6D, //
    0xEA, 0xB5, 0xE7, 0xE3, 0xC9, 0xD3, 0x8F, 0x03, 0x75, 0xE8, 0xD4, 0x42, 0xFD, 0x7E, 0xFF, 0x7F, //
];

/// The inverse of [`SBOX`], built at compile time.
const INV_SBOX: [u8; 256] = {
    let mut inv = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        inv[SBOX[i] as usize] = i as u8;
        i += 1;
    }
    inv
};

/// Byte rotation applied to the previous round key to form round keys 1..=15.
const ROTATIONS: [usize; 16] = [0, 5, 5, 5, 5, 3, 7, 5, 5, 5, 5, 7, 3, 5, 5, 5];
/// XORed into every derived round key.
const ROUND_CONSTANT: [u8; 16] = [0x3C, 0xA7, 0xEC, 0x25, 0x79, 0x57, 0xDF, 0xC0, 0x38, 0x0A, 0x33, 0x1E, 0xF3, 0x8C, 0xF4, 0xF7];

/// The BC block cipher with its 16 round keys (only bytes 4..=15 of each are used).
struct Bc {
    round_keys: [[u8; 16]; 16],
}

impl Bc {
    fn new(key: &[u8; 16]) -> Self {
        let mut round_keys = [[0u8; 16]; 16];
        round_keys[0] = *key;
        for i in 1..16 {
            let prev = round_keys[i - 1];
            round_keys[i] = std::array::from_fn(|j| prev[(j + ROTATIONS[i]) & 0xF] ^ ROUND_CONSTANT[j]);
        }
        Bc { round_keys }
    }

    /// Round function: a chain of S-box lookups over the right half and the round key; the last
    /// eight lookups each contribute one bit to every output byte.
    fn f(rhs: &[u8; 4], rk: &[u8; 16]) -> [u8; 4] {
        let mut s = SBOX[rhs[3].wrapping_add(rk[15]) as usize];
        s = SBOX[(rhs[2].wrapping_add(rk[14]) ^ s) as usize];
        s = SBOX[(rhs[1].wrapping_add(rk[13]) ^ s) as usize];
        s = SBOX[(rhs[0].wrapping_add(rk[12]) ^ s) as usize];
        // (right-half byte, round-key byte) for output bits 0..=7.
        const TAPS: [(usize, usize); 8] = [(3, 11), (1, 10), (2, 9), (0, 8), (1, 7), (3, 6), (0, 5), (2, 4)];
        let mut out = [0u8; 4];
        for (bit, &(r, k)) in TAPS.iter().enumerate() {
            s = SBOX[(rhs[r].wrapping_add(rk[k]) ^ s) as usize];
            for (byte, o) in out.iter_mut().enumerate() {
                *o |= ((s >> (3 - byte)) & 1) << bit;
            }
        }
        out
    }

    fn encrypt(&self, block: &[u8; 8]) -> [u8; 8] {
        let mut lhs: [u8; 4] = block[0..4].try_into().unwrap();
        let mut rhs: [u8; 4] = block[4..8].try_into().unwrap();
        for rk in &self.round_keys {
            let mut t = Self::f(&rhs, rk);
            for (t, l) in t.iter_mut().zip(lhs) {
                *t ^= l;
            }
            lhs = rhs;
            rhs = t;
        }
        let mut out = [0u8; 8];
        out[0..4].copy_from_slice(&rhs);
        out[4..8].copy_from_slice(&lhs);
        out
    }
}

/// EXP4 (clause 5.4.4): expands an 80-bit key to the 128-bit BC key.
fn exp4(k: &[u8; 10]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for i in 0..5 {
        out[1 + 3 * i] = k[i].wrapping_add(k[9 - i]);
        out[2 + 3 * i] = k[i];
        out[3 + 3 * i] = k[9 - i];
    }
    out[0] = out[1] ^ out[4] ^ out[7] ^ out[10] ^ out[13];
    out
}

/// Permutation P on three bytes (clause 5.12.4).
fn p(x: [u8; 3]) -> [u8; 3] {
    let idx = |a: u8, b: u8, c: u8| a.wrapping_add(b).wrapping_shl(1).wrapping_sub(c);
    [SBOX[idx(x[1], x[0], x[2]) as usize], SBOX[idx(x[2], x[0], x[1]) as usize], SBOX[idx(x[2], x[1], x[0]) as usize]]
}

/// The inverse of [`p`].
fn p_inv(x: [u8; 3]) -> [u8; 3] {
    let [a, b, c] = x.map(|v| INV_SBOX[v as usize]);
    let lin = |a: u8, b: u8, c: u8| a.wrapping_mul(114).wrapping_add(b.wrapping_mul(114)).wrapping_sub(c.wrapping_mul(57));
    [lin(a, b, c), lin(a, c, b), lin(b, c, a)]
}

/// TA61 for one cipher key (SCK in class 2). The BC work is done once; each identity then costs a
/// few table lookups, so the BS can keep an ESI table for every identity it serves.
pub struct Ta61 {
    /// K-strings (clause 5.12.3): the three 24-bit masks taken from the BC output.
    k: [[u8; 3]; 3],
}

impl Ta61 {
    pub fn new(key: &[u8; 10]) -> Self {
        let bc = Bc::new(&exp4(key));
        let shrunk: [u8; 8] = std::array::from_fn(|i| key[i] ^ key[i + 2]);
        let c = bc.encrypt(&shrunk);
        Ta61 {
            k: [[c[0], c[3], c[6]], [c[1], c[4], c[7]], [c[2], c[5], c[0]]],
        }
    }

    /// SSI → ESI.
    pub fn encrypt(&self, ssi: u32) -> u32 {
        let mut x = to_bytes(ssi);
        x = xor(x, self.k[0]);
        x = p(x);
        x = xor(x, self.k[1]);
        x = p(x);
        from_bytes(xor(x, self.k[2]))
    }

    /// ESI → SSI.
    pub fn decrypt(&self, esi: u32) -> u32 {
        let mut x = xor(to_bytes(esi), self.k[2]);
        x = p_inv(x);
        x = xor(x, self.k[1]);
        x = p_inv(x);
        from_bytes(xor(x, self.k[0]))
    }
}

fn xor(a: [u8; 3], b: [u8; 3]) -> [u8; 3] {
    [a[0] ^ b[0], a[1] ^ b[1], a[2] ^ b[2]]
}

fn to_bytes(v: u32) -> [u8; 3] {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

fn from_bytes(b: [u8; 3]) -> u32 {
    (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Operator-supplied vectors, also reproduced by MidnightBlueLabs/TETRA_crypto `ta61()`.
    #[test]
    fn supplied_vectors() {
        let cases: [([u8; 10], u32, u32); 4] = [
            ([0xC6, 0x2E, 0x22, 0x85, 0x03, 0x40, 0xBC, 0xEB, 0x55, 0x52], 0x565A72, 0xC44853),
            ([0x77, 0xE7, 0x9F, 0xEE, 0x7F, 0xC6, 0x54, 0xDC, 0x65, 0x44], 0x000000, 0x019887),
            ([0x4E, 0xBB, 0x68, 0x9D, 0x87, 0x4A, 0xD6, 0x41, 0x79, 0x05], 0x935E49, 0xEF70E4),
            ([0x67, 0xFB, 0x13, 0x4D, 0xD7, 0x9C, 0x7D, 0x77, 0xF5, 0x2A], 0xB824FF, 0xE69AD6),
        ];
        for (key, ssi, esi) in cases {
            let t = Ta61::new(&key);
            assert_eq!(t.encrypt(ssi), esi, "SSI {ssi:06X}");
            assert_eq!(t.decrypt(esi), ssi, "ESI {esi:06X}");
        }
    }

    /// Generated with MidnightBlueLabs/TETRA_crypto `ta61()` from random keys and identities.
    #[test]
    fn reference_vectors() {
        let cases: [([u8; 10], u32, u32); 6] = [
            ([0xE4, 0x62, 0xA5, 0x1C, 0x2E, 0xE6, 0x86, 0x88, 0x52, 0xF5], 0x780AD8, 0x710BB6),
            ([0xC9, 0x1D, 0xB2, 0x57, 0x87, 0xD4, 0x95, 0x7D, 0x61, 0x9F], 0x47DF6B, 0x48AFB7),
            ([0xD6, 0x15, 0x81, 0x3E, 0x8D, 0x07, 0x25, 0x4F, 0xC6, 0x99], 0x540D57, 0x68C633),
            ([0x33, 0x2D, 0xBC, 0x9A, 0x6E, 0xF0, 0x14, 0xD6, 0x05, 0x4A], 0x98709D, 0x5B1392),
            ([0x5E, 0xB8, 0xD1, 0x35, 0x98, 0xA6, 0x16, 0x6B, 0x30, 0xF8], 0x7B7480, 0x9C80F9),
            ([0x02, 0x85, 0xD9, 0x07, 0xF1, 0xF1, 0xF2, 0x39, 0xA8, 0x5F], 0x230C5A, 0x99EEA2),
        ];
        for (key, ssi, esi) in cases {
            let t = Ta61::new(&key);
            assert_eq!(t.encrypt(ssi), esi, "SSI {ssi:06X}");
            assert_eq!(t.decrypt(esi), ssi, "ESI {esi:06X}");
        }
    }

    #[test]
    fn sbox_is_a_permutation() {
        for i in 0..=255u8 {
            assert_eq!(INV_SBOX[SBOX[i as usize] as usize], i);
        }
    }

    #[test]
    fn p_inv_undoes_p() {
        for v in [[0, 0, 0], [1, 2, 3], [0xFF, 0x80, 0x7F], [0x12, 0xAB, 0xFE]] {
            assert_eq!(p_inv(p(v)), v);
        }
    }

    #[test]
    fn esi_is_a_bijection_on_a_sample() {
        let t = Ta61::new(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        for ssi in (0..0xFF_FFFF).step_by(9973) {
            assert_eq!(t.decrypt(t.encrypt(ssi)), ssi);
        }
    }
}
