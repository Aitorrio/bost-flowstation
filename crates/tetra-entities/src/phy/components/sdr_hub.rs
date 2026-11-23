//! Several cells on one SDR (multi-cell phase 7).
//!
//! The PHY is synchronous: a cell's stack thread calls [`RxTxDev::rxtx_timeslot`] once per
//! timeslot and the device does all its DSP inside that call. Two cells on one SDR therefore run in
//! lockstep: each cell's [`HubCellDev`] deposits its downlink slots, and once every cell of the
//! device has done so (or a deadline passes) one step of the real device runs with the union of
//! them. The uplink slots it returns are handed back to the cell that owns each carrier.
//!
//! A cell that misses the deadline transmits silence on its carrier for that timeslot, so it cannot
//! stall the others. Generic over the device so it is testable without hardware.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tetra_core::{TdmaTime, TrainingSequence};
use tetra_pdus::phy::traits::rxtx_dev::{RxBurstBits, RxSlotBits, RxTxDev, RxTxDevError, TxSlotBits};

/// How long a cell waits for the others before the device steps without them.
pub const DEFAULT_STEP_TIMEOUT: Duration = Duration::from_millis(250);

struct OwnedTx {
    carrier_num: u16,
    time: TdmaTime,
    slot: Option<Vec<u8>>,
}

#[derive(Default, Clone)]
struct OwnedBurst {
    train_type: TrainingSequence,
    bits: Vec<u8>,
    rssi_dbfs: f32,
}

impl OwnedBurst {
    fn from_burst(b: &RxBurstBits<'_>) -> Self {
        Self {
            train_type: b.train_type,
            bits: b.bits.to_vec(),
            rssi_dbfs: b.rssi_dbfs,
        }
    }
    fn as_burst(&self) -> RxBurstBits<'_> {
        RxBurstBits {
            train_type: self.train_type,
            bits: &self.bits,
            rssi_dbfs: self.rssi_dbfs,
        }
    }
}

#[derive(Default, Clone)]
struct OwnedRx {
    carrier_num: u16,
    time: TdmaTime,
    slot: OwnedBurst,
    subslot1: OwnedBurst,
    subslot2: OwnedBurst,
}

struct HubState<D> {
    dev: D,
    /// Bumped after every device step.
    generation: u64,
    /// Downlink slots deposited by each cell for the coming step.
    pending: Vec<Option<Vec<OwnedTx>>>,
    /// Uplink slots of the last step, per cell; `None` = taken.
    results: Vec<Option<Result<Vec<Option<OwnedRx>>, RxTxDevError>>>,
}

pub struct SdrHub<D> {
    state: Mutex<HubState<D>>,
    cv: Condvar,
    /// Carriers each cell owns.
    cell_carriers: Vec<Vec<u16>>,
    /// Carriers in the order the device's modulators expect its downlink slots.
    device_carriers: Vec<u16>,
    timeout: Duration,
}

