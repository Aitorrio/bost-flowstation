//! TAA1 — the TETRA authentication and key management algorithm set
//! (ETSI TS 104 053-3 clause 5), built on the HURDLE-II block cipher.
//!
//! * `TA11` / `TA41`: session authentication key KS from K and the random seed RS
//! * `TA21`: the infrastructure-side session key KS' from K and RS
//! * `TA12` / `TA22`: response RES and derived cipher key DCK from KS and RAND
//!
//! * `TB4`: DCK from DCK1 and DCK2; `TB5`: ECK from a cipher key and the cell identity
//! * `TA61`: encrypted short identity (ESI) and its inverse
//!
//! Still to come: `TA31`/`TA32` (CCK sealing), `TA51`/`TA52` (SCK),
//! `TA71` (modified GCK), `TA81`/`TA82`, `TA91`/`TA92`, and
//! the remaining `TB` combining functions.
//!
//! Byte strings are written as in the specification, most significant byte
//! first: a 128-bit value `[u8; 16]` has B15 at index 0 and B0 at index 15.

pub mod hurdle;

pub mod otar;
pub use otar::{SSCK_LEN, ta51, ta52};

use hurdle::Hurdle;

/// Basic block structure BL1 (clause 5.2): the left 64-bit block is
/// enciphered under K, XORed into the right block, and that is enciphered
/// again — two-block CBC with a zero IV.
pub fn bl1(key: &[u8; 16], data: &[u8; 16]) -> [u8; 16] {
    let bc = Hurdle::new(key);
    let left: [u8; 8] = data[..8].try_into().expect("8 bytes");
    let right: [u8; 8] = data[8..].try_into().expect("8 bytes");
    let c1 = bc.encrypt(&left);
    let mut mixed = [0u8; 8];
    for i in 0..8 {
        mixed[i] = right[i] ^ c1[i];
    }
    let c2 = bc.encrypt(&mixed);
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&c1);
    out[8..].copy_from_slice(&c2);
    out
}

/// Expansion EXP1 (clause 5.4.1): 80 → 120 bits. Each adjacent pair of bytes
/// is followed by their XOR: B9 B8 A B7 B6 C … B1 B0 F.
pub fn exp1(b: &[u8; 10]) -> [u8; 15] {
    let mut out = [0u8; 15];
    for p in 0..5 {
        out[3 * p] = b[2 * p];
        out[3 * p + 1] = b[2 * p + 1];
        out[3 * p + 2] = b[2 * p] ^ b[2 * p + 1];
    }
    out
}

/// Expansion EXP2 (clause 5.4.2): EXP1 followed by G = A + C + D + E + F
/// mod 256 appended on the right.
pub fn exp2(b: &[u8; 10]) -> [u8; 16] {
    let e = exp1(b);
    let g = e[2].wrapping_add(e[5]).wrapping_add(e[8]).wrapping_add(e[11]).wrapping_add(e[14]);
    let mut out = [0u8; 16];
    out[..15].copy_from_slice(&e);
    out[15] = g;
    out
}

/// Expansion EXP3 (clause 5.4.3): 88 → 120 bits, B10 B9 A B8 B7 B6 C B5 B4
/// B3 D B2 B1 B0 E with A = B10⊕B9, C = B8⊕B7⊕B6, D = B5⊕B4⊕B3, E = B2⊕B1⊕B0.
pub fn exp3(b: &[u8; 11]) -> [u8; 15] {
    [
        b[0],
        b[1],
        b[0] ^ b[1],
        b[2],
        b[3],
        b[4],
        b[2] ^ b[3] ^ b[4],
        b[5],
        b[6],
        b[7],
        b[5] ^ b[6] ^ b[7],
        b[8],
        b[9],
        b[10],
        b[8] ^ b[9] ^ b[10],
    ]
}

/// Expansion EXP4 (clause 5.4.4), for 80-bit keys → 128-bit cipher keys. The
/// bytes are paired outside-in — (B9, B0), (B8, B1), … (B5, B4) — each pair
/// preceded by its sum mod 256, and the XOR of the five sums put in front:
/// G A B9 B0 C B8 B1 D B7 B2 E B6 B3 F B5 B4.
pub fn exp4(b: &[u8; 10]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let mut g = 0u8;
    for p in 0..5 {
        let (hi, lo) = (b[p], b[9 - p]);
        let sum = hi.wrapping_add(lo);
        g ^= sum;
        out[1 + 3 * p] = sum;
        out[2 + 3 * p] = hi;
        out[3 + 3 * p] = lo;
    }
    out[0] = g;
    out
}

