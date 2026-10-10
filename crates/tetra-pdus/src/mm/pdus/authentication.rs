//! The authentication PDUs of ETSI EN 300 392-7 (clause A.1): the MM PDU type
//! U-AUTHENTICATION (uplink) / D-AUTHENTICATION (downlink) followed by a
//! 2-bit sub-type that selects DEMAND, RESPONSE, RESULT or REJECT.
//!
//! An 80-bit random seed / challenge and a 32-bit response value are carried
//! as fixed-length byte arrays, most significant byte first.

use core::fmt;

use tetra_core::expect_pdu_type;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::authentication_reject_reason::AuthenticationRejectReason;
use crate::mm::enums::authentication_sub_type::AuthenticationSubType;
use crate::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use crate::mm::enums::mm_pdu_type_ul::MmPduTypeUl;
use crate::mm::enums::type34_elem_id_dl::MmType34ElemIdDl;
use crate::mm::enums::type34_elem_id_ul::MmType34ElemIdUl;

/// Length in bytes of RS, RAND1 and RAND2 (80 bits, clause A.8.62 / A.8.63).
pub const NONCE_LEN: usize = 10;
/// Length in bytes of RES1 and RES2 (32 bits, clause A.8.66).
pub const RES_LEN: usize = 4;

fn read_nonce(buffer: &mut BitBuffer, name: &'static str) -> Result<[u8; NONCE_LEN], PduParseErr> {
    let hi = buffer.read_field(40, name)?;
    let lo = buffer.read_field(40, name)?;
    let mut out = [0u8; NONCE_LEN];
    out[..5].copy_from_slice(&hi.to_be_bytes()[3..]);
    out[5..].copy_from_slice(&lo.to_be_bytes()[3..]);
    Ok(out)
}

fn write_nonce(buffer: &mut BitBuffer, value: &[u8; NONCE_LEN]) {
    let mut hi = [0u8; 8];
    hi[3..].copy_from_slice(&value[..5]);
    let mut lo = [0u8; 8];
    lo[3..].copy_from_slice(&value[5..]);
    buffer.write_bits(u64::from_be_bytes(hi), 40);
    buffer.write_bits(u64::from_be_bytes(lo), 40);
}

fn read_res(buffer: &mut BitBuffer, name: &'static str) -> Result<[u8; RES_LEN], PduParseErr> {
    Ok((buffer.read_field(32, name)? as u32).to_be_bytes())
}

fn write_res(buffer: &mut BitBuffer, value: &[u8; RES_LEN]) {
    buffer.write_bits(u64::from(u32::from_be_bytes(*value)), 32);
}

/// Reads the MM PDU type and the authentication sub-type that follows it.
fn read_header(buffer: &mut BitBuffer, expect_dl: bool) -> Result<AuthenticationSubType, PduParseErr> {
    let pdu_type = buffer.read_field(4, "pdu_type")?;
    if expect_dl {
        expect_pdu_type!(pdu_type, MmPduTypeDl::DAuthentication)?;
    } else {
        expect_pdu_type!(pdu_type, MmPduTypeUl::UAuthentication)?;
    }
    let sub_type = buffer.read_field(2, "authentication_sub_type")?;
    Ok(AuthenticationSubType::try_from(sub_type).expect("2-bit value")) // never fails
}

fn expect_sub_type(got: AuthenticationSubType, want: AuthenticationSubType) -> Result<(), PduParseErr> {
    if got == want {
        Ok(())
    } else {
        Err(PduParseErr::InvalidPduType {
            expected: want.into_raw(),
            found: got.into_raw(),
        })
    }
}

/// Peek the sub-type of an authentication PDU without consuming it, so the
/// MM entity can dispatch to the right parser.
pub fn peek_sub_type(buffer: &BitBuffer) -> Option<AuthenticationSubType> {
    buffer.peek_bits(6).and_then(|v| AuthenticationSubType::try_from(v & 0b11).ok())
}

