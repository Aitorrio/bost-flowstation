//! LST Dispatch entity — cell-local operator bridged like Brew SAPs, without Brew protocol.
//!
//! Registered as [`TetraEntity::Brew`] only when real Brew is absent, so CMCE
//! `NetworkCallReady` / circuit media route here unchanged.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::SharedConfig;
use tetra_core::{Sap, TdmaTime, tetra_entities::TetraEntity};
use tetra_saps::{
    SapMsg, SapMsgInner,
    control::call_control::{CallControl, NetworkCircuitCall},
    tmd::TmdCircuitDataReq,
};

use super::handle::{LstDispatchHandle, LstUiCommand};
use super::media::LstCodec;

const MAX_CMDS_PER_TICK: usize = 16;
const MAX_UL_PCM_PER_TICK: usize = 24;
const MAX_UL_BLOCKS_PER_TICK: usize = 4;

struct ActiveGroup {
    uuid: Uuid,
    gssi: u32,
    call_id: Option<u16>,
    carrier_num: Option<u16>,
    ts: Option<u8>,
    ptt: bool,
    last_activity: Instant,
    last_keepalive: Instant,
}

struct ActivePrivate {
    uuid: Uuid,
    dest: u32,
    duplex: bool,
    call_id: Option<u16>,
    carrier_num: Option<u16>,
    ts: Option<u8>,
    ptt: bool,
}

pub struct LstDispatchEntity {
    #[allow(dead_code)]
    config: SharedConfig,
    handle: LstDispatchHandle,
    operator_issi: u32,
    codec: Option<LstCodec>,
    group: Option<ActiveGroup>,
    private: Option<ActivePrivate>,
    pending_ul: VecDeque<Vec<u8>>,
    dltime: TdmaTime,
}

impl LstDispatchEntity {
    pub fn new(config: SharedConfig, handle: LstDispatchHandle) -> Self {
        let operator_issi = config
            .config()
            .lst_dispatch
            .as_ref()
            .map(|c| c.operator_issi)
            .unwrap_or(9_990_001);
        let codec = LstCodec::new();
        handle.set_status(|s| {
            s.enabled = true;
            s.operator_issi = operator_issi;
            s.codec_available = codec.is_some();
            s.media_ready = codec.is_some();
        });
        Self {
            config,
            handle,
            operator_issi,
            codec,
            group: None,
            private: None,
            pending_ul: VecDeque::with_capacity(8),
            dltime: TdmaTime::default(),
        }
    }

