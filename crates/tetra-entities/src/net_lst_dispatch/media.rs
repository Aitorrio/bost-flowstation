//! PCM16 ↔ TETRA ACELP for LST dispatch (requires `asterisk` feature / libtetra-codec).

#[cfg(feature = "asterisk")]
mod ffi {
    use std::ptr::NonNull;

    pub const PCM_SAMPLES_PER_FRAME: usize = 240;
    pub const PCM_SAMPLES_PER_BLOCK: usize = PCM_SAMPLES_PER_FRAME * 2;
    const CODED_BITS_PER_FRAME: usize = 137;
    const CODED_BYTES_PER_FRAME: usize = (CODED_BITS_PER_FRAME + 7) / 8;
    const TMD_BITS_PER_BLOCK: usize = CODED_BITS_PER_FRAME * 2;
    pub const TMD_PACKED_BYTES: usize = (TMD_BITS_PER_BLOCK + 7) / 8;

    #[repr(C)]
    struct RawTetraCodec {
        _private: [u8; 0],
    }

    #[link(name = "tetra-codec")]
    unsafe extern "C" {
        fn tetra_encoder_create() -> *mut RawTetraCodec;
        fn tetra_decoder_create() -> *mut RawTetraCodec;
        fn tetra_codec_destroy(st: *mut RawTetraCodec);
        fn tetra_encode(st: *mut RawTetraCodec, pcm: *const i16, coded: *mut u8);
        fn tetra_decode(st: *mut RawTetraCodec, coded: *const u8, pcm: *mut i16, bfi: i32);
    }

    struct CodecHandle {
        ptr: NonNull<RawTetraCodec>,
    }

    unsafe impl Send for CodecHandle {}

    impl CodecHandle {
        fn from_raw(ptr: *mut RawTetraCodec) -> Option<Self> {
            NonNull::new(ptr).map(|ptr| Self { ptr })
        }
    }

    impl Drop for CodecHandle {
        fn drop(&mut self) {
            unsafe {
                tetra_codec_destroy(self.ptr.as_ptr());
            }
        }
    }

    pub struct LstCodec {
        encoder: CodecHandle,
        decoder: CodecHandle,
        ul_pcm: Vec<i16>,
    }

    impl LstCodec {
        pub fn new() -> Option<Self> {
            let encoder = CodecHandle::from_raw(unsafe { tetra_encoder_create() })?;
            let decoder = CodecHandle::from_raw(unsafe { tetra_decoder_create() })?;
            Some(Self {
                encoder,
                decoder,
                ul_pcm: Vec::with_capacity(PCM_SAMPLES_PER_BLOCK * 2),
            })
        }

        /// Append PCM16 @ 8 kHz; return packed TMD blocks when ready.
        pub fn encode_pcm(&mut self, pcm: &[i16]) -> Vec<Vec<u8>> {
            self.ul_pcm.extend_from_slice(pcm);
            let mut out = Vec::new();
            while self.ul_pcm.len() >= PCM_SAMPLES_PER_BLOCK {
                let mut a = [0i16; PCM_SAMPLES_PER_FRAME];
                let mut b = [0i16; PCM_SAMPLES_PER_FRAME];
                a.copy_from_slice(&self.ul_pcm[..PCM_SAMPLES_PER_FRAME]);
                b.copy_from_slice(&self.ul_pcm[PCM_SAMPLES_PER_FRAME..PCM_SAMPLES_PER_BLOCK]);
                self.ul_pcm.drain(..PCM_SAMPLES_PER_BLOCK);

                let mut ca = [0u8; CODED_BYTES_PER_FRAME];
                let mut cb = [0u8; CODED_BYTES_PER_FRAME];
                unsafe {
                    tetra_encode(self.encoder.ptr.as_ptr(), a.as_ptr(), ca.as_mut_ptr());
                    tetra_encode(self.encoder.ptr.as_ptr(), b.as_ptr(), cb.as_mut_ptr());
                }
                out.push(pack_tmd_block(&ca, &cb));
            }
            out
        }

        pub fn decode_tmd(&mut self, tmd: &[u8]) -> Option<Vec<i16>> {
            let (fa, fb) = split_tmd(tmd)?;
            let mut out = Vec::with_capacity(PCM_SAMPLES_PER_BLOCK);
            for frame in [fa, fb] {
                let mut pcm = [0i16; PCM_SAMPLES_PER_FRAME];
                unsafe {
                    tetra_decode(self.decoder.ptr.as_ptr(), frame.as_ptr(), pcm.as_mut_ptr(), 0);
                }
                out.extend_from_slice(&pcm);
            }
            Some(out)
        }

        pub fn reset_ul(&mut self) {
            self.ul_pcm.clear();
        }
    }

    fn pack_tmd_block(a: &[u8; CODED_BYTES_PER_FRAME], b: &[u8; CODED_BYTES_PER_FRAME]) -> Vec<u8> {
        // Same packing as asterisk audio: two 137-bit frames → packed bytes.
        let mut bits = Vec::with_capacity(TMD_BITS_PER_BLOCK);
        for frame in [a.as_slice(), b.as_slice()] {
            for i in 0..CODED_BITS_PER_FRAME {
                let byte = frame[i / 8];
                let bit = (byte >> (7 - (i % 8))) & 1;
                bits.push(bit);
            }
        }
        let mut out = vec![0u8; TMD_PACKED_BYTES];
        for (i, bit) in bits.iter().enumerate() {
            if *bit != 0 {
                out[i / 8] |= 1 << (7 - (i % 8));
            }
        }
        out
    }

    fn split_tmd(tmd: &[u8]) -> Option<([u8; CODED_BYTES_PER_FRAME], [u8; CODED_BYTES_PER_FRAME])> {
        if tmd.len() < TMD_PACKED_BYTES {
            return None;
        }
        let mut bits = Vec::with_capacity(TMD_BITS_PER_BLOCK);
        for i in 0..TMD_BITS_PER_BLOCK {
            let byte = tmd[i / 8];
            bits.push((byte >> (7 - (i % 8))) & 1);
        }
        let mut a = [0u8; CODED_BYTES_PER_FRAME];
        let mut b = [0u8; CODED_BYTES_PER_FRAME];
        for (fi, dest) in [&mut a, &mut b].into_iter().enumerate() {
            let base = fi * CODED_BITS_PER_FRAME;
            for i in 0..CODED_BITS_PER_FRAME {
                if bits[base + i] != 0 {
                    dest[i / 8] |= 1 << (7 - (i % 8));
                }
            }
        }
        Some((a, b))
    }
}

#[cfg(feature = "asterisk")]
pub use ffi::LstCodec;

#[cfg(not(feature = "asterisk"))]
pub struct LstCodec;

#[cfg(not(feature = "asterisk"))]
impl LstCodec {
    pub fn new() -> Option<Self> {
        None
    }

    pub fn encode_pcm(&mut self, _pcm: &[i16]) -> Vec<Vec<u8>> {
        Vec::new()
    }

    pub fn decode_tmd(&mut self, _tmd: &[u8]) -> Option<Vec<i16>> {
        None
    }

    pub fn reset_ul(&mut self) {}
}

pub fn codec_available() -> bool {
    #[cfg(feature = "asterisk")]
    {
        LstCodec::new().is_some()
    }
    #[cfg(not(feature = "asterisk"))]
    {
        false
    }
}