// ───────────────────────── downlink (SwMI → MS) ─────────────────────────

/// D-AUTHENTICATION DEMAND (clause A.1.1): the infrastructure challenges the MS.
/// Response expected: U-AUTHENTICATION RESPONSE.
#[derive(Debug, PartialEq, Eq)]
pub struct DAuthenticationDemand {
    /// Type1, 80 bits, Random challenge RAND1
    pub rand1: [u8; NONCE_LEN],
    /// Type1, 80 bits, Random seed RS
    pub rs: [u8; NONCE_LEN],
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DAuthenticationDemand {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, true)?, AuthenticationSubType::Demand)?;
        let rand1 = read_nonce(buffer, "rand1")?;
        let rs = read_nonce(buffer, "rs")?;
        let mut obit = delimiters::read_obit(buffer)?;
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdDl::Proprietary)?;
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }
        Ok(DAuthenticationDemand { rand1, rs, proprietary })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeDl::DAuthentication.into_raw(), 4);
        buffer.write_bits(AuthenticationSubType::Demand.into_raw(), 2);
        write_nonce(buffer, &self.rand1);
        write_nonce(buffer, &self.rs);
        let obit = self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdDl::Proprietary)?;
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

/// D-AUTHENTICATION REJECT (clause A.1.2): the infrastructure refuses an
/// authentication demand from the MS.
#[derive(Debug, PartialEq, Eq)]
pub struct DAuthenticationReject {
    /// Type1, 3 bits
    pub reject_reason: AuthenticationRejectReason,
}

impl DAuthenticationReject {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, true)?, AuthenticationSubType::Reject)?;
        let reason = buffer.read_field(3, "authentication_reject_reason")?;
        Ok(DAuthenticationReject {
            reject_reason: AuthenticationRejectReason::try_from(reason).expect("3-bit value"),
        })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeDl::DAuthentication.into_raw(), 4);
        buffer.write_bits(AuthenticationSubType::Reject.into_raw(), 2);
        buffer.write_bits(self.reject_reason.into_raw(), 3);
        Ok(())
    }
}

/// D-AUTHENTICATION RESPONSE (clause A.1.3): the infrastructure answers an
/// MS's challenge with RES2, optionally making the exchange mutual by adding
/// its own challenge RAND1. Response expected: U-AUTHENTICATION RESULT.
#[derive(Debug, PartialEq, Eq)]
pub struct DAuthenticationResponse {
    /// Type1, 80 bits, Random seed RS
    pub rs: [u8; NONCE_LEN],
    /// Type1, 32 bits, Response value RES2
    pub res2: [u8; RES_LEN],
    /// Conditional on the mutual authentication flag: RAND1
    pub rand1: Option<[u8; NONCE_LEN]>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DAuthenticationResponse {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, true)?, AuthenticationSubType::Response)?;
        let rs = read_nonce(buffer, "rs")?;
        let res2 = read_res(buffer, "res2")?;
        let mutual = buffer.read_field(1, "mutual_authentication_flag")? == 1;
        let rand1 = if mutual { Some(read_nonce(buffer, "rand1")?) } else { None };
        let mut obit = delimiters::read_obit(buffer)?;
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdDl::Proprietary)?;
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }
        Ok(DAuthenticationResponse {
            rs,
            res2,
            rand1,
            proprietary,
        })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeDl::DAuthentication.into_raw(), 4);
        buffer.write_bits(AuthenticationSubType::Response.into_raw(), 2);
        write_nonce(buffer, &self.rs);
        write_res(buffer, &self.res2);
        buffer.write_bits(self.rand1.is_some() as u64, 1);
        if let Some(ref rand1) = self.rand1 {
            write_nonce(buffer, rand1);
        }
        let obit = self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdDl::Proprietary)?;
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

