//! End-to-end encryption of an SDS user-data payload.
//!
//! Confidentiality is AES-CTR; integrity is AES-CMAC (encrypt-then-MAC) so a
//! wrong key or a tampered message is rejected rather than silently decrypted.
//! Both use the hand-rolled AES in [`super::aes`]; no external crypto.
//!
//! Wire format of a protected payload (FlowStation E2EE v1 — aligned to SFPG
//! Rec 02 practice, but the exact Rec 02 framing is members-only, so this is
//! interoperable between FlowStation-aware endpoints, not guaranteed with a
//! COTS E2EE radio):
//!
//! ```text
//! 0      marker 0xE2
//! 1      flags: bits0-1 = algorithm (0 AES-128, 1 AES-256); rest reserved 0
//! 2..4   TEK id (u16, big-endian)
//! 4..12  nonce (8 bytes, caller-supplied unique-per-message value)
//! 12..M  ciphertext (AES-CTR, same length as the plaintext)
//! M..M+8 truncated AES-CMAC tag over bytes 0..M
//! ```
//!
//! The caller supplies the nonce so this module stays pure computation (no
//! RNG/I/O) — it MUST be unique per message under a given TEK.

use std::collections::HashMap;

use super::aes::Aes;
use super::cmac::cmac;

const MARKER: u8 = 0xE2;
const HEADER_LEN: usize = 12;
const TAG_LEN: usize = 8;
const NONCE_LEN: usize = 8;

/// E2EE bulk algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E2eeAlg {
    Aes128,
    Aes256,
}

impl E2eeAlg {
    pub fn key_len(self) -> usize {
        match self {
            E2eeAlg::Aes128 => 16,
            E2eeAlg::Aes256 => 32,
        }
    }
    fn code(self) -> u8 {
        match self {
            E2eeAlg::Aes128 => 0,
            E2eeAlg::Aes256 => 1,
        }
    }
    fn from_code(c: u8) -> Option<Self> {
        match c {
            0 => Some(E2eeAlg::Aes128),
            1 => Some(E2eeAlg::Aes256),
            _ => None,
        }
    }
}

/// A Traffic Encryption Key: the end-to-end key shared between endpoints,
/// identified by a 16-bit id carried in each protected message.
#[derive(Clone)]
pub struct Tek {
    pub id: u16,
    pub alg: E2eeAlg,
    key: Vec<u8>,
}

impl Tek {
    /// Build a TEK, checking the key length matches the algorithm.
    pub fn new(id: u16, alg: E2eeAlg, key: Vec<u8>) -> Option<Self> {
        (key.len() == alg.key_len()).then_some(Self { id, alg, key })
    }
    fn aes(&self) -> Aes {
        // Length already validated in `new`.
        Aes::new(&self.key).expect("TEK key length validated at construction")
    }
}

impl std::fmt::Debug for Tek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        f.debug_struct("Tek").field("id", &self.id).field("alg", &self.alg).finish_non_exhaustive()
    }
}

/// A set of TEKs, looked up by id when opening a protected message.
#[derive(Default)]
pub struct TekStore {
    teks: HashMap<u16, Tek>,
}

impl TekStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&mut self, tek: Tek) {
        self.teks.insert(tek.id, tek);
    }
    pub fn get(&self, id: u16) -> Option<&Tek> {
        self.teks.get(&id)
    }
    pub fn remove(&mut self, id: u16) -> bool {
        self.teks.remove(&id).is_some()
    }
    pub fn len(&self) -> usize {
        self.teks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.teks.is_empty()
    }
}

/// Why opening a protected payload failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E2eeError {
    /// Too short to be a protected payload.
    TooShort,
    /// Not an E2EE payload (wrong marker).
    NotProtected,
    /// Reserved/unknown algorithm code in the header.
    BadAlgorithm,
    /// No TEK with the id carried in the header.
    UnknownTek(u16),
    /// The TEK's algorithm differs from the header's.
    AlgorithmMismatch,
    /// Integrity check failed: wrong key or the message was altered.
    AuthFailed,
}

/// Build the 16-byte AES-CTR initial counter from the 8-byte nonce.
fn iv_from_nonce(nonce: &[u8; NONCE_LEN]) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[..NONCE_LEN].copy_from_slice(nonce);
    iv
}

/// Constant-time comparison of two equal-length byte slices.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Is this byte string an E2EE-protected payload (by marker and minimum length)?
pub fn is_protected(bytes: &[u8]) -> bool {
    bytes.len() >= HEADER_LEN + TAG_LEN && bytes[0] == MARKER
}

