use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::mle::enums::mle_pdu_type_dl::MlePduTypeDl;

/// Representation of the D-RESTORE-FAIL PDU (Clause 18.4.1.4.5).
/// Upon receipt from the SwMI, the message shall indicate to the MS-MLE a failure in the restoration of the C-Plane on the new selected cell.
/// Response expected: -
/// Response to: U-RESTORE

#[derive(Debug, Clone)]
pub struct DRestoreFail {
    /// Type1, 2 bits, Fail cause
    pub fail_cause: u8,
}

impl DRestoreFail {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeDl::DRestoreFail)?;
        let fail_cause = buffer.read_field(2, "fail_cause")? as u8;
        let obit = delimiters::read_obit(buffer)?;
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(DRestoreFail { fail_cause })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MlePduTypeDl::DRestoreFail.into_raw(), 3);
        buffer.write_bits(self.fail_cause as u64, 2);
        delimiters::write_obit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for DRestoreFail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DRestoreFail {{ fail_cause: {:?} }}", self.fail_cause)
    }
}