/// D-AUTHENTICATION RESULT (clause A.1.4): the infrastructure reports R1 to
/// the MS and, in a mutual exchange, its answer RES2 to the MS's challenge.
#[derive(Debug, PartialEq, Eq)]
pub struct DAuthenticationResult {
    /// Type1, 1 bit, Authentication result R1 (true = successful)
    pub r1: bool,
    /// Conditional on the mutual authentication flag: RES2
    pub res2: Option<[u8; RES_LEN]>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DAuthenticationResult {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, true)?, AuthenticationSubType::Result)?;
        let r1 = buffer.read_field(1, "authentication_result")? == 1;
        let mutual = buffer.read_field(1, "mutual_authentication_flag")? == 1;
        let res2 = if mutual { Some(read_res(buffer, "res2")?) } else { None };
        let mut obit = delimiters::read_obit(buffer)?;
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdDl::Proprietary)?;
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }
        Ok(DAuthenticationResult { r1, res2, proprietary })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeDl::DAuthentication.into_raw(), 4);
        buffer.write_bits(AuthenticationSubType::Result.into_raw(), 2);
        buffer.write_bits(self.r1 as u64, 1);
        buffer.write_bits(self.res2.is_some() as u64, 1);
        if let Some(ref res2) = self.res2 {
            write_res(buffer, res2);
        }
        let obit = self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdDl::Proprietary)?;
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

// ───────────────────────── uplink (MS → SwMI) ─────────────────────────

/// U-AUTHENTICATION DEMAND (clause A.1.5): the MS challenges the infrastructure.
#[derive(Debug, PartialEq, Eq)]
pub struct UAuthenticationDemand {
    /// Type1, 80 bits, Random challenge RAND2
    pub rand2: [u8; NONCE_LEN],
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UAuthenticationDemand {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, false)?, AuthenticationSubType::Demand)?;
        let rand2 = read_nonce(buffer, "rand2")?;
        let mut obit = delimiters::read_obit(buffer)?;
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdUl::Proprietary)?;
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }
        Ok(UAuthenticationDemand { rand2, proprietary })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeUl::UAuthentication.into_raw(), 4);
        buffer.write_bits(AuthenticationSubType::Demand.into_raw(), 2);
        write_nonce(buffer, &self.rand2);
        let obit = self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdUl::Proprietary)?;
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

/// U-AUTHENTICATION REJECT (clause A.1.6): the MS refuses the infrastructure's demand.
#[derive(Debug, PartialEq, Eq)]
pub struct UAuthenticationReject {
    /// Type1, 3 bits
    pub reject_reason: AuthenticationRejectReason,
}

impl UAuthenticationReject {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, false)?, AuthenticationSubType::Reject)?;
        let reason = buffer.read_field(3, "authentication_reject_reason")?;
        Ok(UAuthenticationReject {
            reject_reason: AuthenticationRejectReason::try_from(reason).expect("3-bit value"),
        })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeUl::UAuthentication.into_raw(), 4);
        buffer.write_bits(AuthenticationSubType::Reject.into_raw(), 2);
        buffer.write_bits(self.reject_reason.into_raw(), 3);
        Ok(())
    }
}

/// U-AUTHENTICATION RESPONSE (clause A.1.7): the MS answers the
/// infrastructure's challenge with RES1, optionally making the exchange
/// mutual by adding RAND2. Response expected: D-AUTHENTICATION RESULT.
#[derive(Debug, PartialEq, Eq)]
pub struct UAuthenticationResponse {
    /// Type1, 32 bits, Response value RES1
    pub res1: [u8; RES_LEN],
    /// Conditional on the mutual authentication flag: RAND2
    pub rand2: Option<[u8; NONCE_LEN]>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UAuthenticationResponse {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, false)?, AuthenticationSubType::Response)?;
        let res1 = read_res(buffer, "res1")?;
        let mutual = buffer.read_field(1, "mutual_authentication_flag")? == 1;
        let rand2 = if mutual { Some(read_nonce(buffer, "rand2")?) } else { None };
        let mut obit = delimiters::read_obit(buffer)?;
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdUl::Proprietary)?;
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }
        Ok(UAuthenticationResponse { res1, rand2, proprietary })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeUl::UAuthentication.into_raw(), 4);
        buffer.write_bits(AuthenticationSubType::Response.into_raw(), 2);
        write_res(buffer, &self.res1);
        buffer.write_bits(self.rand2.is_some() as u64, 1);
        if let Some(ref rand2) = self.rand2 {
            write_nonce(buffer, rand2);
        }
        let obit = self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdUl::Proprietary)?;
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

