use std::collections::HashMap;

use tetra_core::{BitBuffer, TdmaTime, TetraAddress, Todo, multiframes};

use crate::umac::subcomp::defrag::{DefragBuffer, DefragBufferState};

const DEFRAG_BUF_MAX_LEN: usize = 4096;
/// One multiframe. A granting delay may push the next fragment up to 13 opportunities
/// (~13 frames) out, so anything shorter can drop a legitimately slow burst mid-reassembly.
const DEFRAG_TS_BEFORE_TIMEOUT: i32 = multiframes!(1);
/// Max concurrent reassembly contexts per timeslot. The uplink schedule is 18 frames deep,
/// so a handful of MSes can legitimately be reassembling at once; the cap only bounds the
/// map against an MS (or attacker) starting bursts it never finishes.
const DEFRAG_MAX_CONTEXTS_PER_TS: usize = 32;

/// Defragmenter suitable for BS use
/// Maintains a set of DefragBuffers per timeslot, indexed by SSI.
/// This allows multiple MSes to send fragmented data in the same timeslot.
pub struct BsDefrag {
    pub buffers: [HashMap<u32, DefragBuffer>; 4],
}

impl BsDefrag {
    pub fn new() -> Self {
        Self {
            buffers: [HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new()],
        }
    }

    pub fn reset(&mut self) {
        for map in &mut self.buffers {
            map.clear();
        }
    }

    /// Drops reassembly contexts that have gone quiet. Called every tick from the UMAC.
    pub fn age_buffers(&mut self, t: TdmaTime) {
        for map in &mut self.buffers {
            // Remove the key, don't just reset the buffer: uplink SSIs are attacker-chosen,
            // so leaving a reset entry behind still lets the map grow without bound.
            map.retain(|ssi, buffer| {
                let stale = t.diff(buffer.t_last) > DEFRAG_TS_BEFORE_TIMEOUT;
                if stale {
                    tracing::debug!("defrag_buffer for ssi {} last seen {} timed out, dropping", ssi, buffer.t_last);
                }
                !stale
            });
        }
    }

    /// Inserts a first fragment into a fragbuffer.
    pub fn insert_first(&mut self, bitbuffer: &mut BitBuffer, t: TdmaTime, addr: TetraAddress, aie_info: Option<Todo>) {
        // Check if buffer already exists for this ssi/timeslot
        // Remove and discard, if so.
        let ts = (t.t - 1) as usize;
        let ssi = addr.ssi;
        let mut buf = if let Some(mut buf) = self.buffers[ts].remove(&ssi) {
            // MS sent a new burst before the previous one completed — normal under RF loss.
            // Drop the incomplete burst silently and start fresh.
            tracing::debug!(
                "defrag_buffer: ts {} ssi {} started new burst before previous completed, resetting",
                t.t,
                ssi
            );
            buf.reset();
            buf
        } else {
            DefragBuffer::new()
        };

        // Initialize target buffer
        buf.state = DefragBufferState::Active;
        buf.addr = addr;
        buf.t_first = t;
        buf.t_last = t;
        buf.num_frags = 1;
        buf.aie_info = aie_info;

        // Copy the bitbuffer data from pos to end into our fragbuffer
        buf.buffer.copy_bits(bitbuffer, bitbuffer.get_len_remaining());

        tracing::debug!(
            "defrag_buffer for ts {} ssi: {}, t: {}-{}, frags: {}: {}",
            t.t,
            buf.addr.ssi,
            buf.t_first,
            buf.t_last,
            buf.num_frags,
            buf.buffer.dump_bin()
        );

        // Bound the number of concurrent contexts per timeslot, evicting the least recently
        // fed one. Aging alone is not enough: a flood of starts with a rotating SSI can add
        // thousands of entries within a single timeout window.
        while self.buffers[ts].len() >= DEFRAG_MAX_CONTEXTS_PER_TS {
            let Some(oldest) = self.buffers[ts].iter().max_by_key(|(_, b)| t.diff(b.t_last)).map(|(k, _)| *k) else {
                break;
            };
            tracing::warn!("defrag_buffer: ts {} at capacity, evicting oldest context ssi {}", t.t, oldest);
            self.buffers[ts].remove(&oldest);
        }

        self.buffers[ts].insert(ssi, buf);
    }

