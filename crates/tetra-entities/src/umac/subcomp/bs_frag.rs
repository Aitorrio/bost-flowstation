use std::cmp::min;

use tetra_core::{BitBuffer, SsiType, TxReporter};

use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
use tetra_pdus::umac::pdus::{mac_end_dl::MacEndDl, mac_frag_dl::MacFragDl, mac_resource::MacResource};

use crate::aie::cipher::{CellCipher, Kss};
use crate::umac::subcomp::fillbits;

/// Air interface encryption for one downlink timeslot (TS 100 392-7 clause 6.4.2.2): its key
/// stream segment and where in it the slot (or half slot) starts. With TEA set A every encrypted
/// MAC PDU restarts at that point, so a slot carries at most one encrypted PDU — `used` records it.
pub struct SlotCipher<'a> {
    pub cipher: &'a CellCipher,
    pub carrier: u16,
    pub time: tetra_core::TdmaTime,
    pub kss: Kss,
    pub kss_offset: usize,
    pub used: bool,
}

#[derive(Debug)]
pub struct BsFragger {
    resource: MacResource,
    /// The channel allocation of a fragmented PDU. It goes in the MAC-END PDU and not in the
    /// MAC-RESOURCE PDU with the first fragment (TS 100 392-2 clauses 23.4.2.1.1 and 23.5.4.2.1).
    end_chan_alloc: Option<ChanAllocElement>,
    mac_hdr_is_written: bool,
    is_fully_transmitted: bool,
    sdu: BitBuffer,
    tx_reporter: Option<TxReporter>,
    /// The PDU is to be sent encrypted (its MAC-RESOURCE carried a non-zero encryption mode).
    encrypted: bool,
    discarded: bool,
    /// Bind fragments to the same DCK; its Debug implementation redacts the key.
    dck: Option<tetra_config::bluestation::CipherKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStatus { Complete, Deferred, Discarded }

impl ChunkStatus {
    pub fn is_complete(self) -> bool { self == Self::Complete }
    pub fn needs_retry(self) -> bool { self == Self::Deferred }
}

/// We won't start fragmentation if less than MIN_SLOT_CAP_FOR_FRAG_START bits are free in the slot
const MIN_SLOT_CAP_FOR_RES_FRAG_START: usize = 32;

/// We won't insert a fragment if less than MIN_SLOT_CAP_FOR_FRAG bits are free in the slot
const MIN_SLOT_CAP_FOR_FRAG: usize = 16;

impl BsFragger {
    pub fn new(resource: MacResource, sdu: BitBuffer, tx_reporter: Option<TxReporter>) -> Self {
        assert!(sdu.get_pos() == 0, "SDU must be at the start of the buffer");
        // We set the length field now. If we do fragmentation, we'll set it to -1 later.
        // resource.update_len_and_fill_ind(sdu.get_len());
        let encrypted = resource.encryption_mode != 0;
        BsFragger {
            resource,
            end_chan_alloc: None,
            mac_hdr_is_written: false,
            is_fully_transmitted: false,
            sdu,
            tx_reporter,
            encrypted,
            discarded: false,
            dck: None,
        }
    }

    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    /// Writes MAC-RESOURCE to dest_buf, starting fragmentation if needed.
    /// Then, writes as many SDU bits as possible.
    /// Returns true if the entire SDU was consumed, false if the PDU is fragmented
    /// and more chunks are needed.
    fn get_resource_chunk(&mut self, mac_block: &mut BitBuffer, cipher: Option<&mut SlotCipher>) -> bool {
        // Some sanity checks
        assert!(self.sdu.get_pos() == 0, "SDU must be at the start of the buffer");
        assert!(!self.mac_hdr_is_written, "MAC header should not be written yet");
        assert!(
            !(self.resource.is_null_pdu() && self.sdu.get_len_remaining() > 0),
            "Null PDU cannot have SDU data"
        );

        // A start of fragmentation that had to wait set the channel allocation aside
        if self.end_chan_alloc.is_some() {
            self.resource.chan_alloc_element = self.end_chan_alloc.take();
        }

        // Compute len of full resource, including sdu and fill bits
        let hdr_len_bits = self.resource.compute_header_len();
        let chan_alloc_len_bits = self.resource.chan_alloc_element.as_ref().map_or(0, |c| c.compute_len());
        let sdu_len_bits = self.sdu.get_len_remaining();
        let slot_cap_bits = mac_block.get_len_remaining();

        let num_fill_bits = fillbits::addition::compute_required(hdr_len_bits + sdu_len_bits, slot_cap_bits);

        let total_len_bits = hdr_len_bits + sdu_len_bits + num_fill_bits;
        let total_len_bytes = total_len_bits / 8;

        // Check if we can fit all in a single MAC-RESOURCE
        if total_len_bits <= slot_cap_bits {
            // Fits in one MAC-RESOURCE
            assert!(
                total_len_bits % 8 == 0 || total_len_bits == mac_block.get_len_remaining(),
                "PDU must fill slot or have byte aligned end, got len {} for remaining cap {}",
                total_len_bits,
                mac_block.get_len_remaining()
            );

            // Update PDU fields
            self.resource.length_ind = total_len_bytes as u8;
            self.resource.fill_bits = num_fill_bits > 0;

            tracing::debug!(
                "-> {:?} sdu {}",
                self.resource,
                self.sdu
                    .raw_dump_bin(false, false, self.sdu.get_pos(), self.sdu.get_pos() + sdu_len_bits)
            );

            // Write MAC-RESOURCE header, followed by TM-SDU, to MAC block
            let start = mac_block.get_pos();
            self.write_resource_header(mac_block, cipher.as_deref());
            mac_block.copy_bits(&mut self.sdu, sdu_len_bits);
            self.encrypt_resource(mac_block, start, hdr_len_bits, sdu_len_bits, cipher);
            fillbits::addition::write(mac_block, Some(num_fill_bits));

            // We're done with this packet
            self.mac_hdr_is_written = true;
            true
        } else if slot_cap_bits < MIN_SLOT_CAP_FOR_RES_FRAG_START
            || slot_cap_bits < hdr_len_bits - chan_alloc_len_bits
            || (chan_alloc_len_bits > 0 && sdu_len_bits < 2)
        {
            // Not enough room to start fragmentation: either the remaining slot capacity is
            // below the minimum threshold, or the MAC-RESOURCE header alone doesn't fit in the
            // remaining space. Defer the entire PDU to the next frame.
            tracing::debug!(
                "-> does_not_fit (cap={} hdr={}), trying again next frame",
                slot_cap_bits,
                hdr_len_bits - chan_alloc_len_bits
            );
            false
        } else {
            // We need to start fragmentation. The channel allocation of a fragmented message
            // "shall be included within the MAC-END PDU and shall not be included within the
            // MAC-RESOURCE PDU" (TS 100 392-2 clauses 23.4.2.1.1 and 23.5.4.2.1); the usage marker
            // stays in the MAC-RESOURCE (clause 23.5.4.2.1).
            self.end_chan_alloc = self.resource.chan_alloc_element.take();
            let hdr_len_bits = hdr_len_bits - chan_alloc_len_bits;
            self.resource.length_ind = 0b111111; // Start of fragmentation
            assert!(num_fill_bits == 0, "Got {} fill bits upon frag start", num_fill_bits);
            // Without the channel allocation the whole TM-SDU may fit; one bit is kept for the
            // MAC-END, and the block is completed with fill bits (clause 23.4.2.2).
            let sdu_bits = if self.end_chan_alloc.is_some() {
                min(slot_cap_bits - hdr_len_bits, sdu_len_bits - 1)
            } else {
                slot_cap_bits - hdr_len_bits
            };
            self.resource.fill_bits = hdr_len_bits + sdu_bits < slot_cap_bits;

            tracing::debug!(
                "-> Fragged {:?} sdu {}",
                self.resource,
                self.sdu
                    .raw_dump_bin(false, false, self.sdu.get_pos(), self.sdu.get_pos() + sdu_bits)
            );

            let start = mac_block.get_pos();
            self.write_resource_header(mac_block, cipher.as_deref());
            mac_block.copy_bits(&mut self.sdu, sdu_bits);
            self.encrypt_resource(mac_block, start, hdr_len_bits, sdu_bits, cipher);
            fillbits::addition::write(mac_block, None);

            // More fragments follow
            self.mac_hdr_is_written = true;
            false
        }
    }

