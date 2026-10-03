//! The "SDU" element of MLE PDUs (EN 300 392-2 clause 18.4.1.4): an MM or CMCE PDU carried
//! inside the MLE PDU. It has no P-bit and no length field, so it runs to the end of the PDU.

use tetra_core::BitBuffer;

/// Take everything from the current position to the end as the SDU (None if nothing is left).
pub(crate) fn read_rest(buffer: &mut BitBuffer) -> Option<BitBuffer> {
    let len = buffer.get_len_remaining();
    if len == 0 {
        return None;
    }
    let mut sdu = BitBuffer::new(len);
    sdu.copy_bits(buffer, len);
    sdu.seek(0);
    Some(sdu)
}

/// Append the whole SDU.
pub(crate) fn write(buffer: &mut BitBuffer, sdu: &BitBuffer) {
    let mut src = sdu.clone();
    src.seek(0);
    let len = src.get_len();
    buffer.copy_bits(&mut src, len);
}
