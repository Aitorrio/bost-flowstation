use core::fmt;

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

/// Group deassignment IE, TS 100 392-12-22 V1.5.1 Table 47.
///
/// Carried (repeated) in the DEASSIGN PDU (Table 20), one entry per group to
/// remove/detach. It names the group only; the action (detach vs. remove) is
/// decided by the MS and reported back in the DEASSIGN ACK.
///
/// Layout:
/// ```text
///   Group SSI               24b  M
///   Group extension present  1b  M
///   Group extension         24b  C  (only if present = 1)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDeassignment {
    /// Group SSI (GSSI), 24 bits.
    pub group_ssi: u32,
    /// Group extension, 24 bits. `Some` sets the present bit.
    pub group_extension: Option<u32>,
}

impl GroupDeassignment {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let group_ssi = buf.read_field(24, "group_ssi")? as u32;
        let ext_present = buf.read_field(1, "group_extension_present")? == 1;
        let group_extension = if ext_present {
            Some(buf.read_field(24, "group_extension")? as u32)
        } else {
            None
        };
        Ok(GroupDeassignment {
            group_ssi,
            group_extension,
        })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        // Group SSI and group extension are 24-bit fields; reject a wider value as InvalidValue
        // rather than letting write_bits' range assertion panic the cell (defence in depth).
        if self.group_ssi > 0xFF_FFFF {
            return Err(PduParseErr::InvalidValue {
                field: "group_ssi",
                value: self.group_ssi as u64,
            });
        }
        buf.write_bits(self.group_ssi as u64, 24);
        buf.write_bits(self.group_extension.is_some() as u64, 1);
        if let Some(ext) = self.group_extension {
            if ext > 0xFF_FFFF {
                return Err(PduParseErr::InvalidValue {
                    field: "group_extension",
                    value: ext as u64,
                });
            }
            buf.write_bits(ext as u64, 24);
        }
        Ok(())
    }
}

impl fmt::Display for GroupDeassignment {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "GroupDeassignment {{ group_ssi: {} group_extension: {:?} }}",
            self.group_ssi, self.group_extension
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GSSI is a 24-bit field; a wider value used to panic the whole cell in the bit writer and
    /// must now be a recoverable InvalidValue.
    #[test]
    fn to_bitbuf_rejects_out_of_range_gssi_instead_of_panicking() {
        let mut buf = BitBuffer::new_autoexpand(8);
        let de = GroupDeassignment {
            group_ssi: 0x100_0000,
            group_extension: None,
        };
        assert!(matches!(
            de.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue { field: "group_ssi", .. })
        ));
    }

    #[test]
    fn to_bitbuf_accepts_max_valid_gssi() {
        let mut buf = BitBuffer::new_autoexpand(8);
        let de = GroupDeassignment {
            group_ssi: 0xFF_FFFF,
            group_extension: None,
        };
        assert!(de.to_bitbuf(&mut buf).is_ok());
    }
}
