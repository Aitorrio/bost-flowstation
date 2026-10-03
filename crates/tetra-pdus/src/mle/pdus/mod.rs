pub mod d_mle_sync;
pub mod d_mle_sysinfo;

pub mod d_channel_response;
pub mod d_new_cell;
pub mod d_nwrk_broadcast;
pub mod d_nwrk_broadcast_remove;
pub mod d_prepare_fail;
pub mod d_restore_ack;
pub mod d_restore_fail;
mod sdu;
pub mod u_channel_class_advice;
pub mod u_prepare;
pub mod u_restore;

#[cfg(test)]
mod handover_tests {
    use tetra_core::BitBuffer;

    use super::{d_new_cell::DNewCell, d_restore_ack::DRestoreAck, d_restore_fail::DRestoreFail, u_prepare::UPrepare, u_restore::URestore};

    fn bits(s: &str) -> BitBuffer {
        BitBuffer::from_bitstr(s)
    }

    fn encode(f: impl FnOnce(&mut BitBuffer)) -> BitBuffer {
        let mut b = BitBuffer::new_autoexpand(64);
        f(&mut b);
        let len = b.get_pos();
        let mut out = BitBuffer::new(len);
        b.seek(0);
        out.copy_bits(&mut b, len);
        out.seek(0);
        out
    }

    #[test]
    fn u_prepare_round_trips_with_and_without_sdu() {
        let p = UPrepare {
            cell_identifier_ca: Some(3),
            sdu: Some(bits("1011001")),
        };
        let mut b = encode(|b| p.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "000" .to_string() + "1" + "1" + "00011" + "1011001");
        let q = UPrepare::from_bitbuf(&mut b).unwrap();
        assert_eq!(q.cell_identifier_ca, Some(3));
        assert_eq!(q.sdu.unwrap().to_bitstr(), "1011001");

        let mut b = encode(|b| UPrepare { cell_identifier_ca: Some(7), sdu: None }.to_bitbuf(b).unwrap());
        let q = UPrepare::from_bitbuf(&mut b).unwrap();
        assert_eq!((q.cell_identifier_ca, q.sdu.is_none()), (Some(7), true));
    }

    #[test]
    fn u_restore_carries_the_cmce_sdu() {
        let r = URestore {
            mcc: None,
            mnc: None,
            la: Some(2),
            sdu: Some(bits("0111000110")),
        };
        let mut b = encode(|b| r.to_bitbuf(b).unwrap());
        let q = URestore::from_bitbuf(&mut b).unwrap();
        assert_eq!((q.mcc, q.mnc, q.la), (None, None, Some(2)));
        assert_eq!(q.sdu.unwrap().to_bitstr(), "0111000110");
    }

    #[test]
    fn downlink_pdus_round_trip() {
        let mut b = encode(|b| DNewCell { channel_command_valid: 1, sdu: None }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "000010");
        assert_eq!(DNewCell::from_bitbuf(&mut b).unwrap().channel_command_valid, 1);

        let mut b = encode(|b| DRestoreAck { sdu: Some(bits("01110")) }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "100101110");
        assert_eq!(DRestoreAck::from_bitbuf(&mut b).unwrap().sdu.unwrap().to_bitstr(), "01110");

        let mut b = encode(|b| DRestoreFail { fail_cause: 2 }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "101100");
        assert_eq!(DRestoreFail::from_bitbuf(&mut b).unwrap().fail_cause, 2);
    }
}
