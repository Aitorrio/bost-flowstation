use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, bounded};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::net_telemetry::events::TelemetryEvent;

/// Queue depth per telemetry channel.
///
/// Telemetry is lossy by nature, so the queue is bounded: under an RF-driven event flood with a
/// slow consumer (dashboard / Telegram / Snom / network worker) an unbounded queue grows toward
/// OOM, and since the core thread never blocks on it the health watchdog never sees the growth.
/// Deep enough to ride out several seconds of a stalled consumer at realistic event rates.
pub const TELEMETRY_QUEUE_CAP: usize = 4096;

/// Events dropped because a queue was full, process-wide. Never reset; read by operators (and
/// the log line below) so telemetry loss is visible instead of silent.
static DROPPED_EVENTS: AtomicU64 = AtomicU64::new(0);

/// Total telemetry events dropped for lack of queue space since process start.
pub fn dropped_events() -> u64 {
    DROPPED_EVENTS.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// TelemetrySink  (cloneable, push‑only handle given to entities)
//
// crossbeam Sender is Arc‑backed; cloning is a single atomic increment.
// send() is lock‑free — it claims a slot via atomic FAA and memcpys the
// TelemetryEvent into it.  Small events require zero heap allocation.
// Larger events should use a Box to keep the TelemetryEvent size small
// and avoid heap allocation on send.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TelemetrySink {
    tx: Sender<TelemetryEvent>,
    /// Multi-cell: the cell whose stack sends through this sink (None = single-cell station).
    cell: Option<u8>,
}

impl TelemetrySink {
    /// The same stream, tagged for one cell of a multi-cell station: registrations are followed
    /// by `MsCell`, and an additional cell's call events are dropped (call ids are per cell and
    /// would collide with the primary's; calls between cells show on the primary anyway).
    pub fn for_cell(&self, cell: u8) -> Self {
        Self {
            tx: self.tx.clone(),
            cell: Some(cell),
        }
    }

    /// Push a telemetry event. Lock‑free and never blocks — the core loop must not be paced by a
    /// slow telemetry consumer. Fire‑and‑forget: silently drops if the receiver is gone, and
    /// drops the newest event (counted) if the queue is full.
    #[inline]
    pub fn send(&self, event: TelemetryEvent) {
        let Some(cell) = self.cell else {
            return self.push(event);
        };
        match event {
            TelemetryEvent::MsRegistration { issi } => {
                self.push(event);
                self.push(TelemetryEvent::MsCell { issi, cell });
            }
            TelemetryEvent::GroupCallStarted { .. }
            | TelemetryEvent::GroupCallEnded { .. }
            | TelemetryEvent::CallSpeakerChanged { .. }
            | TelemetryEvent::IndividualCallStarted { .. }
            | TelemetryEvent::IndividualCallEnded { .. }
                if cell != 0 => {}
            _ => self.push(event),
        }
    }

