//! SDR RX gain calibration — pure core.
//!
//! Boards built around the Semtech SX1255/SX1257 I/Q front end (SXceiver, µCell) and
//! other SDRs run *without* a power amplifier on many amateur TETRA sites. On those the
//! single most impactful setup step is getting the RX gain distribution right: too little
//! LNA/PGA and the ADC is under-driven (quantisation noise dominates, weak radios are
//! lost); too much and the front end overloads or the LO/DC self-mixing ("leak") swamps
//! the wanted signal. Operators currently guess these values.
//!
//! This module is the device-independent brain of the "Calibrate gains" feature: it plans
//! the LNA×PGA sweep, turns a block of raw RX IQ into the two numbers that matter
//! (wideband power and DC/LO leak), and scores the collected points into a single
//! recommendation. It deliberately has **no** SoapySDR or threading dependencies so it can
//! be unit-tested in isolation; the live glue (setting gains on the device, running the
//! sweep on the PHY thread, shipping the result to the dashboard) lives in
//! [`super::soapy_dev`] and the dashboard layer, driven through [`CalControl`].

use super::dsp_types::ComplexSample;

// ── Measurement primitives (pure DSP over one RX block) ──────────────────────

/// Wideband RX power of an IQ block, in dBFS, using the same convention as the
/// demodulator's per-burst RSSI (`20·log10(rms_magnitude)`). Full scale (|x| = 1) is
/// 0 dBFS; an empty or all-zero block is `-inf`.
pub fn block_power_dbfs(block: &[ComplexSample]) -> f32 {
    if block.is_empty() {
        return f32::NEG_INFINITY;
    }
    let sum_sq: f64 = block.iter().map(|s| (s.re as f64) * (s.re as f64) + (s.im as f64) * (s.im as f64)).sum();
    let mean_sq = sum_sq / block.len() as f64;
    if mean_sq > 0.0 {
        (10.0 * mean_sq.log10()) as f32
    } else {
        f32::NEG_INFINITY
    }
}

/// DC / LO self-leak of an IQ block, as the ratio of the DC component's power to the
/// block's total power, in dB (always ≤ 0; closer to 0 = worse leak). A clean front end
/// with no self-mixing has its energy spread across the band and a DC term near zero, so
/// this is strongly negative; an overloaded front end or bad LO leakage pushes it up
/// toward 0 dB. `-inf` for an empty/zero block.
pub fn block_dc_leak_db(block: &[ComplexSample]) -> f32 {
    if block.is_empty() {
        return f32::NEG_INFINITY;
    }
    let (mut sum_re, mut sum_im, mut sum_sq) = (0.0f64, 0.0f64, 0.0f64);
    for s in block {
        sum_re += s.re as f64;
        sum_im += s.im as f64;
        sum_sq += (s.re as f64) * (s.re as f64) + (s.im as f64) * (s.im as f64);
    }
    let n = block.len() as f64;
    let dc_power = (sum_re / n).powi(2) + (sum_im / n).powi(2);
    let total_power = sum_sq / n;
    if total_power > 0.0 && dc_power > 0.0 {
        (10.0 * (dc_power / total_power).log10()) as f32
    } else {
        f32::NEG_INFINITY
    }
}

// ── Sweep plan ───────────────────────────────────────────────────────────────

/// One LNA/PGA operating point, in dB. These map onto the SoapySDR gain elements the
/// SXceiver/µCell expose (`"lna"`, `"pga"`); for a Lime a third `"tia"` stage exists but
/// calibration sweeps only the two that dominate sensitivity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainPoint {
    pub lna: f64,
    pub pga: f64,
}

/// Inclusive min/max/step for one gain axis, in dB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisRange {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl AxisRange {
    /// Expand to the concrete dB values, inclusive of `max` (within a small epsilon) and
    /// always yielding at least `min`. A non-positive step collapses to a single point.
    pub fn values(&self) -> Vec<f64> {
        let mut out = Vec::new();
        if !self.step.is_finite() || self.step <= 0.0 || self.max < self.min {
            out.push(self.min);
            return out;
        }
        let mut v = self.min;
        // round-trip through an integer step count to avoid float drift accumulating.
        let steps = ((self.max - self.min) / self.step).floor() as i64;
        for i in 0..=steps {
            v = self.min + (i as f64) * self.step;
            out.push(v);
        }
        if (self.max - v).abs() > self.step * 1e-6 && v < self.max {
            out.push(self.max);
        }
        out
    }
}

