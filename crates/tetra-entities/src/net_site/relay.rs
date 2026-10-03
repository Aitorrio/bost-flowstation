use std::collections::HashMap;

use tetra_config::bluestation::SharedConfig;
use tetra_core::{CellId, TdmaTime, tetra_entities::TetraEntity};
use tetra_saps::{SapMsg, SapMsgInner, control::call_control::CallControl};
use uuid::Uuid;

use super::SharedDirectory;
use super::switch::{LinkMsg, RelayPorts, carrier_map, circuit_uuid};
use crate::{MessageQueue, TetraEntityTrait};

/// Wraps the Asterisk SIP/RTP entity in the primary router so radios on every cell can use it.
///
/// Asterisk speaks the same circuit-call signalling as Brew (keyed by session UUID) plus traffic
/// voice keyed by carrier/timeslot, so routing is: voice by carrier (unique per cell), a call from
/// SIP to a radio by where that radio is registered (the site switch's directory), everything
/// else by the session's cell. Additional cells reach it through their `Asterisk`-slot
/// [`super::CellLink`]; its answers go back through their Brew-slot link, and voice for them is
/// played out on their own timing like any voice copied between cells.
pub struct SiteRelay {
    inner: Box<dyn TetraEntityTrait>,
    ports: RelayPorts,
    directory: SharedDirectory,
    carrier_cell: HashMap<u16, CellId>,
    /// Asterisk session → cell of the radio in that call.
    calls: HashMap<Uuid, CellId>,
}

impl SiteRelay {
    pub fn new(
        inner: Box<dyn TetraEntityTrait>,
        primary: &SharedConfig,
        extra: &[(CellId, SharedConfig)],
        ports: RelayPorts,
        directory: SharedDirectory,
    ) -> Self {
        Self {
            inner,
            carrier_cell: carrier_map(primary, extra),
            ports,
            directory,
            calls: HashMap::new(),
        }
    }

    fn deliver(&self, queue: &mut MessageQueue, cell: CellId, message: SapMsg) {
        if cell.is_primary() {
            queue.push_back(message);
            return;
        }
        let Some(tx) = self.ports.to_cells.get(&cell) else {
            tracing::warn!("SiteRelay: no link to {cell}, dropping {:?}", message.msg);
            return;
        };
        let link_msg = match message.msg {
            // Asterisk paces voice on the primary's clock; the other cell re-times it.
            SapMsgInner::TmdCircuitDataReq(req) => LinkMsg::Voice {
                carrier_num: req.carrier_num,
                ts: req.ts,
                data: req.data,
            },
            _ => LinkMsg::Msg(message),
        };
        let _ = tx.send(link_msg);
    }

    fn with_inner(&mut self, queue: &mut MessageQueue, f: impl FnOnce(&mut dyn TetraEntityTrait, &mut MessageQueue)) {
        let mut out = MessageQueue::new();
        f(self.inner.as_mut(), &mut out);
        while let Some(message) = out.pop_front() {
            self.route_from_asterisk(queue, message);
        }
    }

    /// A cell (primary included) sent `message` to Asterisk.
    fn from_cell(&mut self, queue: &mut MessageQueue, cell: CellId, message: SapMsg) {
        if let SapMsgInner::CmceCallControl(cc) = &message.msg
            && let Some(uuid) = circuit_uuid(cc)
        {
            self.calls.insert(uuid, cell);
        }
        self.with_inner(queue, |inner, out| inner.rx_prim(out, message));
    }

    fn route_from_asterisk(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        let cell = match &message.msg {
            SapMsgInner::TmdCircuitDataReq(req) => self.carrier_cell.get(&req.carrier_num).copied().unwrap_or(CellId::PRIMARY),
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => {
                let cell = self
                    .directory
                    .read()
                    .expect("site directory")
                    .location(call.destination)
                    .unwrap_or(CellId::PRIMARY);
                self.calls.insert(*brew_uuid, cell);
                cell
            }
            SapMsgInner::CmceCallControl(cc) => match circuit_uuid(cc) {
                Some(uuid) if matches!(cc, CallControl::NetworkCircuitRelease { .. }) => {
                    self.calls.remove(&uuid).unwrap_or(CellId::PRIMARY)
                }
                Some(uuid) => self.calls.get(&uuid).copied().unwrap_or(CellId::PRIMARY),
                None => CellId::PRIMARY,
            },
            _ => CellId::PRIMARY,
        };
        self.deliver(queue, cell, message);
    }
}

impl TetraEntityTrait for SiteRelay {
    fn entity(&self) -> TetraEntity {
        self.inner.entity()
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        self.from_cell(queue, CellId::PRIMARY, message);
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        let incoming: Vec<(CellId, SapMsg)> = self.ports.from_cells.try_iter().collect();
        for (cell, message) in incoming {
            self.from_cell(queue, cell, message);
        }
        self.with_inner(queue, |inner, out| inner.tick_start(out, ts));
    }

    fn tick_end(&mut self, queue: &mut MessageQueue, ts: TdmaTime) -> bool {
        let mut result = false;
        self.with_inner(queue, |inner, out| result = inner.tick_end(out, ts));
        result
    }
}