/// U-AUTHENTICATION RESULT (clause A.1.8): the MS reports R2 and, in a
/// mutual exchange, its answer RES1 to the infrastructure's challenge.
#[derive(Debug, PartialEq, Eq)]
pub struct UAuthenticationResult {
    /// Type1, 1 bit, Authentication result R2 (true = successful)
    pub r2: bool,
    /// Conditional on the mutual authentication flag: RES1
    pub res1: Option<[u8; RES_LEN]>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UAuthenticationResult {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, false)?, AuthenticationSubType::Result)?;
        let r2 = buffer.read_field(1, "authentication_result")? == 1;
        let mutual = buffer.read_field(1, "mutual_authentication_flag")? == 1;
        let res1 = if mutual { Some(read_res(buffer, "res1")?) } else { None };
        let mut obit = delimiters::read_obit(buffer)?;
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdUl::Proprietary)?;
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }
        Ok(UAuthenticationResult { r2, res1, proprietary })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeUl::UAuthentication.into_raw(), 4);
        buffer.write_bits(AuthenticationSubType::Result.into_raw(), 2);
        buffer.write_bits(self.r2 as u64, 1);
        buffer.write_bits(self.res1.is_some() as u64, 1);
        if let Some(ref res1) = self.res1 {
            write_res(buffer, res1);
        }
        let obit = self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdUl::Proprietary)?;
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

macro_rules! display_debug {
    ($($t:ty),*) => {$(
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "{self:?}")
            }
        }
    )*};
}
display_debug!(
    DAuthenticationDemand,
    DAuthenticationReject,
    DAuthenticationResponse,
    DAuthenticationResult,
    UAuthenticationDemand,
    UAuthenticationReject,
    UAuthenticationResponse,
    UAuthenticationResult
);

#[cfg(test)]
mod tests {
    use super::*;

