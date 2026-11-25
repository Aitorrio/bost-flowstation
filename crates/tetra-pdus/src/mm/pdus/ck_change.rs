//! Notification of a cipher key change (ETSI EN 300 392-7 clause 4.5.5): D-CK CHANGE DEMAND
//! (clause A.4.1, Table A.26) and U-CK CHANGE RESULT (clause A.4.2, Table A.27a).
//!
//! Built here: trunked mode SCK changes (key change type SCK, SCK use TMO, single SCKs) and CCK
//! changes, with the time given as an absolute IV or "immediate". Neither PDU has type 2 or
//! type 3 elements, so there is no O-bit.

use tetra_core::expect_pdu_type;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use crate::mm::enums::mm_pdu_type_ul::MmPduTypeUl;

/// Key change type (clause A.8.39, Table A.71).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyChange {
    /// 000: single trunked mode SCKs, each (SCKN, SCK-VN) as "SCK data" (Table A.93).
    /// SCKN is zero-based on air (A.8.70).
    Sck(Vec<(u8, u16)>),
    /// 001: the CCK with this CCK-id.
    Cck(u16),
    /// 101: no cipher key, with "transition to security class 1" (clause 4.5.6.1).
    NoCipherKey,
}

/// Time type (clause A.8.86, Table A.111).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeTime {
    /// 00: on this IV (slot 1-4, frame 1-18, multiframe 1-60, hyperframe 16 bits).
    AbsoluteIv { t: u8, f: u8, m: u8, h: u16 },
    /// 10: first slot of the first frame of the next multiframe.
    Immediate,
    /// 11: currently in use.
    CurrentlyInUse,
}

fn write_sck_data(buffer: &mut BitBuffer, keys: &[(u8, u16)]) {
    buffer.write_bits(keys.len() as u64 & 0xf, 4);
    for &(sckn, vn) in keys.iter().take(15) {
        buffer.write_bits(u64::from(sckn & 0x1f), 5);
        buffer.write_bits(u64::from(vn), 16);
    }
}

fn read_sck_data(buffer: &mut BitBuffer) -> Result<Vec<(u8, u16)>, PduParseErr> {
    let n = buffer.read_field(4, "number_of_scks_changed")?;
    let mut keys = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let sckn = buffer.read_field(5, "sckn")? as u8;
        let vn = buffer.read_field(16, "sck_vn")? as u16;
        keys.push((sckn, vn));
    }
    Ok(keys)
}

/// D-CK CHANGE DEMAND (clause A.4.1): the SwMI tells the MS when a key becomes valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DCkChangeDemand {
    /// "1": the MS answers with U-CK CHANGE RESULT.
    pub ack_required: bool,
    /// Change of security class (A.8.11): 0 = no change.
    pub change_of_class: u8,
    pub key: KeyChange,
    pub time: ChangeTime,
}

impl DCkChangeDemand {
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeDl::DCkChangeDemand.into_raw(), 4);
        buffer.write_bits(self.ack_required as u64, 1);
        buffer.write_bits(u64::from(self.change_of_class & 3), 2);
        match &self.key {
            KeyChange::Sck(keys) => {
                buffer.write_bits(0b000, 3);
                buffer.write_bits(0, 1); // SCK use: TMO
                if keys.is_empty() {
                    // "0000" would mean a DMO subset; a TMO change always names its keys.
                    return Err(PduParseErr::InvalidValue { field: "number_of_scks_changed", value: 0 });
                }
                write_sck_data(buffer, keys);
            }
            KeyChange::Cck(id) => {
                buffer.write_bits(0b001, 3);
                buffer.write_bits(u64::from(*id), 16);
            }
            KeyChange::NoCipherKey => buffer.write_bits(0b101, 3),
        }
        match self.time {
            ChangeTime::AbsoluteIv { t, f, m, h } => {
                buffer.write_bits(0b00, 2);
                // IV coding of TS 100 392-7 clause 6.3.2.0a: slot 2 bits (0 = slot 1), frame 5,
                // multiframe 6, hyperframe 16.
                buffer.write_bits(u64::from(t.saturating_sub(1) & 3), 2);
                buffer.write_bits(u64::from(f & 0x1f), 5);
                buffer.write_bits(u64::from(m & 0x3f), 6);
                buffer.write_bits(u64::from(h), 16);
            }
            ChangeTime::Immediate => buffer.write_bits(0b10, 2),
            ChangeTime::CurrentlyInUse => buffer.write_bits(0b11, 2),
        }
        Ok(())
    }

    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeDl::DCkChangeDemand)?;
        let ack_required = buffer.read_field(1, "acknowledgement_flag")? == 1;
        let change_of_class = buffer.read_field(2, "change_of_security_class")? as u8;
        let key = match buffer.read_field(3, "key_change_type")? {
            0b000 => {
                if buffer.read_field(1, "sck_use")? != 0 {
                    return Err(PduParseErr::InvalidValue { field: "sck_use", value: 1 });
                }
                let keys = read_sck_data(buffer)?;
                if keys.is_empty() {
                    return Err(PduParseErr::InvalidValue { field: "number_of_scks_changed", value: 0 });
                }
                KeyChange::Sck(keys)
            }
            0b001 => KeyChange::Cck(buffer.read_field(16, "cck_id")? as u16),
            0b101 => KeyChange::NoCipherKey,
            other => return Err(PduParseErr::InvalidValue { field: "key_change_type", value: other }),
        };
        let time = match buffer.read_field(2, "time_type")? {
            0b00 => ChangeTime::AbsoluteIv {
                t: buffer.read_field(2, "slot_number")? as u8 + 1,
                f: buffer.read_field(5, "frame_number")? as u8,
                m: buffer.read_field(6, "multiframe_number")? as u8,
                h: buffer.read_field(16, "hyperframe_number")? as u16,
            },
            0b10 => ChangeTime::Immediate,
            0b11 => ChangeTime::CurrentlyInUse,
            other => return Err(PduParseErr::InvalidValue { field: "time_type", value: other }),
        };
        Ok(DCkChangeDemand { ack_required, change_of_class, key, time })
    }
}