    /// After MAC-RESOURCE was output using get_first_chunk, call this function to consume
    /// next chunks. Based on capacity, will determine whether to make a MAC-FRAG or
    /// MAC-END.
    /// Returns true when MAC-END (DL) was created and no further fragments are needed.
    /// The MAC-END carries the channel allocation of the fragmented PDU, if it has one.
    fn get_frag_or_end_chunk(&mut self, mac_block: &mut BitBuffer, cipher: Option<&mut SlotCipher>) -> bool {
        // Some sanity checks
        assert!(self.mac_hdr_is_written, "MAC header should be previously written");

        // Check if we can fit all in a MAC-END message
        let sdu_bits = self.sdu.get_len_remaining();
        let chan_alloc_len_bits = self.end_chan_alloc.as_ref().map_or(0, |c| c.compute_len());
        let macend_len_bits = MacEndDl::compute_hdr_len(false, false) + chan_alloc_len_bits + sdu_bits;
        let macend_len_bytes = (macend_len_bits + 7) / 8;
        let slot_cap_bits = mac_block.get_len_remaining();

        // tracing::trace!("MAC-END would have length: {} bits, {} bytes, slot capacity: {} bits",
        //     macend_len_bits, macend_len_bytes, slot_cap);
        if macend_len_bytes * 8 <= slot_cap_bits {
            // Fits in single MAC-END
            let num_fill_bits = fillbits::addition::compute_required(macend_len_bits, slot_cap_bits);
            let pdu = MacEndDl {
                fill_bits: num_fill_bits > 0,
                pos_of_grant: 0,
                length_ind: macend_len_bytes as u8,
                slot_granting_element: None,
                chan_alloc_element: self.end_chan_alloc.take(),
            };

            tracing::debug!(
                "-> {:?} sdu {}",
                pdu,
                self.sdu
                    .raw_dump_bin(false, false, self.sdu.get_pos(), self.sdu.get_pos() + sdu_bits)
            );

            // Write MAC-END header followed by TM-SDU. Encrypted: everything after the channel
            // allocation flag (TS 100 392-7 clause 6.7.1.2), i.e. the channel allocation element,
            // if any, and the TM-SDU.
            pdu.to_bitbuf(mac_block);
            let sdu_start = mac_block.get_pos();
            mac_block.copy_bits(&mut self.sdu, sdu_bits);
            self.encrypt_bits(mac_block, sdu_start - chan_alloc_len_bits, chan_alloc_len_bits + sdu_bits, cipher);

            // Write fill bits (if needed)
            if num_fill_bits > 0 {
                mac_block.write_bit(1);
                mac_block.write_zeroes(num_fill_bits - 1);
            }
            // We're done with this packet
            true
        } else if slot_cap_bits < MIN_SLOT_CAP_FOR_FRAG {
            // Not worth (or possible) to place a fragment here. Rather wait for a new slot
            // We do nothing and simply return that more work is needed
            tracing::debug!("-> does_not_fit, trying again next frame");
            false
        } else {
            // Need MAC-FRAG, fill slot (or don't fill, if the MAC-END hdr size is the reason we go for MAC-FRAG)
            let macfrag_hdr_len = 4;
            let sdu_bits_in_frag = min(slot_cap_bits - macfrag_hdr_len, sdu_bits);
            let num_fill_bits = slot_cap_bits - macfrag_hdr_len - sdu_bits_in_frag;

            let pdu = MacFragDl {
                fill_bits: num_fill_bits > 0,
            };

            tracing::debug!(
                "-> {:?} sdu {}",
                pdu,
                self.sdu
                    .raw_dump_bin(false, false, self.sdu.get_pos(), self.sdu.get_pos() + sdu_bits)
            );

            pdu.to_bitbuf(mac_block);
            let sdu_start = mac_block.get_pos();
            mac_block.copy_bits(&mut self.sdu, sdu_bits_in_frag);
            self.encrypt_bits(mac_block, sdu_start, sdu_bits_in_frag, cipher);

            if num_fill_bits > 0 {
                mac_block.write_bit(1);
                mac_block.write_zeroes(num_fill_bits - 1);
            }

            false
        }
    }