/// Shrinking SHR1 (clause 5.5.1): keep bytes B14 B13 B11 B10 B8 B7 B5 B4 B2 B1
/// of a 120-bit block (every pair, dropping each pair's XOR byte).
pub fn shr1(b: &[u8; 15]) -> [u8; 10] {
    let mut out = [0u8; 10];
    for p in 0..5 {
        out[2 * p] = b[3 * p];
        out[2 * p + 1] = b[3 * p + 1];
    }
    out
}

/// Shrinking SHR2 (clause 5.5.2): keep B14 B13 B11 B10 B9 B7 B6 B5 B3 B2 B1
/// of a 120-bit block (the inverse of EXP3).
pub fn shr2(b: &[u8; 15]) -> [u8; 11] {
    [b[0], b[1], b[3], b[4], b[5], b[7], b[8], b[9], b[11], b[12], b[13]]
}

/// Shrinking SHR3 (clause 5.5.3): the middle 80 bits of a 128-bit block.
pub fn shr3(b: &[u8; 16]) -> [u8; 10] {
    b[3..13].try_into().expect("10 bytes")
}

/// TA11 (and TA41): session authentication key KS = BL1_K(EXP2(RS))
/// (clause 5.6). `k` is the subscriber's authentication key K, `rs` the
/// random seed; TA41 is the same function used for the ESI key.
pub fn ta11(k: &[u8; 16], rs: &[u8; 10]) -> [u8; 16] {
    bl1(k, &exp2(rs))
}

/// TA41 is identical to TA11 (clause 5.6.0).
pub fn ta41(k: &[u8; 16], rs: &[u8; 10]) -> [u8; 16] {
    ta11(k, rs)
}

/// TA21: the infrastructure-authentication session key KS' — as TA11, but
/// with the bytes of RS reversed first (clause 5.6.2).
pub fn ta21(k: &[u8; 16], rs: &[u8; 10]) -> [u8; 16] {
    let mut rev = [0u8; 10];
    for i in 0..10 {
        rev[i] = rs[9 - i];
    }
    bl1(k, &exp2(&rev))
}

/// Result of TA12 / TA22: the 32-bit response and the 80-bit derived cipher key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthResult {
    /// RES1 (TA12) or RES2 (TA22) — what the challenged side sends back.
    pub res: [u8; 4],
    /// DCK1 (TA12) or DCK2 (TA22) — half of the derived cipher key.
    pub dck: [u8; 10],
}

/// TA12 (and TA22): from the session key KS and the challenge RAND, the
/// response and the derived cipher key (clause 5.7). With B15 … B0 the
/// output of BL1: RES = B15⊕B12 ‖ B9 ‖ B6 ‖ B3⊕B0 and DCK is the ten
/// remaining bytes B14 B13 B11 B10 B8 B7 B5 B4 B2 B1.
pub fn ta12(ks: &[u8; 16], rand: &[u8; 10]) -> AuthResult {
    let b = bl1(ks, &exp2(rand));
    AuthResult {
        res: [b[0] ^ b[3], b[6], b[9], b[12] ^ b[15]],
        dck: [b[1], b[2], b[4], b[5], b[7], b[8], b[10], b[11], b[13], b[14]],
    }
}

/// TA22 is identical to TA12 (clause 5.7.0), applied to KS' and RAND2.
pub fn ta22(ks: &[u8; 16], rand: &[u8; 10]) -> AuthResult {
    ta12(ks, rand)
}

/// TB4 (clause 5.22): the derived cipher key DCK from its two halves,
/// DCK = DCK1 ⊕ DCK2. For a one-way authentication the missing half is
/// all zeros (EN 300 392-7 clause 4.2.1).
pub fn tb4(dck1: &[u8; 10], dck2: &[u8; 10]) -> [u8; 10] {
    let mut out = [0u8; 10];
    for i in 0..10 {
        out[i] = dck1[i] ^ dck2[i];
    }
    out
}

/// TB5 (clause 5.23): the encryption cipher key ECK actually loaded into the
/// KSG, binding a cipher key CK to the carrier number CN (12 bits), the
/// colour code CC (6 bits) and the location area LA (14 bits):
///
/// ```text
///   ECK[79..66] = CK ⊕ LA,  then CN, CC, CN, CC, CN, CC, CN down to ECK[0]
/// ```
pub fn tb5(ck: &[u8; 10], cn: u16, la: u16, cc: u8) -> [u8; 10] {
    let cn = u128::from(cn & 0x0FFF);
    let cc = u128::from(cc & 0x3F);
    let la = u128::from(la & 0x3FFF);
    let mask = la << 66 | cn << 54 | cc << 48 | cn << 36 | cc << 30 | cn << 18 | cc << 12 | cn;
    let mut v = 0u128;
    for &b in ck {
        v = v << 8 | u128::from(b);
    }
    v ^= mask;
    let mut out = [0u8; 10];
    for i in (0..10).rev() {
        out[i] = v as u8;
        v >>= 8;
    }
    out
}