/// What the sweep optimises for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalMode {
    /// No reference signal needed. Picks the most sensitive gain that keeps the ADC out of
    /// clipping and the LO/DC leak in check. The right choice for first-time bench setup of
    /// a PA-less board.
    NoiseFloor,
    /// Scores against a live uplink burst the cell is already receiving (lowest EVM, best
    /// RSSI). Use when a radio is keyed on the cell and you want the gains tuned to real
    /// traffic rather than just the noise floor.
    Uplink,
}

/// A calibration request as it comes from the dashboard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalRequest {
    pub mode: CalMode,
    pub lna: AxisRange,
    pub pga: AxisRange,
    /// How many RX blocks to average at each point before moving on. More = steadier
    /// numbers but a longer sweep (and longer RX disturbance).
    pub dwell_blocks: u32,
    /// Scoring thresholds (dBFS / dB). Defaults suit the SX1255 family.
    pub thresholds: Thresholds,
}

/// Tunable scoring thresholds, all defaulted so the dashboard can send none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// RX power at or above this (dBFS) is treated as heading into ADC clipping and is
    /// penalised hard. Default −6 dBFS (≈ half scale) leaves headroom for bursts.
    pub clip_guard_dbfs: f32,
    /// A floor below this (dBFS) means the ADC is under-driven; mildly penalised so the
    /// sweep prefers lifting the signal into a usable window. Default −45 dBFS.
    pub floor_target_dbfs: f32,
    /// DC/LO leak at or above this (dB, ratio to total) is penalised. Default −20 dB.
    pub leak_target_db: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { clip_guard_dbfs: -6.0, floor_target_dbfs: -45.0, leak_target_db: -20.0 }
    }
}

impl CalRequest {
    /// A sensible default sweep for the SX1255 family: LNA 0–48 dB in 6 dB steps, PGA
    /// 0–30 dB in 6 dB steps, averaging 8 blocks per point.
    pub fn sx1255_default(mode: CalMode) -> Self {
        Self {
            mode,
            lna: AxisRange { min: 0.0, max: 48.0, step: 6.0 },
            pga: AxisRange { min: 0.0, max: 30.0, step: 6.0 },
            dwell_blocks: 8,
            thresholds: Thresholds::default(),
        }
    }

    /// The concrete list of points to visit, PGA varying fastest within each LNA step.
    pub fn plan(&self) -> Vec<GainPoint> {
        let mut pts = Vec::new();
        for lna in self.lna.values() {
            for pga in self.pga.values() {
                pts.push(GainPoint { lna, pga });
            }
        }
        pts
    }
}

// ── Measurements + scoring ───────────────────────────────────────────────────

/// Averaged measurement collected at one gain point during a sweep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalMeasurement {
    pub point: GainPoint,
    /// Mean wideband RX power over the dwell, dBFS.
    pub noise_dbfs: f32,
    /// Mean DC/LO leak over the dwell, dB (ratio to total power; ≤ 0, higher = worse).
    pub leak_db: f32,
    /// Uplink-mode only: peak RX power over the dwell, dBFS (≈ power with the burst present).
    pub uplink_rssi_dbfs: Option<f32>,
    /// Uplink-mode only: burst-vs-floor SNR proxy over the dwell, dB (`peak − floor`). This is
    /// the figure Uplink scoring maximises — demod-free, derived from the spread of block
    /// power while a radio is keyed on the cell.
    pub uplink_snr_db: Option<f32>,
    /// Number of RX blocks actually averaged.
    pub blocks: u32,
}

/// The single recommended operating point plus the figures behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct Recommendation {
    pub lna: f64,
    pub pga: f64,
    pub noise_dbfs: f32,
    pub leak_db: f32,
    pub uplink_rssi_dbfs: Option<f32>,
    pub uplink_snr_db: Option<f32>,
    /// Human-readable one-liner naming what drove the choice (for the dashboard).
    pub reason: String,
}

fn relu(x: f32) -> f32 {
    if x > 0.0 { x } else { 0.0 }
}