    /// Writes the next chunk to the bitbuffer, if there is space.
    /// First chunk is the provided resource, possibly changed to indicate fragmentation.
    /// Subsequent chunks are MAC-FRAG or MAC-END.
    /// Returns bool is_fully_transmitted
    pub fn get_next_chunk(&mut self, mac_block: &mut BitBuffer) -> bool {
        self.get_next_chunk_ciphered(mac_block, None).is_complete()
    }

    fn discard(&mut self, reason: &str) -> ChunkStatus {
        tracing::warn!("UMAC: discarding encrypted downlink {:?}: {}", self.resource.addr, reason);
        self.discarded = true;
        if let Some(reporter) = &self.tx_reporter
            && reporter.get_state() == tetra_core::TxState::Pending
        { reporter.mark_discarded(); }
        ChunkStatus::Discarded
    }

    /// Missing/replaced cipher keys end this PDU, never retry it under a future
    /// session's key. A busy slot remains retryable. No plaintext fallback.
    pub fn get_next_chunk_ciphered(&mut self, mac_block: &mut BitBuffer, mut cipher: Option<&mut SlotCipher>) -> ChunkStatus {
        assert!(!self.is_fully_transmitted && !self.discarded, "PDU already finished");
        if self.encrypted {
            let Some(c) = cipher.as_deref_mut() else { return self.discard("cipher unavailable"); };
            if c.cipher.is_class3() {
                let Some(addr) = self.resource.addr else { return self.discard("address unavailable"); };
                if addr.ssi_type != SsiType::Gssi {
                    let Some(dck) = c.cipher.dck_for_address(addr) else { return self.discard("DCK unavailable (session ended)"); };
                    if self.dck.as_ref().is_some_and(|previous| previous.0 != dck.0) {
                        return self.discard("DCK changed during queued transmission");
                    }
                    c.kss = c.cipher.kss_for_dck(c.carrier, c.time, false, &dck);
                    self.dck = Some(dck);
                } else {
                    c.kss = c.cipher.kss(c.carrier, c.time, false);
                }
            }
            if c.used { return ChunkStatus::Deferred; }
        }
        assert!(
            mac_block.get_len_written() % 8 == 0 || mac_block.get_len_remaining() == 0,
            "mac_block must be full or byte aligned before writing"
        );

        self.is_fully_transmitted = if !self.mac_hdr_is_written {
            // First chunk, write MAC-RESOURCE
            self.get_resource_chunk(mac_block, cipher)
        } else {
            // Subsequent chunks, write MAC-FRAG or MAC-END
            self.get_frag_or_end_chunk(mac_block, cipher)
        };

        // If we're done now, we'll report the PDUs full transmission.
        if self.is_fully_transmitted
            && let Some(tx_reporter) = &self.tx_reporter
        {
            tx_reporter.mark_transmitted();
        }

        if self.is_fully_transmitted { ChunkStatus::Complete } else { ChunkStatus::Deferred }
    }
}

impl BsFragger {
    /// Writes the MAC-RESOURCE header; for an encrypted PDU the SSI is replaced by its ESI
    /// (TS 100 392-7 clause 4.2.6). USSI and SMI addresses are not encrypted.
    fn write_resource_header(&self, mac_block: &mut BitBuffer, cipher: Option<&SlotCipher>) {
        match (self.encrypted, cipher) {
            (true, Some(c)) => {
                let mut hdr = self.resource.clone();
                if let Some(addr) = hdr.addr.as_mut()
                    && matches!(addr.ssi_type, SsiType::Ssi | SsiType::Issi | SsiType::Gssi)
                {
                    addr.ssi = c.cipher.esi(addr.ssi);
                }
                hdr.encryption_mode = c.cipher.encryption_mode();
                hdr.to_bitbuf(mac_block);
            }
            _ => self.resource.to_bitbuf(mac_block),
        }
    }

