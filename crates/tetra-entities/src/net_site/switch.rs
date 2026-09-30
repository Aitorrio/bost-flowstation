use std::collections::{BTreeSet, HashMap};

use crossbeam_channel::{Receiver, Sender, unbounded};
use tetra_core::{CellId, TdmaTime, tetra_entities::TetraEntity};
use tetra_saps::{SapMsg, SapMsgInner, control::call_control::CallControl};
use uuid::Uuid;

use super::SiteDirectory;
use crate::{MessageQueue, TetraEntityTrait};

/// Stand-in for the network entity in an additional cell's router. Everything the cell sends to
/// `TetraEntity::Brew` goes to the [`SiteSwitch`]; whatever the switch routes to this cell is
/// injected into the cell's queue at the start of each tick.
pub struct CellLink {
    id: CellId,
    to_switch: Sender<(CellId, SapMsg)>,
    from_switch: Receiver<SapMsg>,
}

impl TetraEntityTrait for CellLink {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Brew
    }

    fn rx_prim(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        // Unbounded: the switch drains every primary tick, so this can't grow without bound
        // unless the primary stack is stalled (which the health watchdog handles).
        let _ = self.to_switch.send((self.id, message));
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, _ts: TdmaTime) {
        while let Ok(message) = self.from_switch.try_recv() {
            queue.push_back(message);
        }
    }
}

/// Switch-side ends of the channels to the additional cells.
pub struct SitePorts {
    to_cells: HashMap<CellId, Sender<SapMsg>>,
    from_cells: Receiver<(CellId, SapMsg)>,
}