/// U-CK CHANGE RESULT (clause A.4.2): the keys the MS has selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UCkChangeResult {
    pub change_of_class: u8,
    /// `None` for key change types other than SCK (TMO) and CCK.
    pub key: Option<KeyChange>,
}

impl UCkChangeResult {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeUl::UCkChangeResult)?;
        let change_of_class = buffer.read_field(2, "change_of_security_class")? as u8;
        let key = match buffer.read_field(3, "key_change_type")? {
            0b000 if buffer.read_field(1, "sck_use")? == 0 => Some(KeyChange::Sck(read_sck_data(buffer)?)),
            0b001 => Some(KeyChange::Cck(buffer.read_field(16, "cck_id")? as u16)),
            0b101 => Some(KeyChange::NoCipherKey),
            _ => None,
        };
        Ok(UCkChangeResult { change_of_class, key })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(MmPduTypeUl::UCkChangeResult.into_raw(), 4);
        buffer.write_bits(u64::from(self.change_of_class & 3), 2);
        match &self.key {
            Some(KeyChange::Sck(keys)) => {
                buffer.write_bits(0b000, 3);
                buffer.write_bits(0, 1);
                write_sck_data(buffer, keys);
            }
            Some(KeyChange::Cck(id)) => {
                buffer.write_bits(0b001, 3);
                buffer.write_bits(u64::from(*id), 16);
            }
            Some(KeyChange::NoCipherKey) | None => buffer.write_bits(0b101, 3), // no cipher key
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_to_class_1_names_no_cipher_key() {
        let pdu = DCkChangeDemand { ack_required: false, change_of_class: 0b01, key: KeyChange::NoCipherKey, time: ChangeTime::AbsoluteIv { t: 1, f: 1, m: 4, h: 9 } };
        let mut b = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut b).unwrap();
        // Type 4 + ack 1 + class 2 + key change type 3 + time type 2 + IV 29 (Table A.26).
        assert_eq!(b.get_pos(), 4 + 1 + 2 + 3 + 2 + 29);
        b.seek(7);
        assert_eq!(b.read_field(3, "key_change_type").unwrap(), 0b101);
        b.seek(0);
        assert_eq!(DCkChangeDemand::from_bitbuf(&mut b).unwrap(), pdu);
    }

    #[test]
    fn sck_change_on_an_absolute_iv_round_trips_with_the_table_a26_lengths() {
        let pdu = DCkChangeDemand {
            ack_required: false,
            change_of_class: 0,
            key: KeyChange::Sck(vec![(1, 0x1234)]),
            time: ChangeTime::AbsoluteIv { t: 1, f: 1, m: 17, h: 0xBEEF },
        };
        let mut b = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut b).unwrap();
        // 4 + 1 + 2 + 3 + 1 (SCK use) + 4 (number) + 21 (SCK data) + 2 + 29 (IV)
        assert_eq!(b.get_pos(), 4 + 1 + 2 + 3 + 1 + 4 + 21 + 2 + 29);
        b.seek(0);
        assert_eq!(b.peek_bits(4), Some(0b0010), "PDU type D-CK CHANGE DEMAND (Table A.86)");
        assert_eq!(DCkChangeDemand::from_bitbuf(&mut b).unwrap(), pdu);
    }

    #[test]
    fn immediate_cck_change_and_the_result_parse() {
        let pdu = DCkChangeDemand { ack_required: true, change_of_class: 0, key: KeyChange::Cck(7), time: ChangeTime::Immediate };
        let mut b = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut b).unwrap();
        assert_eq!(b.get_pos(), 4 + 1 + 2 + 3 + 16 + 2);
        b.seek(0);
        assert_eq!(DCkChangeDemand::from_bitbuf(&mut b).unwrap(), pdu);

        let res = UCkChangeResult { change_of_class: 0, key: Some(KeyChange::Sck(vec![(0, 5), (1, 6)])) };
        let mut b = BitBuffer::new_autoexpand(64);
        res.to_bitbuf(&mut b).unwrap();
        b.seek(0);
        assert_eq!(b.peek_bits(4), Some(0b0100), "PDU type U-CK CHANGE RESULT (Table A.86)");
        assert_eq!(UCkChangeResult::from_bitbuf(&mut b).unwrap(), res);
    }

    #[test]
    fn a_tmo_change_without_keys_is_refused() {
        let pdu = DCkChangeDemand { ack_required: false, change_of_class: 0, key: KeyChange::Sck(vec![]), time: ChangeTime::Immediate };
        assert!(pdu.to_bitbuf(&mut BitBuffer::new_autoexpand(16)).is_err());
    }
}