    /// Encrypts a MAC-RESOURCE just written at `start`: all information after the channel
    /// allocation flag (the channel allocation element, if any, then the TM-SDU) — TS 100 392-7
    /// clause 6.7.1.2.
    fn encrypt_resource(&self, mac_block: &mut BitBuffer, start: usize, hdr_len: usize, sdu_len: usize, cipher: Option<&mut SlotCipher>) {
        let chan_alloc_len = self.resource.chan_alloc_element.as_ref().map_or(0, |c| c.compute_len());
        self.encrypt_bits(mac_block, start + hdr_len - chan_alloc_len, chan_alloc_len + sdu_len, cipher);
    }

    /// XORs `len` bits at `start` with the slot's key stream, if this PDU is encrypted.
    fn encrypt_bits(&self, mac_block: &mut BitBuffer, start: usize, len: usize, cipher: Option<&mut SlotCipher>) {
        if !self.encrypted {
            return;
        }
        let Some(c) = cipher else { return };
        c.kss.apply(mac_block, start, len, c.kss_offset);
        c.used = true;
    }
}

impl Drop for BsFragger {
    fn drop(&mut self) {
        if !self.is_fully_transmitted
            && let Some(tx_reporter) = &self.tx_reporter
            && tx_reporter.get_state() == tetra_core::TxState::Pending
        {
            tx_reporter.mark_discarded();
        }
    }
}

#[cfg(test)]
mod tests {
    use tetra_core::{
        TxState,
        address::{SsiType, TetraAddress},
        debug,
    };

    use crate::umac::subcomp::bs_sched::{SCH_F_CAP, SCH_HD_CAP};

    use super::*;
    fn get_default_resource() -> MacResource {
        MacResource {
            fill_bits: false,
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: false,
            length_ind: 0,
            addr: Some(TetraAddress {
                ssi_type: SsiType::Issi,
                ssi: 1234,
            }),
            event_label: None,
            usage_marker: None,
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: None,
        }
    }

    fn test_cipher() -> CellCipher {
        let aie = tetra_config::bluestation::CfgAie {
            class3: None,
        enabled: true,
            ksg: 1,
            sckn: 1,
            sck: tetra_config::bluestation::CipherKey([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23]),
            sck_vn: 2,
            clear_groups: vec![],
        };
        CellCipher::new(&aie, &[3681], 2, 1)
    }

    /// An encrypted MAC-RESOURCE carries the ESI and the encryption mode, and decrypting
    /// everything after the channel allocation flag gives back the clear TM-SDU.
    #[test]
    fn encrypted_resource_round_trip() {
        let cipher = test_cipher();
        let t = tetra_core::TdmaTime { t: 1, f: 3, m: 7, h: 42 };
        let sdu_bits = "1100101011110000101011001100110101010101111000011110000";

        let mut pdu = get_default_resource();
        pdu.encryption_mode = 1; // any non-zero value marks the PDU for encryption
        let mut slot = SlotCipher {
            cipher: &cipher,
            carrier: 3681, time: tetra_core::TdmaTime { t: 1, f: 3, m: 7, h: 42 },
            kss: cipher.kss(3681, t, false),
            kss_offset: 0,
            used: false,
        };
        let mut block = BitBuffer::new(SCH_F_CAP);
        assert!(BsFragger::new(pdu, BitBuffer::from_bitstr(sdu_bits), None).get_next_chunk_ciphered(&mut block, Some(&mut slot)).is_complete());
        assert!(slot.used);

        block.seek(0);
        let parsed = MacResource::from_bitbuf(&mut block).unwrap();
        assert_eq!(parsed.encryption_mode, 0b10, "even SCK-VN");
        assert_eq!(parsed.addr.unwrap().ssi, cipher.esi(1234));
        let hdr_len = parsed.compute_header_len();

        // Decrypt the TM-SDU with the same key stream: the clear bits come back.
        let kss = cipher.kss(3681, t, false);
        kss.apply(&mut block, hdr_len, sdu_bits.len(), 0);
        block.seek(hdr_len);
        let clear: String = (0..sdu_bits.len()).map(|_| if block.read_bit().unwrap() == 1 { '1' } else { '0' }).collect();
        assert_eq!(clear, sdu_bits);
    }

