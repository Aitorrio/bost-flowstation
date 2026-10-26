//! Entity-side end-to-end encryption helper for SDS.
//!
//! Bridges the config ([`tetra_config::bluestation::CfgE2ee`]) to the crypto
//! core ([`tetra_security::e2ee`]): it holds the configured TEKs and seals /
//! opens SDS payloads, supplying a fresh per-message nonce (the crypto crate
//! is I/O-free, so the nonce is generated here).
//!
//! DISABLED unless the operator configured `[security.e2ee]`. E2EE is lawful
//! only on licensed private/commercial networks, never on amateur allocations.
//!
//! Scope today: confidentiality + integrity of an SDS user-data payload, keyed
//! by a configured TEK. Wiring this into the live SDS send/receive dispatch
//! (inbound auto-decrypt and an outbound per-peer policy, mirroring the AIE
//! `clear_groups` approach) is the remaining integration step.

use std::sync::atomic::{AtomicU64, Ordering};

use tetra_config::bluestation::{CfgE2ee, CfgE2eeAlg};
use tetra_security::e2ee::{self, E2eeAlg, E2eeError, Tek, TekStore};

/// Holds the configured TEKs and generates per-message nonces for sealing.
pub struct SdsE2ee {
    store: TekStore,
    /// The TEK used when sealing outbound payloads (the first configured one).
    seal_tek: Option<u16>,
    enabled: bool,
    /// Monotonic nonce counter, seeded from the OS RNG at startup so it does not
    /// repeat a (key, nonce) pair across restarts.
    nonce: AtomicU64,
}

fn map_alg(a: CfgE2eeAlg) -> E2eeAlg {
    match a {
        CfgE2eeAlg::Aes128 => E2eeAlg::Aes128,
        CfgE2eeAlg::Aes256 => E2eeAlg::Aes256,
    }
}

/// Seed a 64-bit nonce counter from the OS RNG (Linux `/dev/urandom`), falling
/// back to the wall clock if that is unavailable. Only uniqueness matters for
/// CTR, so a random start plus a monotonic increment is sufficient.
fn seed_nonce() -> u64 {
    use std::io::Read;
    // Read EXACTLY 8 bytes — `/dev/urandom` is an endless stream, so reading "the whole file"
    // (e.g. fs::read) never terminates and exhausts memory. Open + read_exact is instant.
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let mut b = [0u8; 8];
        if f.read_exact(&mut b).is_ok() {
            return u64::from_le_bytes(b);
        }
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5A5A_5A5A_5A5A_5A5A)
}

impl SdsE2ee {
    /// Build from the resolved `[security.e2ee]` config (`None` = off).
    pub fn new(cfg: Option<&CfgE2ee>) -> Self {
        let mut store = TekStore::new();
        let mut seal_tek = None;
        let mut enabled = false;
        if let Some(c) = cfg {
            enabled = c.enabled;
            for t in &c.teks {
                if let Some(tek) = Tek::new(t.id, map_alg(t.alg), t.key.clone()) {
                    if seal_tek.is_none() {
                        seal_tek = Some(t.id);
                    }
                    store.insert(tek);
                }
            }
        }
        Self { store, seal_tek, enabled, nonce: AtomicU64::new(seed_nonce()) }
    }

    /// Whether E2EE is enabled and has at least one usable TEK.
    pub fn enabled(&self) -> bool {
        self.enabled && !self.store.is_empty()
    }

    fn next_nonce(&self) -> [u8; 8] {
        self.nonce.fetch_add(1, Ordering::Relaxed).to_be_bytes()
    }

    /// Seal an outbound SDS payload under the sealing TEK. `None` if E2EE is off
    /// or no TEK is configured.
    pub fn seal(&self, plaintext: &[u8]) -> Option<Vec<u8>> {
        if !self.enabled() {
            return None;
        }
        let id = self.seal_tek?;
        let tek = self.store.get(id)?;
        Some(e2ee::seal_sds(tek, &self.next_nonce(), plaintext))
    }

    /// Is this inbound payload an E2EE-protected one we should try to open?
    pub fn is_protected(&self, payload: &[u8]) -> bool {
        self.enabled() && e2ee::is_protected(payload)
    }

    /// Open (verify + decrypt) an inbound protected payload, selecting the TEK
    /// by the id in its header.
    pub fn open(&self, wrapped: &[u8]) -> Result<Vec<u8>, E2eeError> {
        e2ee::open_sds(&self.store, wrapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_config::bluestation::CfgTek;

    fn cfg(enabled: bool) -> CfgE2ee {
        CfgE2ee { enabled, teks: vec![CfgTek { id: 3, alg: CfgE2eeAlg::Aes256, key: vec![0x7Au8; 32] }] }
    }

    #[test]
    fn off_without_config() {
        let e = SdsE2ee::new(None);
        assert!(!e.enabled());
        assert!(e.seal(b"hi").is_none());
    }

    #[test]
    fn off_when_disabled() {
        let e = SdsE2ee::new(Some(&cfg(false)));
        assert!(!e.enabled());
        assert!(e.seal(b"hi").is_none());
    }

    #[test]
    fn seal_open_round_trip() {
        let e = SdsE2ee::new(Some(&cfg(true)));
        assert!(e.enabled());
        let msg = b"all units, rendezvous grid 123456";
        let sealed = e.seal(msg).expect("seals when enabled");
        assert!(e.is_protected(&sealed));
        assert_eq!(e.open(&sealed).unwrap(), msg);
    }

    #[test]
    fn nonces_do_not_repeat() {
        let e = SdsE2ee::new(Some(&cfg(true)));
        let a = e.seal(b"x").unwrap();
        let b = e.seal(b"x").unwrap();
        // Same plaintext, different nonce → different ciphertext bytes.
        assert_ne!(a, b);
    }

    #[test]
    fn opens_only_known_teks() {
        let e = SdsE2ee::new(Some(&cfg(true)));
        // A payload sealed under a TEK this store doesn't have is rejected.
        let other = Tek::new(99, E2eeAlg::Aes256, vec![0x11; 32]).unwrap();
        let foreign = e2ee::seal_sds(&other, &[0; 8], b"secret");
        assert_eq!(e.open(&foreign), Err(E2eeError::UnknownTek(99)));
    }
}
