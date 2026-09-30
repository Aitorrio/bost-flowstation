use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::mle::components::broadcast::MleBroadcast;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, EndpointId, Layer2Service, LinkId, Sap, TdmaTime, TetraAddress, unimplemented_log};
use tetra_pdus::cmce::enums::cmce_pdu_type_dl::CmcePduTypeDl;
use tetra_pdus::mle::enums::mle_pdu_type_ul::MlePduTypeUl;
use tetra_pdus::mle::pdus::{
    d_new_cell::DNewCell, d_prepare_fail::DPrepareFail, d_restore_ack::DRestoreAck, d_restore_fail::DRestoreFail,
    u_prepare::UPrepare, u_restore::URestore,
};
use tetra_saps::control::call_control::CallControl;
use tetra_saps::lcmc::LcmcMleUnitdataInd;
use tetra_saps::lmm::LmmMleUnitdataInd;
use tetra_saps::ltpd::LtpdMleUnitdataInd;
use tetra_saps::tla::{TlaTlDataReqBl, TlaTlUnitdataReqBl};
use tetra_saps::{SapMsg, SapMsgInner};

use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;

pub struct MleBs {
    config: SharedConfig,
    broadcast: MleBroadcast,
    /// MSs whose U-RESTORE (cell reselection) we handed to CMCE: CMCE's answer to the U-CALL
    /// RESTORE it carried goes back inside D-RESTORE-ACK / D-RESTORE-FAIL.
    pending_restores: HashMap<u32, Instant>,
}

/// How long CMCE may take to answer the U-CALL RESTORE carried by a U-RESTORE.
const RESTORE_ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// D-NEW-CELL "channel command valid": change channel immediately (EN 300 392-2 clause 18.5.3).
const CHANNEL_COMMAND_CHANGE_IMMEDIATELY: u8 = 1;
/// D-PREPARE-FAIL / D-RESTORE-FAIL "fail cause" 0.
const FAIL_CAUSE_UNSPECIFIED: u8 = 0;

/// Multiframes at which D-NWRK-BROADCAST is sent within each hyperframe.
/// Two broadcasts per hyperframe (~30.6s interval) for faster time/date display on terminals.
/// BlueStation default was 1 per hyperframe (~61.2s) which is slow on cold attach.
/// We don't use the first multiframe to avoid congestion with other hyperframe-triggered events.
const MLE_BROADCAST_MULTIFRAMES: [u8; 2] = [20, 50];
/// Frame at which D-NWRK-BROADCAST is sent within the broadcast multiframe.
const MLE_BROADCAST_FRAME: u8 = 1;

impl MleBs {
    pub fn new(config: SharedConfig) -> Self {
        let broadcast = MleBroadcast::new(config.clone());
        Self {
            config,
            broadcast,
            pending_restores: HashMap::new(),
        }
    }