    /// A slot carries at most one encrypted PDU, and an encrypted PDU never goes out without a
    /// cipher.
    #[test]
    fn encrypted_pdu_waits_for_a_fresh_slot() {
        let cipher = test_cipher();
        let mut pdu = get_default_resource();
        pdu.encryption_mode = 1;
        let mut block = BitBuffer::new(SCH_F_CAP);
        let mut fragger = BsFragger::new(pdu, BitBuffer::from_bitstr("1010"), None);

        let mut slot = SlotCipher {
            cipher: &cipher,
            carrier: 3681, time: tetra_core::TdmaTime { t: 1, f: 3, m: 7, h: 42 },
            kss: cipher.kss(3681, tetra_core::TdmaTime { t: 1, f: 1, m: 1, h: 0 }, false),
            kss_offset: 0,
            used: true,
        };
        assert!(fragger.get_next_chunk_ciphered(&mut block, Some(&mut slot)).needs_retry(), "slot already used");
        slot.used = false;
        assert!(fragger.get_next_chunk_ciphered(&mut block, Some(&mut slot)).is_complete());
    }

    #[test]
    fn sc3_fragmentation_uses_the_recipient_dck_for_every_fragment() {
        use tetra_config::bluestation::{SharedConfig, CipherKey, CfgAie, CfgClass3};
        let cfg = tetra_config::bluestation::from_toml_str(include_str!("../../../../../example_config/config.toml")).unwrap();
        let shared = SharedConfig::from_parts(cfg,None);
        let aie = CfgAie { enabled: true, ksg: 2, sck: CipherKey([0;10]), sckn: 1, sck_vn: 0, clear_groups: vec![],
            class3: Some(CfgClass3 { cck: CipherKey([0x11;10]), cck_id: 42 }) };
        let cipher = CellCipher::new(&aie,&[3681],2,1).with_config(shared.clone());
        shared.state_write().sc3_dcks.insert(1234,CipherKey([0x22;10]));
        let mut resource = get_default_resource(); resource.encryption_mode = 1;
        let bits = "10110011".repeat(60);
        let mut fragger = BsFragger::new(resource,BitBuffer::from_bitstr(&bits),None);
        let mut reconstructed = String::new();
        for frame in 1..20 {
            let t = tetra_core::TdmaTime { t: 1, f: frame, m: 1, h: 1 };
            let mut slot = SlotCipher { cipher: &cipher, carrier: 3681, time: t, kss: cipher.kss(3681,t,false), kss_offset: 0, used: false };
            let mut block = BitBuffer::new(SCH_HD_CAP);
            let before = fragger.sdu.get_len_remaining();
            let first = !fragger.mac_hdr_is_written;
            let done = fragger.get_next_chunk_ciphered(&mut block,Some(&mut slot)).is_complete();
            let n = before-fragger.sdu.get_len_remaining();
            assert!(slot.used); block.seek(0);
            let offset = if first {
                let header = MacResource::from_bitbuf(&mut block).unwrap();
                assert_eq!(header.addr.unwrap().ssi,cipher.esi(1234));
                assert_eq!(header.encryption_mode,0b10); block.get_pos()
            } else if done { MacEndDl::from_bitbuf(&mut block).unwrap(); block.get_pos() }
              else { MacFragDl::from_bitbuf(&mut block).unwrap(); block.get_pos() };
            let dck_kss = cipher.kss_for(3681,t,false,TetraAddress::issi(1234)).unwrap();
            dck_kss.apply(&mut block,offset,n,0); block.seek(offset);
            for _ in 0..n { reconstructed.push(if block.read_bit().unwrap()==1 {'1'} else {'0'}); }
            if done { break; }
        }
        assert_eq!(reconstructed,bits);
    }

    #[test]
    fn test_single_chunk() {
        debug::setup_logging_verbose();
        let pdu = get_default_resource();
        let sdu = BitBuffer::from_bitstr("111000111");
        let mut mac_block = BitBuffer::new(SCH_F_CAP);

        let mut fragger = BsFragger::new(pdu, sdu, None);
        let done = fragger.get_next_chunk(&mut mac_block);
        mac_block.seek(0);

        assert!(done, "Should be done in single chunk");
        tracing::info!("MAC block: {}", mac_block.dump_bin());
    }

