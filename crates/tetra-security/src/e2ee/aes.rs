//! AES-128 / AES-256 block cipher and CTR keystream (FIPS-197 + NIST SP 800-38A).
//!
//! Hand-implemented from the published standard — no external crates, no
//! `unsafe` — to match the rest of this crate (TAA1/HURDLE-II, TEA). Used only
//! by the end-to-end encryption layer ([`super`]); it is not an air-interface
//! algorithm. Correctness is pinned by the FIPS-197 and SP 800-38A worked
//! examples in the unit tests.

/// AES S-box (FIPS-197 Fig. 7).
const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// Round constants (FIPS-197 §5.2); enough rounds for AES-256.
const RCON: [u8; 15] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36, 0x6c, 0xd8, 0xab, 0x4d,
];

/// GF(2^8) multiply-by-x (xtime), reducing modulo the AES polynomial 0x11b.
#[inline]
fn xtime(b: u8) -> u8 {
    let hi = b & 0x80;
    let s = b << 1;
    if hi != 0 { s ^ 0x1b } else { s }
}

/// GF(2^8) multiply (only the small constants used by MixColumns are needed).
#[inline]
fn gmul(a: u8, b: u8) -> u8 {
    match b {
        1 => a,
        2 => xtime(a),
        3 => xtime(a) ^ a,
        _ => unreachable!("gmul only used with 1/2/3"),
    }
}

/// An expanded AES key schedule. Supports 128-bit (`Nk`=4, 11 round keys) and
/// 256-bit (`Nk`=8, 15 round keys) keys.
pub struct Aes {
    /// Round keys as 16-byte blocks; `nr + 1` of them are valid.
    round_keys: [[u8; 16]; 15],
    nr: usize,
}

impl Aes {
    /// Build the key schedule for a 16-byte (AES-128) or 32-byte (AES-256) key.
    pub fn new(key: &[u8]) -> Option<Self> {
        let nk = match key.len() {
            16 => 4,
            32 => 8,
            _ => return None,
        };
        let nr = nk + 6; // 10 or 14
        let total_words = 4 * (nr + 1);
        let mut w = [[0u8; 4]; 60]; // max 4*(14+1)=60 words
        for i in 0..nk {
            w[i] = [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]];
        }
        for i in nk..total_words {
            let mut temp = w[i - 1];
            if i % nk == 0 {
                // RotWord, SubWord, XOR Rcon.
                temp = [temp[1], temp[2], temp[3], temp[0]];
                for b in &mut temp {
                    *b = SBOX[*b as usize];
                }
                temp[0] ^= RCON[i / nk];
            } else if nk > 6 && i % nk == 4 {
                // Extra SubWord for AES-256.
                for b in &mut temp {
                    *b = SBOX[*b as usize];
                }
            }
            let prev = w[i - nk];
            w[i] = [prev[0] ^ temp[0], prev[1] ^ temp[1], prev[2] ^ temp[2], prev[3] ^ temp[3]];
        }
        let mut round_keys = [[0u8; 16]; 15];
        for r in 0..=nr {
            for c in 0..4 {
                let word = w[4 * r + c];
                round_keys[r][4 * c..4 * c + 4].copy_from_slice(&word);
            }
        }
        Some(Self { round_keys, nr })
    }

    /// Encrypt one 16-byte block in place (FIPS-197 §5.1 Cipher).
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        add_round_key(block, &self.round_keys[0]);
        for r in 1..self.nr {
            sub_bytes(block);
            shift_rows(block);
            mix_columns(block);
            add_round_key(block, &self.round_keys[r]);
        }
        sub_bytes(block);
        shift_rows(block);
        add_round_key(block, &self.round_keys[self.nr]);
    }

    /// Produce `out.len()` bytes of AES-CTR keystream starting from the 16-byte
    /// initial counter block `iv`, and XOR it into `out` (so this both encrypts
    /// and decrypts). The counter is the full 128-bit block, incremented big-endian.
    pub fn ctr_xor(&self, iv: &[u8; 16], out: &mut [u8]) {
        let mut counter = *iv;
        let mut offset = 0;
        while offset < out.len() {
            let mut ks = counter;
            self.encrypt_block(&mut ks);
            let n = (out.len() - offset).min(16);
            for i in 0..n {
                out[offset + i] ^= ks[i];
            }
            offset += n;
            increment_be(&mut counter);
        }
    }
}