    pub fn insert_next(&mut self, bitbuffer: &mut BitBuffer, ssi: u32, t: TdmaTime) {
        let ts = (t.t - 1) as usize;
        let buf = match self.buffers[ts].get_mut(&ssi) {
            Some(b) => b,
            None => {
                tracing::debug!(
                    "defrag_buffer for ts {} ssi {} not found (start burst not seen — normal on RF loss)",
                    t.t,
                    ssi
                );
                return;
            }
        };

        if buf.state != DefragBufferState::Active {
            tracing::warn!("defrag_buffer for ts {} ssi {} not active", t.t, ssi);
            return;
        }

        if buf.buffer.get_len() + bitbuffer.get_len_remaining() > DEFRAG_BUF_MAX_LEN {
            tracing::warn!("defrag_buffer for ts {} ssi {} would exceed max len", t.t, ssi);
            buf.reset();
            return;
        }

        buf.t_last = t;
        buf.num_frags += 1;

        // Copy the bitbuffer data from pos to end into our fragbuffer
        buf.buffer.copy_bits(bitbuffer, bitbuffer.get_len_remaining());

        tracing::debug!(
            "defrag_buffer for ts {} ssi: {}, t: {}-{}, frags: {}: {}",
            t.t,
            ssi,
            buf.t_first,
            buf.t_last,
            buf.num_frags,
            buf.buffer.dump_bin()
        );
    }

    /// Inserts the last fragment into a DefragBuffer, and returns the completed object
    pub fn insert_last(&mut self, bitbuffer: &mut BitBuffer, ssi: u32, t: TdmaTime) -> Option<DefragBuffer> {
        // First, insert the last fragment, then reset buffer pos to start
        self.insert_next(bitbuffer, ssi, t);

        // Now take the buffer out of the map
        let ts = (t.t - 1) as usize;
        let mut buf = match self.buffers[ts].remove(&ssi) {
            Some(b) => b,
            None => {
                tracing::debug!(
                    "defrag_buffer for ts {} ssi {} not found (start burst not seen — normal on RF loss)",
                    t.t,
                    ssi
                );
                return None;
            }
        };

        // Update state to complete and return
        buf.state = DefragBufferState::Complete;
        buf.buffer.set_raw_pos(0);
        Some(buf)
    }

    /// Retrieves a read-only reference to the AIE info associated with a DefragBuffer
    pub fn get_aie_info(&self, ssi: u32, t: TdmaTime) -> Option<&Todo> {
        let ts = (t.t - 1) as usize;
        let buf = match self.buffers[ts].get(&ssi) {
            Some(b) => b,
            None => {
                tracing::debug!(
                    "defrag_buffer for ts {} ssi {} not found (start burst not seen — normal on RF loss)",
                    t.t,
                    ssi
                );
                return None;
            }
        };
        if buf.state == DefragBufferState::Inactive {
            tracing::warn!("defrag_buffer for ts {} ssi {} not active", t.t, ssi);
            return None;
        };
        buf.aie_info.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::{address::SsiType, bitbuffer::BitBuffer, debug};

    #[test]
    fn test_3_chunks() {
        debug::setup_logging_verbose();

        let ssi = 1234;
        let mut buf1 = BitBuffer::from_bitstr("000");
        let t1 = TdmaTime::default().add_timeslots(2); // UL time 0
        let mut buf2 = BitBuffer::from_bitstr("111");
        let t2 = t1.add_timeslots(4);
        let mut buf3 = BitBuffer::from_bitstr("0011");
        let t3 = t2.add_timeslots(4);

        let mut defragger = BsDefrag::new();
        let addr = TetraAddress {
            ssi,
            ssi_type: SsiType::Issi,
        };
        defragger.insert_first(&mut buf1, t1, addr, None);
        defragger.insert_next(&mut buf2, ssi, t2);
        let out = defragger.insert_last(&mut buf3, ssi, t3).unwrap();
        assert_eq!(out.buffer.to_bitstr(), "0001110011");
        assert_eq!(out.buffer.get_pos(), 0);
    }
}