    /// An uplink MLE PDU (protocol discriminator already consumed from `sdu`).
    fn rx_mle_pdu(&mut self, queue: &mut MessageQueue, mut sdu: BitBuffer, addr: TetraAddress, link_id: LinkId, endpoint_id: EndpointId) {
        let Some(bits) = sdu.peek_bits(3) else {
            tracing::warn!("MLE: uplink MLE PDU too short: {}", sdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = MlePduTypeUl::try_from(bits) else {
            tracing::warn!("MLE: invalid uplink MLE PDU type {} from {}", bits, addr.ssi);
            return;
        };
        match pdu_type {
            MlePduTypeUl::UPrepare => match UPrepare::from_bitbuf(&mut sdu) {
                Ok(pdu) => self.rx_u_prepare(queue, addr, link_id, endpoint_id, pdu),
                Err(e) => tracing::warn!("MLE: bad U-PREPARE from {}: {:?}", addr.ssi, e),
            },
            MlePduTypeUl::URestore => match URestore::from_bitbuf(&mut sdu) {
                Ok(pdu) => self.rx_u_restore(queue, addr, link_id, endpoint_id, pdu),
                Err(e) => tracing::warn!("MLE: bad U-RESTORE from {}: {:?}", addr.ssi, e),
            },
            MlePduTypeUl::UPrepareDa => unimplemented_log!("UPrepareDa"),
            MlePduTypeUl::UIrregularChannelAdvice => unimplemented_log!("UIrregularChannelAdvice"),
            MlePduTypeUl::UChannelClassAdvice => unimplemented_log!("UChannelClassAdvice"),
            MlePduTypeUl::UChannelRequest => unimplemented_log!("UChannelRequest"),
            MlePduTypeUl::ExtPdu => unimplemented_log!("ExtPdu"),
        }
    }

    /// Announced cell reselection (EN 300 392-2 clause 18.3.4.7): the MS asks to move to the
    /// neighbour `cell_identifier_ca`. Accept with D-NEW-CELL if that is one of the neighbours we
    /// advertise, else D-PREPARE-FAIL. In a linked multi-cell station the site switch is told, so
    /// the target cell can join the MS's group calls before it arrives.
    fn rx_u_prepare(&mut self, queue: &mut MessageQueue, addr: TetraAddress, link_id: LinkId, endpoint_id: EndpointId, pdu: UPrepare) {
        let cfg = self.config.config();
        let target = pdu
            .cell_identifier_ca
            .and_then(|ci| cfg.cell.neighbor_cells_ca.iter().find(|n| n.cell_identifier_ca as u64 == ci));
        let Some(target) = target else {
            tracing::info!(
                "MLE: U-PREPARE from {} for unknown neighbour {:?} → D-PREPARE-FAIL",
                addr.ssi,
                pdu.cell_identifier_ca
            );
            let fail = DPrepareFail {
                fail_cause: FAIL_CAUSE_UNSPECIFIED,
                sdu: None,
            };
            self.send_mle_pdu(queue, addr, link_id, endpoint_id, |b| fail.to_bitbuf(b));
            return;
        };

        tracing::info!(
            "MLE: U-PREPARE from {} → D-NEW-CELL (neighbour ci={} carrier={})",
            addr.ssi,
            target.cell_identifier_ca,
            target.main_carrier_number
        );
        if crate::net_brew::is_site_linked(&self.config) {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Mle,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::SiteHandoverPrepare {
                    issi: addr.ssi,
                    target_carrier: target.main_carrier_number,
                }),
            });
        }
        let new_cell = DNewCell {
            channel_command_valid: CHANNEL_COMMAND_CHANGE_IMMEDIATELY,
            sdu: None,
        };
        self.send_mle_pdu(queue, addr, link_id, endpoint_id, |b| new_cell.to_bitbuf(b));
    }

    /// The MS arrived after cell reselection and restores its call: hand the carried U-CALL
    /// RESTORE to CMCE; its answer is wrapped in D-RESTORE-ACK / D-RESTORE-FAIL (see
    /// `rx_lcmc_mle_unitdata_req`).
    fn rx_u_restore(&mut self, queue: &mut MessageQueue, addr: TetraAddress, link_id: LinkId, endpoint_id: EndpointId, pdu: URestore) {
        let Some(sdu) = pdu.sdu else {
            tracing::info!("MLE: U-RESTORE from {} without U-CALL RESTORE → D-RESTORE-FAIL", addr.ssi);
            let fail = DRestoreFail {
                fail_cause: FAIL_CAUSE_UNSPECIFIED,
            };
            self.send_mle_pdu(queue, addr, link_id, endpoint_id, |b| fail.to_bitbuf(b));
            return;
        };
        tracing::info!("MLE: U-RESTORE from {} → CMCE", addr.ssi);
        self.pending_restores.insert(addr.ssi, Instant::now());
        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
                sdu,
                handle: 0,
                received_tetra_address: addr,
                endpoint_id,
                link_id,
                chan_change_resp_req: false,
                chan_change_handle: None,
            }),
        });
    }

    /// Send an MLE PDU (protocol discriminator MLE) to one MS, acknowledged.
    fn send_mle_pdu(
        &self,
        queue: &mut MessageQueue,
        addr: TetraAddress,
        link_id: LinkId,
        endpoint_id: EndpointId,
        write: impl FnOnce(&mut BitBuffer) -> Result<(), tetra_core::PduParseErr>,
    ) {
        let mut body = BitBuffer::new_autoexpand(32);
        body.write_bits(MleProtocolDiscriminator::Mle.into_raw(), 3);
        if let Err(e) = write(&mut body) {
            tracing::error!("MLE: failed to encode PDU for {}: {:?}", addr.ssi, e);
            return;
        }
        let len = body.get_pos();
        body.seek(0);
        let mut pdu = BitBuffer::new(len);
        pdu.copy_bits(&mut body, len);
        pdu.seek(0);
        queue.push_back(SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                main_address: addr,
                link_id,
                endpoint_id,
                tl_sdu: pdu,
                stealing_permission: false,
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_class_info: None,
                req_handle: 0,
                graceful_degradation: None,
                chan_alloc: None,
                tx_reporter: None,
            }),
        });
    }

    fn rx_tla_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tla_prim");
        match message.msg {
            SapMsgInner::TlaTlDataIndBl(_) => {
                self.rx_tla_data_ind_bl(queue, message);
            }
            SapMsgInner::TlaTlUnitdataIndBl(_) => {
                // self.rx_tla_unitdata_ind_bl(queue, message);
                tracing::warn!("MLE: BS received unexpected TL-UNITDATA, ignoring");
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    fn rx_tla_data_ind_bl(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        // Take ownership of bitbuf and read protocol discriminator
        let SapMsgInner::TlaTlDataIndBl(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let Some(mut sdu) = prim.tl_sdu.take() else {
            tracing::warn!("MLE: rx_tla_data_ind_bl received message with no tl_sdu, ignoring");
            return;
        };
        if sdu.get_pos() != 0 {
            tracing::warn!(
                "MLE: rx_tla_data_ind_bl sdu not at start position (pos={}), seeking to 0",
                sdu.get_pos()
            );
            sdu.seek(0);
        }
        let Some(bits) = sdu.read_bits(3) else {
            tracing::warn!("insufficient bits: {}", sdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = MleProtocolDiscriminator::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, sdu.dump_bin());
            return;
        };

        // Dispatch to appropriate component (or to self if for MLE)
        match pdu_type {
            MleProtocolDiscriminator::Mm => {
                let m = LmmMleUnitdataInd {
                    sdu,
                    handle: 0,
                    received_address: prim.main_address,
                };
                let msg = SapMsg {
                    sap: Sap::LmmSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Mm,
                    msg: SapMsgInner::LmmMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Cmce => {
                let m = LcmcMleUnitdataInd {
                    sdu,
                    handle: 0,
                    received_tetra_address: prim.main_address,
                    endpoint_id: prim.endpoint_id,
                    link_id: prim.link_id,
                    chan_change_resp_req: false, // TODO FIXME
                    chan_change_handle: None,    // TODO FIXME
                };
                let msg = SapMsg {
                    sap: Sap::LcmcSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::LcmcMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Sndcp => {
                let m = LtpdMleUnitdataInd {
                    sdu,
                    endpoint_id: prim.endpoint_id,
                    link_id: prim.link_id,
                    received_tetra_address: prim.main_address,
                    chan_change_resp_req: false, // TODO FIXME
                    chan_change_handle: None,    // TODO FIXME
                };
                // SNDCP (packet data, MLE protocol discriminator 4) belongs to the SNDCP entity,
                // not CMCE. Route it over the TLPD SAP so the packet-data layer receives it.
                let msg = SapMsg {
                    sap: Sap::TlpdSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Sndcp,
                    msg: SapMsgInner::LtpdMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Mle => {
                let (addr, link_id, endpoint_id) = (prim.main_address, prim.link_id, prim.endpoint_id);
                self.rx_mle_pdu(queue, sdu, addr, link_id, endpoint_id);
            }
            MleProtocolDiscriminator::TetraManagementEntity => {
                unimplemented_log!("MleProtocolDiscriminator::TetraManagementEntity");
            }
        }
    }

    fn rx_tlmc_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
        tracing::trace!("rx_tlmc_prim");
        // TLMC SAP not implemented yet. Log instead of panicking so an unexpected
        // primitive doesn't kill the whole MLE worker.
        unimplemented_log!("rx_tlmc_prim called but TLMC SAP is not implemented");
    }

    fn rx_lmm_mle_unitdata_req(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_lmm_mle_unitdata_req");
        let SapMsgInner::LmmMleUnitdataReq(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let mle_prot_discriminator = MleProtocolDiscriminator::Mm;
        let sdu_len = prim.sdu.get_len();
        let mut pdu = BitBuffer::new(3 + sdu_len);
        pdu.write_bits(mle_prot_discriminator.into_raw(), 3);
        pdu.copy_bits(&mut prim.sdu, sdu_len);
        pdu.seek(0);

        if prim.layer2service == Layer2Service::Unacknowledged {
            tracing::warn!("MLE: rx_lmm_mle_unitdata_req with Unacknowledged layer2service not implemented, ignoring");
            return;
        }

        // let (addr, link, endpoint) = self.router.use_handle(prim.handle, message.dltime);
        // assert_eq!(addr.ssi, prim.address.ssi);
        let sapmsg = SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                main_address: prim.address,
                link_id: 0,
                endpoint_id: 0,
                tl_sdu: pdu,
                stealing_permission: false,
                subscriber_class: 0, // TODO fixme
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_class_info: None,
                req_handle: 0, // TODO FIXME; should we pass the same handle here?
                graceful_degradation: None,
                chan_alloc: None,
                tx_reporter: prim.tx_reporter.take(),
            }),
        };
        queue.push_back(sapmsg);
    }

    fn rx_lmm_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_lmm_prim");
        match &message.msg {
            SapMsgInner::LmmMleUnitdataReq(_prim) => {
                self.rx_lmm_mle_unitdata_req(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }

    fn rx_tlpd_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
        tracing::trace!("rx_tlpd_prim");
        unimplemented_log!("rx_tlpd_prim called but TLPD SAP is not implemented");
        // match &message.msg {
        //     _ => {
        //         panic!();
        //     }
        // }
    }

    fn rx_lcmc_mle_unitdata_req(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_lcmc_mle_unitdata_req");
        let SapMsgInner::LcmcMleUnitdataReq(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // Answer to a U-CALL RESTORE that arrived inside a U-RESTORE: return it in the MLE
        // restore PDU instead (D-CALL RESTORE → D-RESTORE-ACK, D-RELEASE → D-RESTORE-FAIL).
        let ssi = prim.main_address.ssi;
        self.pending_restores.retain(|_, at| at.elapsed() < RESTORE_ANSWER_TIMEOUT);
        if self.pending_restores.contains_key(&ssi) {
            let cmce_type = prim.sdu.peek_bits_startoffset(0, 5).and_then(|t| CmcePduTypeDl::try_from(t).ok());
            let (addr, link_id, endpoint_id) = (prim.main_address, prim.link_id, prim.endpoint_id);
            match cmce_type {
                Some(CmcePduTypeDl::DCallRestore) => {
                    self.pending_restores.remove(&ssi);
                    tracing::info!("MLE: D-RESTORE-ACK to {} (call restored)", ssi);
                    let ack = DRestoreAck {
                        sdu: Some(prim.sdu.clone()),
                    };
                    self.send_mle_pdu(queue, addr, link_id, endpoint_id, |b| ack.to_bitbuf(b));
                    return;
                }
                Some(CmcePduTypeDl::DRelease) => {
                    self.pending_restores.remove(&ssi);
                    tracing::info!("MLE: D-RESTORE-FAIL to {} (call could not be restored)", ssi);
                    let fail = DRestoreFail {
                        fail_cause: FAIL_CAUSE_UNSPECIFIED,
                    };
                    self.send_mle_pdu(queue, addr, link_id, endpoint_id, |b| fail.to_bitbuf(b));
                    return;
                }
                _ => {}
            }
        }

        let mle_prot_discriminator = MleProtocolDiscriminator::Cmce;
        let sdu_len = prim.sdu.get_len();
        let mut pdu = BitBuffer::new(3 + sdu_len);
        pdu.write_bits(mle_prot_discriminator.into_raw(), 3);
        pdu.copy_bits(&mut prim.sdu, sdu_len);
        pdu.seek(0);

        // let (_addr, link, endpoint) = self.router.use_handle(prim.handle, message.dltime);
        // assert_eq!(link, prim.link_id);
        // assert_eq!(endpoint, prim.endpoint_id);
        // Take Channel Allocation Request if any
        let chan_alloc = prim.chan_alloc.take();

        let sapmsg = if prim.layer2service == Layer2Service::Unacknowledged {
            // Unacknowledged service, send a TlUnitdataReqBl
            SapMsg {
                sap: Sap::TlaSap,
                src: TetraEntity::Mle,
                dest: TetraEntity::Llc,
                msg: SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
                    main_address: prim.main_address,
                    link_id: prim.link_id,
                    endpoint_id: prim.endpoint_id,
                    tl_sdu: pdu,
                    stealing_permission: prim.stealing_permission,
                    subscriber_class: 0, // TODO fixme
                    fcs_flag: false,
                    air_interface_encryption: None,
                    packet_data_flag: false,
                    n_tlsdu_repeats: 0,
                    data_class_info: None,
                    req_handle: 0,

                    chan_alloc,
                    tx_reporter: prim.tx_reporter.take(),
                }),
            }
        } else {
            // Acknowledged service, send a TlDataReqBl
            SapMsg {
                sap: Sap::TlaSap,
                src: TetraEntity::Mle,
                dest: TetraEntity::Llc,
                msg: SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                    main_address: prim.main_address,
                    link_id: prim.link_id,
                    endpoint_id: prim.endpoint_id,
                    tl_sdu: pdu,
                    stealing_permission: prim.stealing_permission,
                    subscriber_class: 0, // TODO fixme
                    fcs_flag: false,
                    air_interface_encryption: None,
                    stealing_repeats_flag: None,
                    data_class_info: None,
                    req_handle: 0, // TODO FIXME
                    graceful_degradation: None,
                    chan_alloc,
                    tx_reporter: prim.tx_reporter.take(),
                }),
            }
        };

        queue.push_back(sapmsg);
    }

    fn rx_lcmc_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_lcmc_prim");
        match &message.msg {
            SapMsgInner::LcmcMleUnitdataReq(_) => {
                self.rx_lcmc_mle_unitdata_req(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }
}

impl TetraEntityTrait for MleBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Mle
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        // Broadcast D-NWRK-BROADCAST twice per hyperframe (~30.6s interval) if timezone is configured.
        // Two evenly-spaced slots [20, 50] avoid congestion with other hyperframe-triggered events
        // and give terminals a faster time/date update after cold attach.
        if MLE_BROADCAST_MULTIFRAMES.contains(&ts.m) && ts.f == MLE_BROADCAST_FRAME && ts.t == 1 {
            tracing::debug!("MLE: hyperframe broadcast slot (hf={} m={} f={} t={})", ts.h, ts.m, ts.f, ts.t);
            self.broadcast.send_broadcast(queue);
        }
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::TlaSap => {
                self.rx_tla_prim(queue, message);
            }
            Sap::TlmbSap => {
                tracing::warn!("MLE: BS received unexpected broadcast message on TlmbSap, ignoring");
            }
            Sap::TlmcSap => {
                self.rx_tlmc_prim(queue, message);
            }
            Sap::LmmSap => {
                self.rx_lmm_prim(queue, message);
            }
            Sap::TlpdSap => {
                self.rx_tlpd_prim(queue, message);
            }
            Sap::LcmcSap => {
                self.rx_lcmc_prim(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tetra_core::SsiType;
    use tetra_saps::lcmc::LcmcMleUnitdataReq;
    use tetra_saps::tla::TlaTlDataIndBl;

    use super::*;

    const MS: u32 = 2_260_001;

    fn config(site_linked: bool) -> SharedConfig {
        let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1521
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 1
cell_reselection_types_supported = 1
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1525
"#;
        let mut cfg = tetra_config::bluestation::parsing::from_toml_str(toml).unwrap();
        cfg.site_linked = site_linked;
        SharedConfig::from_parts(cfg, None)
    }

    fn addr() -> TetraAddress {
        TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: MS,
        }
    }

    /// An uplink MLE PDU from the MS, as LLC delivers it.
    fn uplink_mle(write: impl FnOnce(&mut BitBuffer)) -> SapMsg {
        let mut b = BitBuffer::new_autoexpand(64);
        b.write_bits(MleProtocolDiscriminator::Mle.into_raw(), 3);
        write(&mut b);
        let len = b.get_pos();
        b.seek(0);
        let mut sdu = BitBuffer::new(len);
        sdu.copy_bits(&mut b, len);
        sdu.seek(0);
        SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::TlaTlDataIndBl(TlaTlDataIndBl {
                main_address: addr(),
                link_id: 0,
                endpoint_id: 0,
                new_endpoint_id: None,
                css_endpoint_id: None,
                tl_sdu: Some(sdu),
                scrambling_code: 0,
                fcs_flag: false,
                air_interface_encryption: 0,
                chan_change_resp_req: false,
                chan_change_handle: None,
                chan_info: None,
                req_handle: 0,
            }),
        }
    }

    fn drain(q: &mut MessageQueue) -> Vec<SapMsg> {
        std::iter::from_fn(|| q.pop_front()).collect()
    }

    /// The MLE PDUs sent down to LLC, as bit strings (discriminator stripped).
    fn sent_mle_pdus(msgs: &[SapMsg]) -> Vec<String> {
        msgs.iter()
            .filter_map(|m| match &m.msg {
                SapMsgInner::TlaTlDataReqBl(r) => Some(r.tl_sdu.to_bitstr()),
                _ => None,
            })
            .filter_map(|s| s.strip_prefix("101").map(str::to_string))
            .collect()
    }

    #[test]
    fn u_prepare_to_advertised_neighbour_gets_d_new_cell() {
        let mut mle = MleBs::new(config(true));
        let mut q = MessageQueue::new();
        let prepare = UPrepare {
            cell_identifier_ca: Some(1),
            sdu: None,
        };
        mle.rx_prim(&mut q, uplink_mle(|b| prepare.to_bitbuf(b).unwrap()));
        let msgs = drain(&mut q);
        assert_eq!(sent_mle_pdus(&msgs), vec!["000010".to_string()], "D-NEW-CELL, change channel immediately");
        assert!(
            msgs.iter().any(|m| m.dest == TetraEntity::Brew
                && matches!(
                    m.msg,
                    SapMsgInner::CmceCallControl(CallControl::SiteHandoverPrepare {
                        issi: MS,
                        target_carrier: 1525
                    })
                )),
            "site switch is told"
        );
    }

    #[test]
    fn u_prepare_to_unknown_cell_gets_d_prepare_fail() {
        let mut mle = MleBs::new(config(false));
        let mut q = MessageQueue::new();
        let prepare = UPrepare {
            cell_identifier_ca: Some(9),
            sdu: None,
        };
        mle.rx_prim(&mut q, uplink_mle(|b| prepare.to_bitbuf(b).unwrap()));
        let msgs = drain(&mut q);
        assert_eq!(sent_mle_pdus(&msgs), vec!["001000".to_string()], "D-PREPARE-FAIL");
        assert!(!msgs.iter().any(|m| m.dest == TetraEntity::Brew));
    }

    fn cmce_answer(sdu_bits: &str) -> SapMsg {
        SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: BitBuffer::from_bitstr(sdu_bits),
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                layer2service: Layer2Service::Acknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                main_address: addr(),
                chan_alloc: None,
                tx_reporter: None,
            }),
        }
    }

    #[test]
    fn u_restore_goes_to_cmce_and_its_answer_comes_back_in_the_mle_pdu() {
        let mut mle = MleBs::new(config(false));
        let mut q = MessageQueue::new();
        let restore = URestore {
            mcc: None,
            mnc: None,
            la: None,
            sdu: Some(BitBuffer::from_bitstr("0101100000000000100")),
        };
        mle.rx_prim(&mut q, uplink_mle(|b| restore.to_bitbuf(b).unwrap()));
        let msgs = drain(&mut q);
        let to_cmce = msgs.iter().find_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataInd(ind) if m.dest == TetraEntity::Cmce => Some(ind.sdu.to_bitstr()),
            _ => None,
        });
        assert_eq!(to_cmce.as_deref(), Some("0101100000000000100"), "U-CALL RESTORE handed to CMCE");

        // CMCE answers with D-CALL RESTORE (type 14 = 01110): wrapped in D-RESTORE-ACK.
        mle.rx_prim(&mut q, cmce_answer("01110000000000010"));
        assert_eq!(sent_mle_pdus(&drain(&mut q)), vec!["1001".to_string() + "01110000000000010"]);

        // A later, unrelated CMCE PDU for the same MS goes out normally (discriminator CMCE).
        mle.rx_prim(&mut q, cmce_answer("01110000000000010"));
        let msgs = drain(&mut q);
        assert!(sent_mle_pdus(&msgs).is_empty());
    }

    #[test]
    fn failed_call_restore_becomes_d_restore_fail() {
        let mut mle = MleBs::new(config(false));
        let mut q = MessageQueue::new();
        let restore = URestore {
            mcc: None,
            mnc: None,
            la: None,
            sdu: Some(BitBuffer::from_bitstr("01011")),
        };
        mle.rx_prim(&mut q, uplink_mle(|b| restore.to_bitbuf(b).unwrap()));
        drain(&mut q);
        // CMCE rejects with D-RELEASE (type 6 = 00110).
        mle.rx_prim(&mut q, cmce_answer("001100000000000"));
        assert_eq!(sent_mle_pdus(&drain(&mut q)), vec!["101000".to_string()], "D-RESTORE-FAIL");
    }
}