/// TA61 (clause 5.12): the encrypted short identity ESI from a cipher key
/// (SCK in class 2, CCK in class 3) and a 24-bit SSI. The key both drives
/// HURDLE-II (expanded with EXP4) and, shrunk to 64 bits, is its data input;
/// the cipher output yields three 24-bit strings K1 K2 K3 that are XORed
/// into the identity around two applications of the permutation P.
pub fn ta61(key: &[u8; 10], ssi: u32) -> u32 {
    let c = ta61_core(key);
    let (k1, k2, k3) = ta61_kstrings(&c);
    let a = xor3(ssi_bytes(ssi), k1);
    let a = xor3(ta61_p(a), k2);
    let a = xor3(ta61_p(a), k3);
    u32::from(a[0]) << 16 | u32::from(a[1]) << 8 | u32::from(a[2])
}

/// Inverse of [`ta61`]: the SSI behind an ESI, for the infrastructure
/// resolving the encrypted addresses radios send.
pub fn ta61_inverse(key: &[u8; 10], esi: u32) -> u32 {
    let c = ta61_core(key);
    let (k1, k2, k3) = ta61_kstrings(&c);
    let a = xor3(ssi_bytes(esi), k3);
    let a = xor3(ta61_p_inverse(a), k2);
    let a = xor3(ta61_p_inverse(a), k1);
    u32::from(a[0]) << 16 | u32::from(a[1]) << 8 | u32::from(a[2])
}

/// The BC output of TA61 (clauses 5.12.1–5.12.2): HURDLE-II under EXP4(key)
/// over the shrunk input (A9⊕A7)(A8⊕A6) … (A2⊕A0).
fn ta61_core(key: &[u8; 10]) -> [u8; 8] {
    let mut data = [0u8; 8];
    for j in 0..8 {
        data[j] = key[j] ^ key[j + 2];
    }
    Hurdle::new(&exp4(key)).encrypt(&data)
}

/// K1 = B7 B4 B1, K2 = B6 B3 B0, K3 = B5 B2 B7 of the BC output (clause 5.12.3).
fn ta61_kstrings(c: &[u8; 8]) -> ([u8; 3], [u8; 3], [u8; 3]) {
    ([c[0], c[3], c[6]], [c[1], c[4], c[7]], [c[2], c[5], c[0]])
}

/// Permutation P on three bytes a2 a1 a0 (clause 5.12.4):
/// S(2a2+2a1−a0) S(2a2+2a0−a1) S(2a1+2a0−a2), arithmetic mod 256.
fn ta61_p(a: [u8; 3]) -> [u8; 3] {
    let (a2, a1, a0) = (a[0] as i32, a[1] as i32, a[2] as i32);
    let s = |x: i32| hurdle::S[(x.rem_euclid(256)) as usize];
    [s(2 * a2 + 2 * a1 - a0), s(2 * a2 + 2 * a0 - a1), s(2 * a1 + 2 * a0 - a2)]
}

/// Inverse of P: undo S, then solve the linear map (its inverse mod 256 is
/// 114·u + 114·v − 57·w and rotations, since the determinant −27 has inverse −19).
fn ta61_p_inverse(a: [u8; 3]) -> [u8; 3] {
    let inv = hurdle::s_inverse();
    let (u, v, w) = (inv[a[0] as usize] as i32, inv[a[1] as usize] as i32, inv[a[2] as usize] as i32);
    let m = |x: i32| x.rem_euclid(256) as u8;
    [
        m(114 * u + 114 * v - 57 * w),
        m(114 * u - 57 * v + 114 * w),
        m(-57 * u + 114 * v + 114 * w),
    ]
}

fn ssi_bytes(ssi: u32) -> [u8; 3] {
    [(ssi >> 16) as u8, (ssi >> 8) as u8, ssi as u8]
}

