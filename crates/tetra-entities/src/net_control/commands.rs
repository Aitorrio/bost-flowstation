// ---------------------------------------------------------------------------
// Command / CommandResponse — concrete enums sent through the channel
//
// The command server sends a Command; the stack processes it and returns
// a CommandResponse.  Placeholder variants are provided for now.
// ---------------------------------------------------------------------------

use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use tetra_core::tetra_entities::TetraEntity;

/// Command received from the remote command server.
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub enum ControlCommand {
    /// Send an SDS for local delivery
    SendSds {
        handle: u32,
        source_ssi: u32,
        dest_ssi: u32,
        dest_is_group: bool,
        len_bits: u16,
        payload: Vec<u8>,
    },

    /// Send an already-built SDS Type-4 payload for local delivery.
    SendRawSdsType4 {
        handle: u32,
        source_ssi: u32,
        dest_ssi: u32,
        dest_is_group: bool,
        len_bits: u16,
        payload: Vec<u8>,
    },

    /// Forcibly deregister a terminal from the BS
    KickMs { issi: u32 },

    /// Send the cell's static cipher key to one radio over the air (D-OTAR SCK PROVIDE,
    /// EN 300 392-7 clause 4.5.2.2). The radio must hold the authentication key K listed
    /// for it under [[security.subscribers]]. Local-only.
    OtarSck { issi: u32 },

    /// Dynamic Group Number Assignment (SS-DGNA, ETSI EN 300 392-2 §16).
    ///
    /// BS-initiated: attach (or detach) a single GSSI on an already-registered
    /// terminal over the air, by sending it an unsolicited D-ATTACH/DETACH GROUP
    /// IDENTITY. Local-only — no Brew propagation is performed for this command.
    Dgna {
        /// Target terminal (must be registered on the cell).
        issi: u32,
        /// Group to assign/remove.
        gssi: u32,
        /// Optional TG alias sent in the SS-DGNA mnemonic group name.
        mnemonic: Option<String>,
        /// SS-DGNA attachment mode (Table 51). Used for assign only; deassign ignores it.
        attachment_mode: u8,
        /// `true` = assign/attach the group, `false` = deassign/detach it.
        attach: bool,
    },

    /// Restart the FlowStation service (systemctl restart tetra)
    RestartService,

    /// Stop the FlowStation service (systemctl stop tetra)
    ShutdownService,

    /// Add a live SDS message to the broadcast queue.
    /// The message will be transmitted to all MSs on the cell at the next HMD interval,
    /// round-robining with the static Home Mode Display text.
    /// `repeat_count = 0` means repeat indefinitely; `> 0` auto-removes after N transmissions.
    AddLiveSds {
        text: String,
        protocol_id: u8,
        source_issi: u32,
        /// `Some(issi)` = only to this radio; `None` = whole cell.
        dest_issi: Option<u32>,
        repeat_count: u32,
    },

    /// Remove a live SDS message from the queue by its ID.
    DeleteLiveSds { id: u32 },

    /// Remove all live SDS messages from the queue.
    ClearLiveSds,

    /// Operator-clear an active emergency for one ISSI (`issi == 0` clears all). Local-only;
    /// clears the source session so a subsequent emergency re-send raises a fresh alarm.
    ClearEmergency { issi: u32 },

    /// Ambience listening (SS-AL). Set up a receive-only speech call to `issi` in which the
    /// target radio answers and keys on its own (indicated on the radio — not covert);
    /// `enable == false` releases it. Wire-compatible with brew-server's
    /// `AmbienceListen { issi, enable }`, which fires it at the base station hosting the ISSI.
    AmbienceListen { issi: u32, enable: bool },

    /// Placeholder command A.
    CommandA { handle: u32, parameter: u32 },
    /// Placeholder command B.
    TestCmdB {
        handle: u32,
        source_ssi: u32,
        is_group: bool,
        payload: Vec<u8>,
    },
    /// Resend the current SC3 CCK, sealed with the registered radio's active DCK.
    /// Does not rotate keys or bypass authentication. Local operator action.
    OtarCck { issi: u32 },

    /// Fleet Control (`fleet-v1`): round-trip check. The control worker answers it itself with a
    /// [`ControlResponse::CommandResult`]: `ok` while Fleet Control is on, else `fleet_control_off`.
    FleetPing { handle: u32 },

    /// Fleet Control (`update-v1`): run the update check against the station's own OTA source
    /// (fresh, no cache). Answered by the control worker with a [`ControlResponse::UpdateInfo`]
    /// when the check is done (it asks GitHub, so it can take some seconds).
    CheckUpdate {
        handle: u32,
        /// Read token for the update source, for this check only (never stored or logged).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<crate::net_dashboard::source_auth::SourceToken>,
    },

    /// Fleet Control (`update-v1`): start the OTA like the dashboard's Update button, from the
    /// station's own OTA source. Answered at once with a [`ControlResponse::CommandResult`]
    /// (`ok`, `busy`, `call_active`, `unavailable`, `fleet_control_off`); the run then reports
    /// [`ControlResponse::UpdateProgress`]. `during_call`: start even while a call is in progress.
    StartUpdate {
        handle: u32,
        #[serde(default)]
        during_call: bool,
        /// Live update (capability `update-live-v1`): install this repository and branch with the
        /// installer, radio kept on until the service restart. `None`: the normal OTA from the
        /// station's own source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<UpdateSource>,
        /// Read token for the update source, for this update only (never stored or logged).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<crate::net_dashboard::source_auth::SourceToken>,
    },

    /// Fleet Control (`maintenance-v1`): switch the maintenance flag. While it is on, every page
    /// of the dashboard shows a red banner "work on this base station in progress". `minutes`:
    /// how long it stays on without being sent again (default 10, at most 240), so a closed
    /// browser on the server never leaves it on. `by` / `reason` are shown in the banner.
    /// Answered with a [`ControlResponse::CommandResult`].
    SetMaintenance {
        handle: u32,
        on: bool,
        #[serde(default)]
        minutes: u32,
        #[serde(default)]
        by: String,
        #[serde(default)]
        reason: String,
    },

    /// Fleet Control (`restart-v1`): restart the service (`host` false) or reboot the host
    /// (`host` true), 5 s after the answer. Answered with a [`ControlResponse::CommandResult`]:
    /// `ok`, `busy` (an update is running), `call_active` (a call is in progress and
    /// `during_call` is false), `fleet_control_off`.
    RestartStation {
        handle: u32,
        #[serde(default)]
        host: bool,
        #[serde(default)]
        during_call: bool,
    },

    /// Fleet Control (`config-v1`): read the config. Answered with a [`ControlResponse::ConfigInfo`]
    /// (the settings of the Infrastructure page as JSON, the file as TOML, both without passwords
    /// and keys, and the config hash).
    GetConfig { handle: u32 },

    /// Fleet Control (`config-v1`): change the config. `changes` has the keys of the dashboard's
    /// Infrastructure page (see `net_control::fleet_config::apply_changes`); the station checks the
    /// result like a local save, refuses it when its config hash is not `base_hash` (`conflict`),
    /// saves the old file, writes the new one and restarts the service. The change must then be
    /// confirmed with [`ControlCommand::ConfirmConfig`] within `confirm_secs` (default 180) after
    /// the restart, else the station restores the old file and restarts again. Answered with a
    /// [`ControlResponse::CommandResult`]: `ok`, `conflict`, `invalid`, `busy`, `call_active`,
    /// `fleet_control_off`.
    SetConfig {
        handle: u32,
        base_hash: String,
        changes: JsonDoc,
        #[serde(default)]
        confirm_secs: u32,
        #[serde(default)]
        during_call: bool,
        /// Who changed it (for the station's log).
        #[serde(default)]
        by: String,
    },

    /// Fleet Control (`config-v1`): keep the config written by the last `SetConfig` (the old file
    /// is dropped). Answered with a [`ControlResponse::CommandResult`]: `ok`, `not_pending`.
    ConfirmConfig { handle: u32 },

    /// Fleet Control (`config-v1`): go back to the config before the last `SetConfig` at once and
    /// restart. Answered with a [`ControlResponse::CommandResult`]: `ok`, `not_pending`.
    RollbackConfig { handle: u32 },
}

