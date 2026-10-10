use core::fmt;

use tetra_core::pdu_parse_error::PduParseErr;

/// OTAR sub-type (ETSI EN 300 392-7 clause A.8.58): the second 4-bit field
/// of every D-OTAR / U-OTAR PDU. Each value names a demand/provide pair on
/// one direction and a result/reject pair on the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtarSubType {
    /// CCK Demand (uplink) or CCK Provide (downlink)
    CckDemandProvide = 0,
    /// CCK Result (uplink) or CCK Reject (downlink)
    CckResultReject = 1,
    /// SCK Demand (uplink) or SCK Provide (downlink)
    SckDemandProvide = 2,
    /// SCK Result (uplink) or SCK Reject (downlink)
    SckResultReject = 3,
    /// GCK Demand (uplink) or GCK Provide (downlink)
    GckDemandProvide = 4,
    /// GCK Result (uplink) or GCK Reject (downlink)
    GckResultReject = 5,
    /// GSKO Demand (uplink) or GSKO Provide (downlink)
    GskoDemandProvide = 6,
    /// GSKO Result (uplink) or GSKO Reject (downlink)
    GskoResultReject = 7,
    /// Key Associate Demand / Status (downlink) or the matching uplink results
    KeyAssociate = 8,
    /// Key delete / status / crypto management group (values 9 … 11)
    KeyDelete = 9,
    KeyStatus = 10,
    CmgGtsiProvide = 11,
    /// DM-SDS OTAR
    DmSdsOtar = 12,
}

impl TryFrom<u64> for OtarSubType {
    type Error = PduParseErr;
    fn try_from(v: u64) -> Result<Self, Self::Error> {
        Ok(match v {
            0 => OtarSubType::CckDemandProvide,
            1 => OtarSubType::CckResultReject,
            2 => OtarSubType::SckDemandProvide,
            3 => OtarSubType::SckResultReject,
            4 => OtarSubType::GckDemandProvide,
            5 => OtarSubType::GckResultReject,
            6 => OtarSubType::GskoDemandProvide,
            7 => OtarSubType::GskoResultReject,
            8 => OtarSubType::KeyAssociate,
            9 => OtarSubType::KeyDelete,
            10 => OtarSubType::KeyStatus,
            11 => OtarSubType::CmgGtsiProvide,
            12 => OtarSubType::DmSdsOtar,
            _ => return Err(PduParseErr::InvalidValue { field: "otar_sub_type", value: v }),
        })
    }
}

impl OtarSubType {
    pub fn into_raw(self) -> u64 {
        self as u64
    }
}

impl fmt::Display for OtarSubType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Provision result (clause A.8.61), sent by the MS in U-OTAR … RESULT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionResult {
    Accepted = 0,
    FailedToDecrypt = 1,
    IncorrectKeyNumber = 2,
    OtarRejected = 3,
    IncorrectKeyVersion = 4,
    GskoVnNotPresent = 5,
    KsgNotSupported = 6,
    Reserved = 7,
}

impl ProvisionResult {
    pub fn from_raw(v: u64) -> ProvisionResult {
        match v {
            0 => ProvisionResult::Accepted,
            1 => ProvisionResult::FailedToDecrypt,
            2 => ProvisionResult::IncorrectKeyNumber,
            3 => ProvisionResult::OtarRejected,
            4 => ProvisionResult::IncorrectKeyVersion,
            5 => ProvisionResult::GskoVnNotPresent,
            6 => ProvisionResult::KsgNotSupported,
            _ => ProvisionResult::Reserved,
        }
    }

    pub fn into_raw(self) -> u64 {
        self as u64
    }

    /// Operator-facing wording.
    pub fn describe(self) -> &'static str {
        match self {
            ProvisionResult::Accepted => "sealed key accepted",
            ProvisionResult::FailedToDecrypt => "sealed key failed to decrypt (radio's K differs, or no K loaded)",
            ProvisionResult::IncorrectKeyNumber => "incorrect key number",
            ProvisionResult::OtarRejected => "OTAR rejected by the radio",
            ProvisionResult::IncorrectKeyVersion => "incorrect key version number",
            ProvisionResult::GskoVnNotPresent => "identified GSKO-VN not present",
            ProvisionResult::KsgNotSupported => "KSG (algorithm) not supported by the radio",
            ProvisionResult::Reserved => "reserved result value",
        }
    }
}

/// OTAR reject reason (clause A.8.57b), sent by the SwMI in D-OTAR … REJECT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtarRejectReason {
    KeyNotAvailable = 0,
    InvalidKeyNumber = 1,
    InvalidAddress = 2,
    KsgNotSupported = 3,
}

impl OtarRejectReason {
    pub fn into_raw(self) -> u64 {
        self as u64
    }
}
