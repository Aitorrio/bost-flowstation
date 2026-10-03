use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use super::sdu;
use crate::mle::enums::mle_pdu_type_dl::MlePduTypeDl;

/// Representation of the D-RESTORE-ACK PDU (Clause 18.4.1.4.4).
/// Upon receipt from the SwMI, the message shall indicate to the MS-MLE an acknowledgement of the C-Plane restoration on the new selected cell.
/// Response expected: -
/// Response to: U-RESTORE

// note 1: This PDU shall carry a CMCE D-CALL RESTORE PDU which can be used to restore a call after cell reselection. The SDU is coded according to the CMCE protocol description. There shall be no P-bit in the PDU coding preceding the SDU information element.
#[derive(Debug, Clone)]
pub struct DRestoreAck {
    /// Conditional SDU, see note
    pub sdu: Option<BitBuffer>,
}

impl DRestoreAck {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeDl::DRestoreAck)?;
        let obit = delimiters::read_obit(buffer)?;
        let sdu = if obit { sdu::read_rest(buffer) } else { None };

        Ok(DRestoreAck { sdu })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MlePduTypeDl::DRestoreAck.into_raw(), 3);
        delimiters::write_obit(buffer, self.sdu.is_some() as u8);
        if let Some(ref s) = self.sdu {
            sdu::write(buffer, s);
        }
        Ok(())
    }
}

impl fmt::Display for DRestoreAck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DRestoreAck {{ sdu: {} bits }}", self.sdu.as_ref().map_or(0, |s| s.get_len()))
    }
}