/// A JSON document inside a control message: plain JSON on the wire (the server sends and gets an
/// object, not a string), kept as text for the binary codec.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct JsonDoc(pub String);

impl JsonDoc {
    pub fn from_value(v: &serde_json::Value) -> Self {
        JsonDoc(v.to_string())
    }

    pub fn value(&self) -> serde_json::Value {
        serde_json::from_str(&self.0).unwrap_or(serde_json::Value::Null)
    }
}

impl Serialize for JsonDoc {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for JsonDoc {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(JsonDoc::from_value(&serde_json::Value::deserialize(d)?))
    }
}

/// Repository and branch of a live update (`StartUpdate.source`).
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, Serialize, Deserialize)]
pub struct UpdateSource {
    pub repo: String,
    pub branch: String,
}

impl ControlCommand {
    /// Return the stack entity that owns this command. Keep this mapping shared by the remote
    /// control worker and local dashboard so local commands cannot bypass entity routing.
    pub(crate) fn target_entity(&self) -> TetraEntity {
        match self {
            Self::SendSds { .. }
            | Self::SendRawSdsType4 { .. }
            | Self::KickMs { .. }
            | Self::RestartService
            | Self::ShutdownService
            | Self::AddLiveSds { .. }
            | Self::DeleteLiveSds { .. }
            | Self::ClearLiveSds
            | Self::ClearEmergency { .. }
            | Self::AmbienceListen { .. }
            | Self::TestCmdB { .. }
            // Answered by the control worker before routing; never reaches CMCE.
            | Self::FleetPing { .. }
            | Self::CheckUpdate { .. }
            | Self::StartUpdate { .. }
            | Self::SetMaintenance { .. }
            | Self::RestartStation { .. }
            | Self::GetConfig { .. }
            | Self::SetConfig { .. }
            | Self::ConfirmConfig { .. }
            | Self::RollbackConfig { .. } => TetraEntity::Cmce,
            Self::OtarSck { .. } | Self::OtarCck { .. } | Self::Dgna { .. } | Self::CommandA { .. } => TetraEntity::Mm,
        }
    }
}