    #[test]
    fn test_four_chunks() {
        debug::setup_logging_verbose();
        let vec = "01010110010011000010101010010010110101010110010011001011111110101011001010010110111001011111111111100010011000000011010011001110010111110010100100010111010110000010010001101000011000000111101011010001001111001110110100000101010111110100010000100101001100011110010111001010101001110110111010001001101101111100111001000001111100101010000010111";
        let mut reconstructed = String::new();
        let pdu = get_default_resource();
        let sdu = BitBuffer::from_bitstr(vec);
        let mut fragger = BsFragger::new(pdu, sdu, None);

        let mut mac_block = BitBuffer::new(SCH_HD_CAP);
        let done = fragger.get_next_chunk(&mut mac_block);
        mac_block.seek(0);
        let pdu = MacResource::from_bitbuf(&mut mac_block).unwrap();
        mac_block.set_raw_start(mac_block.get_raw_pos());
        tracing::info!("[1]: {}: {}", pdu, mac_block.dump_bin());
        reconstructed += &mac_block.to_bitstr();
        // tracing::info!("[1] reconstructed so far: {}", reconstructed);
        assert!(!done, "Should take four blocks");

        let mut mac_block = BitBuffer::new(SCH_HD_CAP);
        let done = fragger.get_next_chunk(&mut mac_block);
        mac_block.seek(0);
        let pdu = MacFragDl::from_bitbuf(&mut mac_block).unwrap();
        mac_block.set_raw_start(mac_block.get_raw_pos());
        tracing::info!("[2]: {}: {}", pdu, mac_block.dump_bin());
        reconstructed += &mac_block.to_bitstr();
        // tracing::info!("[1] reconstructed so far: {}", reconstructed);
        assert!(!done, "Should take four blocks");

        let mut mac_block = BitBuffer::new(SCH_HD_CAP);
        let done = fragger.get_next_chunk(&mut mac_block);
        mac_block.seek(0);
        let pdu = MacFragDl::from_bitbuf(&mut mac_block).unwrap();
        mac_block.set_raw_start(mac_block.get_raw_pos());
        tracing::info!("[3]: {}: {}", pdu, mac_block.dump_bin());
        reconstructed += &mac_block.to_bitstr();
        // tracing::info!("[1] reconstructed so far: {}", reconstructed);
        assert!(!done, "Should take four blocks");

        let mut mac_block = BitBuffer::new(SCH_HD_CAP);
        let done = fragger.get_next_chunk(&mut mac_block);
        mac_block.seek(0);
        let pdu = MacEndDl::from_bitbuf(&mut mac_block).unwrap();
        mac_block.set_raw_start(mac_block.get_raw_pos());
        tracing::info!("[4]: {}: {}", pdu, mac_block.dump_bin());
        reconstructed += &mac_block.to_bitstr();
        tracing::info!("     Reconstructed: {}", reconstructed);
        assert!(done, "Should take four blocks");

        // Test that the original vec is contained in the reconstructed string
        // We'll just assume the fill bits check out..
        assert!(
            reconstructed.starts_with(vec),
            "Original vec should be contained in reconstructed string"
        );
    }

