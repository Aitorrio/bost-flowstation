/// Pass TMD circuit data to UMAC for TX scheduling
#[derive(Debug, Clone)]
pub struct TmdCircuitDataReq {
    // call_id: CallId,
    pub carrier_num: u16,
    pub ts: u8,
    pub data: Vec<u8>,
}

/// Rx'ed traffic
#[derive(Debug, Clone)]
pub struct TmdCircuitDataInd {
    // call_id: CallId,
    pub carrier_num: u16,
    pub ts: u8,
    pub data: Vec<u8>,
    /// Level of the burst in dBFS when the frame decoded (CRC ok), else `f32::NEG_INFINITY`:
    /// a frame that did not decode may be noise and says nothing about the radio's level.
    pub rssi_dbfs: f32,
}
