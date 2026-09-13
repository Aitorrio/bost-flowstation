use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GroupFloorGrant {
    pub(super) call_id: u16,
    pub(super) source_issi: u32,
    pub(super) dest_gssi: u32,
    pub(super) carrier_num: u16,
    pub(super) ts: u8,
    pub(super) is_group: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CallTimeslot {
    pub(super) call_id: u16,
    pub(super) carrier_num: u16,
    pub(super) ts: u8,
}

/// After network hard-preempt of a local group speaker: hold AssignedControl, re-send
/// cease, and defer NetworkCallReady / RemoteFloorGranted until UL is quiet (or deadline).
/// Applies to LST Dispatch and Brew (shared NetworkCallStart path).
/// `PartialEq` only: `TdmaTime` / `Uuid` pairing — `TdmaTime` does not implement `Eq`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PreemptPendingReady {
    pub(super) brew_uuid: uuid::Uuid,
    pub(super) call_id: u16,
    pub(super) dest_gssi: u32,
    pub(super) carrier_num: u16,
    pub(super) ts: u8,
    pub(super) usage: u8,
    pub(super) network_speaker: u32,
    pub(super) preempted_issi: u32,
    pub(super) next_cease_at: TdmaTime,
    /// Earliest Ready (quiet or timeout) — avoid flash Ready before cease can land (~400 ms).
    pub(super) min_ready_at: TdmaTime,
    /// Force Ready even if UL still up (~1.25 s).
    pub(super) ready_deadline: TdmaTime,
    /// Keep cease retries after Ready until this time.
    pub(super) post_cease_until: TdmaTime,
    pub(super) ready_sent: bool,
    /// Assumed true at arm (MS was transmitting); cleared by TrafficUlActivity(false).
    pub(super) ul_active: bool,
}

/// Re-send D-TX CEASED / NotGranted while preempt pending (~300 ms).
pub(super) const PREEMPT_CEASE_INTERVAL_TS: i32 = 22;
/// Minimum hangtime+cease before Ready (~400 ms).
pub(super) const PREEMPT_MIN_HOLD_TS: i32 = 30;
/// Defer NetworkCallReady at most ~1.25 s waiting for UL quiet.
pub(super) const PREEMPT_READY_DEADLINE_TS: i32 = 90;
/// Continue cease FACCH briefly after Ready (~2.5 s from preempt start).
pub(super) const PREEMPT_POST_CEASE_TS: i32 = 180;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BrewNotification {
    Never,
    IfGroupRoutable(u32),
}

impl BrewNotification {
    fn enabled(self, config: &SharedConfig) -> bool {
        match self {
            BrewNotification::Never => false,
            // Real Brew: only routable GSSIs. LST Dispatch occupies the Brew slot and needs
            // FloorGranted/Released for every local group floor so the console can highlight RX
            // and filter multi-TG listen.
            BrewNotification::IfGroupRoutable(gssi) => {
                brew::is_brew_gssi_routable(config, gssi) || brew::is_lst_dispatch_active(config)
            }
        }
    }
}

impl CcBsSubentity {
    fn push_control(queue: &mut MessageQueue, dest: TetraEntity, control: CallControl) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest,
            msg: SapMsgInner::CmceCallControl(control),
        });
    }

    pub(super) fn notify_floor_granted(
        &self,
        queue: &mut MessageQueue,
        grant: GroupFloorGrant,
        notify_umac: bool,
        notify_brew: BrewNotification,
    ) {
        self.emit(crate::net_telemetry::TelemetryEvent::CallSpeakerChanged {
            call_id: grant.call_id,
            is_group: grant.is_group,
            dest_addr: grant.dest_gssi,
            speaker_issi: grant.source_issi,
            carrier_num: grant.carrier_num,
            ts: grant.ts,
        });

        if notify_umac {
            Self::push_control(
                queue,
                TetraEntity::Umac,
                CallControl::FloorGranted {
                    call_id: grant.call_id,
                    source_issi: grant.source_issi,
                    dest_gssi: grant.dest_gssi,
                    carrier_num: grant.carrier_num,
                    ts: grant.ts,
                },
            );
        }

        if notify_brew.enabled(&self.config) {
            Self::push_control(
                queue,
                TetraEntity::Brew,
                CallControl::FloorGranted {
                    call_id: grant.call_id,
                    source_issi: grant.source_issi,
                    dest_gssi: grant.dest_gssi,
                    carrier_num: grant.carrier_num,
                    ts: grant.ts,
                },
            );
        }
    }

    pub(super) fn notify_remote_floor_granted(&self, queue: &mut MessageQueue, slot: CallTimeslot) {
        Self::push_control(
            queue,
            TetraEntity::Umac,
            CallControl::RemoteFloorGranted {
                call_id: slot.call_id,
                carrier_num: slot.carrier_num,
                ts: slot.ts,
            },
        );
    }

    pub(super) fn notify_floor_released(
        &self,
        queue: &mut MessageQueue,
        slot: CallTimeslot,
        notify_umac: bool,
        notify_brew: BrewNotification,
    ) {
        if notify_umac {
            Self::push_control(
                queue,
                TetraEntity::Umac,
                CallControl::FloorReleased {
                    call_id: slot.call_id,
                    carrier_num: slot.carrier_num,
                    ts: slot.ts,
                },
            );
        }

        if notify_brew.enabled(&self.config) {
            Self::push_control(
                queue,
                TetraEntity::Brew,
                CallControl::FloorReleased {
                    call_id: slot.call_id,
                    carrier_num: slot.carrier_num,
                    ts: slot.ts,
                },
            );
        }
    }

    pub(super) fn notify_call_ended(&self, queue: &mut MessageQueue, slot: CallTimeslot, notify_umac: bool, notify_brew: BrewNotification) {
        if notify_umac {
            Self::push_control(
                queue,
                TetraEntity::Umac,
                CallControl::CallEnded {
                    call_id: slot.call_id,
                    carrier_num: slot.carrier_num,
                    ts: slot.ts,
                },
            );
        }

        if notify_brew.enabled(&self.config) {
            Self::push_control(
                queue,
                TetraEntity::Brew,
                CallControl::CallEnded {
                    call_id: slot.call_id,
                    carrier_num: slot.carrier_num,
                    ts: slot.ts,
                },
            );
        }
    }

    pub(super) fn notify_network_call_end(&self, queue: &mut MessageQueue, brew_uuid: uuid::Uuid) {
        Self::push_control(queue, TetraEntity::Brew, CallControl::NetworkCallEnd { brew_uuid });
    }

    pub(super) fn notify_network_circuit_release(
        &self,
        queue: &mut MessageQueue,
        network_entity: TetraEntity,
        brew_uuid: uuid::Uuid,
        cause: DisconnectCause,
    ) {
        Self::push_control(
            queue,
            network_entity,
            CallControl::NetworkCircuitRelease {
                brew_uuid,
                cause: cause.into_raw() as u8,
            },
        );
    }
}
