//! AES-CMAC (RFC 4493) — message authentication built on the AES block cipher
//! in [`super::aes`]. Used by the E2EE SDS layer for an encrypt-then-MAC
//! integrity tag, so a wrong key or a tampered ciphertext is detected rather
//! than silently decrypted to garbage. Pinned by the RFC 4493 test vectors.

use super::aes::Aes;

const RB: u8 = 0x87; // RFC 4493 constant for the 128-bit block.

/// Left-shift a 16-byte block by one bit (big-endian), returning the carry out.
fn shift_left(block: &[u8; 16]) -> ([u8; 16], u8) {
    let mut out = [0u8; 16];
    let mut carry = 0u8;
    for i in (0..16).rev() {
        out[i] = (block[i] << 1) | carry;
        carry = block[i] >> 7;
    }
    (out, carry)
}

/// Derive the CMAC subkeys K1, K2 (RFC 4493 §2.3).
fn subkeys(aes: &Aes) -> ([u8; 16], [u8; 16]) {
    let mut l = [0u8; 16];
    aes.encrypt_block(&mut l);
    let (mut k1, carry) = shift_left(&l);
    if carry != 0 {
        k1[15] ^= RB;
    }
    let (mut k2, carry) = shift_left(&k1);
    if carry != 0 {
        k2[15] ^= RB;
    }
    (k1, k2)
}

/// Full AES-CMAC over `msg`, returning the 16-byte tag (RFC 4493 §2.4).
pub fn cmac(aes: &Aes, msg: &[u8]) -> [u8; 16] {
    let (k1, k2) = subkeys(aes);

    let n = msg.len().div_ceil(16);
    let (n, last_complete) = if n == 0 { (1, false) } else { (n, msg.len() % 16 == 0) };

    // Final block: either M_n ^ K1 (complete) or (M_n || 10*) ^ K2 (padded).
    let mut last = [0u8; 16];
    let last_start = (n - 1) * 16;
    if last_complete {
        last.copy_from_slice(&msg[last_start..]);
        for i in 0..16 {
            last[i] ^= k1[i];
        }
    } else {
        let rem = &msg[last_start..];
        last[..rem.len()].copy_from_slice(rem);
        last[rem.len()] = 0x80; // padding
        for i in 0..16 {
            last[i] ^= k2[i];
        }
    }

    let mut x = [0u8; 16];
    for i in 0..n - 1 {
        for j in 0..16 {
            x[j] ^= msg[i * 16 + j];
        }
        aes.encrypt_block(&mut x);
    }
    for j in 0..16 {
        x[j] ^= last[j];
    }
    aes.encrypt_block(&mut x);
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn aes() -> Aes {
        Aes::new(&hex("2b7e151628aed2a6abf7158809cf4f3c")).unwrap()
    }

    // RFC 4493 §4 worked examples (AES-128).
    #[test]
    fn cmac_len0() {
        assert_eq!(cmac(&aes(), &[]).to_vec(), hex("bb1d6929e95937287fa37d129b756746"));
    }

    #[test]
    fn cmac_len16() {
        let m = hex("6bc1bee22e409f96e93d7e117393172a");
        assert_eq!(cmac(&aes(), &m).to_vec(), hex("070a16b46b4d4144f79bdd9dd04a287c"));
    }

    #[test]
    fn cmac_len40() {
        let m = hex(
            "6bc1bee22e409f96e93d7e117393172a\
             ae2d8a571e03ac9c9eb76fac45af8e51\
             30c81c46a35ce411",
        );
        assert_eq!(cmac(&aes(), &m).to_vec(), hex("dfa66747de9ae63030ca32611497c827"));
    }

    #[test]
    fn cmac_len64() {
        let m = hex(
            "6bc1bee22e409f96e93d7e117393172a\
             ae2d8a571e03ac9c9eb76fac45af8e51\
             30c81c46a35ce411e5fbc1191a0a52ef\
             f69f2445df4f9b17ad2b417be66c3710",
        );
        assert_eq!(cmac(&aes(), &m).to_vec(), hex("51f0bebf7e3b9d92fc49741779363cfe"));
    }
}