    #[inline]
    fn push(&self, event: TelemetryEvent) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(event) {
            let n = DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed) + 1;
            // Loud on the first loss, then every 1000th — enough for an operator to see
            // "telemetry is lossy right now" without turning a flood into a log flood.
            if n == 1 || n % 1000 == 0 {
                tracing::warn!(
                    "Telemetry queue full ({} slots) — consumer too slow, {} events dropped so far",
                    TELEMETRY_QUEUE_CAP,
                    n
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TelemetrySource  (receive side, owned by the Telemetry component)
// ---------------------------------------------------------------------------

pub struct TelemetrySource {
    rx: Receiver<TelemetryEvent>,
}

/// Result of a receive-with-timeout operation.
pub enum RecvEvent {
    /// A telemetry event was received.
    Event(TelemetryEvent),
    /// Timed out waiting — channel is still open.
    Timeout,
    /// All sinks were dropped — channel is closed.
    Closed,
}

impl TelemetrySource {
    /// Blocking receive.  Returns `None` when all sinks have been dropped.
    pub fn recv(&self) -> Option<TelemetryEvent> {
        self.rx.recv().ok()
    }

    /// Blocking receive with timeout, distinguishing timeout from channel close.
    pub fn recv_timeout(&self, timeout: Duration) -> RecvEvent {
        match self.rx.recv_timeout(timeout) {
            Ok(event) => RecvEvent::Event(event),
            Err(RecvTimeoutError::Timeout) => RecvEvent::Timeout,
            Err(RecvTimeoutError::Disconnected) => RecvEvent::Closed,
        }
    }

    /// Non-blocking try_recv.
    pub fn try_recv(&self) -> Option<TelemetryEvent> {
        self.rx.try_recv().ok()
    }
}

// ---------------------------------------------------------------------------
// Channel constructor
// ---------------------------------------------------------------------------

/// Create a linked (sink, source) pair. Bounded — see [`TELEMETRY_QUEUE_CAP`].
pub fn telemetry_channel() -> (TelemetrySink, TelemetrySource) {
    let (tx, rx) = bounded(TELEMETRY_QUEUE_CAP);
    (TelemetrySink { tx, cell: None }, TelemetrySource { rx })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_two_events() {
        let (sink, source) = telemetry_channel();

        sink.send(TelemetryEvent::MsRegistration { issi: 12345 });

        // Clone the sink (simulating a second entity) and send an Attach event
        let sink2 = sink.clone();
        sink2.send(TelemetryEvent::MsGroupAttach {
            issi: 12345,
            gssis: vec![1, 2, 3],
        });

        // Receive and verify
        let a = source.try_recv().expect("should receive Registration");
        assert!(matches!(a, TelemetryEvent::MsRegistration { issi: 12345 }));

        let b = source.try_recv().expect("should receive Attach");
        if let TelemetryEvent::MsGroupAttach { issi, gssis } = &b {
            assert_eq!(*issi, 12345);
            assert_eq!(*gssis, vec![1, 2, 3]);
        } else {
            panic!("expected Attach variant");
        }

        // No more items
        assert!(source.try_recv().is_none());
    }

    #[test]
    fn full_queue_drops_newest_and_counts() {
        let (sink, source) = telemetry_channel();
        let before = dropped_events();

        // Fill to capacity — nothing dropped yet.
        for issi in 0..TELEMETRY_QUEUE_CAP as u32 {
            sink.send(TelemetryEvent::MsRegistration { issi });
        }
        assert_eq!(dropped_events(), before, "queue should absorb exactly its capacity");

        // Overflow: send() must not block and the extra events are dropped + counted.
        for issi in 0..10u32 {
            sink.send(TelemetryEvent::MsRegistration { issi });
        }
        assert_eq!(dropped_events(), before + 10);

        // The oldest events survived (drop-newest policy).
        assert!(matches!(source.try_recv(), Some(TelemetryEvent::MsRegistration { issi: 0 })));
    }

    #[test]
    fn cell_tagged_sink_reports_cell_and_drops_extra_cell_call_events() {
        let (sink, source) = telemetry_channel();
        let cell1 = sink.for_cell(1);
        let cell0 = sink.for_cell(0);

        cell1.send(TelemetryEvent::MsRegistration { issi: 7 });
        assert!(matches!(source.try_recv(), Some(TelemetryEvent::MsRegistration { issi: 7 })));
        assert!(matches!(source.try_recv(), Some(TelemetryEvent::MsCell { issi: 7, cell: 1 })));

        cell1.send(TelemetryEvent::GroupCallEnded { call_id: 4, gssi: 91 });
        assert!(source.try_recv().is_none(), "extra cell call ids would collide");
        cell0.send(TelemetryEvent::GroupCallEnded { call_id: 4, gssi: 91 });
        assert!(matches!(source.try_recv(), Some(TelemetryEvent::GroupCallEnded { .. })));

        sink.send(TelemetryEvent::MsRegistration { issi: 8 });
        assert!(matches!(source.try_recv(), Some(TelemetryEvent::MsRegistration { issi: 8 })));
        assert!(source.try_recv().is_none(), "untagged (single-cell) sink adds nothing");
    }
}
