use super::{tests::get_testing_slotter, BsChannelScheduler};
use crate::aie::cipher::CellCipher;
use tetra_config::bluestation::{CfgAie, CfgClass3, CipherKey, SharedConfig};
use tetra_core::{BitBuffer, TetraAddress, TxReporter, TxState};
use tetra_pdus::umac::pdus::mac_resource::MacResource;

pub fn station() -> (BsChannelScheduler, SharedConfig) {
    let cfg = tetra_config::bluestation::from_toml_str(include_str!("../../../../../example_config/config.toml")).unwrap();
    let shared = SharedConfig::from_parts(cfg, None);
    let aie = CfgAie { enabled: true, ksg: 2, sck: CipherKey([0; 10]), sckn: 1, sck_vn: 0,
        clear_groups: vec![], class3: Some(CfgClass3 { cck: CipherKey([0x33; 10]), cck_id: 42 }) };
    let mut sched = get_testing_slotter();
    sched.set_cipher(Some(std::sync::Arc::new(CellCipher::new(&aie, &[sched.carrier_num()], 101, 1).with_config(shared.clone()))));
    (sched, shared)
}

pub fn resource(encrypted: bool) -> MacResource {
    let mut pdu = BsChannelScheduler::dl_make_minimal_resource(&TetraAddress::issi(200), None, false);
    pdu.encryption_mode = if encrypted { 2 } else { 0 };
    pdu
}

#[test]
fn r3_detached_ack_does_not_live_forever_or_block_fragmented_registration() {
    let (mut sched, _) = station();
    let old_ack = TxReporter::new_unacked();
    sched.dl_enqueue_tma(resource(true), BitBuffer::from_bitstr("00111"), Some(old_ack.clone()));
    let accept = TxReporter::new();
    sched.dl_enqueue_tma(resource(false), BitBuffer::from_bitstr(&"01".repeat(150)), Some(accept.clone()));
    for _ in 0..16 {
        let next = sched.cur_dltime.add_timeslots(1);
        sched.tick_start(next);
        sched.finalize_ts_for_tick();
    }
    assert!(accept.is_transmitted(), "CCK-bearing location accept must finish MAC-END");
    assert_eq!(old_ack.get_state(), TxState::Discarded, "no missing-key ACK may be requeued forever");
    assert!(sched.dltx_queues.iter().all(|q| q.is_empty()));
    assert!(sched.dltx_next_slot_queue.is_empty());
}

#[test]
fn r3_stale_facch_without_dck_does_not_hold_traffic_slot_forever() {
    let (mut sched, _) = station();
    let receipt = TxReporter::new();
    sched.dl_enqueue_stealing_ciphered(2, resource(true), BitBuffer::from_bitstr("00111"), None, Some(receipt.clone()));
    let (_, stch, _) = sched.dl_build_traffic_block(tetra_core::TdmaTime { t: 2, f: 1, m: 1, h: 0 });
    assert!(stch.is_none());
    assert_eq!(receipt.get_state(), TxState::Discarded);
    assert!(!receipt.has_been_transmitted());
    assert!(sched.dltx_queues[1].is_empty());
}

#[test]
fn r3_no_cipher_never_emits_plaintext_or_leaves_pending_reporter() {
    use super::BsFragger;
    use crate::umac::subcomp::bs_frag::ChunkStatus;
    let receipt = TxReporter::new();
    let mut fragger = BsFragger::new(resource(true), BitBuffer::from_bitstr("101010"), Some(receipt.clone()));
    let mut block = BitBuffer::new(super::SCH_F_CAP);
    assert_eq!(fragger.get_next_chunk_ciphered(&mut block, None), ChunkStatus::Discarded);
    assert_eq!(block.get_pos(), 0);
    assert_eq!(receipt.get_state(), TxState::Discarded);
    assert!(!receipt.has_been_transmitted());
}

#[test]
fn r3_fragment_cannot_continue_under_a_replacement_dck() {
    use super::BsFragger;
    use crate::umac::subcomp::bs_frag::{ChunkStatus, SlotCipher};
    for remove in [false, true] {
        let (sched, shared) = station();
        shared.state_write().sc3_dcks.insert(200, CipherKey([0x19; 10]));
        let cipher = sched.cipher.as_ref().unwrap();
        let receipt = TxReporter::new();
        let mut fragger = BsFragger::new(resource(true), BitBuffer::from_bitstr(&"01".repeat(150)), Some(receipt.clone()));
        for frame in 1..=2 {
            let time = tetra_core::TdmaTime { t: 1, f: frame, m: 1, h: 0 };
            let mut slot = SlotCipher { cipher, carrier: sched.carrier_num(), time,
                kss: cipher.kss(sched.carrier_num(), time, false), kss_offset: 0, used: false };
            let mut block = BitBuffer::new(super::SCH_F_CAP);
            let status = fragger.get_next_chunk_ciphered(&mut block, Some(&mut slot));
            if frame == 1 {
                assert_eq!(status, ChunkStatus::Deferred);
                assert!(block.get_pos() > 0);
                if remove { shared.state_write().sc3_dcks.remove(&200); }
                else { shared.state_write().sc3_dcks.insert(200, CipherKey([0x29; 10])); }
            } else {
                assert_eq!(status, ChunkStatus::Discarded);
                assert_eq!(block.get_pos(), 0, "no fragment under another session key");
                assert_eq!(receipt.get_state(), TxState::Discarded);
                assert!(!receipt.has_been_transmitted(), "incomplete PDU is not delivery evidence");
            }
        }
    }
}