/// Create the channels between the switch and one [`CellLink`] per additional cell.
pub fn site_links(extra_cells: &[CellId]) -> (SitePorts, HashMap<CellId, CellLink>) {
    let (to_switch, from_cells) = unbounded();
    let mut to_cells = HashMap::new();
    let mut links = HashMap::new();
    for &id in extra_cells {
        let (tx, rx) = unbounded();
        to_cells.insert(id, tx);
        links.insert(
            id,
            CellLink {
                id,
                to_switch: to_switch.clone(),
                from_switch: rx,
            },
        );
    }
    (SitePorts { to_cells, from_cells }, links)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellCircuit {
    cell: CellId,
    carrier_num: u16,
    ts: u8,
}

/// One network group call fanned out to several cells.
#[derive(Debug)]
struct GroupSession {
    gssi: u32,
    source_issi: u32,
    priority: u8,
    /// Cells sent a NetworkCallStart that have not answered Ready/Hold/End yet.
    pending: BTreeSet<CellId>,
    /// Circuit reported to the network entity; its downlink voice is copied to `replicas`.
    anchor: Option<CellCircuit>,
    /// False once the anchor's cell left the call (its voice is still copied to the others).
    anchor_live: bool,
    replicas: Vec<CellCircuit>,
}

impl GroupSession {
    fn cells(&self) -> BTreeSet<CellId> {
        let mut cells = self.pending.clone();
        if let Some(a) = self.anchor.filter(|_| self.anchor_live) {
            cells.insert(a.cell);
        }
        cells.extend(self.replicas.iter().map(|r| r.cell));
        cells
    }

    fn remove_cell(&mut self, cell: CellId) {
        self.pending.remove(&cell);
        self.replicas.retain(|r| r.cell != cell);
        if self.anchor.is_some_and(|a| a.cell == cell) {
            self.anchor_live = false;
        }
    }
}

/// Call identifiers are allocated per cell; the network entity needs them unique site-wide.
#[derive(Debug, Default)]
struct CallIdMap {
    to_global: HashMap<(CellId, u16), u16>,
    to_local: HashMap<u16, (CellId, u16)>,
    next: u16,
}

impl CallIdMap {
    fn global(&mut self, cell: CellId, local: u16) -> u16 {
        if let Some(&g) = self.to_global.get(&(cell, local)) {
            return g;
        }
        loop {
            self.next = self.next.wrapping_add(1);
            if self.next != 0 && !self.to_local.contains_key(&self.next) {
                break;
            }
        }
        self.to_global.insert((cell, local), self.next);
        self.to_local.insert(self.next, (cell, local));
        self.next
    }

    fn local(&self, global: u16) -> Option<(CellId, u16)> {
        self.to_local.get(&global).copied()
    }

    fn release(&mut self, cell: CellId, local: u16) {
        if let Some(g) = self.to_global.remove(&(cell, local)) {
            self.to_local.remove(&g);
        }
    }
}

/// Wraps the network entity (Brew / LST Dispatch) in the primary router; see the module docs.
pub struct SiteSwitch {
    inner: Box<dyn TetraEntityTrait>,
    all_cells: Vec<CellId>,
    carrier_cell: HashMap<u16, CellId>,
    ports: SitePorts,
    directory: SiteDirectory,
    sessions: HashMap<Uuid, GroupSession>,
    /// Individual (circuit) calls with the network: Brew session → cell of the local party.
    circuit_calls: HashMap<Uuid, CellId>,
    call_ids: CallIdMap,
}

impl SiteSwitch {
    /// `carriers` lists every cell (primary included) with its carrier numbers.
    pub fn new(inner: Box<dyn TetraEntityTrait>, carriers: &[(CellId, Vec<u16>)], ports: SitePorts) -> Self {
        let mut all_cells: Vec<CellId> = carriers.iter().map(|(c, _)| *c).collect();
        all_cells.sort();
        let carrier_cell = carriers
            .iter()
            .flat_map(|(cell, cs)| cs.iter().map(move |c| (*c, *cell)))
            .collect();
        Self {
            inner,
            all_cells,
            carrier_cell,
            ports,
            directory: SiteDirectory::default(),
            sessions: HashMap::new(),
            circuit_calls: HashMap::new(),
            call_ids: CallIdMap::default(),
        }
    }

    fn deliver(&self, queue: &mut MessageQueue, cell: CellId, message: SapMsg) {
        if cell.is_primary() {
            queue.push_back(message);
        } else if let Some(tx) = self.ports.to_cells.get(&cell) {
            let _ = tx.send(message);
        } else {
            tracing::warn!("SiteSwitch: no link to {cell}, dropping {:?}", message.msg);
        }
    }

    fn deliver_all(&self, queue: &mut MessageQueue, cells: impl IntoIterator<Item = CellId>, message: &SapMsg) {
        for cell in cells {
            self.deliver(queue, cell, message.clone());
        }
    }

    fn cell_of_carrier(&self, carrier_num: u16) -> CellId {
        self.carrier_cell.get(&carrier_num).copied().unwrap_or(CellId::PRIMARY)
    }

    /// Run the network entity with a private queue and route what it emits.
    fn with_inner(&mut self, queue: &mut MessageQueue, f: impl FnOnce(&mut dyn TetraEntityTrait, &mut MessageQueue)) {
        let mut out = MessageQueue::new();
        f(self.inner.as_mut(), &mut out);
        while let Some(message) = out.pop_front() {
            self.route_from_network(queue, message);
        }
    }

    fn to_inner(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        self.with_inner(queue, |inner, out| inner.rx_prim(out, message));
    }

    /// A cell (primary included) sent `message` to the network entity.
    fn handle_from_cell(&mut self, queue: &mut MessageQueue, cell: CellId, mut message: SapMsg) {
        match &mut message.msg {
            SapMsgInner::MmSubscriberUpdate(update) => {
                self.directory.apply(cell, update);
            }
            SapMsgInner::CmceSdsData(sds) => {
                let dest = sds.dest_issi;
                if let Some(at) = self.directory.location(dest)
                    && at != cell
                {
                    // Radio on a sibling cell: deliver directly, never via the network.
                    let copy = SapMsg::new(message.sap, TetraEntity::Brew, TetraEntity::Cmce, message.msg.clone());
                    self.deliver(queue, at, copy);
                    return;
                }
                let others: Vec<CellId> = self.directory.group_cells(dest).into_iter().filter(|c| *c != cell).collect();
                if !others.is_empty() {
                    let copy = SapMsg::new(message.sap, TetraEntity::Brew, TetraEntity::Cmce, message.msg.clone());
                    self.deliver_all(queue, others, &copy);
                }
            }
            SapMsgInner::CmceCallControl(cc) => {
                if !self.handle_call_control_from_cell(queue, cell, cc) {
                    return;
                }
            }
            _ => {}
        }
        self.to_inner(queue, message);
    }

    /// Returns false when the message is consumed by the switch and must not reach the network
    /// entity. Renumbers call identifiers in place.
    fn handle_call_control_from_cell(&mut self, queue: &mut MessageQueue, cell: CellId, cc: &mut CallControl) -> bool {
        match cc {
            CallControl::NetworkCallReady {
                brew_uuid,
                carrier_num,
                ts,
                ..
            } => {
                if let Some(s) = self.sessions.get_mut(brew_uuid) {
                    s.pending.remove(&cell);
                    let circuit = CellCircuit {
                        cell,
                        carrier_num: *carrier_num,
                        ts: *ts,
                    };
                    // The anchor's own cell answering again (speaker change) just refreshes it.
                    if s.anchor.is_some_and(|a| a.cell != cell) {
                        s.replicas.retain(|r| r.cell != cell);
                        s.replicas.push(circuit);
                        return false;
                    }
                    s.anchor = Some(circuit);
                    s.anchor_live = true;
                }
            }
            CallControl::NetworkCallHold { brew_uuid, .. } => {
                if let Some(s) = self.sessions.get_mut(brew_uuid) {
                    s.pending.remove(&cell);
                    // Only the last cell to answer may put the network call on hold.
                    if s.anchor.is_some() || !s.pending.is_empty() {
                        return false;
                    }
                }
            }
            CallControl::NetworkCallEnd { brew_uuid } => {
                if let Some(s) = self.sessions.get_mut(brew_uuid) {
                    s.remove_cell(cell);
                    if !s.cells().is_empty() {
                        return false;
                    }
                    self.sessions.remove(brew_uuid);
                }
            }
            CallControl::GroupListenersAvailable { gssi } => {
                // A cell gained its first member of a group whose network call is already on air
                // elsewhere: bring this cell into the call too.
                let gssi = *gssi;
                let join = self
                    .sessions
                    .iter_mut()
                    .find(|(_, s)| s.gssi == gssi && !s.cells().contains(&cell))
                    .map(|(uuid, s)| {
                        s.pending.insert(cell);
                        CallControl::NetworkCallStart {
                            brew_uuid: *uuid,
                            source_issi: s.source_issi,
                            dest_gssi: gssi,
                            priority: s.priority,
                        }
                    });
                if let Some(start) = join {
                    let msg = SapMsg::new(
                        tetra_core::Sap::Control,
                        TetraEntity::Brew,
                        TetraEntity::Cmce,
                        SapMsgInner::CmceCallControl(start),
                    );
                    self.deliver(queue, cell, msg);
                }
            }
            _ => {
                if let Some(uuid) = circuit_uuid(cc) {
                    self.circuit_calls.insert(uuid, cell);
                }
            }
        }

        let ended = matches!(cc, CallControl::CallEnded { .. });
        if let Some(id) = call_id_mut(cc) {
            let local = *id;
            *id = self.call_ids.global(cell, local);
            if ended {
                self.call_ids.release(cell, local);
            }
        }
        true
    }

    /// The network entity emitted `message` towards the stack.
    fn route_from_network(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        match &mut message.msg {
            SapMsgInner::TmdCircuitDataReq(req) => {
                let (carrier_num, ts) = (req.carrier_num, req.ts);
                let session = self
                    .sessions
                    .values()
                    .find(|s| s.anchor.is_some_and(|a| a.carrier_num == carrier_num && a.ts == ts));
                if let Some(s) = session {
                    for r in &s.replicas {
                        let mut copy = message.clone();
                        if let SapMsgInner::TmdCircuitDataReq(c) = &mut copy.msg {
                            c.carrier_num = r.carrier_num;
                            c.ts = r.ts;
                        }
                        self.deliver(queue, r.cell, copy);
                    }
                    if s.anchor_live {
                        self.deliver(queue, self.cell_of_carrier(carrier_num), message);
                    }
                    return;
                }
                let cell = self.cell_of_carrier(carrier_num);
                self.deliver(queue, cell, message);
            }
            SapMsgInner::CmceCallControl(cc) => {
                if let Some(id) = call_id_mut(cc)
                    && let Some((cell, local)) = self.call_ids.local(*id)
                {
                    *id = local;
                    self.deliver(queue, cell, message);
                    return;
                }
                let targets = self.network_call_targets(cc);
                self.deliver_all(queue, targets, &message);
            }
            SapMsgInner::CmceSdsData(sds) => {
                let dest = sds.dest_issi;
                let targets = match self.directory.location(dest) {
                    Some(cell) => BTreeSet::from([cell]),
                    None => self.directory.group_cells(dest),
                };
                let targets = if targets.is_empty() {
                    BTreeSet::from([CellId::PRIMARY])
                } else {
                    targets
                };
                self.deliver_all(queue, targets, &message);
            }
            // External-subscriber presence from the network: every cell keeps its own view.
            SapMsgInner::MmSubscriberUpdate(_) => {
                let cells = self.all_cells.clone();
                self.deliver_all(queue, cells, &message);
            }
            _ => self.deliver(queue, CellId::PRIMARY, message),
        }
    }

    fn network_call_targets(&mut self, cc: &CallControl) -> BTreeSet<CellId> {
        let primary = || BTreeSet::from([CellId::PRIMARY]);
        match cc {
            CallControl::NetworkCallStart {
                brew_uuid,
                source_issi,
                dest_gssi,
                priority,
            } => {
                let mut targets = self.directory.group_cells(*dest_gssi);
                let s = self.sessions.entry(*brew_uuid).or_insert_with(|| GroupSession {
                    gssi: *dest_gssi,
                    source_issi: *source_issi,
                    priority: *priority,
                    pending: BTreeSet::new(),
                    anchor: None,
                    anchor_live: false,
                    replicas: Vec::new(),
                });
                s.gssi = *dest_gssi;
                s.source_issi = *source_issi;
                s.priority = *priority;
                targets.extend(s.cells());
                if targets.is_empty() {
                    // Nobody affiliated anywhere: the primary answers (Hold) as before.
                    targets = primary();
                }
                let already = s.cells();
                s.pending.extend(targets.iter().filter(|c| !already.contains(c)));
                targets
            }
            CallControl::NetworkCallEnd { brew_uuid } => match self.sessions.remove(brew_uuid) {
                Some(s) => {
                    let mut cells = s.cells();
                    if let Some(a) = s.anchor {
                        cells.insert(a.cell);
                    }
                    cells
                }
                None => self.all_cells.iter().copied().collect(),
            },
            CallControl::NetworkCallMediaActivity { brew_uuid } => match self.sessions.get(brew_uuid) {
                Some(s) => s.cells(),
                None => self.all_cells.iter().copied().collect(),
            },
            CallControl::NetworkCircuitSetupRequest { brew_uuid, call } => {
                let cell = self.directory.location(call.destination).unwrap_or(CellId::PRIMARY);
                self.circuit_calls.insert(*brew_uuid, cell);
                BTreeSet::from([cell])
            }
            _ => {
                if let Some(uuid) = circuit_uuid(cc) {
                    let cell = if matches!(cc, CallControl::NetworkCircuitRelease { .. }) {
                        self.circuit_calls.remove(&uuid)
                    } else {
                        self.circuit_calls.get(&uuid).copied()
                    };
                    return match cell {
                        Some(cell) => BTreeSet::from([cell]),
                        None => self.all_cells.iter().copied().collect(),
                    };
                }
                match carrier_of(cc) {
                    Some(carrier) => BTreeSet::from([self.cell_of_carrier(carrier)]),
                    None => primary(),
                }
            }
        }
    }
}

impl TetraEntityTrait for SiteSwitch {
    fn entity(&self) -> TetraEntity {
        self.inner.entity()
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        self.handle_from_cell(queue, CellId::PRIMARY, message);
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        let incoming: Vec<(CellId, SapMsg)> = self.ports.from_cells.try_iter().collect();
        for (cell, message) in incoming {
            self.handle_from_cell(queue, cell, message);
        }
        self.with_inner(queue, |inner, out| inner.tick_start(out, ts));
    }

    fn tick_end(&mut self, queue: &mut MessageQueue, ts: TdmaTime) -> bool {
        let mut result = false;
        self.with_inner(queue, |inner, out| result = inner.tick_end(out, ts));
        result
    }
}

fn call_id_mut(cc: &mut CallControl) -> Option<&mut u16> {
    match cc {
        CallControl::FloorGranted { call_id, .. }
        | CallControl::RemoteFloorGranted { call_id, .. }
        | CallControl::FloorReleased { call_id, .. }
        | CallControl::CallEnded { call_id, .. }
        | CallControl::NetworkCallReady { call_id, .. }
        | CallControl::OngoingGroupCall { call_id, .. }
        | CallControl::NetworkCircuitMediaReady { call_id, .. } => Some(call_id),
        _ => None,
    }
}

fn carrier_of(cc: &CallControl) -> Option<u16> {
    match cc {
        CallControl::Open(c) => Some(c.carrier_num),
        CallControl::CloseSlot { carrier_num, .. }
        | CallControl::SetDlMediaSource { carrier_num, .. }
        | CallControl::FloorGranted { carrier_num, .. }
        | CallControl::RemoteFloorGranted { carrier_num, .. }
        | CallControl::FloorReleased { carrier_num, .. }
        | CallControl::CallEnded { carrier_num, .. }
        | CallControl::NetworkCallReady { carrier_num, .. }
        | CallControl::OngoingGroupCall { carrier_num, .. }
        | CallControl::UlInactivityTimeout { carrier_num, .. }
        | CallControl::TrafficUlActivity { carrier_num, .. }
        | CallControl::NetworkCircuitMediaReady { carrier_num, .. } => Some(*carrier_num),
        _ => None,
    }
}

/// Brew session of an individual (circuit-mode) call message.
fn circuit_uuid(cc: &CallControl) -> Option<Uuid> {
    match cc {
        CallControl::NetworkCircuitSetupRequest { brew_uuid, .. }
        | CallControl::NetworkCircuitSetupAccept { brew_uuid }
        | CallControl::NetworkCircuitSetupReject { brew_uuid, .. }
        | CallControl::NetworkCircuitAlert { brew_uuid }
        | CallControl::NetworkCircuitConnectRequest { brew_uuid, .. }
        | CallControl::NetworkCircuitConnectConfirm { brew_uuid, .. }
        | CallControl::NetworkCircuitSimplexGranted { brew_uuid, .. }
        | CallControl::NetworkCircuitSimplexIdle { brew_uuid, .. }
        | CallControl::NetworkCircuitMediaReady { brew_uuid, .. }
        | CallControl::NetworkCircuitDtmf { brew_uuid, .. }
        | CallControl::NetworkCircuitRelease { brew_uuid, .. } => Some(*brew_uuid),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tetra_core::Sap;
    use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
    use tetra_saps::control::enums::sds_user_data::SdsUserData;
    use tetra_saps::control::sds::CmceSdsData;
    use tetra_saps::tmd::TmdCircuitDataReq;

    use super::*;

    const C0: u16 = 1521;
    const C1: u16 = 1525;

    /// Network entity stand-in: records what it receives, emits `outbox` on the next tick.
    #[derive(Default, Clone)]
    struct MockNet {
        got: Arc<Mutex<Vec<SapMsg>>>,
        outbox: Arc<Mutex<Vec<SapMsg>>>,
    }

    impl TetraEntityTrait for MockNet {
        fn entity(&self) -> TetraEntity {
            TetraEntity::Brew
        }
        fn rx_prim(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
            self.got.lock().unwrap().push(message);
        }
        fn tick_start(&mut self, queue: &mut MessageQueue, _ts: TdmaTime) {
            for m in self.outbox.lock().unwrap().drain(..) {
                queue.push_back(m);
            }
        }
    }

    struct Site {
        net: MockNet,
        switch: SiteSwitch,
        link: CellLink,
        q0: MessageQueue,
        q1: MessageQueue,
    }

    impl Site {
        fn new() -> Self {
            let net = MockNet::default();
            let (ports, mut links) = site_links(&[CellId(1)]);
            let carriers = vec![(CellId(0), vec![C0]), (CellId(1), vec![C1])];
            Self {
                switch: SiteSwitch::new(Box::new(net.clone()), &carriers, ports),
                net,
                link: links.remove(&CellId(1)).unwrap(),
                q0: MessageQueue::new(),
                q1: MessageQueue::new(),
            }
        }

        /// A cell's stack sends `msg` to the network entity.
        fn from_cell(&mut self, cell: u8, msg: SapMsgInner) {
            let m = SapMsg::new(Sap::Control, TetraEntity::Cmce, TetraEntity::Brew, msg);
            if cell == 0 {
                self.switch.rx_prim(&mut self.q0, m);
            } else {
                self.link.rx_prim(&mut self.q1, m);
            }
        }

        /// The network entity emits `msg`.
        fn from_net(&mut self, msg: SapMsgInner) {
            let m = SapMsg::new(Sap::Control, TetraEntity::Brew, TetraEntity::Cmce, msg);
            self.net.outbox.lock().unwrap().push(m);
        }

        /// One primary tick, then collect what each cell received.
        fn tick(&mut self) -> (Vec<SapMsgInner>, Vec<SapMsgInner>) {
            self.switch.tick_start(&mut self.q0, TdmaTime::default());
            self.link.tick_start(&mut self.q1, TdmaTime::default());
            let drain = |q: &mut MessageQueue| std::iter::from_fn(|| q.pop_front()).map(|m| m.msg).collect();
            (drain(&mut self.q0), drain(&mut self.q1))
        }

        fn net_got(&self) -> Vec<SapMsgInner> {
            self.net.got.lock().unwrap().drain(..).map(|m| m.msg).collect()
        }
    }

    fn affiliate(issi: u32, gssi: u32) -> SapMsgInner {
        SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: vec![gssi],
            action: BrewSubscriberAction::Affiliate,
        })
    }

    fn cc(c: CallControl) -> SapMsgInner {
        SapMsgInner::CmceCallControl(c)
    }

    fn is_start(m: &SapMsgInner) -> bool {
        matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCallStart { .. }))
    }

    #[test]
    fn network_group_call_fans_out_and_copies_voice() {
        let mut site = Site::new();
        site.from_cell(0, affiliate(100, 91));
        site.from_cell(1, affiliate(200, 91));
        site.tick();
        site.net_got();

        let uuid = Uuid::from_u128(1);
        site.from_net(cc(CallControl::NetworkCallStart {
            brew_uuid: uuid,
            source_issi: 9000,
            dest_gssi: 91,
            priority: 0,
        }));
        let (c0, c1) = site.tick();
        assert!(c0.iter().any(is_start) && c1.iter().any(is_start), "both cells have members");

        let ready = |call_id, carrier_num| {
            cc(CallControl::NetworkCallReady {
                brew_uuid: uuid,
                call_id,
                carrier_num,
                ts: 2,
                usage: 4,
            })
        };
        // Cell 1 answers first (its link is drained on the next primary tick).
        site.from_cell(1, ready(7, C1));
        site.tick();
        site.from_cell(0, ready(7, C0));
        site.tick();
        let got = site.net_got();
        let readies: Vec<_> = got
            .iter()
            .filter(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCallReady { .. })))
            .collect();
        assert_eq!(readies.len(), 1, "only the first Ready reaches the network entity");
        assert!(matches!(
            readies[0],
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady { carrier_num: C1, .. })
        ));

        // DL voice for the anchor circuit (cell 1) is copied to cell 0's circuit.
        site.net.outbox.lock().unwrap().push(SapMsg::new(
            Sap::TmdSap,
            TetraEntity::Brew,
            TetraEntity::Umac,
            SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
                carrier_num: C1,
                ts: 2,
                data: vec![1, 2, 3],
            }),
        ));
        let (c0, c1) = site.tick();
        let voice_on = |msgs: &[SapMsgInner], carrier| {
            msgs.iter().any(|m| matches!(m, SapMsgInner::TmdCircuitDataReq(r) if r.carrier_num == carrier && r.ts == 2))
        };
        assert!(voice_on(&c1, C1) && voice_on(&c0, C0));

        // Cell 1 leaving alone is swallowed; the call goes on for cell 0.
        site.from_cell(1, cc(CallControl::NetworkCallEnd { brew_uuid: uuid }));
        site.tick();
        assert!(site.net_got().is_empty());

        // Network ends the call: reaches every cell still in it.
        site.from_net(cc(CallControl::NetworkCallEnd { brew_uuid: uuid }));
        let (c0, _) = site.tick();
        assert!(c0.iter().any(|m| matches!(m, SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { .. }))));
    }

    #[test]
    fn hold_is_forwarded_only_when_every_cell_holds() {
        let mut site = Site::new();
        site.from_cell(0, affiliate(100, 91));
        site.from_cell(1, affiliate(200, 91));
        site.tick();
        site.net_got();

        let uuid = Uuid::from_u128(2);
        site.from_net(cc(CallControl::NetworkCallStart {
            brew_uuid: uuid,
            source_issi: 9000,
            dest_gssi: 91,
            priority: 0,
        }));
        site.tick();
        let hold = cc(CallControl::NetworkCallHold {
            brew_uuid: uuid,
            dest_gssi: 91,
        });
        site.from_cell(0, hold.clone());
        site.tick();
        assert!(site.net_got().is_empty(), "cell 1 has not answered yet");
        site.from_cell(1, hold);
        site.tick();
        assert_eq!(site.net_got().len(), 1);
    }

    #[test]
    fn sds_between_cells_is_delivered_directly() {
        let mut site = Site::new();
        site.from_cell(0, affiliate(100, 91));
        site.tick();
        site.net_got();

        site.from_cell(
            1,
            SapMsgInner::CmceSdsData(CmceSdsData {
                source_issi: 200,
                dest_issi: 100,
                user_defined_data: SdsUserData::Type1(0x8001),
            }),
        );
        let (c0, _) = site.tick();
        assert!(c0.iter().any(|m| matches!(m, SapMsgInner::CmceSdsData(s) if s.dest_issi == 100)));
        assert!(site.net_got().is_empty(), "local SDS must not go out to the network");
    }

    #[test]
    fn call_ids_are_unique_towards_the_network() {
        let mut site = Site::new();
        let granted = |carrier_num| {
            cc(CallControl::FloorGranted {
                call_id: 4,
                source_issi: 1,
                dest_gssi: 91,
                carrier_num,
                ts: 2,
            })
        };
        site.from_cell(0, granted(C0));
        site.from_cell(1, granted(C1));
        site.tick();
        let ids: Vec<u16> = site
            .net_got()
            .iter()
            .filter_map(|m| match m {
                SapMsgInner::CmceCallControl(CallControl::FloorGranted { call_id, .. }) => Some(*call_id),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);

        // A reply carrying cell 1's global id goes back to cell 1 with its own id.
        site.from_net(cc(CallControl::FloorReleased {
            call_id: ids[1],
            carrier_num: C1,
            ts: 2,
        }));
        let (c0, c1) = site.tick();
        assert!(c0.is_empty());
        assert!(matches!(
            c1.as_slice(),
            [SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id: 4, .. })]
        ));
    }
}
