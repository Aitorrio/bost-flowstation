//! Dashboard glue for the SDR RX gain calibration feature.
//!
//! The browser talks to four endpoints; each one either reads the shared control slot
//! ([`crate::phy::components::calibration::global_control`]) or pushes a [`PhyAction`] onto it
//! for the PHY thread to carry out. Nothing here touches the SDR directly — that only ever
//! happens on the PHY tick.

use serde_json::{json, Value};

use crate::phy::components::calibration::{
    global_control, AxisRange, CalMode, CalRequest, GainPoint, PhyAction, Recommendation, Thresholds,
};
use crate::phy::components::soapy_settings::detected_sdr_name;

/// `GET /api/rf/calibration` — everything the RF calibration panel needs to render.
pub fn calibration_json() -> Value {
    let control = global_control();
    let c = control.lock().ok();
    let c = match c {
        Some(c) => c,
        None => return json!({ "error": "calibration state unavailable" }),
    };
    json!({
        "device": detected_sdr_name(),
        "running": c.running,
        "progress": { "done": c.progress.0, "total": c.progress.1 },
        "current": c.current.map(point_json),
        "result": c.result.as_ref().map(recommendation_json),
        "error": c.error,
        "last_action": c.last_action,
        "last_register": c.last_register.map(|(addr, value)| json!({ "addr": addr, "value": value })),
        "register_ifaces": c.register_ifaces,
    })
}

fn point_json(p: GainPoint) -> Value {
    json!({ "lna": p.lna, "pga": p.pga })
}

fn recommendation_json(r: &Recommendation) -> Value {
    json!({
        "lna": r.lna,
        "pga": r.pga,
        "noise_dbfs": r.noise_dbfs,
        "leak_db": r.leak_db,
        "uplink_rssi_dbfs": r.uplink_rssi_dbfs,
        "uplink_snr_db": r.uplink_snr_db,
        "reason": r.reason,
    })
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

/// `POST /api/rf/calibration` — start a sweep. Body:
/// `{mode:"noise"|"uplink", lna_min,lna_max,lna_step, pga_min,pga_max,pga_step, dwell}`.
/// Missing numeric fields fall back to the SX1255 defaults.
pub fn start_calibration(body: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    let mode = match v.get("mode").and_then(|m| m.as_str()).unwrap_or("noise") {
        "uplink" => CalMode::Uplink,
        _ => CalMode::NoiseFloor,
    };
    let def = CalRequest::sx1255_default(mode);
    let lna = AxisRange {
        min: num(&v, "lna_min").unwrap_or(def.lna.min),
        max: num(&v, "lna_max").unwrap_or(def.lna.max),
        step: num(&v, "lna_step").unwrap_or(def.lna.step),
    };
    let pga = AxisRange {
        min: num(&v, "pga_min").unwrap_or(def.pga.min),
        max: num(&v, "pga_max").unwrap_or(def.pga.max),
        step: num(&v, "pga_step").unwrap_or(def.pga.step),
    };
    validate_axis("LNA", &lna)?;
    validate_axis("PGA", &pga)?;
    let dwell = num(&v, "dwell").map(|d| d.max(1.0) as u32).unwrap_or(def.dwell_blocks);
    let req = CalRequest { mode, lna, pga, dwell_blocks: dwell, thresholds: Thresholds::default() };

    let control = global_control();
    let mut c = control.lock().map_err(|_| "calibration state poisoned".to_string())?;
    if c.running {
        return Err("a calibration is already running".to_string());
    }
    // Clear any stale result so the UI shows this run cleanly.
    c.result = None;
    c.error = None;
    c.queue.push_back(PhyAction::Calibrate(req));
    Ok(json!({ "started": true, "points": req.plan().len() }))
}

fn validate_axis(name: &str, a: &AxisRange) -> Result<(), String> {
    if !(a.min.is_finite() && a.max.is_finite() && a.step.is_finite()) {
        return Err(format!("{name} range must be finite numbers"));
    }
    if a.max < a.min {
        return Err(format!("{name} max is below min"));
    }
    if !(-30.0..=90.0).contains(&a.min) || !(-30.0..=90.0).contains(&a.max) {
        return Err(format!("{name} values must be between -30 and 90 dB"));
    }
    if a.step < 0.0 {
        return Err(format!("{name} step must not be negative"));
    }
    Ok(())
}

/// `POST /api/rf/gains` — apply gains to the live device without a restart. Body:
/// `{rx:[["lna",40],["pga",20]], tx:[...]}`. Either list may be omitted.
pub fn apply_gains(body: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    let rx = parse_gain_list(v.get("rx"))?;
    let tx = parse_gain_list(v.get("tx"))?;
    if rx.is_empty() && tx.is_empty() {
        return Err("no gains given".to_string());
    }
    let control = global_control();
    let mut c = control.lock().map_err(|_| "calibration state poisoned".to_string())?;
    if c.running {
        return Err("cannot change gains while a calibration is running".to_string());
    }
    if !rx.is_empty() {
        c.queue.push_back(PhyAction::ApplyRxGains(rx));
    }
    if !tx.is_empty() {
        c.queue.push_back(PhyAction::ApplyTxGains(tx));
    }
    Ok(json!({ "applied": true }))
}

fn parse_gain_list(v: Option<&Value>) -> Result<Vec<(String, f64)>, String> {
    let Some(arr) = v.and_then(|x| x.as_array()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for item in arr {
        let pair = item.as_array().ok_or("each gain must be a [stage, dB] pair")?;
        let stage = pair.first().and_then(|s| s.as_str()).ok_or("gain stage must be a string")?;
        let db = pair.get(1).and_then(|d| d.as_f64()).ok_or("gain value must be a number")?;
        if !db.is_finite() || !(-30.0..=90.0).contains(&db) {
            return Err(format!("gain for '{stage}' must be between -30 and 90 dB"));
        }
        out.push((stage.to_string(), db));
    }
    Ok(out)
}

/// `POST /api/rf/register` — read or write a raw SX1255/SX1257 register. Body:
/// `{op:"read"|"write", iface:"", addr:12, value:63}`. `iface` defaults to the device's
/// first register interface. Advanced: wrong values can deafen or mistune the front end.
pub fn register_op(body: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    let op = v.get("op").and_then(|o| o.as_str()).unwrap_or("read");
    let iface = v.get("iface").and_then(|i| i.as_str()).unwrap_or("").to_string();
    let addr = num(&v, "addr").ok_or("addr is required")?;
    if !(0.0..=4_294_967_295.0).contains(&addr) {
        return Err("addr out of range".to_string());
    }
    let addr = addr as u32;
    let control = global_control();
    let mut c = control.lock().map_err(|_| "calibration state poisoned".to_string())?;
    if c.running {
        return Err("cannot access registers while a calibration is running".to_string());
    }
    match op {
        "write" => {
            let value = num(&v, "value").ok_or("value is required for write")?;
            if !(0.0..=4_294_967_295.0).contains(&value) {
                return Err("value out of range".to_string());
            }
            c.queue.push_back(PhyAction::WriteRegister { name: iface, addr, value: value as u32 });
            Ok(json!({ "queued": "write" }))
        }
        _ => {
            c.queue.push_back(PhyAction::ReadRegister { name: iface, addr });
            Ok(json!({ "queued": "read" }))
        }
    }
}