/// Score one measurement — higher is better. Both modes penalise ADC clipping and LO/DC
/// leak identically; they differ only in the reward term.
fn score(m: &CalMeasurement, req: &CalRequest) -> f32 {
    let th = &req.thresholds;
    let clip_pen = 2.0 * relu(m.noise_dbfs - th.clip_guard_dbfs);
    let leak_pen = 1.5 * relu(m.leak_db - th.leak_target_db);
    let underdrive_pen = 0.2 * relu(th.floor_target_dbfs - m.noise_dbfs);
    let reward = match req.mode {
        // Favour sensitivity (more LNA+PGA) when there is no reference signal.
        CalMode::NoiseFloor => 0.1 * (m.point.lna + m.point.pga) as f32,
        // Favour the best copy of the live uplink: highest burst-vs-floor SNR, nudged by
        // absolute strength. Points where no burst lifted the power above the floor get no
        // reward (so the sweep won't pick a dead point).
        CalMode::Uplink => match (m.uplink_snr_db, m.uplink_rssi_dbfs) {
            (Some(snr), Some(rssi)) if snr > 0.0 => snr + 0.05 * (rssi + 60.0),
            _ => f32::NEG_INFINITY,
        },
    };
    reward - clip_pen - leak_pen - underdrive_pen
}

/// Pick the best point from a completed sweep. Returns `None` if there is nothing to score
/// (empty sweep, or Uplink mode with no burst detected at any point).
pub fn choose(measurements: &[CalMeasurement], req: &CalRequest) -> Option<Recommendation> {
    let best = measurements
        .iter()
        .map(|m| (m, score(m, req)))
        .filter(|(_, s)| s.is_finite())
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;
    let (m, _) = best;
    let th = &req.thresholds;
    let reason = if m.noise_dbfs >= th.clip_guard_dbfs {
        "limited by ADC headroom (near clipping)".to_string()
    } else if m.leak_db >= th.leak_target_db {
        "limited by LO/DC leak".to_string()
    } else {
        match req.mode {
            CalMode::NoiseFloor => "best sensitivity within headroom + leak limits".to_string(),
            CalMode::Uplink => "best uplink SNR within headroom + leak limits".to_string(),
        }
    };
    Some(Recommendation {
        lna: m.point.lna,
        pga: m.point.pga,
        noise_dbfs: m.noise_dbfs,
        leak_db: m.leak_db,
        uplink_rssi_dbfs: m.uplink_rssi_dbfs,
        uplink_snr_db: m.uplink_snr_db,
        reason,
    })
}

// ── Device abstraction (keeps the engine SoapySDR-free + testable) ───────────

/// The subset of SDR operations the calibration engine performs on the PHY thread.
/// Implemented by [`super::soapyio::SoapyIo`]; mocked in tests.
pub trait PhyDevice {
    fn set_rx_gain(&mut self, element: &str, db: f64) -> Result<(), String>;
    fn set_tx_gain(&mut self, element: &str, db: f64) -> Result<(), String>;
    fn read_rx_gains_db(&self) -> Vec<(String, f64)>;
    fn read_sx_register(&self, name: &str, addr: u32) -> Result<u32, String>;
    fn write_sx_register(&mut self, name: &str, addr: u32, value: u32) -> Result<(), String>;
    /// Register-interface names the device exposes (SoapySDR `listRegisterInterfaces`). The
    /// first/empty one is the default; the dashboard register editor offers these.
    fn list_sx_register_ifaces(&self) -> Vec<String>;
}

/// A one-shot action the dashboard asks the PHY thread to perform. All device mutations the
/// dashboard can request funnel through here so exactly one place (the PHY tick) ever touches
/// the SoapySDR handle — honouring the FH-BUG-023 rule that off-thread Soapy calls stall TX.
#[derive(Debug, Clone, PartialEq)]
pub enum PhyAction {
    Calibrate(CalRequest),
    ApplyRxGains(Vec<(String, f64)>),
    ApplyTxGains(Vec<(String, f64)>),
    ReadRegister { name: String, addr: u32 },
    WriteRegister { name: String, addr: u32, value: u32 },
}

/// Shared state between the dashboard (producer of actions / reader of results) and the PHY
/// thread (consumer). Guarded by a mutex; the PHY side only ever `try_lock`s so a busy
/// dashboard can never stall the radio.
#[derive(Debug, Default)]
pub struct CalShared {
    /// Pending actions, oldest first.
    pub queue: std::collections::VecDeque<PhyAction>,
    /// A sweep is in progress.
    pub running: bool,
    /// (points done, points total) for the running sweep.
    pub progress: (u32, u32),
    /// Gain point currently being measured.
    pub current: Option<GainPoint>,
    /// Result of the last completed sweep.
    pub result: Option<Recommendation>,
    /// Last error (sweep produced no usable point, a device call failed, …).
    pub error: Option<String>,
    /// (addr, value) from the last successful register read.
    pub last_register: Option<(u32, u32)>,
    /// Short human-readable status of the last completed one-shot action.
    pub last_action: Option<String>,
    /// Register-interface names the device exposes, filled once at startup.
    pub register_ifaces: Vec<String>,
}