/// Response sent back after processing a [`ControlCommand`].
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub enum ControlResponse {
    CommandAResponse { handle: u32, result: u32 },
    SendSdsResponse { handle: u32, success: bool },
    KickMsResponse { issi: u32, success: bool },
    AmbienceListenResponse { issi: u32, success: bool },

    /// Fleet Control: sent by the station right after the control channel connects (only when the
    /// server offered `fleet-control` in `X-Brew-Extensions`) and again whenever the local switch
    /// changes. See `Docs/fleet-control.md`.
    StationHello {
        /// Fleet Control protocol version ([`super::fleet::FLEET_PROTOCOL`]).
        protocol: u32,
        /// Software version (`STACK_VERSION`, e.g. `v0.5.9-8d7a363`).
        version: String,
        /// The station's Brew login (its number on the Brew server), when Brew is configured.
        brew_login: Option<String>,
        /// Network of the primary cell.
        mcc: u16,
        mnc: u16,
        /// The local Fleet Control switch.
        fleet_control: bool,
        /// Fleet Control commands this station understands (empty while the switch is off).
        capabilities: Vec<String>,
        /// Hash of the config file (`config-v1`, while the switch is on), to see changes made on
        /// the station. Older stations leave it out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_hash: Option<String>,
        /// Seconds left to confirm the config written by `SetConfig` (`config-v1`), `None` when
        /// nothing waits for confirmation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_pending: Option<u32>,
    },

    /// Fleet Control: the answer to every Fleet Control command, correlated by `handle`.
    CommandResult {
        handle: u32,
        ok: bool,
        /// Machine-readable outcome: `ok`, `fleet_control_off`, ... (see `fleet::code`).
        code: String,
        /// Short English text for the server's dashboard.
        message: String,
    },

    /// Fleet Control (`update-v1`): the answer to [`ControlCommand::CheckUpdate`].
    UpdateInfo {
        handle: u32,
        /// The check reached the repository.
        ok: bool,
        /// Running software (`STACK_VERSION`).
        current: String,
        /// Newest version of the source, when known.
        latest: Option<String>,
        /// A newer version is available (the server only updates such stations).
        update_available: bool,
        /// OTA source of the station: `installed`, `stable`, `beta` or `custom`.
        channel: String,
        /// Repository (without credentials) and branch the station updates from.
        repo: String,
        branch: String,
        message: String,
    },

    /// Fleet Control (`config-v1`): the answer to [`ControlCommand::GetConfig`].
    ConfigInfo {
        handle: u32,
        ok: bool,
        /// `ok`, `fleet_control_off`, `unavailable` (config not readable).
        code: String,
        message: String,
        /// Hash of the config file (the `base_hash` for `SetConfig`).
        hash: String,
        /// The settings of the Infrastructure page (passwords only as `password_set`).
        config: JsonDoc,
        /// The config file as TOML, passwords and keys replaced by `"***"`.
        text: String,
        /// Seconds left to confirm the last `SetConfig`, `None` when nothing waits.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pending: Option<u32>,
    },

    /// Fleet Control (`update-v1`): progress of an OTA run (whoever started it), sent when the step
    /// or the percentage changes and at least every 10 s while it runs.
    UpdateProgress {
        /// `download`, `build`, `install`, `restart`, `up_to_date`, `failed`.
        stage: String,
        /// 0–100, estimated.
        percent: u8,
        /// Last line of the update log.
        message: String,
        /// Software that is running now (the old one until the restart).
        version: String,
        /// Seconds since the run started.
        elapsed_secs: u64,
    },

    /// `dgna-report`: a DGNA assign (`assigned`) or remove went into the station's DGNA registry,
    /// from the server or from the station's own dashboard. Sent only to a server that offers
    /// `dgna-report` on the control channel. See `net_control::dgna_report`.
    DgnaChanged {
        issi: u32,
        gssi: u32,
        mnemonic: Option<String>,
        /// SS-DGNA attachment mode (TS 100 392-12-22 table 51); 0 for a remove.
        attachment_mode: u8,
        assigned: bool,
    },

    /// `dgna-report`: every DGNA group the station remembers, right after the control channel
    /// connects (the server only adds from it: a station forgets radios over its own restart).
    DgnaGroups { groups: Vec<DgnaReportEntry> },

    /// Fleet Control (`metrics-v1`): load and temperatures, sent every `interval_secs` while the
    /// server offers Fleet Control and the switch is on (graphs and alarms on the server).
    FleetMetrics {
        /// Seconds between two reports.
        interval_secs: u32,
        /// CPU load in percent, averaged since the previous report (`None`: not readable yet).
        cpu_pct: Option<f32>,
        /// CPU temperature in °C, when the host has a sensor.
        cpu_temp_c: Option<f32>,
        /// One entry per cell, primary first.
        cells: Vec<CellMetrics>,
        /// Registered radios.
        radios: u32,
        /// Calls in progress.
        calls: u32,
        /// Stack health: `ok`, `degraded` or `critical`.
        health: String,
    },
}

/// One group in [`ControlResponse::DgnaGroups`].
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, Serialize, Deserialize)]
pub struct DgnaReportEntry {
    pub issi: u32,
    pub gssi: u32,
    pub mnemonic: Option<String>,
    pub attachment_mode: u8,
}

/// One cell in [`ControlResponse::FleetMetrics`].
#[derive(Debug, Clone, PartialEq, Encode, Decode, Serialize, Deserialize)]
pub struct CellMetrics {
    pub cell: u8,
    /// RF state: `online`, `off` (no SDR configured), `error` (the SDR could not be opened),
    /// `starting`.
    pub rf: String,
    /// SDR temperature in °C, when the SDR has a sensor.
    pub sdr_temp_c: Option<f32>,
    /// Error text while `rf` is `error`, else empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}