    fn push_cc(&self, queue: &mut MessageQueue, cc: CallControl) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(cc),
        });
    }

    fn process_cmds(&mut self, queue: &mut MessageQueue) {
        for cmd in self.handle.drain_cmds(MAX_CMDS_PER_TICK) {
            match cmd {
                LstUiCommand::SetOperatorIssi { issi } => {
                    self.operator_issi = issi.clamp(1, 16_777_214);
                    self.handle.set_status(|s| s.operator_issi = self.operator_issi);
                }
                LstUiCommand::JoinGroup { gssi } => self.join_group(queue, gssi),
                LstUiCommand::LeaveGroup => self.leave_group(queue),
                LstUiCommand::Ptt { down } => self.set_ptt(queue, down),
                LstUiCommand::PrivateCall { dest_issi, duplex } => {
                    self.start_private(queue, dest_issi, duplex);
                }
                LstUiCommand::Hangup => {
                    self.hangup_private(queue);
                    // Also cease group TX if holding PTT.
                    if self.group.as_ref().is_some_and(|g| g.ptt || g.call_id.is_some()) {
                        self.set_ptt(queue, false);
                    }
                    self.handle.set_status(|s| s.ptt = false);
                }
            }
        }
        for pcm in self.handle.drain_ul_pcm(MAX_UL_PCM_PER_TICK) {
            self.on_ul_pcm(pcm);
        }
    }

    fn join_group(&mut self, queue: &mut MessageQueue, gssi: u32) {
        if gssi == 0 || gssi > 16_777_214 {
            self.handle
                .set_status(|s| s.last_error = Some("invalid GSSI".into()));
            return;
        }
        // End any prior group call; Join only selects the TG (Brew-style GROUP_TX on PTT).
        self.leave_group(queue);
        self.group = Some(ActiveGroup {
            uuid: Uuid::new_v4(),
            gssi,
            call_id: None,
            carrier_num: None,
            ts: None,
            ptt: false,
            last_activity: Instant::now(),
            last_keepalive: Instant::now(),
        });
        self.handle.set_status(|s| {
            s.active_gssi = Some(gssi);
            s.call_kind = Some("group".into());
            s.call_peer = Some(gssi);
            s.ptt = false;
            s.last_error = None;
        });
        tracing::info!("LST: selected group GSSI={} as ISSI={}", gssi, self.operator_issi);
    }

    fn leave_group(&mut self, queue: &mut MessageQueue) {
        if let Some(g) = self.group.take() {
            if g.call_id.is_some() || g.ptt {
                self.push_cc(
                    queue,
                    CallControl::NetworkCallEnd {
                        brew_uuid: g.uuid,
                    },
                );
            }
        }
        if self.private.is_none() {
            self.handle.set_status(|s| {
                s.active_gssi = None;
                s.call_kind = None;
                s.call_peer = None;
                s.ptt = false;
            });
        }
    }

    fn set_ptt(&mut self, queue: &mut MessageQueue, down: bool) {
        // Group: PTT down starts NetworkCallStart; PTT up ends the call (no hangtime yet).
        if self.group.is_some() {
            if down {
                let (uuid, gssi, need_new_uuid) = {
                    let g = self.group.as_ref().unwrap();
                    // Fresh UUID if previous call already ended / never got ready.
                    let need_new = g.call_id.is_none() && !g.ptt;
                    (g.uuid, g.gssi, need_new)
                };
                let uuid = if need_new_uuid {
                    let u = Uuid::new_v4();
                    if let Some(g) = self.group.as_mut() {
                        g.uuid = u;
                        g.call_id = None;
                        g.carrier_num = None;
                        g.ts = None;
                    }
                    u
                } else {
                    uuid
                };
                if let Some(g) = self.group.as_mut() {
                    g.ptt = true;
                    g.last_activity = Instant::now();
                }
                let operator_issi = self.operator_issi;
                self.push_cc(
                    queue,
                    CallControl::NetworkCallStart {
                        brew_uuid: uuid,
                        source_issi: operator_issi,
                        dest_gssi: gssi,
                        priority: 0,
                    },
                );
                self.handle.set_status(|s| {
                    s.ptt = true;
                    s.last_error = None;
                });
            } else {
                let uuid = self.group.as_ref().map(|g| g.uuid);
                if let Some(g) = self.group.as_mut() {
                    g.ptt = false;
                    g.last_activity = Instant::now();
                    g.call_id = None;
                    g.carrier_num = None;
                    g.ts = None;
                }
                if let Some(uuid) = uuid {
                    self.push_cc(
                        queue,
                        CallControl::NetworkCallEnd { brew_uuid: uuid },
                    );
                }
                if let Some(ref mut c) = self.codec {
                    c.reset_ul();
                }
                self.pending_ul.clear();
                self.handle.set_status(|s| s.ptt = false);
            }
            return;
        }

        if let Some(p) = self.private.as_mut() {
            p.ptt = down;
        }
        let private_snap = self.private.as_ref().map(|p| (p.uuid, p.duplex));
        if let Some((uuid, duplex)) = private_snap {
            if down {
                if !duplex {
                    self.push_cc(
                        queue,
                        CallControl::NetworkCircuitSimplexGranted {
                            brew_uuid: uuid,
                            grant: 0,
                            permission: 0,
                        },
                    );
                }
            } else {
                if !duplex {
                    self.push_cc(
                        queue,
                        CallControl::NetworkCircuitSimplexIdle {
                            brew_uuid: uuid,
                            grant: 0,
                            permission: 0,
                        },
                    );
                }
                if let Some(ref mut c) = self.codec {
                    c.reset_ul();
                }
                self.pending_ul.clear();
            }
            self.handle.set_status(|s| s.ptt = down);
            return;
        }

        self.handle
            .set_status(|s| s.last_error = Some("no active call for PTT — Join a GSSI first".into()));
    }

    fn start_private(&mut self, queue: &mut MessageQueue, dest: u32, duplex: bool) {
        if dest == 0 || dest > 16_777_214 {
            self.handle
                .set_status(|s| s.last_error = Some("invalid ISSI".into()));
            return;
        }
        self.hangup_private(queue);
        let uuid = Uuid::new_v4();
        let call = make_circuit_call(self.operator_issi, dest, duplex);
        self.push_cc(
            queue,
            CallControl::NetworkCircuitSetupRequest {
                brew_uuid: uuid,
                call,
            },
        );
        self.private = Some(ActivePrivate {
            uuid,
            dest,
            duplex,
            call_id: None,
            carrier_num: None,
            ts: None,
            ptt: false,
        });
        self.handle.set_status(|s| {
            s.call_kind = Some(if duplex { "duplex" } else { "simplex" }.into());
            s.call_peer = Some(dest);
            s.last_error = None;
        });
        tracing::info!(
            "LST: private {} -> {} duplex={}",
            self.operator_issi,
            dest,
            duplex
        );
    }

    fn hangup_private(&mut self, queue: &mut MessageQueue) {
        if let Some(p) = self.private.take() {
            self.push_cc(
                queue,
                CallControl::NetworkCircuitRelease {
                    brew_uuid: p.uuid,
                    cause: 0,
                },
            );
        }
        if self.group.is_none() {
            self.handle.set_status(|s| {
                s.call_kind = None;
                s.call_peer = None;
                s.ptt = false;
            });
        } else {
            self.handle.set_status(|s| {
                s.call_kind = Some("group".into());
                s.call_peer = self.group.as_ref().map(|g| g.gssi);
                s.ptt = false;
            });
        }
    }

    fn on_ul_pcm(&mut self, pcm: Vec<i16>) {
        let allow = self
            .group
            .as_ref()
            .map(|g| g.ptt)
            .or_else(|| {
                self.private.as_ref().map(|p| {
                    // Duplex: stream while call is up (slot assigned). Simplex: only with PTT.
                    if p.duplex {
                        p.carrier_num.is_some() && p.ts.is_some()
                    } else {
                        p.ptt
                    }
                })
            })
            .unwrap_or(false);
        if !allow {
            return;
        }
        let Some(ref mut codec) = self.codec else {
            return;
        };
        for block in codec.encode_pcm(&pcm) {
            while self.pending_ul.len() >= 8 {
                self.pending_ul.pop_front();
            }
            self.pending_ul.push_back(block);
        }
    }

    fn flush_ul(&mut self, queue: &mut MessageQueue) {
        let (carrier_num, ts) = if let Some(ref g) = self.group {
            if !g.ptt {
                return;
            }
            match (g.carrier_num, g.ts) {
                (Some(car), Some(t)) => (car, t),
                _ => return,
            }
        } else if let Some(ref p) = self.private {
            if p.duplex {
                match (p.carrier_num, p.ts) {
                    (Some(car), Some(t)) => (car, t),
                    _ => return,
                }
            } else if p.ptt {
                match (p.carrier_num, p.ts) {
                    (Some(car), Some(t)) => (car, t),
                    _ => return,
                }
            } else {
                return;
            }
        } else {
            return;
        };

        let mut n = 0;
        while n < MAX_UL_BLOCKS_PER_TICK {
            let Some(data) = self.pending_ul.pop_front() else {
                break;
            };
            queue.push_back(SapMsg {
                sap: Sap::TmdSap,
                src: TetraEntity::Brew,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
                    carrier_num,
                    ts,
                    data,
                }),
            });
            n += 1;
        }
    }

    fn on_network_call_ready(
        &mut self,
        brew_uuid: Uuid,
        call_id: u16,
        carrier_num: u16,
        ts: u8,
    ) {
        if let Some(ref mut g) = self.group
            && g.uuid == brew_uuid
        {
            g.call_id = Some(call_id);
            g.carrier_num = Some(carrier_num);
            g.ts = Some(ts);
            g.last_activity = Instant::now();
            tracing::debug!("LST: group ready call_id={} ts={}", call_id, ts);
        }
        if let Some(ref mut p) = self.private
            && p.uuid == brew_uuid
        {
            p.call_id = Some(call_id);
            p.carrier_num = Some(carrier_num);
            p.ts = Some(ts);
        }
    }

    fn on_dl_voice(&mut self, data: &[u8]) {
        let Some(ref mut codec) = self.codec else {
            return;
        };
        if let Some(pcm) = codec.decode_tmd(data) {
            self.handle.push_dl_pcm(pcm);
        }
    }
}

