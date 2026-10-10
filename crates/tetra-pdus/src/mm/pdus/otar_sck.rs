//! The SCK over-the-air rekeying PDUs of ETSI EN 300 392-7 (clauses A.2.7 …
//! A.2.9a): the MM PDU type D-OTAR / U-OTAR followed by a 4-bit OTAR sub-type.
//!
//! Only individually addressed delivery with a session key KSO is produced
//! here (clause 4.5.2.2); group delivery under a GSKO is parsed but not built.

use core::fmt;

use tetra_core::expect_pdu_type;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use crate::mm::enums::mm_pdu_type_ul::MmPduTypeUl;
use crate::mm::enums::otar_sub_type::{OtarRejectReason, OtarSubType, ProvisionResult};
use crate::mm::enums::type34_elem_id_dl::MmType34ElemIdDl;
use crate::mm::enums::type34_elem_id_ul::MmType34ElemIdUl;

/// Random seed for OTAR, 80 bits (clause A.8.64).
pub const RSO_LEN: usize = 10;
/// Sealed key, 120 bits (clause A.8.76).
pub const SEALED_LEN: usize = 15;

fn read_bytes<const N: usize>(buffer: &mut BitBuffer, name: &'static str) -> Result<[u8; N], PduParseErr> {
    let mut out = [0u8; N];
    for b in out.iter_mut() {
        *b = buffer.read_field(8, name)? as u8;
    }
    Ok(out)
}

fn write_bytes(buffer: &mut BitBuffer, value: &[u8]) {
    for &b in value {
        buffer.write_bits(u64::from(b), 8);
    }
}

fn read_header(buffer: &mut BitBuffer, expect_dl: bool) -> Result<OtarSubType, PduParseErr> {
    let pdu_type = buffer.read_field(4, "pdu_type")?;
    if expect_dl {
        expect_pdu_type!(pdu_type, MmPduTypeDl::DOtar)?;
    } else {
        expect_pdu_type!(pdu_type, MmPduTypeUl::UOtar)?;
    }
    OtarSubType::try_from(buffer.read_field(4, "otar_sub_type")?)
}

fn expect_sub_type(got: OtarSubType, want: OtarSubType) -> Result<(), PduParseErr> {
    if got == want {
        Ok(())
    } else {
        Err(PduParseErr::InvalidPduType {
            expected: want.into_raw(),
            found: got.into_raw(),
        })
    }
}

/// Peek the OTAR sub-type of a U-OTAR / D-OTAR PDU without consuming it.
pub fn peek_sub_type(buffer: &BitBuffer) -> Option<OtarSubType> {
    buffer.peek_bits(8).and_then(|v| OtarSubType::try_from(v & 0xF).ok())
}

/// Reads the optional tail shared by these PDUs: O-bit, type-2 Address
/// extension (24 bits), type-3 Proprietary.
fn read_tail(buffer: &mut BitBuffer, uplink: bool) -> Result<(Option<u64>, Option<Type3FieldGeneric>), PduParseErr> {
    let mut obit = delimiters::read_obit(buffer)?;
    let address_extension = typed::parse_type2_generic(obit, buffer, 24, "address_extension")?;
    let proprietary = if uplink {
        typed::parse_type3_generic(obit, buffer, MmType34ElemIdUl::Proprietary)?
    } else {
        typed::parse_type3_generic(obit, buffer, MmType34ElemIdDl::Proprietary)?
    };
    obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
    if obit {
        return Err(PduParseErr::InvalidTrailingMbitValue);
    }
    Ok((address_extension, proprietary))
}

fn write_tail(buffer: &mut BitBuffer, address_extension: Option<u64>) {
    let obit = address_extension.is_some();
    delimiters::write_obit(buffer, obit as u8);
    if !obit {
        return;
    }
    typed::write_type2_generic(obit, buffer, address_extension, 24);
    delimiters::write_mbit(buffer, 0);
}

// ───────────────────────── downlink (SwMI → MS) ─────────────────────────

/// One "SCK key and identifier" element (clause A.8.69), 143 bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SckKeyAndIdentifier {
    /// SCK number, 5 bits (1 … 32 on air as 0 … 31)
    pub sckn: u8,
    /// SCK version number, 16 bits
    pub sck_vn: u16,
    /// SCK use: false = trunked mode, true = direct mode
    pub dmo: bool,
    /// Sealed SCK (TA51 output), 120 bits
    pub ssck: [u8; SEALED_LEN],
}