    #[test]
    fn test_four_chunks_with_tx_reporter() {
        debug::setup_logging_verbose();
        let vec = "01010110010011000010101010010010110101010110010011001011111110101011001010010110111001011111111111100010011000000011010011001110010111110010100100010111010110000010010001101000011000000111101011010001001111001110110100000101010111110100010000100101001100011110010111001010101001110110111010001001101101111100111001000001111100101010000010111";
        let mut reconstructed = String::new();
        let pdu = get_default_resource();
        let sdu = BitBuffer::from_bitstr(vec);
        let reporter = TxReporter::new_unacked();
        let mut fragger = BsFragger::new(pdu, sdu, Some(reporter.clone()));

        let mut mac_block = BitBuffer::new(SCH_HD_CAP);
        let done = fragger.get_next_chunk(&mut mac_block);
        mac_block.seek(0);
        let pdu = MacResource::from_bitbuf(&mut mac_block).unwrap();
        mac_block.set_raw_start(mac_block.get_raw_pos());
        tracing::info!("[1]: {}: {}", pdu, mac_block.dump_bin());
        reconstructed += &mac_block.to_bitstr();
        // tracing::info!("[1] reconstructed so far: {}", reconstructed);
        assert!(!done, "Should take four blocks");
        assert!(!reporter.is_in_final_state() && !reporter.is_transmitted());

        let mut mac_block = BitBuffer::new(SCH_HD_CAP);
        let done = fragger.get_next_chunk(&mut mac_block);
        mac_block.seek(0);
        let pdu = MacFragDl::from_bitbuf(&mut mac_block).unwrap();
        mac_block.set_raw_start(mac_block.get_raw_pos());
        tracing::info!("[2]: {}: {}", pdu, mac_block.dump_bin());
        reconstructed += &mac_block.to_bitstr();
        // tracing::info!("[1] reconstructed so far: {}", reconstructed);
        assert!(!done, "Should take four blocks");
        assert!(!reporter.is_in_final_state() && !reporter.is_transmitted());

        let mut mac_block = BitBuffer::new(SCH_HD_CAP);
        let done = fragger.get_next_chunk(&mut mac_block);
        mac_block.seek(0);
        let pdu = MacFragDl::from_bitbuf(&mut mac_block).unwrap();
        mac_block.set_raw_start(mac_block.get_raw_pos());
        tracing::info!("[3]: {}: {}", pdu, mac_block.dump_bin());
        reconstructed += &mac_block.to_bitstr();
        // tracing::info!("[1] reconstructed so far: {}", reconstructed);
        assert!(!done, "Should take four blocks");
        assert!(!reporter.is_in_final_state() && !reporter.is_transmitted());

        let mut mac_block = BitBuffer::new(SCH_HD_CAP);
        let done = fragger.get_next_chunk(&mut mac_block);
        mac_block.seek(0);
        let pdu = MacEndDl::from_bitbuf(&mut mac_block).unwrap();
        mac_block.set_raw_start(mac_block.get_raw_pos());
        tracing::info!("[4]: {}: {}", pdu, mac_block.dump_bin());
        reconstructed += &mac_block.to_bitstr();
        tracing::info!("     Reconstructed: {}", reconstructed);
        assert!(done, "Should take four blocks");
        assert!(reporter.is_in_final_state() && reporter.is_transmitted());

        // Test that the original vec is contained in the reconstructed string
        // We'll just assume the fill bits check out..
        assert!(
            reconstructed.starts_with(vec),
            "Original vec should be contained in reconstructed string"
        );
    }

    #[test]
    fn test_drop_marks_discarded_when_not_fully_transmitted() {
        debug::setup_logging_verbose();
        let pdu = get_default_resource();
        let sdu = BitBuffer::from_bitstr("10101010");
        let reporter = TxReporter::new_unacked();

        let _fragger = BsFragger::new(pdu, sdu, Some(reporter.clone()));
        drop(_fragger);

        assert_eq!(reporter.get_state(), TxState::Discarded);
        assert!(reporter.is_in_final_state());
        assert!(!reporter.is_transmitted());
    }

    fn group_setup_resource() -> MacResource {
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        let mut pdu = get_default_resource();
        pdu.addr = Some(TetraAddress { ssi_type: SsiType::Gssi, ssi: 9 });
        pdu.usage_marker = Some(7);
        pdu.chan_alloc_element = Some(ChanAllocElement {
            alloc_type: ChanAllocType::Replace,
            ts_assigned: [false, true, false, false],
            ul_dl_assigned: UlDlAssignment::Both,
            clch_permission: true,
            cell_change_flag: false,
            carrier_num: 1530,
            ext: None,
            mon_pattern: 1,
            frame18_mon_pattern: None,
        });
        pdu
    }

    fn read_bitstr(buf: &mut BitBuffer, n: usize) -> String {
        (0..n).map(|_| if buf.read_bit().unwrap() == 1 { '1' } else { '0' }).collect()
    }

    /// Sends `sdu` with the group D-SETUP header in a block that has `first_cap` bits left, then
    /// in a fresh block. Returns the first fragment's header, the MAC-END and the TM-SDU bits.
    fn fragment_group_setup(sdu: &str, first_cap: usize) -> (MacResource, MacEndDl, String) {
        let mut fragger = BsFragger::new(group_setup_resource(), BitBuffer::from_bitstr(sdu), None);
        let mut block1 = BitBuffer::new(first_cap);
        assert!(!fragger.get_next_chunk(&mut block1), "does not fit, fragmented");
        assert_eq!(block1.get_len_remaining(), 0, "the first fragment completes the block");
        block1.seek(0);
        let first = MacResource::from_bitbuf(&mut block1).unwrap();
        let first_bits = sdu.len() - fragger.sdu.get_len_remaining();
        let mut got = read_bitstr(&mut block1, first_bits);
        if first.fill_bits {
            assert_eq!(block1.read_bit().unwrap(), 1, "fill bits start with a 1");
        } else {
            assert_eq!(block1.get_len_remaining(), 0);
        }

        let mut block2 = BitBuffer::new(SCH_F_CAP);
        assert!(fragger.get_next_chunk(&mut block2), "the MAC-END completes the PDU");
        block2.seek(0);
        let end = MacEndDl::from_bitbuf(&mut block2).unwrap();
        got.push_str(&read_bitstr(&mut block2, sdu.len() - first_bits));
        (first, end, got)
    }

