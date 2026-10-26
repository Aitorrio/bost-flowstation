use super::*;

impl CcBsSubentity {
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_call_restore(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        pdu: UCallRestore,
    ) {
        let mut call_id = pdu.call_identifier;

        // Multi-cell: an MS that reselected from a sibling cell restores with that cell's call
        // identifier. Find the same group call here by its GSSI; D-CALL RESTORE carries our id.
        if !self.individual_calls.contains_key(&call_id)
            && !self.active_calls.contains_key(&call_id)
            && brew::is_site_linked(&self.config)
            && let Some(gssi) = pdu.other_party_ssi
            && let Some((&id, _)) = self.active_calls.iter().find(|(_, c)| c.dest_gssi as u64 == gssi)
        {
            tracing::info!(
                "CMCE: U-CALL RESTORE from ISSI {} for call_id={} (sibling cell) → local call_id={} gssi={}",
                sender.ssi,
                call_id,
                id,
                gssi
            );
            call_id = id;
        }

        // Multi-cell individual (private/duplex) call: an MS that reselected mid-call restores with
        // the OLD cell's call identifier, which does not exist here. Match the same private call by
        // the (this MS, peer) ISSI pair; the leg was pre-provisioned on this cell by the site
        // switch's individual-call handover (see net_site prepare_handover). D-CALL RESTORE then
        // carries our local id.
        if !self.individual_calls.contains_key(&call_id)
            && !self.active_calls.contains_key(&call_id)
            && brew::is_site_linked(&self.config)
            && let Some(peer) = pdu.other_party_ssi
            && let Some((&id, _)) = self.individual_calls.iter().find(|(_, c)| {
                c.is_active()
                    && ((c.calling_addr.ssi == sender.ssi && c.called_addr.ssi as u64 == peer)
                        || (c.called_addr.ssi == sender.ssi && c.calling_addr.ssi as u64 == peer))
            })
        {
            tracing::info!(
                "CMCE: U-CALL RESTORE from ISSI {} for call_id={} (sibling cell, individual) → local call_id={} peer={}",
                sender.ssi,
                call_id,
                id,
                peer
            );
            call_id = id;
        }

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            if !call.is_active() || (sender.ssi != call.calling_addr.ssi && sender.ssi != call.called_addr.ssi) {
                self.reject_call_restore(queue, sender, handle, link_id, endpoint_id, call_id);
                return;
            }

            if call.begin_restore().is_err() {
                self.reject_call_restore(queue, sender, handle, link_id, endpoint_id, call_id);
                return;
            }
            call.active_timer_started = Some(self.dltime);
            // Only grant when the floor is free or the sender already holds it (same decision as
            // the U-TX DEMAND path). Granting while the peer is talking gives two simultaneous
            // transmitters and a stale floor_holder the UL-inactivity watchdog never ceases.
            // Duplex calls keep floor_holder = None, so they still always grant.
            let grant = if !pdu.request_to_transmit_send_data {
                TransmissionGrant::NotGranted
            } else if call.floor_holder.is_none() || call.is_floor_held_by(sender.ssi) {
                if call.is_simplex() {
                    call.grant_floor(sender);
                }
                TransmissionGrant::Granted
            } else {
                TransmissionGrant::GrantedToOtherUser
            };
            // A leg pre-provisioned for cross-cell handover has the MS arriving from another cell,
            // so it is not yet on this cell's traffic channel: the restore reply must carry a
            // channel allocation to move it there. An ordinary same-cell restore needs none (the
            // MS never left the channel). `restoring_cross_cell` is cleared by complete_restore, so
            // capture the channel first.
            let cross_cell_chan = call
                .restoring_cross_cell
                .then_some((call.calling_carrier_num, call.calling_ts, call.calling_usage));
            call.complete_restore();
            if let Some((carrier_num, ts, usage)) = cross_cell_chan {
                self.send_d_call_restore_with_chan_alloc(queue, sender, handle, link_id, endpoint_id, call_id, grant, carrier_num, ts, usage);
            } else {
                self.send_d_call_restore(queue, sender, handle, link_id, endpoint_id, call_id, grant);
            }
            return;
        }

