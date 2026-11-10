//! CCK sealing, TA31/TA32 (ETSI TS 104 053-3, clauses 5.8/5.9).
use super::{bl1, exp1, exp4, hurdle::Hurdle, shr1};
use zeroize::Zeroizing;

fn sealing_key(dck: &[u8; 10], cck_id: u16) -> Zeroizing<[u8; 16]> {
    let id = cck_id.to_be_bytes();
    let mixed = Zeroizing::new(std::array::from_fn(|i| dck[i] ^ id[i % 2]));
    Zeroizing::new(exp4(&mixed))
}

/// Seal an 80-bit CCK under the authenticated terminal's DCK and CCK-id.
pub fn ta31(cck: &[u8; 10], cck_id: u16, dck: &[u8; 10]) -> [u8; 15] {
    let mut plain = Zeroizing::new([0u8; 16]);
    plain[..15].copy_from_slice(&exp1(cck));
    let encrypted = bl1(&sealing_key(dck, cck_id), &plain);
    let mut sealed = [0u8; 15];
    sealed[..7].copy_from_slice(&encrypted[..7]);
    sealed[7..].copy_from_slice(&encrypted[8..]);
    sealed
}

/// Unseal and check all five redundancy bytes. Never return a manipulated key.
pub fn ta32(sealed: &[u8; 15], cck_id: u16, dck: &[u8; 10]) -> Result<[u8; 10], ()> {
    let bc = Hurdle::new(&sealing_key(dck, cck_id));
    let second = Zeroizing::new(bc.decrypt(sealed[7..].try_into().expect("8 bytes")));
    let mut first = [0u8; 8];
    first[..7].copy_from_slice(&sealed[..7]);
    first[7] = second[7];
    let mut plain = Zeroizing::new([0u8; 15]);
    plain[..8].copy_from_slice(&bc.decrypt(&first));
    for i in 0..7 {
        plain[8 + i] = second[i] ^ first[i];
    }
    let bad = (0..5).fold(0u8, |bad, i| bad | (plain[3 * i] ^ plain[3 * i + 1] ^ plain[3 * i + 2]));
    if bad != 0 {
        return Err(());
    }
    Ok(shr1(&plain))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_ta31_ta32_vectors() {
        // Copyright 2023 Midnight Blue, Apache-2.0, TETRA_crypto/tests.c.
        // https://github.com/MidnightBlueLabs/TETRA_crypto/blob/master/tests.c
        for (cck, id, dck, sealed) in [
            (
                "00000000000000000000",
                0x6b18,
                "5fb0442f4b5ee2f0ea91",
                "a34885fc277d8d9611d40e22400a14",
            ),
            (
                "7d33968ef5b5343d3d0e",
                0xb2e3,
                "084ac4809156ce544cb2",
                "bbe6f1bc44850bc7353cbbb4af5b39",
            ),
            (
                "39ec0e26f227eff7648f",
                0xa086,
                "fdafc21cc865a7a085ec",
                "bb07ebb67817a2c0efa9e33cbc7b7b",
            ),
            (
                "d6075e8e93d95014c375",
                0x2286,
                "fc4ca2297fbaf0962e5b",
                "18e090004ac3bd2cc1453a338f16d3",
            ),
        ] {
            let cck: [u8; 10] = hex::decode(cck).unwrap().try_into().unwrap();
            let dck: [u8; 10] = hex::decode(dck).unwrap().try_into().unwrap();
            let sealed: [u8; 15] = hex::decode(sealed).unwrap().try_into().unwrap();
            assert_eq!(ta31(&cck, id, &dck), sealed);
            assert_eq!(ta32(&sealed, id, &dck), Ok(cck));
        }
    }

    #[test]
    fn cck_sealing_binds_dck_and_identifier() {
        let cck = [0x31; 10];
        let dck = [0x72; 10];
        let sealed = ta31(&cck, 0x1234, &dck);
        assert_eq!(ta32(&sealed, 0x1234, &dck), Ok(cck));
        assert!(ta32(&sealed, 0x1235, &dck).is_err());
        assert!(ta32(&sealed, 0x1234, &[0x73; 10]).is_err());
        for i in 0..120 {
            let mut damaged = sealed;
            damaged[i / 8] ^= 1 << (i % 8);
            assert!(ta32(&damaged, 0x1234, &dck).is_err());
        }
    }
}
