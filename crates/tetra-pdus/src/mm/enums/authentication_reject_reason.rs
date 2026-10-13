/// EN 300 392-7 clause A.8.4 Authentication reject reason.
/// Bits: 3
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuthenticationRejectReason {
    AuthenticationNotSupported = 0,
    Reserved1 = 1,
    Reserved2 = 2,
    Reserved3 = 3,
    Reserved4 = 4,
    Reserved5 = 5,
    Reserved6 = 6,
    Reserved7 = 7,
}

impl std::convert::TryFrom<u64> for AuthenticationRejectReason {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            0 => Ok(AuthenticationRejectReason::AuthenticationNotSupported),
            1 => Ok(AuthenticationRejectReason::Reserved1),
            2 => Ok(AuthenticationRejectReason::Reserved2),
            3 => Ok(AuthenticationRejectReason::Reserved3),
            4 => Ok(AuthenticationRejectReason::Reserved4),
            5 => Ok(AuthenticationRejectReason::Reserved5),
            6 => Ok(AuthenticationRejectReason::Reserved6),
            7 => Ok(AuthenticationRejectReason::Reserved7),
            _ => Err(()),
        }
    }
}

impl AuthenticationRejectReason {
    /// Convert this enum back into the raw integer value
    pub fn into_raw(self) -> u64 {
        self as u64
    }
}

impl std::fmt::Display for AuthenticationRejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
