use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use super::sdu;
use crate::mle::enums::mle_pdu_type_ul::MlePduTypeUl;

/// Representation of the U-PREPARE PDU (Clause 18.4.1.4.6).
/// The message shall be sent on the serving cell to the SwMI by the MS-MLE, when preparation of cell reselection to a neighbour cell is in progress.
/// Response expected: D-NEW-CELL / D-NWRK-BROADCAST / D-PREPARE-FAIL
/// Response to: -

// note 1: The SDU may carry an MM registration PDU which is used to forward register to a new CA cell during announced type 1 cell reselection or a U-OTAR CCK DEMAND PDU which is used to request the Common Cipher Key (CCK) of the new cell. The SDU is coded according to the MM protocol description. There shall be no P-bit in the PDU coding preceding the SDU information element.
#[derive(Debug, Clone)]
pub struct UPrepare {
    /// Type2, 5 bits, Cell identifier CA
    pub cell_identifier_ca: Option<u64>,
    /// Conditional: MM registration or U-OTAR CCK DEMAND, see note
    pub sdu: Option<BitBuffer>,
}

impl UPrepare {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeUl::UPrepare)?;

        // obit designates presence of the type2 element and/or the SDU
        let obit = delimiters::read_obit(buffer)?;
        let cell_identifier_ca = typed::parse_type2_generic(obit, buffer, 5, "cell_identifier_ca")?;
        let sdu = if obit { sdu::read_rest(buffer) } else { None };

        Ok(UPrepare { cell_identifier_ca, sdu })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MlePduTypeUl::UPrepare.into_raw(), 3);
        let obit = self.cell_identifier_ca.is_some() || self.sdu.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }
        typed::write_type2_generic(obit, buffer, self.cell_identifier_ca, 5);
        if let Some(ref s) = self.sdu {
            sdu::write(buffer, s);
        }
        Ok(())
    }
}

impl fmt::Display for UPrepare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "UPrepare {{ cell_identifier_ca: {:?} sdu: {} bits }}",
            self.cell_identifier_ca,
            self.sdu.as_ref().map_or(0, |s| s.get_len()),
        )
    }
}