impl<D: RxTxDev> SdrHub<D> {
    /// `cell_carriers[i]` are the carriers of cell `i`; `device_carriers` is the device's carrier
    /// order (the order its modulators were built in). Returns one device view per cell.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(dev: D, cell_carriers: Vec<Vec<u16>>, device_carriers: Vec<u16>, timeout: Duration) -> Vec<HubCellDev<D>> {
        let n = cell_carriers.len();
        let hub = Arc::new(Self {
            state: Mutex::new(HubState {
                dev,
                generation: 0,
                pending: (0..n).map(|_| None).collect(),
                results: (0..n).map(|_| None).collect(),
            }),
            cv: Condvar::new(),
            cell_carriers,
            device_carriers,
            timeout,
        });
        (0..n)
            .map(|idx| HubCellDev {
                hub: hub.clone(),
                idx,
                rx: Vec::new(),
            })
            .collect()
    }

    /// Run one device step with whatever the cells deposited. Caller holds the lock.
    fn step(&self, st: &mut HubState<D>) {
        let reference_time = st.pending.iter().flatten().flatten().map(|t| t.time).next();
        let any_tx = st.pending.iter().flatten().any(|v| !v.is_empty());

        // Union of the cells' slots in the device's carrier order; a missing carrier is silent.
        let mut owned: Vec<TxSlotBits<'_>> = Vec::new();
        if any_tx {
            for &carrier in &self.device_carriers {
                let found = st.pending.iter().flatten().flatten().find(|t| t.carrier_num == carrier);
                owned.push(TxSlotBits {
                    carrier_num: carrier,
                    time: found.map(|t| t.time).or(reference_time).unwrap_or_default(),
                    slot: found.and_then(|t| t.slot.as_deref()),
                });
            }
        }
        // Borrow of `st.pending` ends before the device is touched: copy the result out first.
        let rx_result: Result<Vec<Option<OwnedRx>>, RxTxDevError> = {
            let tx: &[TxSlotBits<'_>] = &owned;
            // SAFETY-free split borrow: `owned` borrows `st.pending`, `dev` is a different field.
            let dev = &mut st.dev;
            dev.rxtx_timeslot(tx).map(|v| {
                v.into_iter()
                    .map(|o| {
                        o.map(|r| OwnedRx {
                            carrier_num: r.carrier_num,
                            time: r.time,
                            slot: OwnedBurst::from_burst(&r.slot),
                            subslot1: OwnedBurst::from_burst(&r.subslot1),
                            subslot2: OwnedBurst::from_burst(&r.subslot2),
                        })
                    })
                    .collect()
            })
        };
        drop(owned);

        for (idx, carriers) in self.cell_carriers.iter().enumerate() {
            st.results[idx] = Some(match &rx_result {
                Ok(v) => Ok(v
                    .iter()
                    .filter(|o| o.as_ref().map_or(false, |r| carriers.contains(&r.carrier_num)))
                    .cloned()
                    .collect()),
                Err(e) => Err(*e),
            });
        }
        for p in st.pending.iter_mut() {
            *p = None;
        }
        st.generation += 1;
        self.cv.notify_all();
    }
}

/// One cell's view of a shared SDR; give it to that cell's `PhyBs`.
pub struct HubCellDev<D> {
    hub: Arc<SdrHub<D>>,
    idx: usize,
    rx: Vec<Option<OwnedRx>>,
}

impl<D: RxTxDev> RxTxDev for HubCellDev<D> {
    fn rxtx_timeslot(&mut self, tx_slot: &[TxSlotBits]) -> Result<Vec<Option<RxSlotBits<'_>>>, RxTxDevError> {
        let hub = self.hub.clone();
        let mut st = hub.state.lock().unwrap_or_else(|e| e.into_inner());

        // A second call before the others stepped: wait for the step that takes our first.
        while st.pending[self.idx].is_some() {
            st = hub.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }

        st.pending[self.idx] = Some(
            tx_slot
                .iter()
                .map(|t| OwnedTx {
                    carrier_num: t.carrier_num,
                    time: t.time,
                    slot: t.slot.map(|s| s.to_vec()),
                })
                .collect(),
        );
        let my_gen = st.generation;

