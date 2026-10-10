//! Key material and the per-subscriber key store.
//!
//! Key sizes follow ETSI EN 300 392-7 clause 4: the authentication key K is
//! 128 bits (clause 4.1.5); the cipher keys (DCK, CCK, GCK, SCK) are 80 bits.

use std::collections::HashMap;

use zeroize::{Zeroize, ZeroizeOnDrop};

/// Short subscriber identity (24 bits) — the ISSI a key belongs to.
pub type Ssi = u32;

/// Length in bytes of the authentication key K (128 bits).
pub const AUTH_KEY_LEN: usize = 16;

/// Length in bytes of every air-interface cipher key (80 bits).
pub const KEY_LEN: usize = 10;

/// The authentication key K shared between a subscriber and the network.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct AuthKey(pub [u8; AUTH_KEY_LEN]);

/// An 80-bit air-interface cipher key (DCK, CCK, GCK or SCK).
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct CipherKey(pub [u8; KEY_LEN]);

impl std::fmt::Debug for AuthKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthKey(…)") // never log key material
    }
}

impl std::fmt::Debug for CipherKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CipherKey(…)")
    }
}

/// Error parsing a key from its hexadecimal form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyParseError {
    /// Wrong number of hexadecimal digits (the expected count is given).
    Length { got: usize, want: usize },
    /// A character that is not a hexadecimal digit.
    Digit(char),
}

impl std::fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyParseError::Length { got, want } => write!(f, "key must be {want} hex digits ({} bits), got {got}", want * 4),
            KeyParseError::Digit(c) => write!(f, "key contains a non-hex character {c:?}"),
        }
    }
}

impl std::error::Error for KeyParseError {}

fn parse_hex_key<const N: usize>(s: &str) -> Result<[u8; N], KeyParseError> {
    let s: String = s.chars().filter(|c| !c.is_whitespace() && *c != ':').collect();
    if s.len() != N * 2 {
        return Err(KeyParseError::Length { got: s.len(), want: N * 2 });
    }
    let mut out = [0u8; N];
    for (i, pair) in s.as_bytes().chunks(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16).ok_or(KeyParseError::Digit(pair[0] as char))?;
        let lo = (pair[1] as char).to_digit(16).ok_or(KeyParseError::Digit(pair[1] as char))?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Ok(out)
}

impl std::str::FromStr for AuthKey {
    type Err = KeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_hex_key(s).map(AuthKey)
    }
}

impl std::str::FromStr for CipherKey {
    type Err = KeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_hex_key(s).map(CipherKey)
    }
}

/// Authentication keys by subscriber, plus the cell's static cipher keys.
///
/// The store is deliberately simple: the base station holds K for the
/// subscribers it serves on its own (no AuC); when a Brew core takes over
/// authentication the store is empty and challenges are forwarded instead.
#[derive(Default)]
pub struct KeyStore {
    k: HashMap<Ssi, AuthKey>,
    /// Static cipher keys by SCK number (1 … 32) for security class 2.
    sck: HashMap<u8, CipherKey>,
}

impl KeyStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) the authentication key of a subscriber.
    pub fn insert_k(&mut self, ssi: Ssi, k: AuthKey) {
        self.k.insert(ssi, k);
    }

    pub fn remove_k(&mut self, ssi: Ssi) -> bool {
        self.k.remove(&ssi).is_some()
    }

    pub fn k(&self, ssi: Ssi) -> Option<&AuthKey> {
        self.k.get(&ssi)
    }

    pub fn has_k(&self, ssi: Ssi) -> bool {
        self.k.contains_key(&ssi)
    }

    pub fn subscribers(&self) -> impl Iterator<Item = Ssi> + '_ {
        self.k.keys().copied()
    }

    /// Set a static cipher key. `sckn` is the SCK number, 1 … 32.
    pub fn insert_sck(&mut self, sckn: u8, key: CipherKey) {
        debug_assert!((1..=32).contains(&sckn));
        self.sck.insert(sckn, key);
    }

    pub fn sck(&self, sckn: u8) -> Option<&CipherKey> {
        self.sck.get(&sckn)
    }

    pub fn len(&self) -> usize {
        self.k.len()
    }

    pub fn is_empty(&self) -> bool {
        self.k.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_keys() {
        let k: AuthKey = "00112233445566778899aabbccddeeff".parse().unwrap();
        assert_eq!(k.0[..4], [0x00, 0x11, 0x22, 0x33]);
        assert_eq!(k.0[15], 0xff);
        let k2: AuthKey = "00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff".parse().unwrap();
        assert_eq!(k, k2);
        let c: CipherKey = "00112233445566778899".parse().unwrap();
        assert_eq!(c.0, [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99]);
        assert_eq!("0011".parse::<AuthKey>().unwrap_err(), KeyParseError::Length { got: 4, want: 32 });
        assert_eq!("0011223344556677889g".parse::<CipherKey>().unwrap_err(), KeyParseError::Digit('g'));
    }

    #[test]
    fn debug_never_shows_key_bytes() {
        let k: AuthKey = "ffffffffffffffffffffffffffffffff".parse().unwrap();
        assert_eq!(format!("{k:?}"), "AuthKey(…)");
    }

    #[test]
    fn store_round_trip() {
        let mut s = KeyStore::new();
        assert!(s.is_empty());
        s.insert_k(2260571, "00112233445566778899aabbccddeeff".parse().unwrap());
        assert!(s.has_k(2260571));
        assert!(!s.has_k(2260572));
        assert_eq!(s.len(), 1);
        assert!(s.remove_k(2260571));
        assert!(s.is_empty());
    }
}
