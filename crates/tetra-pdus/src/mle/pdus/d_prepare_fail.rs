use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use super::sdu;
use crate::mle::enums::mle_pdu_type_dl::MlePduTypeDl;

/// Representation of the D-PREPARE-FAIL PDU (Clause 18.4.1.4.3).
/// Upon receipt from the SwMI the message shall be used by the MS-MLE as a preparation failure, while announcing cell reselection to the old cell.
/// Response expected: -
/// Response to: U-PREPARE/U-PREPARE-DA

// note 1: The SDU may carry an MM registration PDU. The SDU is coded according to the MM protocol description. There shall be no P-bit in the PDU coding preceding the SDU information element.
#[derive(Debug, Clone)]
pub struct DPrepareFail {
    /// Type1, 2 bits, Fail cause
    pub fail_cause: u8,
    /// Conditional SDU, see note
    pub sdu: Option<BitBuffer>,
}

impl DPrepareFail {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeDl::DPrepareFail)?;
        let fail_cause = buffer.read_field(2, "fail_cause")? as u8;
        let obit = delimiters::read_obit(buffer)?;
        let sdu = if obit { sdu::read_rest(buffer) } else { None };

        Ok(DPrepareFail { fail_cause, sdu })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MlePduTypeDl::DPrepareFail.into_raw(), 3);
        buffer.write_bits(self.fail_cause as u64, 2);
        delimiters::write_obit(buffer, self.sdu.is_some() as u8);
        if let Some(ref s) = self.sdu {
            sdu::write(buffer, s);
        }
        Ok(())
    }
}

impl fmt::Display for DPrepareFail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DPrepareFail {{ fail_cause: {:?} sdu: {} bits }}", self.fail_cause, self.sdu.as_ref().map_or(0, |s| s.get_len()))
    }
}