/// Handle shared by the dashboard and the PHY thread.
pub type CalControl = std::sync::Arc<std::sync::Mutex<CalShared>>;

/// Create a fresh, empty control handle.
pub fn new_control() -> CalControl {
    std::sync::Arc::new(std::sync::Mutex::new(CalShared::default()))
}

// ── Process-global control (dashboard ↔ primary SDR) ─────────────────────────
//
// There is one calibration target per process: the primary cell's SDR. Rather than thread a
// handle through every stack-construction signature, the dashboard and the PHY share a global
// singleton — the same pattern the PHY already uses for `DETECTED_SDR_NAME` / `rf_status`.

static GLOBAL_CONTROL: std::sync::OnceLock<CalControl> = std::sync::OnceLock::new();
static PHY_CLAIMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The shared control the dashboard posts actions to and reads results from.
pub fn global_control() -> CalControl {
    GLOBAL_CONTROL.get_or_init(new_control).clone()
}

/// Called once by each PHY device as it constructs. The first caller (the primary cell,
/// attached first) gets the shared global control so its engine services dashboard requests;
/// any later cell gets an independent, unshared control that never receives dashboard actions.
/// This keeps calibration pointed at the primary SDR on multi-cell stations.
pub fn claim_phy_control() -> CalControl {
    if PHY_CLAIMED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        new_control()
    } else {
        global_control()
    }
}

/// Hard cap on sweep size so a bad range can never turn into a multi-minute off-air sweep.
const MAX_SWEEP_POINTS: usize = 256;

struct SweepState {
    req: CalRequest,
    points: Vec<GainPoint>,
    idx: usize,
    settle: u32,
    dwell_left: u32,
    sum_power: f64,
    sum_leak: f64,
    n: u32,
    peak_power: f32,
    floor_power: f32,
    measurements: Vec<CalMeasurement>,
    saved_rx_gains: Vec<(String, f64)>,
}

/// Drives a sweep (and one-shot actions) forward one PHY tick at a time. Lives on the PHY
/// thread inside the SDR device; see [`super::soapy_dev`].
pub struct CalEngine {
    sweep: Option<SweepState>,
    /// Ticks to wait after changing gain before trusting measurements (gain settle + AGC-free
    /// front end still needs the stream to flush the pre-change samples).
    settle_ticks: u32,
}

impl Default for CalEngine {
    fn default() -> Self {
        Self { sweep: None, settle_ticks: 2 }
    }
}