        if st.pending.iter().all(|p| p.is_some()) {
            hub.step(&mut st);
        } else {
            let deadline = std::time::Instant::now() + hub.timeout;
            while st.generation == my_gen {
                let now = std::time::Instant::now();
                if now >= deadline {
                    // The others did not show up: step without them (silence on their carriers).
                    hub.step(&mut st);
                    break;
                }
                st = hub.cv.wait_timeout(st, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
            }
        }

        let res = st.results[self.idx].take().unwrap_or(Ok(Vec::new()));
        drop(st);
        self.rx = res?;
        Ok(self
            .rx
            .iter()
            .map(|o| {
                o.as_ref().map(|r| RxSlotBits {
                    carrier_num: r.carrier_num,
                    time: r.time,
                    slot: r.slot.as_burst(),
                    subslot1: r.subslot1.as_burst(),
                    subslot2: r.subslot2.as_burst(),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records the carriers of each step; reports one uplink slot per device carrier.
    struct FakeDev {
        carriers: Vec<u16>,
        steps: Arc<Mutex<Vec<Vec<(u16, Option<Vec<u8>>)>>>>,
    }

    impl RxTxDev for FakeDev {
        fn rxtx_timeslot(&mut self, tx: &[TxSlotBits]) -> Result<Vec<Option<RxSlotBits<'_>>>, RxTxDevError> {
            self.steps
                .lock()
                .unwrap()
                .push(tx.iter().map(|t| (t.carrier_num, t.slot.map(|s| s.to_vec()))).collect());
            Ok(self
                .carriers
                .iter()
                .map(|&c| {
                    Some(RxSlotBits {
                        carrier_num: c,
                        ..Default::default()
                    })
                })
                .collect())
        }
    }

    fn hub(timeout: Duration) -> (Vec<HubCellDev<FakeDev>>, Arc<Mutex<Vec<Vec<(u16, Option<Vec<u8>>)>>>>) {
        let steps = Arc::new(Mutex::new(Vec::new()));
        let dev = FakeDev {
            carriers: vec![1521, 1522],
            steps: steps.clone(),
        };
        (SdrHub::new(dev, vec![vec![1521], vec![1522]], vec![1521, 1522], timeout), steps)
    }

    #[test]
    fn lockstep_merges_both_cells_in_one_step_and_splits_uplink() {
        let (mut cells, steps) = hub(Duration::from_secs(5));
        let mut b = cells.pop().unwrap();
        let mut a = cells.pop().unwrap();
        let burst_b = [2u8; 4];
        let t = std::thread::spawn(move || {
            for _ in 0..3 {
                let tx = [TxSlotBits {
                    carrier_num: 1522,
                    slot: Some(&burst_b),
                    ..Default::default()
                }];
                let rx = b.rxtx_timeslot(&tx).unwrap();
                assert_eq!(rx.len(), 1);
                assert_eq!(rx[0].as_ref().unwrap().carrier_num, 1522);
            }
        });
        let burst_a = [1u8; 4];
        for _ in 0..3 {
            let tx = [TxSlotBits {
                carrier_num: 1521,
                slot: Some(&burst_a),
                ..Default::default()
            }];
            let rx = a.rxtx_timeslot(&tx).unwrap();
            assert_eq!(rx.len(), 1);
            assert_eq!(rx[0].as_ref().unwrap().carrier_num, 1521);
        }
        t.join().unwrap();
        let steps = steps.lock().unwrap();
        assert_eq!(steps.len(), 3, "one device step per lockstep tick");
        for s in steps.iter() {
            assert_eq!(s.iter().map(|(c, _)| *c).collect::<Vec<_>>(), vec![1521, 1522], "device order");
            assert_eq!(s[0].1.as_deref(), Some(&[1u8; 4][..]));
            assert_eq!(s[1].1.as_deref(), Some(&[2u8; 4][..]));
        }
    }

    #[test]
    fn absent_cell_is_silent_after_the_deadline() {
        let (mut cells, steps) = hub(Duration::from_millis(30));
        let _b = cells.pop().unwrap(); // never calls
        let mut a = cells.pop().unwrap();
        let burst = [1u8; 4];
        let tx = [TxSlotBits {
            carrier_num: 1521,
            slot: Some(&burst),
            ..Default::default()
        }];
        let rx = a.rxtx_timeslot(&tx).unwrap();
        assert_eq!(rx.len(), 1);
        let steps = steps.lock().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0][0].1.as_deref(), Some(&[1u8; 4][..]));
        assert_eq!(steps[0][1], (1522, None), "missing cell transmits silence");
    }
}

#[cfg(test)]
fn _assert_soapy_dev_send() {
    fn is_send<T: Send>() {}
    is_send::<crate::phy::components::soapy_dev::RxTxDevSoapySdr>();
}
