use core::fmt;

/// EN 300 392-7 clause A.8 Ciphering parameters, 10 bits, carried in U-LOCATION UPDATE DEMAND and
/// D-LOCATION UPDATE COMMAND when "Cipher control" is 1.
///
/// Layout (MSB first): KSG number (4), security class (1: 0 = class 2, 1 = class 3), then 5 bits
/// that are the SCK number for class 2 or reserved for class 3.
/// [verify] against the EN 300 392-7 text before AIE goes on air (Docs/aie-class2-plan.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CipheringParameters {
    /// KSG number as sent: 0 = TEA1, 1 = TEA2, 2 = TEA3, 3 = TEA4, 8..=15 proprietary.
    pub ksg_number: u8,
    /// True for security class 3 (DCK), false for class 2 (SCK).
    pub class3: bool,
    /// Class 2: SCK number as sent (0 = SCK 1 … 31 = SCK 32). Class 3: the 5 reserved bits.
    pub sckn_field: u8,
}

impl CipheringParameters {
    pub fn from_bits(v: u64) -> Self {
        CipheringParameters {
            ksg_number: ((v >> 6) & 0xF) as u8,
            class3: (v >> 5) & 1 != 0,
            sckn_field: (v & 0x1F) as u8,
        }
    }

    pub fn to_bits(self) -> u64 {
        ((self.ksg_number as u64 & 0xF) << 6) | ((self.class3 as u64) << 5) | (self.sckn_field as u64 & 0x1F)
    }

    /// Class 2 parameters for TEA`tea` (1..=4) and SCK number `sckn` (1..=32).
    pub fn class2(tea: u8, sckn: u8) -> Self {
        CipheringParameters {
            ksg_number: tea - 1,
            class3: false,
            sckn_field: sckn - 1,
        }
    }

    /// The TEA algorithm number (1..=4), or `None` for a proprietary KSG.
    pub fn tea(self) -> Option<u8> {
        (self.ksg_number < 4).then_some(self.ksg_number + 1)
    }

    /// The SCK number (1..=32) for class 2.
    pub fn sckn(self) -> Option<u8> {
        (!self.class3).then_some(self.sckn_field + 1)
    }
}

impl fmt::Display for CipheringParameters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.tea(), self.sckn()) {
            (Some(tea), Some(sckn)) => write!(f, "TEA{tea} class 2 SCKN {sckn}"),
            (Some(tea), None) => write!(f, "TEA{tea} class 3"),
            (None, _) => write!(f, "proprietary KSG {} class {}", self.ksg_number, if self.class3 { 3 } else { 2 }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class2_round_trip() {
        let p = CipheringParameters::class2(1, 1);
        assert_eq!(p.to_bits(), 0);
        let p = CipheringParameters::class2(2, 32);
        assert_eq!(p.to_bits(), 0b0001_0_11111);
        assert_eq!(CipheringParameters::from_bits(p.to_bits()), p);
        assert_eq!((p.tea(), p.sckn()), (Some(2), Some(32)));
    }

    #[test]
    fn class3_has_no_sckn() {
        let p = CipheringParameters::from_bits(0b0000_1_00000);
        assert!(p.class3);
        assert_eq!(p.sckn(), None);
    }
}
