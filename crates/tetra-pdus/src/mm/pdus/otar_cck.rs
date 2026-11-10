//! Security class 3 CCK delivery (EN 300 392-7 A.2.1–3, A.7.1, A.8.8–9).
use crate::mm::enums::{
    mm_pdu_type_dl::MmPduTypeDl,
    mm_pdu_type_ul::MmPduTypeUl,
    otar_sub_type::{OtarSubType, ProvisionResult},
};
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CckLocations {
    All,
    List(Vec<u16>),
    Mask { mask: u16, selector: u16 },
    Range { first: u16, last: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CckInformation {
    pub cck_id: u16,
    pub future: bool,
    pub sealed: [u8; 15],
    pub locations: CckLocations,
    pub next_sealed: Option<[u8; 15]>,
}

fn invalid(field: &'static str, value: u64) -> PduParseErr {
    PduParseErr::InvalidValue { field, value }
}
fn read_sealed(b: &mut BitBuffer) -> Result<[u8; 15], PduParseErr> {
    let mut out = [0u8; 15];
    for v in &mut out {
        *v = b.read_field(8, "sealed_cck")? as u8;
    }
    Ok(out)
}
fn write_sealed(b: &mut BitBuffer, sealed: &[u8; 15]) {
    for &v in sealed {
        b.write_bits(v as u64, 8);
    }
}
impl CckInformation {
    pub fn from_bitbuf(b: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let cck_id = b.read_field(16, "cck_id")? as u16;
        let future = b.read_field(1, "cck_key_type")? != 0;
        let sealed = read_sealed(b)?;
        let locations = match b.read_field(2, "cck_location_type")? {
            0 => CckLocations::All,
            1 => {
                let n = b.read_field(4, "cck_la_count")?;
                if n == 0 {
                    return Err(invalid("cck_la_count", 0));
                }
                let mut areas = Vec::new();
                for _ in 0..n {
                    areas.push(b.read_field(14, "cck_la")? as u16);
                }
                CckLocations::List(areas)
            }
            2 => CckLocations::Mask {
                mask: b.read_field(14, "cck_la_mask")? as u16,
                selector: b.read_field(14, "cck_la_selector")? as u16,
            },
            _ => CckLocations::Range {
                first: b.read_field(14, "cck_la_first")? as u16,
                last: b.read_field(14, "cck_la_last")? as u16,
            },
        };
        let more = b.read_field(1, "future_key_flag")? != 0;
        if more && future {
            return Err(invalid("future_key_flag", 1));
        }
        let next_sealed = if more { Some(read_sealed(b)?) } else { None };
        Ok(Self {
            cck_id,
            future,
            sealed,
            locations,
            next_sealed,
        })
    }

    pub fn to_bitbuf(&self, b: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.future && self.next_sealed.is_some() {
            return Err(invalid("future_key_flag", 1));
        }
        b.write_bits(self.cck_id as u64, 16);
        b.write_bit(self.future as u8);
        write_sealed(b, &self.sealed);
        match &self.locations {
            CckLocations::All => b.write_bits(0, 2),
            CckLocations::List(areas) => {
                if areas.is_empty() || areas.len() > 15 {
                    return Err(invalid("cck_la_count", areas.len() as u64));
                }
                b.write_bits(1, 2);
                b.write_bits(areas.len() as u64, 4);
                for &la in areas {
                    if la > 0x3fff {
                        return Err(invalid("cck_la", la as u64));
                    }
                    b.write_bits(la as u64, 14);
                }
            }
            CckLocations::Mask { mask, selector } => {
                if *mask > 0x3fff || *selector > 0x3fff {
                    return Err(invalid("cck_la_mask", *mask as u64));
                }
                b.write_bits(2, 2);
                b.write_bits(*mask as u64, 14);
                b.write_bits(*selector as u64, 14);
            }
            CckLocations::Range { first, last } => {
                if first > last || *last > 0x3fff {
                    return Err(invalid("cck_la_range", *last as u64));
                }
                b.write_bits(3, 2);
                b.write_bits(*first as u64, 14);
                b.write_bits(*last as u64, 14);
            }
        }
        b.write_bit(self.next_sealed.is_some() as u8);
        if let Some(s) = &self.next_sealed {
            write_sealed(b, s);
        }
        Ok(())
    }
}

/// EN 300 392-7 A.7.2: CK request and optional embedded RAND2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationUplink {
    pub ck_request: bool,
    pub rand2: Option<[u8; 10]>,
}
impl AuthenticationUplink {
    pub fn from_bitbuf(b: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let ck_request = b.read_field(1, "ck_request")? != 0;
        let optional = b.read_field(1, "authentication_uplink_o")? != 0;
        let rand2 = if optional && b.read_field(1, "rand2_present")? != 0 {
            let mut nonce = [0; 10];
            for v in &mut nonce {
                *v = b.read_field(8, "rand2")? as u8;
            }
            Some(nonce)
        } else {
            None
        };
        Ok(Self { ck_request, rand2 })
    }
    pub fn to_bitbuf(&self, b: &mut BitBuffer) -> Result<(), PduParseErr> {
        b.write_bit(self.ck_request as u8);
        b.write_bit(self.rand2.is_some() as u8);
        if let Some(nonce) = self.rand2 {
            b.write_bit(1);
            for v in nonce {
                b.write_bits(v as u64, 8);
            }
        }
        Ok(())
    }
}

/// Typed IE: a CCK exceeds the old generic field's 128-bit storage limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationDownlink {
    pub r1: bool,
    pub tei_request: bool,
    pub cck: Option<CckInformation>,
}
impl AuthenticationDownlink {
    pub fn from_bitbuf(b: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let r1 = b.read_field(1, "r1")? != 0;
        let tei_request = b.read_field(1, "tei_request")? != 0;
        let provision = b.read_field(1, "ck_provision")? != 0;
        let cck = if provision {
            let sck = b.read_field(1, "sck_provision")?;
            if sck != 0 {
                return Err(invalid("sck_provision_in_authentication_downlink", sck));
            }
            if b.read_field(1, "cck_provision")? != 0 {
                Some(CckInformation::from_bitbuf(b)?)
            } else {
                None
            }
        } else {
            None
        };
        Ok(Self { r1, tei_request, cck })
    }
    pub fn to_bitbuf(&self, b: &mut BitBuffer) -> Result<(), PduParseErr> {
        b.write_bit(self.r1 as u8);
        b.write_bit(self.tei_request as u8);
        b.write_bit(self.cck.is_some() as u8);
        if let Some(cck) = &self.cck {
            b.write_bit(0); // SCK provision
            b.write_bit(1); // CCK provision
            cck.to_bitbuf(b)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DOtarCckProvide {
    pub cck: Option<CckInformation>,
}
impl DOtarCckProvide {
    pub fn to_bitbuf(&self, b: &mut BitBuffer) -> Result<(), PduParseErr> {
        b.write_bits(MmPduTypeDl::DOtar.into_raw(), 4);
        b.write_bits(OtarSubType::CckDemandProvide.into_raw(), 4);
        b.write_bit(self.cck.is_some() as u8);
        if let Some(cck) = &self.cck {
            cck.to_bitbuf(b)?;
        }
        b.write_bit(0); // O-bit, no proprietary IE
        Ok(())
    }
}
pub struct UOtarCckDemand {
    pub location_area: u16,
}
impl UOtarCckDemand {
    pub fn from_bitbuf(b: &mut BitBuffer) -> Result<Self, PduParseErr> {
        read_header(b, OtarSubType::CckDemandProvide)?;
        let location_area = b.read_field(14, "location_area")? as u16;
        read_tail(b)?;
        Ok(Self { location_area })
    }
}
pub struct UOtarCckResult {
    pub result: ProvisionResult,
    pub future_result: Option<ProvisionResult>,
}
impl UOtarCckResult {
    pub fn from_bitbuf(b: &mut BitBuffer) -> Result<Self, PduParseErr> {
        read_header(b, OtarSubType::CckResultReject)?;
        let result = ProvisionResult::from_raw(b.read_field(3, "cck_result")?);
        let future_result = if b.read_field(1, "future_key_flag")? != 0 {
            Some(ProvisionResult::from_raw(b.read_field(3, "future_cck_result")?))
        } else {
            None
        };
        read_tail(b)?;
        Ok(Self { result, future_result })
    }
}
fn read_header(b: &mut BitBuffer, expected: OtarSubType) -> Result<(), PduParseErr> {
    let p = b.read_field(4, "pdu_type")?;
    expect_pdu_type!(p, MmPduTypeUl::UOtar)?;
    let sub = b.read_field(4, "otar_sub_type")?;
    if sub != expected.into_raw() {
        return Err(invalid("otar_sub_type", sub));
    }
    Ok(())
}

fn read_tail(b: &mut BitBuffer) -> Result<(), PduParseErr> {
    use crate::mm::enums::type34_elem_id_ul::MmType34ElemIdUl;
    use tetra_core::typed_pdu_fields::typed;
    if b.read_field(1, "obit")? != 0 {
        typed::parse_type3_generic(true, b, MmType34ElemIdUl::Proprietary)?;
        if b.read_field(1, "mbit")? != 0 {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentication_uplink_has_exact_optional_rand2_encoding() {
        // EN 300 392-2 V2.3.2 Annex E: no nested M-bit when only type-2 IEs exist.
        // The next bit is a sentinel belonging to the enclosing PDU.
        for rand2 in [None, Some([0xA5; 10])] {
            let ie = AuthenticationUplink { ck_request: true, rand2 };
            let expected_len = if rand2.is_some() { 83 } else { 2 };
            let mut b = BitBuffer::new_autoexpand(0);
            ie.to_bitbuf(&mut b).unwrap();
            assert_eq!(b.get_len_written(), expected_len);
            b.write_bit(1);
            b.seek(0);
            if rand2.is_some() {
                assert_eq!(b.read_field(3, "CK O P").unwrap(), 0b111);
                for _ in 0..10 {
                    assert_eq!(b.read_field(8, "RAND2").unwrap(), 0xA5);
                }
            } else {
                assert_eq!(b.read_field(2, "CK O").unwrap(), 0b10);
            }
            b.seek(0);
            assert_eq!(AuthenticationUplink::from_bitbuf(&mut b).unwrap(), ie);
            assert_eq!(b.get_pos(), expected_len);
            assert_eq!(b.read_field(1, "next enclosing bit").unwrap(), 1);
        }
    }

    #[test]
    fn full_cck_survives_type3_roundtrip_beyond_128_bits() {
        use crate::mm::enums::type34_elem_id_dl::MmType34ElemIdDl;
        use tetra_core::typed_pdu_fields::typed;
        let ie = AuthenticationDownlink {
            r1: true,
            tei_request: false,
            cck: Some(CckInformation {
                cck_id: 0x1234,
                future: false,
                sealed: [0xA5; 15],
                locations: CckLocations::List(vec![2]),
                next_sealed: None,
            }),
        };
        let mut b = BitBuffer::new_autoexpand(0);
        typed::write_type3_struct(
            true,
            &mut b,
            &Some(ie.clone()),
            MmType34ElemIdDl::AuthenticationDownlink,
            AuthenticationDownlink::to_bitbuf,
        )
        .unwrap();
        assert_eq!(b.get_len_written(), 16 + 163);
        b.seek(0);
        assert_eq!(b.read_field(1, "mbit").unwrap(), 1);
        b.read_field(4, "id").unwrap();
        assert_eq!(b.read_field(11, "length").unwrap(), 163);
        assert_eq!(b.read_field(5, "R1 TEI CK SCK CCK").unwrap(), 0b10101);
        assert_eq!(b.read_field(16, "cck_id").unwrap(), 0x1234);
        b.seek(0);
        let decoded = typed::parse_type3_struct(
            true,
            &mut b,
            MmType34ElemIdDl::AuthenticationDownlink,
            AuthenticationDownlink::from_bitbuf,
        )
        .unwrap();
        assert_eq!(decoded, Some(ie));
    }
    #[test]
    fn cck_result_consumes_exact_wire_bits_and_rejects_truncation() {
        let mut b = BitBuffer::new_autoexpand(0);
        b.write_bits(MmPduTypeUl::UOtar.into_raw(), 4);
        b.write_bits(1, 4);
        b.write_bits(0, 3);
        b.write_bit(0);
        b.write_bit(0);
        b.seek(0);
        let result = UOtarCckResult::from_bitbuf(&mut b).unwrap();
        assert_eq!(result.result, ProvisionResult::Accepted);
        assert!(result.future_result.is_none());
        assert_eq!(b.get_pos(), 13);
        b.seek(0);
        b.set_raw_end(12);
        assert!(UOtarCckResult::from_bitbuf(&mut b).is_err());
    }
}