fn xor3(a: [u8; 3], b: [u8; 3]) -> [u8; 3] {
    [a[0] ^ b[0], a[1] ^ b[1], a[2] ^ b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h<const N: usize>(s: &str) -> [u8; N] {
        let v = hex::decode(s).unwrap();
        v.try_into().unwrap()
    }

    #[test]
    fn expansions_follow_the_figures() {
        let b: [u8; 10] = [9, 8, 7, 6, 5, 4, 3, 2, 1, 0];
        assert_eq!(exp1(&b), [9, 8, 1, 7, 6, 1, 5, 4, 1, 3, 2, 1, 1, 0, 1]);
        let e2 = exp2(&b);
        assert_eq!(
            e2[15],
            (9 ^ 8u8)
                .wrapping_add(7 ^ 6)
                .wrapping_add(5 ^ 4)
                .wrapping_add(3 ^ 2)
                .wrapping_add(1)
        );
        let e4 = exp4(&b);
        assert_eq!(&e4[1..4], &[9, 9, 0]);
        assert_eq!(&e4[13..16], &[5 + 4, 5, 4]);
        assert_eq!(e4[0], 9); // every pair sums to 9, five XORed together
        assert_eq!(shr1(&exp1(&b)), b);
        let b11: [u8; 11] = [10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0];
        assert_eq!(shr2(&exp3(&b11)), b11);
        assert_eq!(shr3(&e4), [e4[3], e4[4], e4[5], e4[6], e4[7], e4[8], e4[9], e4[10], e4[11], e4[12]]);
    }

    #[test]
    fn ta11_and_ta21_match_reference_vectors() {
        // Independent implementation (Midnight Blue, TETRA_crypto, Apache-2.0).
        let cases = [
            (
                "77e79fee7fc654dc6544644fdf476815",
                "00000000000000000000",
                "9C8451A35695D33C3094371202485453",
                "9C8451A35695D33C3094371202485453",
            ),
            (
                "c62e22850340bceb5552222860173d7e",
                "565a72d63cceed0b6f30",
                "77BC47F65C87C1E749B74FDEA6B54661",
                "FCFAF45592DFC65D8A1F5C45DCA293DA",
            ),
            (
                "4ebb689d874ad6417905c0edaa3f90ec",
                "935e49fcdcbb47581955",
                "489C79EA052FDEFA902A833F26CF127C",
                "5E4C241E21915A4807052942AF14ACCD",
            ),
            (
                "67fb134dd79c7d77f52a5dcef23de6fd",
                "b824ffb137a4ef87e07a",
                "B71421BA11CFD54AD6C4D257925A53B2",
                "AD310AEF61B06B2A6C8330C6145B7FEE",
            ),
        ];
        for (k, rs, ks, ksp) in cases {
            assert_eq!(ta11(&h(k), &h(rs)), h::<16>(ks), "TA11 k={k}");
            assert_eq!(ta21(&h(k), &h(rs)), h::<16>(ksp), "TA21 k={k}");
        }
    }

    #[test]
    fn ta12_matches_reference_vectors() {
        // Generated with the reference implementation's ta12_ta22().
        let cases = [
            (
                "9c8451a35695d33c3094371202485453",
                "00000000000000000000",
                "fd12ea2e",
                "dd5425ea9066f2b4b1d3",
            ),
            (
                "77bc47f65c87c1e749b74fdea6b54661",
                "565a72d63cceed0b6f30",
                "6484e85e",
                "d6fc1a70bf5987eb32e3",
            ),
        ];
        for (ks, rand, res, dck) in cases {
            let r = ta12(&h(ks), &h(rand));
            assert_eq!(r.res, h::<4>(res));
            assert_eq!(r.dck, h::<10>(dck));
        }
    }

    #[test]
    fn tb4_and_tb5_match_reference_vectors() {
        let same = h::<10>("0123456789abcdefaabb");
        assert_eq!(tb4(&same, &same), [0; 10]);
        assert_eq!(
            tb4(&h("bdf8e8d47ca2edae0cfb"), &h("563b92c2a2275a0f6113")),
            h::<10>("ebc37a16de85b7a16de8")
        );
        let cases = [
            (0x02BC, 0x1DCC, 0x05, "0123456789abcdefaabb", "7613ea62a26a871ff807"),
            (0x0DE8, 0x3AF0, 0x16, "bdf8e8d47ca2edae0cfb", "563b92c2a2275a0f6113"),
            (0x0DF7, 0x29E2, 0x22, "8a41c56175bfbe356891", "2dcab883aac709eb4566"),
            (0x0757, 0x082E, 0x3F, "ba3e0696e83d16608989", "9a87d3699d42cb3f7ede"),
        ];
        for (cn, la, cc, ck, eck) in cases {
            assert_eq!(tb5(&h(ck), cn, la, cc), h::<10>(eck), "cn {cn:03x}");
        }
    }

    #[test]
    fn ta61_matches_reference_vectors_and_inverts() {
        let cases = [
            ("c62e22850340bceb5552", 0x565a72, 0xC44853),
            ("77e79fee7fc654dc6544", 0x000000, 0x019887),
            ("4ebb689d874ad6417905", 0x935e49, 0xEF70E4),
            ("67fb134dd79c7d77f52a", 0xb824ff, 0xE69AD6),
        ];
        for (key, ssi, esi) in cases {
            let k: [u8; 10] = h(key);
            assert_eq!(ta61(&k, ssi), esi, "key {key}");
            assert_eq!(ta61_inverse(&k, esi), ssi, "inverse, key {key}");
        }
        // P and its inverse are a bijection on all three-byte values we try.
        for a in [[0u8, 0, 0], [1, 2, 3], [255, 128, 7], [0x5a, 0xa5, 0xff]] {
            assert_eq!(ta61_p_inverse(ta61_p(a)), a);
        }
    }
}