impl SckKeyAndIdentifier {
    fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let sckn = buffer.read_field(5, "sckn")? as u8;
        let sck_vn = buffer.read_field(16, "sck_vn")? as u16;
        let dmo = buffer.read_field(1, "sck_use")? == 1;
        let _reserved = buffer.read_field(1, "reserved")?;
        let ssck = read_bytes::<SEALED_LEN>(buffer, "ssck")?;
        Ok(SckKeyAndIdentifier { sckn, sck_vn, dmo, ssck })
    }

    fn to_bitbuf(&self, buffer: &mut BitBuffer) {
        buffer.write_bits(u64::from(self.sckn & 0x1f), 5);
        buffer.write_bits(u64::from(self.sck_vn), 16);
        buffer.write_bits(self.dmo as u64, 1);
        buffer.write_bits(0, 1);
        write_bytes(buffer, &self.ssck);
    }
}

/// How the sealed keys in a D-OTAR SCK PROVIDE were protected (clause A.8.78).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtarSessionKey {
    /// Individually: KSO = TA41(K, RSO), the seed is carried in the PDU.
    Individual { rso: [u8; RSO_LEN] },
    /// Group: a GSKO the radio already holds, identified by its version number.
    Group { gsko_vn: u16 },
}

/// D-OTAR SCK PROVIDE (clause A.2.7): the infrastructure delivers up to seven
/// sealed SCKs. Response expected: U-OTAR SCK RESULT.
#[derive(Debug, PartialEq, Eq)]
pub struct DOtarSckProvide {
    /// Acknowledgement flag: a U-OTAR SCK RESULT is required.
    pub ack_required: bool,
    /// Explicit response (only when acknowledged): true = respond whether the
    /// MS state changed or not.
    pub explicit_response: bool,
    /// Max response timer value in seconds; 0 for individually addressed delivery.
    pub max_response_timer: u16,
    pub session_key: OtarSessionKey,
    /// 0 … 7 keys
    pub keys: Vec<SckKeyAndIdentifier>,
    /// KSG number (clause A.8.41): 0 = TEA1, 1 = TEA2, 2 = TEA3, 3 = TEA4.
    pub ksg_number: u8,
    /// OTAR retry interval (clause A.8.57c), 3 bits.
    pub otar_retry_interval: u8,
    /// Type2, 24 bits
    pub address_extension: Option<u64>,
    /// Type3
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DOtarSckProvide {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, true)?, OtarSubType::SckDemandProvide)?;
        let ack_required = buffer.read_field(1, "acknowledgement_flag")? == 1;
        let flag = buffer.read_field(1, "explicit_response")? == 1;
        let explicit_response = ack_required && flag;
        let max_response_timer = buffer.read_field(16, "max_response_timer")? as u16;
        let group = buffer.read_field(1, "session_key")? == 1;
        let session_key = if group {
            OtarSessionKey::Group { gsko_vn: buffer.read_field(16, "gsko_vn")? as u16 }
        } else {
            OtarSessionKey::Individual { rso: read_bytes::<RSO_LEN>(buffer, "rso")? }
        };
        let n = buffer.read_field(3, "number_of_scks_provided")?;
        let mut keys = Vec::with_capacity(n as usize);
        for _ in 0..n {
            keys.push(SckKeyAndIdentifier::from_bitbuf(buffer)?);
        }
        let ksg_number = buffer.read_field(4, "ksg_number")? as u8;
        let otar_retry_interval = buffer.read_field(3, "otar_retry_interval")? as u8;
        let (address_extension, proprietary) = read_tail(buffer, false)?;
        Ok(DOtarSckProvide {
            ack_required,
            explicit_response,
            max_response_timer,
            session_key,
            keys,
            ksg_number,
            otar_retry_interval,
            address_extension,
            proprietary,
        })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.keys.len() > 7 {
            return Err(PduParseErr::InvalidValue { field: "number_of_scks_provided", value: self.keys.len() as u64 });
        }
        buffer.write_bits(MmPduTypeDl::DOtar.into_raw(), 4);
        buffer.write_bits(OtarSubType::SckDemandProvide.into_raw(), 4);
        buffer.write_bits(self.ack_required as u64, 1);
        buffer.write_bits((self.ack_required && self.explicit_response) as u64, 1);
        buffer.write_bits(u64::from(self.max_response_timer), 16);
        match &self.session_key {
            OtarSessionKey::Individual { rso } => {
                buffer.write_bits(0, 1);
                write_bytes(buffer, rso);
            }
            OtarSessionKey::Group { gsko_vn } => {
                buffer.write_bits(1, 1);
                buffer.write_bits(u64::from(*gsko_vn), 16);
            }
        }
        buffer.write_bits(self.keys.len() as u64, 3);
        for k in &self.keys {
            k.to_bitbuf(buffer);
        }
        buffer.write_bits(u64::from(self.ksg_number & 0xF), 4);
        buffer.write_bits(u64::from(self.otar_retry_interval & 0x7), 3);
        write_tail(buffer, self.address_extension);
        Ok(())
    }
}