impl CalEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while a sweep is running — the PHY uses this to keep feeding measurements.
    pub fn is_sweeping(&self) -> bool {
        self.sweep.is_some()
    }

    fn set_point<D: PhyDevice>(dev: &mut D, p: GainPoint) -> Result<(), String> {
        dev.set_rx_gain("lna", p.lna)?;
        dev.set_rx_gain("pga", p.pga)?;
        Ok(())
    }

    fn begin_point(sw: &mut SweepState, dev: &mut impl PhyDevice) -> Result<(), String> {
        let p = sw.points[sw.idx];
        Self::set_point(dev, p)?;
        sw.settle = 0; // settle is applied by the caller via engine.settle_ticks
        sw.dwell_left = sw.req.dwell_blocks.max(1);
        sw.sum_power = 0.0;
        sw.sum_leak = 0.0;
        sw.n = 0;
        sw.peak_power = f32::NEG_INFINITY;
        sw.floor_power = f32::INFINITY;
        Ok(())
    }

    fn finalize_point(sw: &mut SweepState) {
        let noise = if sw.n > 0 { (sw.sum_power / sw.n as f64) as f32 } else { f32::NEG_INFINITY };
        let leak = if sw.n > 0 { (sw.sum_leak / sw.n as f64) as f32 } else { f32::NEG_INFINITY };
        let (snr, rssi) = if sw.req.mode == CalMode::Uplink && sw.peak_power.is_finite() && sw.floor_power.is_finite() {
            (Some(sw.peak_power - sw.floor_power), Some(sw.peak_power))
        } else {
            (None, None)
        };
        sw.measurements.push(CalMeasurement {
            point: sw.points[sw.idx],
            noise_dbfs: noise,
            leak_db: leak,
            uplink_rssi_dbfs: rssi,
            uplink_snr_db: snr,
            blocks: sw.n,
        });
    }

    /// Advance the engine by one PHY tick.
    ///
    /// `measurement` is `Some((power_dbfs, leak_db))` for the RX block just processed, or
    /// `None` when this tick produced no fresh block. `control` is the shared dashboard slot;
    /// the engine only `try_lock`s it.
    pub fn tick<D: PhyDevice>(&mut self, dev: &mut D, measurement: Option<(f32, f32)>, control: &CalControl) {
        if self.sweep.is_some() {
            self.advance_sweep(dev, measurement, control);
        } else {
            self.start_next_action(dev, control);
        }
    }

    fn advance_sweep<D: PhyDevice>(&mut self, dev: &mut D, measurement: Option<(f32, f32)>, control: &CalControl) {
        let settle_ticks = self.settle_ticks;
        // Own the sweep for this tick; put it back only if it should continue.
        let mut sw = self.sweep.take().expect("sweep present");

        // Accumulate this tick's block (after the settle window).
        if let Some((power, leak)) = measurement {
            if sw.settle > 0 {
                sw.settle -= 1;
            } else if sw.dwell_left > 0 {
                sw.sum_power += power as f64;
                sw.sum_leak += leak as f64;
                sw.n += 1;
                sw.peak_power = sw.peak_power.max(power);
                sw.floor_power = sw.floor_power.min(power);
                sw.dwell_left -= 1;
            }
        }

        if sw.dwell_left > 0 || sw.settle > 0 {
            self.sweep = Some(sw); // still measuring this point
            return;
        }

        // Current point done.
        Self::finalize_point(&mut sw);
        sw.idx += 1;
        let total = sw.points.len() as u32;

        if sw.idx >= sw.points.len() {
            // Sweep complete — restore the operator's original gains and publish the result.
            for (elem, db) in &sw.saved_rx_gains {
                let _ = dev.set_rx_gain(elem, *db);
            }
            let rec = choose(&sw.measurements, &sw.req);
            if let Ok(mut c) = control.lock() {
                c.running = false;
                c.current = None;
                c.progress = (total, total);
                match rec {
                    Some(r) => {
                        c.result = Some(r);
                        c.error = None;
                    }
                    None => c.error = Some("calibration found no usable gain point".to_string()),
                }
            }
            return; // self.sweep stays None
        }

        // Move to the next point.
        let next = sw.points[sw.idx];
        let idx = sw.idx as u32;
        if let Err(e) = Self::begin_point(&mut sw, dev) {
            // Abort: restore gains and report.
            for (elem, db) in &sw.saved_rx_gains {
                let _ = dev.set_rx_gain(elem, *db);
            }
            if let Ok(mut c) = control.lock() {
                c.running = false;
                c.current = None;
                c.error = Some(format!("calibration aborted: {e}"));
            }
            return; // self.sweep stays None
        }
        sw.settle = settle_ticks;
        if let Ok(mut c) = control.lock() {
            c.progress = (idx, total);
            c.current = Some(next);
        }
        self.sweep = Some(sw);
    }

    fn start_next_action<D: PhyDevice>(&mut self, dev: &mut D, control: &CalControl) {
        let Ok(mut c) = control.try_lock() else { return };
        let Some(action) = c.queue.pop_front() else { return };
        // Drop the lock while we touch the device, then re-lock to record the outcome.
        drop(c);
        match action {
            PhyAction::Calibrate(req) => {
                let mut points = req.plan();
                if points.len() > MAX_SWEEP_POINTS {
                    points.truncate(MAX_SWEEP_POINTS);
                }
                if points.is_empty() {
                    if let Ok(mut c) = control.lock() {
                        c.error = Some("empty calibration sweep".to_string());
                    }
                    return;
                }
                let saved_rx_gains: Vec<(String, f64)> =
                    dev.read_rx_gains_db().into_iter().map(|(k, v)| (k, v)).collect();
                let total = points.len() as u32;
                let first = points[0];
                let mut sw = SweepState {
                    req,
                    points,
                    idx: 0,
                    settle: self.settle_ticks,
                    dwell_left: req.dwell_blocks.max(1),
                    sum_power: 0.0,
                    sum_leak: 0.0,
                    n: 0,
                    peak_power: f32::NEG_INFINITY,
                    floor_power: f32::INFINITY,
                    measurements: Vec::new(),
                    saved_rx_gains,
                };
                if let Err(e) = Self::set_point(dev, first) {
                    if let Ok(mut c) = control.lock() {
                        c.error = Some(format!("calibration could not set gains: {e}"));
                    }
                    return;
                }
                sw.dwell_left = req.dwell_blocks.max(1);
                self.sweep = Some(sw);
                if let Ok(mut c) = control.lock() {
                    c.running = true;
                    c.progress = (0, total);
                    c.current = Some(first);
                    c.result = None;
                    c.error = None;
                }
            }
            PhyAction::ApplyRxGains(gains) => {
                let mut err = None;
                for (elem, db) in &gains {
                    if let Err(e) = dev.set_rx_gain(elem, *db) {
                        err = Some(e);
                        break;
                    }
                }
                if let Ok(mut c) = control.lock() {
                    match err {
                        Some(e) => c.error = Some(format!("apply RX gains failed: {e}")),
                        None => c.last_action = Some(format!("applied RX gains ({} stage(s))", gains.len())),
                    }
                }
            }
            PhyAction::ApplyTxGains(gains) => {
                let mut err = None;
                for (elem, db) in &gains {
                    if let Err(e) = dev.set_tx_gain(elem, *db) {
                        err = Some(e);
                        break;
                    }
                }
                if let Ok(mut c) = control.lock() {
                    match err {
                        Some(e) => c.error = Some(format!("apply TX gains failed: {e}")),
                        None => c.last_action = Some(format!("applied TX gains ({} stage(s))", gains.len())),
                    }
                }
            }
            PhyAction::ReadRegister { name, addr } => {
                let r = dev.read_sx_register(&name, addr);
                if let Ok(mut c) = control.lock() {
                    match r {
                        Ok(v) => {
                            c.last_register = Some((addr, v));
                            c.last_action = Some(format!("read {name} reg 0x{addr:02x} = 0x{v:02x}"));
                            c.error = None;
                        }
                        Err(e) => c.error = Some(format!("register read failed: {e}")),
                    }
                }
            }
            PhyAction::WriteRegister { name, addr, value } => {
                let r = dev.write_sx_register(&name, addr, value);
                if let Ok(mut c) = control.lock() {
                    match r {
                        Ok(()) => {
                            c.last_action = Some(format!("wrote {name} reg 0x{addr:02x} = 0x{value:02x}"));
                            c.error = None;
                        }
                        Err(e) => c.error = Some(format!("register write failed: {e}")),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_complex::Complex;

    fn block_const(re: f32, im: f32, n: usize) -> Vec<ComplexSample> {
        vec![Complex::new(re, im); n]
    }

    #[test]
    fn power_full_scale_is_zero_dbfs() {
        // |x| = 1 everywhere → 0 dBFS.
        let b = block_const(1.0, 0.0, 256);
        assert!((block_power_dbfs(&b) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn power_half_amplitude_is_minus_six_dbfs() {
        let b = block_const(0.5, 0.0, 256);
        assert!((block_power_dbfs(&b) - (-6.0206)).abs() < 1e-2);
    }

    #[test]
    fn empty_block_is_neg_inf() {
        assert_eq!(block_power_dbfs(&[]), f32::NEG_INFINITY);
        assert_eq!(block_dc_leak_db(&[]), f32::NEG_INFINITY);
    }

    #[test]
    fn pure_dc_is_zero_leak_db() {
        // All energy is DC → dc_power == total_power → 0 dB (worst case).
        let b = block_const(0.3, -0.2, 128);
        assert!((block_dc_leak_db(&b) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn zero_mean_signal_has_low_leak() {
        // Alternating ± cancels the DC term → very negative leak dB (clean).
        let mut b = Vec::new();
        for i in 0..256 {
            let s = if i % 2 == 0 { 0.5 } else { -0.5 };
            b.push(Complex::new(s, 0.0));
        }
        assert!(block_dc_leak_db(&b) < -60.0, "leak = {}", block_dc_leak_db(&b));
    }

    #[test]
    fn axis_range_inclusive_and_single_point() {
        let r = AxisRange { min: 0.0, max: 12.0, step: 6.0 };
        assert_eq!(r.values(), vec![0.0, 6.0, 12.0]);
        let one = AxisRange { min: 5.0, max: 5.0, step: 6.0 };
        assert_eq!(one.values(), vec![5.0]);
        let zero_step = AxisRange { min: 2.0, max: 8.0, step: 0.0 };
        assert_eq!(zero_step.values(), vec![2.0]);
    }

    #[test]
    fn axis_range_appends_ragged_max() {
        let r = AxisRange { min: 0.0, max: 10.0, step: 4.0 };
        assert_eq!(r.values(), vec![0.0, 4.0, 8.0, 10.0]);
    }

    #[test]
    fn plan_varies_pga_fastest() {
        let req = CalRequest {
            mode: CalMode::NoiseFloor,
            lna: AxisRange { min: 0.0, max: 6.0, step: 6.0 },
            pga: AxisRange { min: 0.0, max: 6.0, step: 6.0 },
            dwell_blocks: 1,
            thresholds: Thresholds::default(),
        };
        let p = req.plan();
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], GainPoint { lna: 0.0, pga: 0.0 });
        assert_eq!(p[1], GainPoint { lna: 0.0, pga: 6.0 });
        assert_eq!(p[2], GainPoint { lna: 6.0, pga: 0.0 });
    }

    fn meas(lna: f64, pga: f64, noise: f32, leak: f32) -> CalMeasurement {
        CalMeasurement {
            point: GainPoint { lna, pga },
            noise_dbfs: noise,
            leak_db: leak,
            uplink_rssi_dbfs: None,
            uplink_snr_db: None,
            blocks: 8,
        }
    }

    #[test]
    fn noisefloor_prefers_more_gain_within_limits() {
        let req = CalRequest::sx1255_default(CalMode::NoiseFloor);
        // Three clean points well inside headroom and leak limits; most gain should win.
        let ms = vec![
            meas(0.0, 0.0, -70.0, -60.0),
            meas(24.0, 12.0, -40.0, -50.0),
            meas(42.0, 18.0, -20.0, -40.0),
        ];
        let rec = choose(&ms, &req).unwrap();
        assert_eq!((rec.lna, rec.pga), (42.0, 18.0));
    }

    #[test]
    fn noisefloor_rejects_clipping_point() {
        let req = CalRequest::sx1255_default(CalMode::NoiseFloor);
        // The highest-gain point is clipping (-2 dBFS > -6 guard) and should lose to the
        // clean mid point despite having more gain.
        let ms = vec![
            meas(24.0, 12.0, -30.0, -50.0),
            meas(48.0, 30.0, -2.0, -40.0),
        ];
        let rec = choose(&ms, &req).unwrap();
        assert_eq!((rec.lna, rec.pga), (24.0, 12.0));
        assert!(rec.reason.contains("sensitivity"));
    }

    #[test]
    fn noisefloor_rejects_leaky_point() {
        let req = CalRequest::sx1255_default(CalMode::NoiseFloor);
        let ms = vec![
            meas(24.0, 12.0, -30.0, -50.0),
            meas(48.0, 30.0, -30.0, -5.0), // bad leak
        ];
        let rec = choose(&ms, &req).unwrap();
        assert_eq!((rec.lna, rec.pga), (24.0, 12.0));
    }

    #[test]
    fn uplink_needs_a_burst_and_prefers_high_snr() {
        let req = CalRequest::sx1255_default(CalMode::Uplink);
        let mut a = meas(24.0, 12.0, -30.0, -50.0);
        a.uplink_snr_db = Some(12.0);
        a.uplink_rssi_dbfs = Some(-40.0);
        let mut b = meas(36.0, 18.0, -25.0, -50.0);
        b.uplink_snr_db = Some(26.0);
        b.uplink_rssi_dbfs = Some(-35.0);
        let dead = meas(0.0, 0.0, -80.0, -60.0); // no burst → not eligible
        let rec = choose(&[a, dead, b], &req).unwrap();
        assert_eq!((rec.lna, rec.pga), (36.0, 18.0));
        assert!(rec.reason.contains("SNR"));
    }

    #[test]
    fn uplink_with_no_burst_anywhere_returns_none() {
        let req = CalRequest::sx1255_default(CalMode::Uplink);
        // Measurements present but no SNR lift (no keyed radio) → nothing eligible.
        let ms = vec![meas(0.0, 0.0, -80.0, -60.0), meas(24.0, 12.0, -40.0, -50.0)];
        assert!(choose(&ms, &req).is_none());
    }

    #[test]
    fn empty_sweep_returns_none() {
        let req = CalRequest::sx1255_default(CalMode::NoiseFloor);
        assert!(choose(&[], &req).is_none());
    }

    // ── Engine (state machine) tests, driven by a mock device ────────────────

    #[derive(Default)]
    struct MockDev {
        rx: std::collections::BTreeMap<String, f64>,
        tx: std::collections::BTreeMap<String, f64>,
        regs: std::collections::BTreeMap<u32, u32>,
        set_log: Vec<(String, f64)>,
        fail_lna: bool,
    }
    impl PhyDevice for MockDev {
        fn set_rx_gain(&mut self, element: &str, db: f64) -> Result<(), String> {
            if self.fail_lna && element == "lna" {
                return Err("mock lna failure".into());
            }
            self.rx.insert(element.to_string(), db);
            self.set_log.push((element.to_string(), db));
            Ok(())
        }
        fn set_tx_gain(&mut self, element: &str, db: f64) -> Result<(), String> {
            self.tx.insert(element.to_string(), db);
            Ok(())
        }
        fn read_rx_gains_db(&self) -> Vec<(String, f64)> {
            self.rx.iter().map(|(k, v)| (k.clone(), *v)).collect()
        }
        fn read_sx_register(&self, _name: &str, addr: u32) -> Result<u32, String> {
            Ok(*self.regs.get(&addr).unwrap_or(&0))
        }
        fn write_sx_register(&mut self, _name: &str, addr: u32, value: u32) -> Result<(), String> {
            self.regs.insert(addr, value);
            Ok(())
        }
        fn list_sx_register_ifaces(&self) -> Vec<String> {
            vec![String::new()]
        }
    }

    fn tiny_request() -> CalRequest {
        CalRequest {
            mode: CalMode::NoiseFloor,
            lna: AxisRange { min: 0.0, max: 12.0, step: 12.0 }, // 0, 12
            pga: AxisRange { min: 0.0, max: 0.0, step: 0.0 },   // 0
            dwell_blocks: 2,
            thresholds: Thresholds::default(),
        }
    }

    /// Run the engine to completion feeding a constant measurement each tick.
    fn run_to_done(engine: &mut CalEngine, dev: &mut MockDev, control: &CalControl, power: f32, leak: f32) -> usize {
        for _ in 0..10_000 {
            engine.tick(dev, Some((power, leak)), control);
            let done = { control.lock().unwrap().result.is_some() || control.lock().unwrap().error.is_some() };
            if done && !engine.is_sweeping() {
                return 1;
            }
        }
        0
    }

    #[test]
    fn engine_runs_sweep_and_restores_original_gains() {
        let mut dev = MockDev::default();
        dev.rx.insert("lna".into(), 40.0); // operator's original gains
        dev.rx.insert("pga".into(), 20.0);
        let engine = &mut CalEngine::new();
        let control = new_control();

        control.lock().unwrap().queue.push_back(PhyAction::Calibrate(tiny_request()));
        // First tick starts the sweep (sets point 0), then measurements drive it.
        engine.tick(&mut dev, None, &control);
        assert!(control.lock().unwrap().running, "sweep should be running after start");

        assert_eq!(run_to_done(engine, &mut dev, &control, -30.0, -50.0), 1, "sweep must finish");

        let c = control.lock().unwrap();
        assert!(!c.running);
        let rec = c.result.clone().expect("a recommendation");
        // NoiseFloor with clean points → prefers the higher LNA (12).
        assert_eq!(rec.lna, 12.0);
        // Original gains restored on the device.
        assert_eq!(dev.rx.get("lna"), Some(&40.0));
        assert_eq!(dev.rx.get("pga"), Some(&20.0));
    }

    #[test]
    fn engine_reports_device_failure() {
        let mut dev = MockDev { fail_lna: true, ..Default::default() };
        let engine = &mut CalEngine::new();
        let control = new_control();
        control.lock().unwrap().queue.push_back(PhyAction::Calibrate(tiny_request()));
        engine.tick(&mut dev, None, &control);
        let c = control.lock().unwrap();
        assert!(c.error.is_some(), "device failure should surface as an error");
        assert!(!c.running);
    }

    #[test]
    fn engine_handles_register_and_gain_actions() {
        let mut dev = MockDev::default();
        let engine = &mut CalEngine::new();
        let control = new_control();

        control.lock().unwrap().queue.push_back(PhyAction::WriteRegister { name: "sx1255".into(), addr: 0x0c, value: 0x3f });
        engine.tick(&mut dev, None, &control);
        assert_eq!(dev.regs.get(&0x0c), Some(&0x3f));

        control.lock().unwrap().queue.push_back(PhyAction::ReadRegister { name: "sx1255".into(), addr: 0x0c });
        engine.tick(&mut dev, None, &control);
        assert_eq!(control.lock().unwrap().last_register, Some((0x0c, 0x3f)));

        control.lock().unwrap().queue.push_back(PhyAction::ApplyRxGains(vec![("lna".into(), 33.0), ("pga".into(), 9.0)]));
        engine.tick(&mut dev, None, &control);
        assert_eq!(dev.rx.get("lna"), Some(&33.0));
        assert_eq!(dev.rx.get("pga"), Some(&9.0));
    }
}