fn make_circuit_call(from: u32, to: u32, duplex: bool) -> NetworkCircuitCall {
    NetworkCircuitCall {
        source_issi: from,
        destination: to,
        number: String::new(),
        priority: 0,
        service: 0,
        mode: 0,
        duplex: if duplex { 1 } else { 0 },
        method: 0,
        // 0 = point-to-point (CommunicationType::P2p)
        communication: 0,
        grant: 0,
        permission: 0,
        timeout: 0,
        ownership: 0,
        queued: 0,
    }
}

impl TetraEntityTrait for LstDispatchEntity {
    fn entity(&self) -> TetraEntity {
        // Occupy the Brew slot while real Brew is off (XOR at registration).
        TetraEntity::Brew
    }

    fn rx_prim(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        match message.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid,
                call_id,
                carrier_num,
                ts,
                ..
            }) => {
                self.on_network_call_ready(brew_uuid, call_id, carrier_num, ts);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady {
                brew_uuid,
                call_id,
                carrier_num,
                ts,
            }) => {
                self.on_network_call_ready(brew_uuid, call_id, carrier_num, ts);
                self.handle.set_status(|s| {
                    s.media_ready = true;
                    s.last_error = None;
                });
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid }) => {
                if self.private.as_ref().is_some_and(|p| p.uuid == brew_uuid) {
                    tracing::info!("LST: private setup accepted uuid={}", brew_uuid);
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject { brew_uuid, cause }) => {
                if self.private.as_ref().is_some_and(|p| p.uuid == brew_uuid) {
                    self.private = None;
                    self.handle.set_status(|s| {
                        s.call_kind = None;
                        s.call_peer = None;
                        s.ptt = false;
                        s.last_error = Some(format!("private call rejected (cause {cause})"));
                    });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
                brew_uuid,
                ..
            }) => {
                // MS answered; MediaReady usually follows — keep selection.
                if self.private.as_ref().is_some_and(|p| p.uuid == brew_uuid) {
                    tracing::info!("LST: private connect confirmed uuid={}", brew_uuid);
                }
            }
            SapMsgInner::TmdCircuitDataInd(ind) => {
                self.on_dl_voice(&ind.data);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid })
            | SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid, .. }) => {
                if self.group.as_ref().is_some_and(|g| g.uuid == brew_uuid) {
                    let had_ready = self.group.as_ref().is_some_and(|g| g.call_id.is_some());
                    let still_ptt = self.group.as_ref().is_some_and(|g| g.ptt);
                    if let Some(g) = self.group.as_mut() {
                        g.ptt = false;
                        g.call_id = None;
                        g.carrier_num = None;
                        g.ts = None;
                        // Keep GSSI selection so operator can PTT again.
                    }
                    self.handle.set_status(|s| {
                        s.ptt = false;
                        if still_ptt && !had_ready {
                            s.last_error = Some(
                                "group call rejected — check radios are affiliated to this GSSI"
                                    .into(),
                            );
                        }
                    });
                }
                if self.private.as_ref().is_some_and(|p| p.uuid == brew_uuid) {
                    self.private = None;
                    self.handle.set_status(|s| {
                        s.ptt = false;
                        if matches!(s.call_kind.as_deref(), Some("simplex" | "duplex")) {
                            s.call_kind = None;
                            s.call_peer = None;
                        }
                    });
                }
            }
            _ => {}
        }
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;
        self.process_cmds(queue);
    }

    fn tick_end(&mut self, queue: &mut MessageQueue, _ts: TdmaTime) -> bool {
        self.flush_ul(queue);
        // Keep CMCE group call alive while PTT is held (~2.5 Hz keepalive).
        let keepalive_uuid = self.group.as_mut().and_then(|g| {
            if g.ptt && g.call_id.is_some() && g.last_keepalive.elapsed() >= Duration::from_millis(400)
            {
                g.last_keepalive = Instant::now();
                Some(g.uuid)
            } else {
                None
            }
        });
        if let Some(uuid) = keepalive_uuid {
            self.push_cc(
                queue,
                CallControl::NetworkCallMediaActivity { brew_uuid: uuid },
            );
        }
        // Reap if PTT held but never got NetworkCallReady.
        let timed_out_gssi = self.group.as_ref().and_then(|g| {
            if g.ptt && g.call_id.is_none() && g.last_activity.elapsed() > Duration::from_secs(5) {
                Some(g.gssi)
            } else {
                None
            }
        });
        if let Some(gssi) = timed_out_gssi {
            tracing::warn!("LST: group setup timed out GSSI={}", gssi);
            self.set_ptt(queue, false);
            self.handle.set_status(|s| {
                s.last_error = Some(format!(
                    "group setup timeout on GSSI {gssi} — affiliated radios?"
                ));
            });
        }
        false
    }
}
