use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tetra_config::bluestation::SharedConfig;
use tetra_core::{Sap, TdmaTime, tetra_entities::TetraEntity};
use tetra_saps::SapMsg;

use crate::TetraEntityTrait;

/// Env escape hatch: `FLOWSTATION_PANIC_FATAL=1` restores the old fail-fast behaviour (the panic
/// is re-raised and takes the process down) so a panic can't hide during development.
static PANIC_FATAL: OnceLock<bool> = OnceLock::new();

fn panic_is_fatal() -> bool {
    *PANIC_FATAL.get_or_init(|| {
        std::env::var_os("FLOWSTATION_PANIC_FATAL").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

fn panic_reason(payload: &(dyn Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "<non-string panic payload>"
    }
}

/// Blast-radius containment for one entity callback.
///
/// Entities run directly on the core loop with no unwind boundary, so any panic on the
/// processing path used to exit the whole process (code 101) — one malformed PDU killed the
/// cell. Catching here degrades that to a dropped message. The entity's internal state may be
/// inconsistent afterwards, but a degraded entity beats a dead base station.
///
/// Bugs stay visible: the default panic hook has already printed the message + backtrace by the
/// time we get control, we log loudly with the entity/SAP identity, and every catch is counted
/// in the health registry so a cell quietly swallowing PDUs still surfaces as unhealthy.
///
/// No-panic path: `catch_unwind` only installs a landing pad, so normal ticks are unchanged and
/// allocation-free.
#[inline]
fn guard_entity(entity: TetraEntity, what: &str, sap: Option<Sap>, f: impl FnOnce()) {
    // AssertUnwindSafe: the entity and the message queue are &mut and can be left mid-update by
    // the unwind — that is exactly the trade-off documented above, not an oversight.
    let Err(payload) = catch_unwind(AssertUnwindSafe(f)) else {
        return;
    };

    if panic_is_fatal() {
        std::panic::resume_unwind(payload);
    }

    crate::health::registry().note_caught_panic();
    tracing::error!(
        "PANIC caught in entity {:?} during {} (sap {:?}): {} — message dropped, entity state may be inconsistent",
        entity,
        what,
        sap,
        panic_reason(&*payload)
    );
}

#[derive(Default)]
pub enum MessagePrio {
    Immediate,
    #[default]
    Normal,
}

pub struct MessageQueue {
    messages: VecDeque<SapMsg>,
}

impl MessageQueue {
    pub fn new() -> Self {
        Self { messages: VecDeque::new() }
    }

    pub fn push_back(&mut self, message: SapMsg) {
        self.messages.push_back(message);
    }

    pub fn push_prio(&mut self, message: SapMsg, prio: MessagePrio) {
        match prio {
            MessagePrio::Immediate => {
                // Insert at the front for immediate processing
                self.messages.push_front(message);
            }
            MessagePrio::Normal => {
                // Insert at the back for normal processing
                self.messages.push_back(message);
            }
        }
    }

    pub fn pop_front(&mut self) -> Option<SapMsg> {
        self.messages.pop_front()
    }
}

pub struct MessageRouter {
    /// While currently unused by the MessageRouter, this may change in the future
    /// As such, we provide the MessageRouter with a copy of the SharedConfig.
    /// `None` only in unit tests, which need a router without a full stack config.
    _config: Option<SharedConfig>,
    entities: HashMap<TetraEntity, Box<dyn TetraEntityTrait>>,
    msg_queue: MessageQueue,

    /// The current TDMA time, if applicable.
    /// For Bs mode, this is always available
    /// For Ms/Mon mode, it is recovered from a received SYNC frame and communicated in a different way
    ts: TdmaTime,
}

impl MessageRouter {
    pub fn new(config: SharedConfig) -> Self {
        Self {
            entities: HashMap::new(),
            msg_queue: MessageQueue { messages: VecDeque::new() },
            _config: Some(config),
            ts: TdmaTime::default(),
        }
    }

    /// Router with no config, for unit tests that only exercise dispatch.
    #[cfg(test)]
    fn new_bare() -> Self {
        Self {
            entities: HashMap::new(),
            msg_queue: MessageQueue { messages: VecDeque::new() },
            _config: None,
            ts: TdmaTime::default(),
        }
    }

    /// For BS mode, sets global TDMA time
    /// Incremented each tick and passed to entities in tick() function
    pub fn set_dl_time(&mut self, ts: TdmaTime) {
        self.ts = ts;
    }

    pub fn register_entity(&mut self, entity: Box<dyn TetraEntityTrait>) {
        let comp_type = entity.entity();
        tracing::debug!("register_entity {:?}", comp_type);
        self.entities.insert(comp_type, entity);
    }

    /// Returns a mut ref to a component of the requested type
    pub fn get_entity(&mut self, comp: TetraEntity) -> Option<&mut dyn TetraEntityTrait> {
        self.entities.get_mut(&comp).map(|entity| entity.as_mut())
    }

    pub fn submit_message(&mut self, message: SapMsg) {
        tracing::debug!(
            "submit_message {:?}: {:?} -> {:?}",
            message.get_sap(),
            message.get_source(),
            message.get_dest()
        );
        self.msg_queue.push_back(message);
    }

    pub fn deliver_message(&mut self) {
        let message = self.msg_queue.pop_front();
        if let Some(message) = message {
            tracing::debug!(
                "deliver_message: got {:?}: {:?} -> {:?}",
                message.get_sap(),
                message.get_source(),
                message.get_dest()
            );

            // Determine the destination entity
            let dest = message.get_dest();

            // Check if the destination entity registered and deliver if found
            if let Some(entity) = self.entities.get_mut(dest) {
                let dest_id = *dest;
                let sap = *message.get_sap();
                let queue = &mut self.msg_queue;
                guard_entity(dest_id, "rx_prim", Some(sap), move || entity.rx_prim(queue, message));
            } else {
                tracing::warn!(
                    "deliver_message: entity {:?} not found for {:?}: {:?} -> {:?}",
                    dest,
                    message.get_sap(),
                    message.get_source(),
                    message.get_dest()
                );
            }
        }
    }

    pub fn deliver_all_messages(&mut self) {
        while !self.msg_queue.messages.is_empty() {
            self.deliver_message();
        }
    }

    pub fn get_msgqueue_len(&self) -> usize {
        self.msg_queue.messages.len()
    }

    pub fn tick_start(&mut self) {
        // tracing::info!("--- tick dl {} ul {} txdl {} ----------------------------",
        //     self.ts, self.ts.add_timeslots(-2), self.ts.add_timeslots(MACSCHED_TX_AHEAD as i32));
        tracing::info!("--- tick dl {} ----------------------------", self.ts);

        // Call tick on all entities
        let ts = self.ts;
        for entity in self.entities.values_mut() {
            let entity_id = entity.entity();
            let queue = &mut self.msg_queue;
            guard_entity(entity_id, "tick_start", None, move || entity.tick_start(queue, ts));
        }
    }

    /// Executes all end-of-tick functions:
    /// - LLC sends down all outstanding BL-ACKs
    /// - UMAC finalizes any resources for ts and sends down to LMAC
    ///
    pub fn tick_end(&mut self) {
        tracing::debug!("############################ end-of-tick ############################");

        let ts = self.ts;

        // Llc should send down outstanding BL-ACKs
        let target = TetraEntity::Llc;
        if let Some(entity) = self.entities.get_mut(&target) {
            tracing::trace!("tick_end for entity {:?}", target);
            let queue = &mut self.msg_queue;
            guard_entity(target, "tick_end", None, move || {
                entity.tick_end(queue, ts);
            });
        }
        self.deliver_all_messages();

        // Umac should finalize any resources and send down to Lmac
        let target = TetraEntity::Umac;
        if let Some(entity) = self.entities.get_mut(&target) {
            tracing::trace!("tick_end for entity {:?}", target);
            let queue = &mut self.msg_queue;
            guard_entity(target, "tick_end", None, move || {
                entity.tick_end(queue, ts);
            });
        }
        self.deliver_all_messages();

        // Then call tick_end on all other entities
        for entity in self.entities.values_mut() {
            let entity_id = entity.entity();
            if entity_id == TetraEntity::Llc || entity_id == TetraEntity::Umac {
                continue;
            }
            let queue = &mut self.msg_queue;
            guard_entity(entity_id, "tick_end", None, move || {
                entity.tick_end(queue, ts);
            });
        }
        self.deliver_all_messages();

        // Increment the TDMA time if set
        self.ts = self.ts.add_timeslots(1);
    }

    /// Runs the full stack either forever or for a specified number of ticks.
    /// If `running` is provided, the loop will exit when the flag is set to false
    /// (e.g. by a Ctrl+C signal handler), allowing entities to be dropped cleanly.
    pub fn run_stack(&mut self, num_ticks: Option<usize>, running: Option<Arc<AtomicBool>>) {
        let mut ticks: usize = 0;

        loop {
            // Check if we've been asked to stop (e.g. Ctrl+C)
            if let Some(ref flag) = running {
                if !flag.load(Ordering::Relaxed) {
                    eprintln!("\n[INFO] Shutting down gracefully...");
                    break;
                }
            }

            // Health watchdog: stamp that the core loop is alive this tick (lock-free atomic).
            crate::health::registry().note_tick();

            // Send tick_start event
            self.tick_start();

            // Deliver messages until queue empty
            while self.get_msgqueue_len() > 0 {
                self.deliver_all_messages();
            }

            // Send tick_end event and process final messages
            self.tick_end();

            // Check if we should stop
            ticks += 1;
            if let Some(num_ticks) = num_ticks {
                if ticks >= num_ticks {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use tetra_saps::sapmsg::SapMsgInner;
    use tetra_saps::tnmm::TnmmTestDemand;

    use super::*;

    /// Entity that panics on every message (and optionally on tick_start), like a decoder
    /// tripping over a malformed PDU.
    struct FlakyEntity {
        id: TetraEntity,
        seen: Arc<AtomicUsize>,
        panic_on_rx: bool,
        panic_on_tick: bool,
    }

    impl TetraEntityTrait for FlakyEntity {
        fn entity(&self) -> TetraEntity {
            self.id
        }

        fn rx_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
            self.seen.fetch_add(1, Ordering::SeqCst);
            if self.panic_on_rx {
                panic!("synthetic malformed PDU");
            }
        }

        fn tick_start(&mut self, _queue: &mut MessageQueue, _ts: TdmaTime) {
            if self.panic_on_tick {
                panic!("synthetic tick_start panic");
            }
        }
    }

    fn test_msg(dest: TetraEntity) -> SapMsg {
        SapMsg::new(
            Sap::TnmmSap,
            TetraEntity::User,
            dest,
            SapMsgInner::TnmmTestDemand(TnmmTestDemand { issi: 1 }),
        )
    }

    /// A panicking entity must degrade to a dropped message, not to a dead cell: the loop keeps
    /// running, the healthy entity still gets its message, and the catch is counted.
    #[test]
    fn panicking_entity_does_not_kill_the_loop() {
        let bad_seen = Arc::new(AtomicUsize::new(0));
        let good_seen = Arc::new(AtomicUsize::new(0));

        let mut router = MessageRouter::new_bare();
        router.register_entity(Box::new(FlakyEntity {
            id: TetraEntity::Mm,
            seen: bad_seen.clone(),
            panic_on_rx: true,
            panic_on_tick: true,
        }));
        router.register_entity(Box::new(FlakyEntity {
            id: TetraEntity::Cmce,
            seen: good_seen.clone(),
            panic_on_rx: false,
            panic_on_tick: false,
        }));

        // Counter is process-global; compare deltas.
        let before = crate::health::registry().caught_panics();

        // Two full ticks: tick_start panics once per tick, rx_prim once per delivered message.
        for _ in 0..2 {
            router.tick_start();
            router.submit_message(test_msg(TetraEntity::Mm));
            router.submit_message(test_msg(TetraEntity::Cmce));
            router.deliver_all_messages();
            router.tick_end();
        }

        // The loop survived and the healthy entity was served both times.
        assert_eq!(good_seen.load(Ordering::SeqCst), 2);
        assert_eq!(bad_seen.load(Ordering::SeqCst), 2);

        // 2 × tick_start + 2 × rx_prim caught panics.
        assert_eq!(crate::health::registry().caught_panics(), before + 4);
    }
}