    /// TS 100 392-2 clauses 23.4.2.1.1 and 23.5.4.2.1: the channel allocation of a fragmented
    /// message "shall be included within the MAC-END PDU and shall not be included within the
    /// MAC-RESOURCE PDU". Field case: a group D-SETUP behind an individually addressed PDU in the
    /// same MCCH slot.
    #[test]
    fn fragmented_pdu_carries_its_channel_allocation_in_the_mac_end() {
        let sdu = "001001000111000000000001110111000000010011000001001010010001101101010011001010";
        let (first, end, got) = fragment_group_setup(sdu, 148);
        assert_eq!(first.length_ind, 0b111111, "start of fragmentation");
        assert!(first.chan_alloc_element.is_none(), "no channel allocation with the first fragment");
        assert_eq!(first.usage_marker, Some(7), "the usage marker stays in the MAC-RESOURCE");
        let ca = end.chan_alloc_element.expect("channel allocation in the MAC-END");
        assert_eq!(ca.carrier_num, 1530);
        assert_eq!(ca.ts_assigned, [false, true, false, false]);
        assert_eq!(got, sdu, "both fragments together are the TM-SDU");
    }

    /// Without the channel allocation the whole TM-SDU would fit in the first fragment: one bit
    /// is kept for the MAC-END and the block is completed with fill bits (clause 23.4.2.2).
    #[test]
    fn fragmented_pdu_with_channel_allocation_keeps_a_bit_for_the_mac_end() {
        let sdu = "001001000111000000000001110111000000010011000001001010010001101101010011001010";
        let hdr = group_setup_resource().compute_header_len();
        let (first, end, got) = fragment_group_setup(sdu, hdr + sdu.len() - 8);
        assert_eq!(first.length_ind, 0b111111);
        assert!(first.fill_bits);
        assert!(first.chan_alloc_element.is_none());
        assert!(end.chan_alloc_element.is_some());
        assert_eq!(got, sdu);
    }

    /// A start of fragmentation that has to wait for the next block keeps the channel allocation:
    /// sent whole, the PDU carries it in the MAC-RESOURCE.
    #[test]
    fn deferred_pdu_keeps_its_channel_allocation_in_the_mac_resource() {
        let sdu = "001001000111000000000001110111000000010011000001001010010001101101010011001010";
        let mut fragger = BsFragger::new(group_setup_resource(), BitBuffer::from_bitstr(sdu), None);
        let mut small = BitBuffer::new(24);
        assert!(!fragger.get_next_chunk(&mut small));
        assert_eq!(small.get_len_written(), 0, "nothing sent in the small block");
        let mut block = BitBuffer::new(SCH_F_CAP);
        assert!(fragger.get_next_chunk(&mut block));
        block.seek(0);
        let res = MacResource::from_bitbuf(&mut block).unwrap();
        assert_ne!(res.length_ind, 0b111111);
        assert!(res.chan_alloc_element.is_some());
    }

    /// The channel allocation in an encrypted MAC-END is encrypted with the TM-SDU: everything
    /// after the channel allocation flag (TS 100 392-7 clause 6.7.1.2).
    #[test]
    fn encrypted_mac_end_encrypts_its_channel_allocation() {
        let cipher = test_cipher();
        let sdu = "001001000111000000000001110111000000010011000001001010010001101101010011001010";
        let mut pdu = group_setup_resource();
        pdu.encryption_mode = 1;
        let ca_len = pdu.chan_alloc_element.as_ref().unwrap().compute_len();
        let mut fragger = BsFragger::new(pdu, BitBuffer::from_bitstr(sdu), None);

        let t1 = tetra_core::TdmaTime { t: 1, f: 3, m: 7, h: 42 };
        let mut slot = SlotCipher { cipher: &cipher, carrier: 3681, time: t1, kss: cipher.kss(3681, t1, false), kss_offset: 0, used: false };
        let mut block1 = BitBuffer::new(148);
        assert!(fragger.get_next_chunk_ciphered(&mut block1, Some(&mut slot)).needs_retry());
        let rest = fragger.sdu.get_len_remaining();
        assert!(rest > 0 && rest < sdu.len());

        let t2 = tetra_core::TdmaTime { t: 1, f: 4, m: 7, h: 42 };
        let mut slot = SlotCipher { cipher: &cipher, carrier: 3681, time: t2, kss: cipher.kss(3681, t2, false), kss_offset: 0, used: false };
        let mut block2 = BitBuffer::new(SCH_F_CAP);
        assert!(fragger.get_next_chunk_ciphered(&mut block2, Some(&mut slot)).is_complete());

        // Clear up to and including the channel allocation flag, then key stream from its start
        let clear_hdr = MacEndDl::compute_hdr_len(false, false);
        cipher.kss(3681, t2, false).apply(&mut block2, clear_hdr, ca_len + rest, 0);
        block2.seek(0);
        let end = MacEndDl::from_bitbuf(&mut block2).unwrap();
        assert_eq!(end.chan_alloc_element.expect("channel allocation").carrier_num, 1530);
        assert_eq!(read_bitstr(&mut block2, rest), sdu[sdu.len() - rest..]);
    }
}