/// D-OTAR SCK REJECT (clause A.2.9a): the infrastructure cannot provide the
/// requested SCK(s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DOtarSckReject {
    /// (SCKN, reason) per rejected key, 1 … 7
    pub rejected: Vec<(u8, OtarRejectReason)>,
    pub otar_retry_interval: u8,
    pub address_extension: Option<u64>,
}

impl DOtarSckReject {
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.rejected.is_empty() || self.rejected.len() > 7 {
            return Err(PduParseErr::InvalidValue { field: "number_of_scks_rejected", value: self.rejected.len() as u64 });
        }
        buffer.write_bits(MmPduTypeDl::DOtar.into_raw(), 4);
        buffer.write_bits(OtarSubType::SckResultReject.into_raw(), 4);
        buffer.write_bits(self.rejected.len() as u64, 3);
        for (sckn, reason) in &self.rejected {
            buffer.write_bits(u64::from(sckn & 0x1f), 5);
            buffer.write_bits(reason.into_raw(), 3);
        }
        buffer.write_bits(u64::from(self.otar_retry_interval & 0x7), 3);
        write_tail(buffer, self.address_extension);
        Ok(())
    }

    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, true)?, OtarSubType::SckResultReject)?;
        let n = buffer.read_field(3, "number_of_scks_rejected")?;
        let mut rejected = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let sckn = buffer.read_field(5, "sckn")? as u8;
            let reason = match buffer.read_field(3, "otar_reject_reason")? {
                0 => OtarRejectReason::KeyNotAvailable,
                1 => OtarRejectReason::InvalidKeyNumber,
                2 => OtarRejectReason::InvalidAddress,
                _ => OtarRejectReason::KsgNotSupported,
            };
            rejected.push((sckn, reason));
        }
        let otar_retry_interval = buffer.read_field(3, "otar_retry_interval")? as u8;
        let (address_extension, _) = read_tail(buffer, false)?;
        Ok(DOtarSckReject { rejected, otar_retry_interval, address_extension })
    }
}

// ───────────────────────── uplink (MS → SwMI) ─────────────────────────

/// U-OTAR SCK DEMAND (clause A.2.8): the MS asks for SCK(s) by number.
#[derive(Debug, PartialEq, Eq)]
pub struct UOtarSckDemand {
    pub ksg_number: u8,
    /// 1 … 7 SCK numbers
    pub sckns: Vec<u8>,
    pub address_extension: Option<u64>,
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UOtarSckDemand {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, false)?, OtarSubType::SckDemandProvide)?;
        let ksg_number = buffer.read_field(4, "ksg_number")? as u8;
        let n = buffer.read_field(3, "number_of_scks_requested")?;
        let mut sckns = Vec::with_capacity(n as usize);
        for _ in 0..n {
            sckns.push(buffer.read_field(5, "sckn")? as u8);
        }
        let (address_extension, proprietary) = read_tail(buffer, true)?;
        Ok(UOtarSckDemand { ksg_number, sckns, address_extension, proprietary })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeUl::UOtar.into_raw(), 4);
        buffer.write_bits(OtarSubType::SckDemandProvide.into_raw(), 4);
        buffer.write_bits(u64::from(self.ksg_number & 0xF), 4);
        buffer.write_bits(self.sckns.len() as u64, 3);
        for s in &self.sckns {
            buffer.write_bits(u64::from(s & 0x1f), 5);
        }
        write_tail(buffer, self.address_extension);
        Ok(())
    }
}

/// One "SCK number and result" element (clause A.8.71).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SckNumberAndResult {
    pub sckn: u8,
    pub result: ProvisionResult,
    /// Present when the result is "incorrect key version number".
    pub current_sck_vn: Option<u16>,
}