/// Encrypt and authenticate `plaintext` under `tek`, returning the wire payload.
/// `nonce` must be unique per message under this TEK.
pub fn seal_sds(tek: &Tek, nonce: &[u8; NONCE_LEN], plaintext: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + plaintext.len() + TAG_LEN);
    out.push(MARKER);
    out.push(tek.alg.code());
    out.extend_from_slice(&tek.id.to_be_bytes());
    out.extend_from_slice(nonce);
    let ct_start = out.len();
    out.extend_from_slice(plaintext);

    let aes = tek.aes();
    aes.ctr_xor(&iv_from_nonce(nonce), &mut out[ct_start..]);

    let tag = cmac(&aes, &out);
    out.extend_from_slice(&tag[..TAG_LEN]);
    out
}

/// Verify and decrypt a protected payload, selecting the TEK by the id in its
/// header. Returns the plaintext, or an error (unknown key, bad tag, …).
pub fn open_sds(store: &TekStore, wrapped: &[u8]) -> Result<Vec<u8>, E2eeError> {
    if wrapped.len() < HEADER_LEN + TAG_LEN {
        return Err(E2eeError::TooShort);
    }
    if wrapped[0] != MARKER {
        return Err(E2eeError::NotProtected);
    }
    let alg = E2eeAlg::from_code(wrapped[1] & 0x03).ok_or(E2eeError::BadAlgorithm)?;
    let id = u16::from_be_bytes([wrapped[2], wrapped[3]]);
    let tek = store.get(id).ok_or(E2eeError::UnknownTek(id))?;
    if tek.alg != alg {
        return Err(E2eeError::AlgorithmMismatch);
    }

    let tag_start = wrapped.len() - TAG_LEN;
    let aes = tek.aes();
    let expected = cmac(&aes, &wrapped[..tag_start]);
    if !ct_eq(&expected[..TAG_LEN], &wrapped[tag_start..]) {
        return Err(E2eeError::AuthFailed);
    }

    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&wrapped[4..12]);
    let mut plaintext = wrapped[HEADER_LEN..tag_start].to_vec();
    aes.ctr_xor(&iv_from_nonce(&nonce), &mut plaintext);
    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tek128(id: u16) -> Tek {
        Tek::new(id, E2eeAlg::Aes128, vec![0x42; 16]).unwrap()
    }

    #[test]
    fn round_trip_128() {
        let tek = tek128(7);
        let mut store = TekStore::new();
        store.insert(tek.clone());
        let msg = b"RAYNET net control, all stations report".to_vec();
        let wrapped = seal_sds(&tek, &[1, 2, 3, 4, 5, 6, 7, 8], &msg);
        assert!(is_protected(&wrapped));
        assert_ne!(&wrapped[HEADER_LEN..wrapped.len() - TAG_LEN], &msg[..]); // actually encrypted
        assert_eq!(open_sds(&store, &wrapped).unwrap(), msg);
    }

    #[test]
    fn round_trip_256_and_empty() {
        let tek = Tek::new(9, E2eeAlg::Aes256, vec![0xAB; 32]).unwrap();
        let mut store = TekStore::new();
        store.insert(tek.clone());
        for msg in [b"x".to_vec(), Vec::new(), vec![0u8; 100]] {
            let wrapped = seal_sds(&tek, &[9; 8], &msg);
            assert_eq!(open_sds(&store, &wrapped).unwrap(), msg);
        }
    }

    #[test]
    fn unknown_tek_id() {
        let store = TekStore::new();
        let wrapped = seal_sds(&tek128(5), &[0; 8], b"hi");
        assert_eq!(open_sds(&store, &wrapped), Err(E2eeError::UnknownTek(5)));
    }

    #[test]
    fn wrong_key_fails_auth() {
        let mut store = TekStore::new();
        store.insert(Tek::new(5, E2eeAlg::Aes128, vec![0x11; 16]).unwrap()); // different key, same id
        let wrapped = seal_sds(&tek128(5), &[0; 8], b"secret");
        assert_eq!(open_sds(&store, &wrapped), Err(E2eeError::AuthFailed));
    }

    #[test]
    fn tamper_detected() {
        let tek = tek128(1);
        let mut store = TekStore::new();
        store.insert(tek.clone());
        let mut wrapped = seal_sds(&tek, &[0; 8], b"move to channel 9");
        let n = wrapped.len();
        wrapped[HEADER_LEN + 2] ^= 0x01; // flip a ciphertext bit
        assert_eq!(open_sds(&store, &wrapped), Err(E2eeError::AuthFailed));
        // truncation is rejected too
        assert_eq!(open_sds(&store, &wrapped[..n - 4]), Err(E2eeError::AuthFailed));
    }

    #[test]
    fn non_protected_and_short() {
        let store = TekStore::new();
        assert_eq!(open_sds(&store, b"plain text"), Err(E2eeError::TooShort));
        let longish = vec![0u8; 30];
        assert_eq!(open_sds(&store, &longish), Err(E2eeError::NotProtected));
        assert!(!is_protected(b"hello"));
    }

    #[test]
    fn bad_key_length_rejected() {
        assert!(Tek::new(1, E2eeAlg::Aes128, vec![0; 15]).is_none());
        assert!(Tek::new(1, E2eeAlg::Aes256, vec![0; 16]).is_none());
    }
}