        if let Some(call) = self.active_calls.get_mut(&call_id) {
            if call.begin_restore().is_err() {
                self.reject_call_restore(queue, sender, handle, link_id, endpoint_id, call_id);
                return;
            }
            let grant = if !pdu.request_to_transmit_send_data {
                TransmissionGrant::NotGranted
            } else if !call.tx_active || call.source_issi == sender.ssi {
                call.grant_floor(sender.ssi, Some(sender));
                TransmissionGrant::Granted
            } else {
                TransmissionGrant::GrantedToOtherUser
            };

            call.complete_restore();
            let floor = (grant == TransmissionGrant::Granted).then_some(GroupFloorGrant {
                call_id,
                source_issi: sender.ssi,
                dest_gssi: call.dest_gssi,
                carrier_num: call.carrier_num,
                ts: call.ts,
                is_group: true,
            });
            self.send_d_call_restore(queue, sender, handle, link_id, endpoint_id, call_id, grant);

            // A restore that hands over the floor must arm UMAC's UL-inactivity timer exactly like
            // every other grant path (see setup.rs / U-TX DEMAND): without the FloorGranted a
            // restored talker that then goes silent pins the traffic slot until the absolute call
            // time-out — forever for a duplex/Infinite call, so repeats exhaust the cell.
            if let Some(floor) = floor {
                self.send_d_tx_granted_facch(queue, call_id, floor.source_issi, floor.dest_gssi, floor.carrier_num, floor.ts);
                self.notify_floor_granted(queue, floor, true, BrewNotification::IfGroupRoutable(floor.dest_gssi));
            }
            return;
        }

        self.reject_call_restore(queue, sender, handle, link_id, endpoint_id, call_id);
    }

    fn send_d_call_restore(
        &self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        call_id: u16,
        grant: TransmissionGrant,
    ) {
        let sdu = Self::build_d_call_restore(call_id, grant, Some(CallStatus::Callcontinue));
        let msg = Self::build_sapmsg_direct(sdu, self.dltime, sender, handle, link_id, endpoint_id);
        queue.push_back(msg);
    }

    /// D-CALL RESTORE that also hands the MS a channel allocation, moving it onto this cell's
    /// traffic channel. Used only for a leg pre-provisioned by cross-cell individual-call handover,
    /// where the MS is arriving from a sibling cell and is not yet on channel. Channel allocation
    /// rides as a MAC resource element alongside the CC PDU, exactly as D-CONNECT assigns the
    /// traffic channel at ordinary call setup.
    #[allow(clippy::too_many_arguments)]
    fn send_d_call_restore_with_chan_alloc(
        &self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        call_id: u16,
        grant: TransmissionGrant,
        carrier_num: u16,
        ts: u8,
        usage: u8,
    ) {
        let mut timeslots = [false; 4];
        timeslots[ts as usize - 1] = true;
        let chan_alloc = CmceChanAllocReq {
            usage: Some(usage),
            alloc_type: ChanAllocType::Replace,
            carrier: Some(carrier_num),
            timeslots,
            ul_dl_assigned: UlDlAssignment::Both,
        };
        let sdu = Self::build_d_call_restore(call_id, grant, Some(CallStatus::Callcontinue));
        let msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle,
                endpoint_id,
                link_id,
                // Match the other individual-call CC sends (D-CONNECT/ACK): unacknowledged BL-UDATA
                // carried with FACCH stealing so the allocation reaches the MS on the control path.
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: true,
                stealing_repeats_flag: true,
                chan_alloc: Some(chan_alloc),
                main_address: sender,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    fn reject_call_restore(
        &self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        call_id: u16,
    ) {
        tracing::info!("CMCE: rejecting U-CALL RESTORE for unknown or inactive call_id={}", call_id);
        let sdu = Self::build_d_release(call_id, DisconnectCause::CallRestorationOfTheOtherUserFailed);
        let msg = Self::build_sapmsg_direct(sdu, self.dltime, sender, handle, link_id, endpoint_id);
        queue.push_back(msg);
    }
}
