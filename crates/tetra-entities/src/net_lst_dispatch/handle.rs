//! Shared handle between dashboard and LstDispatchEntity.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use uuid::Uuid;

use super::session::{ClaimResult, SessionLock};

const CMD_CAP: usize = 64;
const UL_FRAME_CAP: usize = 8;
const DL_PCM_CAP: usize = 32;
const POS_CAP: usize = 256;

#[derive(Debug, Clone)]
pub enum LstUiCommand {
    SetOperatorIssi { issi: u32 },
    JoinGroup { gssi: u32 },
    LeaveGroup,
    Ptt { down: bool },
    PrivateCall { dest_issi: u32, duplex: bool },
    Hangup,
    /// Raw PCM16 LE mono @ 8 kHz chunk from browser (only while PTT).
    UlPcm { pcm: Vec<i16> },
}

#[derive(Debug, Clone, Default)]
pub struct LstRuntimeStatus {
    pub enabled: bool,
    pub session_busy: bool,
    pub session_holder: Option<String>,
    pub operator_issi: u32,
    pub active_gssi: Option<u32>,
    pub ptt: bool,
    pub call_kind: Option<String>,
    pub call_peer: Option<u32>,
    pub media_ready: bool,
    pub codec_available: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LstPosition {
    pub lat: f64,
    pub lon: f64,
    pub updated: Instant,
}

struct LstSharedInner {
    session: SessionLock,
    status: LstRuntimeStatus,
    cmd_tx: Sender<LstUiCommand>,
    cmd_rx: Receiver<LstUiCommand>,
    /// Downlink PCM chunks for the owning browser (PCM16 LE @ 8 kHz).
    dl_pcm: VecDeque<Vec<i16>>,
    positions: HashMap<u32, LstPosition>,
}

#[derive(Clone)]
pub struct LstDispatchHandle {
    inner: Arc<Mutex<LstSharedInner>>,
}

impl LstDispatchHandle {
    pub fn new(operator_issi: u32, codec_available: bool) -> Self {
        let (cmd_tx, cmd_rx) = bounded(CMD_CAP);
        Self {
            inner: Arc::new(Mutex::new(LstSharedInner {
                session: SessionLock::default(),
                status: LstRuntimeStatus {
                    enabled: true,
                    operator_issi,
                    codec_available,
                    ..Default::default()
                },
                cmd_tx,
                cmd_rx,
                dl_pcm: VecDeque::with_capacity(DL_PCM_CAP),
                positions: HashMap::new(),
            })),
        }
    }

    pub fn claim(&self, client_label: String) -> ClaimResult {
        let mut g = self.inner.lock().unwrap();
        let r = g.session.claim(client_label);
        g.refresh_busy();
        r
    }

    pub fn heartbeat(&self, token: Uuid) -> bool {
        let mut g = self.inner.lock().unwrap();
        let ok = g.session.heartbeat(token);
        g.refresh_busy();
        ok
    }

    pub fn release(&self, token: Uuid) -> bool {
        let mut g = self.inner.lock().unwrap();
        let ok = g.session.release(token);
        if ok {
            // Drop media / ask entity to idle.
            let _ = g.cmd_tx.try_send(LstUiCommand::Hangup);
            let _ = g.cmd_tx.try_send(LstUiCommand::LeaveGroup);
            g.dl_pcm.clear();
        }
        g.refresh_busy();
        ok
    }

    pub fn is_owner(&self, token: Uuid) -> bool {
        self.inner.lock().unwrap().session.is_owner(token)
    }

    pub fn status_json(&self) -> serde_json::Value {
        let mut g = self.inner.lock().unwrap();
        g.refresh_busy();
        let s = &g.status;
        serde_json::json!({
            "enabled": s.enabled,
            "session_busy": s.session_busy,
            "session_holder": s.session_holder,
            "operator_issi": s.operator_issi,
            "active_gssi": s.active_gssi,
            "ptt": s.ptt,
            "call_kind": s.call_kind,
            "call_peer": s.call_peer,
            "media_ready": s.media_ready,
            "codec_available": s.codec_available,
            "last_error": s.last_error,
            "heartbeat_secs": super::session::HEARTBEAT_HINT_SECS,
        })
    }

    pub fn positions_json(&self) -> serde_json::Value {
        let g = self.inner.lock().unwrap();
        let mut arr = Vec::new();
        for (issi, p) in &g.positions {
            arr.push(serde_json::json!({
                "issi": issi,
                "lat": p.lat,
                "lon": p.lon,
                "age_secs": p.updated.elapsed().as_secs(),
            }));
        }
        serde_json::Value::Array(arr)
    }

    pub fn note_position(&self, issi: u32, lat: f64, lon: f64) {
        let mut g = self.inner.lock().unwrap();
        if g.positions.len() >= POS_CAP && !g.positions.contains_key(&issi) {
            // Drop an arbitrary old entry.
            if let Some(k) = g.positions.keys().next().copied() {
                g.positions.remove(&k);
            }
        }
        g.positions.insert(
            issi,
            LstPosition {
                lat,
                lon,
                updated: Instant::now(),
            },
        );
    }

    pub fn push_cmd(&self, token: Uuid, cmd: LstUiCommand) -> Result<(), String> {
        let g = self.inner.lock().unwrap();
        if !g.session.is_owner(token) {
            return Err("not session owner".into());
        }
        g.cmd_tx.try_send(cmd).map_err(|_| "command queue full".to_string())
    }

    pub fn push_ul_pcm(&self, token: Uuid, pcm: Vec<i16>) -> Result<(), String> {
        if pcm.is_empty() || pcm.len() > 8_000 {
            return Err("bad pcm size".into());
        }
        self.push_cmd(token, LstUiCommand::UlPcm { pcm })
    }

    /// Entity: drain UI commands (cap per tick).
    pub fn drain_cmds(&self, max: usize) -> Vec<LstUiCommand> {
        let g = self.inner.lock().unwrap();
        let mut out = Vec::with_capacity(max.min(16));
        for _ in 0..max {
            match g.cmd_rx.try_recv() {
                Ok(c) => out.push(c),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    pub fn set_status<F: FnOnce(&mut LstRuntimeStatus)>(&self, f: F) {
        let mut g = self.inner.lock().unwrap();
        f(&mut g.status);
        g.refresh_busy();
    }

    pub fn push_dl_pcm(&self, pcm: Vec<i16>) {
        let mut g = self.inner.lock().unwrap();
        if !g.session.has_owner() {
            return;
        }
        while g.dl_pcm.len() >= DL_PCM_CAP {
            g.dl_pcm.pop_front();
        }
        g.dl_pcm.push_back(pcm);
    }

    pub fn take_dl_pcm(&self, token: Uuid, max: usize) -> Vec<Vec<i16>> {
        let mut g = self.inner.lock().unwrap();
        if !g.session.is_owner(token) {
            return Vec::new();
        }
        let n = max.min(g.dl_pcm.len()).min(UL_FRAME_CAP);
        g.dl_pcm.drain(..n).collect()
    }
}

impl LstSharedInner {
    fn refresh_busy(&mut self) {
        self.status.session_busy = self.session.has_owner();
        self.status.session_holder = self.session.busy_holder();
    }
}
