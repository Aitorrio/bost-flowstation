/// EN 300 392-7 clause A.8.6 Authentication sub-type: identifies the PDU
/// when the MM PDU type is U-AUTHENTICATION (0000) or D-AUTHENTICATION
/// (0001). The same values apply in both directions.
/// Bits: 2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuthenticationSubType {
    Demand = 0,
    Response = 1,
    Result = 2,
    Reject = 3,
}

impl std::convert::TryFrom<u64> for AuthenticationSubType {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            0 => Ok(AuthenticationSubType::Demand),
            1 => Ok(AuthenticationSubType::Response),
            2 => Ok(AuthenticationSubType::Result),
            3 => Ok(AuthenticationSubType::Reject),
            _ => Err(()),
        }
    }
}

impl AuthenticationSubType {
    /// Convert this enum back into the raw integer value
    pub fn into_raw(self) -> u64 {
        self as u64
    }
}

impl std::fmt::Display for AuthenticationSubType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
