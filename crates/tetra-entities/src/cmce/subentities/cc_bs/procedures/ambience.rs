use super::*;

/// Basic-service byte a dispatch console puts in a private-call setup to request ambience
/// listening (SS-AL). Matches tetra-dispatch's `SERVICE_AMBIENCE_LISTENING` and brew-server's
/// `AMBIENCE_LISTENING_SERVICE`. Not a TETRA speech-service code: it never goes on the air.
pub(in crate::cmce::subentities::cc_bs) const AMBIENCE_LISTENING_SERVICE: u8 = 9;

/// How long a control-channel AmbienceListen waits for the network setup that carries the
/// speech path. brew-server relays the setup and fires the command right after, so either may
/// arrive first. (~10 s at 170/12 ms per slot ≈ 706 timeslots.)
const AMBIENCE_ARM_TS: i32 = 706;

/// Ambience listening (SS-AL, ETSI EN 300 392-2).
///
/// The speech path is the ordinary network individual call the console sets up and Brew relays
/// to this cell. This base station only marks that call as an ambience call, which the
/// network-terminated setup path (`isi.rs`) turns into a direct simplex set-up, with the floor
/// given to the radio once it connects. The radio indicates the call as any other — this is not covert.
impl CcBsSubentity {
    /// Map an incoming service byte to the 2-bit TETRA speech-service field carried in a
    /// D-SETUP. Only 0 (TETRA encoded speech) and 3 (proprietary) are defined; 1 and 2 are
    /// reserved and radios refuse them (U-DISCONNECT "requested service not available"). The
    /// ambience-listening marker (9) and any other value become 0, so serialization never panics.
    pub(in crate::cmce::subentities::cc_bs) fn d_setup_speech_service(service: u8) -> u8 {
        if service == 3 { 3 } else { 0 }
    }

    /// Control-channel `AmbienceListen { issi, enable }`. Returns whether the request was
    /// actioned (target locally registered for enable; a call found for disable).
    pub fn ambience_listen(&mut self, queue: &mut MessageQueue, issi: u32, enable: bool) -> bool {
        if enable {
            if !self.is_locally_registered_issi(issi) {
                tracing::warn!("CMCE: AmbienceListen issi={} ignored — not registered locally", issi);
                return false;
            }
            // If the relayed network setup is already up, flag that call now; otherwise arm the
            // next setup toward this ISSI (the relay and the command race).
            if let Some((call_id, _)) = self.find_individual_call_by_issi(issi) {
                if let Some(call) = self.individual_calls.get_mut(&call_id) {
                    call.ambience = true;
                }
                tracing::info!("CMCE: AmbienceListen issi={} applied to existing call_id={}", issi, call_id);
            } else {
                self.ambience_armed.insert(issi, self.dltime);
                tracing::info!("CMCE: AmbienceListen issi={} armed for next network setup", issi);
            }
            true
        } else {
            self.ambience_armed.remove(&issi);
            let ids: Vec<u16> = self
                .individual_calls
                .iter()
                .filter(|(_, c)| c.ambience && (c.called_addr.ssi == issi || c.calling_addr.ssi == issi))
                .map(|(&id, _)| id)
                .collect();
            let found = !ids.is_empty();
            for id in ids {
                tracing::info!("CMCE: AmbienceListen stop issi={} releasing call_id={}", issi, id);
                self.release_individual_call(queue, id, DisconnectCause::UserRequestedDisconnection);
            }
            found
        }
    }

    /// Consume a pending ambience arm for `issi`, dropping it if it has expired. Called by the
    /// network-terminated setup path so a relayed setup carrying no service byte still becomes
    /// an ambience call when the control command armed it.
    pub(in crate::cmce::subentities::cc_bs) fn take_ambience_arm(&mut self, issi: u32) -> bool {
        match self.ambience_armed.remove(&issi) {
            Some(armed) if armed.age(self.dltime) < AMBIENCE_ARM_TS => true,
            Some(_) => {
                tracing::debug!("CMCE: ambience arm for issi={} expired before setup", issi);
                false
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_service_maps_to_defined_values() {
        // Never a reserved code (1, 2) on the air: a radio refuses those with U-DISCONNECT.
        assert_eq!(CcBsSubentity::d_setup_speech_service(AMBIENCE_LISTENING_SERVICE), 0);
        assert_eq!(CcBsSubentity::d_setup_speech_service(0), 0);
        assert_eq!(CcBsSubentity::d_setup_speech_service(1), 0);
        assert_eq!(CcBsSubentity::d_setup_speech_service(2), 0);
        assert_eq!(CcBsSubentity::d_setup_speech_service(3), 3);
        assert_eq!(CcBsSubentity::d_setup_speech_service(255), 0);
    }
}