/// U-OTAR SCK RESULT (clause A.2.9): the MS accepts or rejects each provided SCK.
#[derive(Debug, PartialEq, Eq)]
pub struct UOtarSckResult {
    pub results: Vec<SckNumberAndResult>,
    pub address_extension: Option<u64>,
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UOtarSckResult {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        expect_sub_type(read_header(buffer, false)?, OtarSubType::SckResultReject)?;
        let n = buffer.read_field(3, "number_of_scks_provided")?;
        let mut results = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let sckn = buffer.read_field(5, "sckn")? as u8;
            let result = ProvisionResult::from_raw(buffer.read_field(3, "provision_result")?);
            let current_sck_vn = if result == ProvisionResult::IncorrectKeyVersion {
                Some(buffer.read_field(16, "current_sck_vn")? as u16)
            } else {
                None
            };
            results.push(SckNumberAndResult { sckn, result, current_sck_vn });
        }
        let (address_extension, proprietary) = read_tail(buffer, true)?;
        Ok(UOtarSckResult { results, address_extension, proprietary })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeUl::UOtar.into_raw(), 4);
        buffer.write_bits(OtarSubType::SckResultReject.into_raw(), 4);
        buffer.write_bits(self.results.len() as u64, 3);
        for r in &self.results {
            buffer.write_bits(u64::from(r.sckn & 0x1f), 5);
            buffer.write_bits(r.result.into_raw(), 3);
            if r.result == ProvisionResult::IncorrectKeyVersion {
                buffer.write_bits(u64::from(r.current_sck_vn.unwrap_or(0)), 16);
            }
        }
        write_tail(buffer, self.address_extension);
        Ok(())
    }
}

impl fmt::Display for DOtarSckProvide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DOtarSckProvide {{ {} key(s) [{}], ksg {}, {} }}",
            self.keys.len(),
            self.keys.iter().map(|k| format!("SCK{} v{}", k.sckn + 1, k.sck_vn)).collect::<Vec<_>>().join(", "),
            self.ksg_number,
            match self.session_key {
                OtarSessionKey::Individual { .. } => "individual (KSO)",
                OtarSessionKey::Group { .. } => "group (GSKO)",
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provide_round_trips() {
        let pdu = DOtarSckProvide {
            ack_required: true,
            explicit_response: true,
            max_response_timer: 0,
            session_key: OtarSessionKey::Individual { rso: [0xA5; RSO_LEN] },
            keys: vec![SckKeyAndIdentifier { sckn: 0, sck_vn: 7, dmo: false, ssck: [0x3C; SEALED_LEN] }],
            ksg_number: 2,
            otar_retry_interval: 0,
            address_extension: None,
            proprietary: None,
        };
        let mut buf = BitBuffer::new_autoexpand(256);
        pdu.to_bitbuf(&mut buf).unwrap();
        // 4+4+1+1+16+1+80+3+143+4+3+1 = 261 bits
        assert_eq!(buf.get_len(), 261);
        buf.seek(0);
        assert_eq!(peek_sub_type(&buf), Some(OtarSubType::SckDemandProvide));
        assert_eq!(DOtarSckProvide::from_bitbuf(&mut buf).unwrap(), pdu);
    }

    #[test]
    fn demand_and_result_round_trip() {
        let demand = UOtarSckDemand { ksg_number: 1, sckns: vec![0, 5], address_extension: None, proprietary: None };
        let mut buf = BitBuffer::new_autoexpand(64);
        demand.to_bitbuf(&mut buf).unwrap();
        buf.seek(0);
        assert_eq!(peek_sub_type(&buf), Some(OtarSubType::SckDemandProvide));
        assert_eq!(UOtarSckDemand::from_bitbuf(&mut buf).unwrap(), demand);

        let result = UOtarSckResult {
            results: vec![
                SckNumberAndResult { sckn: 0, result: ProvisionResult::Accepted, current_sck_vn: None },
                SckNumberAndResult { sckn: 5, result: ProvisionResult::IncorrectKeyVersion, current_sck_vn: Some(9) },
            ],
            address_extension: Some(0x123456),
            proprietary: None,
        };
        let mut buf = BitBuffer::new_autoexpand(128);
        result.to_bitbuf(&mut buf).unwrap();
        buf.seek(0);
        assert_eq!(peek_sub_type(&buf), Some(OtarSubType::SckResultReject));
        assert_eq!(UOtarSckResult::from_bitbuf(&mut buf).unwrap(), result);
    }

    #[test]
    fn reject_round_trips() {
        let pdu = DOtarSckReject {
            rejected: vec![(3, OtarRejectReason::KeyNotAvailable)],
            otar_retry_interval: 2,
            address_extension: None,
        };
        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).unwrap();
        buf.seek(0);
        assert_eq!(DOtarSckReject::from_bitbuf(&mut buf).unwrap(), pdu);
    }
}