/// Increment a 128-bit big-endian counter block (wraps at the top).
fn increment_be(block: &mut [u8; 16]) {
    for byte in block.iter_mut().rev() {
        let (v, carry) = byte.overflowing_add(1);
        *byte = v;
        if !carry {
            break;
        }
    }
}

fn add_round_key(state: &mut [u8; 16], rk: &[u8; 16]) {
    for i in 0..16 {
        state[i] ^= rk[i];
    }
}

fn sub_bytes(state: &mut [u8; 16]) {
    for b in state.iter_mut() {
        *b = SBOX[*b as usize];
    }
}

/// ShiftRows (FIPS-197 §5.1.2). State is column-major: byte `r + 4c` is row r, col c.
fn shift_rows(state: &mut [u8; 16]) {
    let s = *state;
    for r in 1..4 {
        for c in 0..4 {
            state[r + 4 * c] = s[r + 4 * ((c + r) % 4)];
        }
    }
}

/// MixColumns (FIPS-197 §5.1.3).
fn mix_columns(state: &mut [u8; 16]) {
    for c in 0..4 {
        let col = [state[4 * c], state[4 * c + 1], state[4 * c + 2], state[4 * c + 3]];
        state[4 * c] = gmul(col[0], 2) ^ gmul(col[1], 3) ^ col[2] ^ col[3];
        state[4 * c + 1] = col[0] ^ gmul(col[1], 2) ^ gmul(col[2], 3) ^ col[3];
        state[4 * c + 2] = col[0] ^ col[1] ^ gmul(col[2], 2) ^ gmul(col[3], 3);
        state[4 * c + 3] = gmul(col[0], 3) ^ col[1] ^ col[2] ^ gmul(col[3], 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn aes128_fips197_c1() {
        // FIPS-197 Appendix C.1.
        let key = hex("000102030405060708090a0b0c0d0e0f");
        let mut block = [0u8; 16];
        block.copy_from_slice(&hex("00112233445566778899aabbccddeeff"));
        Aes::new(&key).unwrap().encrypt_block(&mut block);
        assert_eq!(block.to_vec(), hex("69c4e0d86a7b0430d8cdb78070b4c55a"));
    }

    #[test]
    fn aes256_fips197_c3() {
        // FIPS-197 Appendix C.3.
        let key = hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let mut block = [0u8; 16];
        block.copy_from_slice(&hex("00112233445566778899aabbccddeeff"));
        Aes::new(&key).unwrap().encrypt_block(&mut block);
        assert_eq!(block.to_vec(), hex("8ea2b7ca516745bfeafc49904b496089"));
    }

    #[test]
    fn aes128_ctr_nist_sp800_38a_f51() {
        // NIST SP 800-38A F.5.1 CTR-AES128.Encrypt, first two blocks.
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        let iv = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let mut data = hex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        let expected = hex("874d6191b620e3261bef6864990db6ce9806f66b7970fdff8617187bb9fffdff");
        let mut ivb = [0u8; 16];
        ivb.copy_from_slice(&iv);
        Aes::new(&key).unwrap().ctr_xor(&ivb, &mut data);
        assert_eq!(data, expected);
    }

    #[test]
    fn ctr_round_trips() {
        let key = [0x11u8; 16];
        let iv = [0x22u8; 16];
        let plain = b"FlowStation E2EE round-trip test payload, > 2 blocks long!!".to_vec();
        let mut buf = plain.clone();
        let aes = Aes::new(&key).unwrap();
        aes.ctr_xor(&iv, &mut buf);
        assert_ne!(buf, plain);
        aes.ctr_xor(&iv, &mut buf);
        assert_eq!(buf, plain);
    }

    #[test]
    fn rejects_bad_key_len() {
        assert!(Aes::new(&[0u8; 24]).is_none()); // AES-192 not supported here
        assert!(Aes::new(&[0u8; 15]).is_none());
    }
}