    const RS: [u8; 10] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc];
    const RAND: [u8; 10] = [0xba, 0x98, 0x76, 0x54, 0x32, 0x10, 0x0f, 0x1e, 0x2d, 0x3c];
    const RES: [u8; 4] = [0xde, 0xad, 0xbe, 0xef];

    fn round_trip<T: PartialEq + fmt::Debug>(
        pdu: &T,
        write: fn(&T, &mut BitBuffer) -> Result<(), PduParseErr>,
        read: fn(&mut BitBuffer) -> Result<T, PduParseErr>,
        bits: usize,
    ) {
        let mut buf = BitBuffer::new_autoexpand(256);
        write(pdu, &mut buf).unwrap();
        assert_eq!(buf.get_len_written(), bits, "encoded length of {pdu:?}");
        let mut back = BitBuffer::from_bitbuffer(&buf);
        assert_eq!(&read(&mut back).unwrap(), pdu);
    }

    #[test]
    fn downlink_pdus_round_trip() {
        // 4 + 2 + 80 + 80 + 1 (o-bit)
        round_trip(
            &DAuthenticationDemand {
                rand1: RAND,
                rs: RS,
                proprietary: None,
            },
            DAuthenticationDemand::to_bitbuf,
            DAuthenticationDemand::from_bitbuf,
            167,
        );
        round_trip(
            &DAuthenticationReject {
                reject_reason: AuthenticationRejectReason::AuthenticationNotSupported,
            },
            DAuthenticationReject::to_bitbuf,
            DAuthenticationReject::from_bitbuf,
            9,
        );
        round_trip(
            &DAuthenticationResponse {
                rs: RS,
                res2: RES,
                rand1: None,
                proprietary: None,
            },
            DAuthenticationResponse::to_bitbuf,
            DAuthenticationResponse::from_bitbuf,
            4 + 2 + 80 + 32 + 1 + 1,
        );
        round_trip(
            &DAuthenticationResponse {
                rs: RS,
                res2: RES,
                rand1: Some(RAND),
                proprietary: None,
            },
            DAuthenticationResponse::to_bitbuf,
            DAuthenticationResponse::from_bitbuf,
            4 + 2 + 80 + 32 + 1 + 80 + 1,
        );
        round_trip(
            &DAuthenticationResult {
                r1: true,
                res2: None,
                proprietary: None,
            },
            DAuthenticationResult::to_bitbuf,
            DAuthenticationResult::from_bitbuf,
            4 + 2 + 1 + 1 + 1,
        );
        round_trip(
            &DAuthenticationResult {
                r1: false,
                res2: Some(RES),
                proprietary: None,
            },
            DAuthenticationResult::to_bitbuf,
            DAuthenticationResult::from_bitbuf,
            4 + 2 + 1 + 1 + 32 + 1,
        );
    }

    #[test]
    fn uplink_pdus_round_trip() {
        round_trip(
            &UAuthenticationDemand {
                rand2: RAND,
                proprietary: None,
            },
            UAuthenticationDemand::to_bitbuf,
            UAuthenticationDemand::from_bitbuf,
            4 + 2 + 80 + 1,
        );
        round_trip(
            &UAuthenticationReject {
                reject_reason: AuthenticationRejectReason::AuthenticationNotSupported,
            },
            UAuthenticationReject::to_bitbuf,
            UAuthenticationReject::from_bitbuf,
            9,
        );
        round_trip(
            &UAuthenticationResponse {
                res1: RES,
                rand2: None,
                proprietary: None,
            },
            UAuthenticationResponse::to_bitbuf,
            UAuthenticationResponse::from_bitbuf,
            4 + 2 + 32 + 1 + 1,
        );
        round_trip(
            &UAuthenticationResponse {
                res1: RES,
                rand2: Some(RAND),
                proprietary: None,
            },
            UAuthenticationResponse::to_bitbuf,
            UAuthenticationResponse::from_bitbuf,
            4 + 2 + 32 + 1 + 80 + 1,
        );
        round_trip(
            &UAuthenticationResult {
                r2: true,
                res1: Some(RES),
                proprietary: None,
            },
            UAuthenticationResult::to_bitbuf,
            UAuthenticationResult::from_bitbuf,
            4 + 2 + 1 + 1 + 32 + 1,
        );
    }

    #[test]
    fn wire_layout_matches_the_standard() {
        // U-AUTHENTICATION RESPONSE, not mutual: 0000 | 01 | RES1 | 0 | o-bit 0
        let mut buf = BitBuffer::new_autoexpand(64);
        UAuthenticationResponse {
            res1: RES,
            rand2: None,
            proprietary: None,
        }
        .to_bitbuf(&mut buf)
        .unwrap();
        assert_eq!(
            buf.to_bitstr(),
            "000001".to_string() + "11011110101011011011111011101111" + "0" + "0"
        );
        // Sub-type can be peeked for dispatch.
        let back = BitBuffer::from_bitbuffer(&buf);
        assert_eq!(peek_sub_type(&back), Some(AuthenticationSubType::Response));
        // A D- PDU parser refuses a U- PDU.
        let mut back = BitBuffer::from_bitbuffer(&buf);
        assert!(DAuthenticationResponse::from_bitbuf(&mut back).is_err());
    }
}
