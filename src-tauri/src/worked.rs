//! Worked time per session ("turni_oggi", 18/11): the hook records when each turn starts (UserPromptSubmit) and ends (Stop),
//! and the Juggl payload gives, for each session, the turns of today as [[start_s, end_s or null if running], ...].
//! Only numbers: no content. The UI computes the per-project union (parallel chats are not counted twice).
use serde_json::{json, Value};

/// How long the record of finished turns is kept in the session state (the payload only shows today).
const KEEP_SECS: f64 = 3.0 * 86400.0;
/// At most this many turns are kept per session.
const MAX_TURNS: usize = 300;

fn last_is_open(turns: &[Value]) -> bool { turns.last().map(|t| t.get(1).map_or(true, |e| e.is_null())).unwrap_or(false) }

/// A turn starts at `when`. An earlier turn still open (the Stop never arrived) ends here.
pub fn record_start(state: &mut Value, when: f64) {
    let mut turns: Vec<Value> = state["turni"].as_array().cloned().unwrap_or_default();
    if last_is_open(&turns) {
        let start = turns.last().and_then(|t| t[0].as_f64()).unwrap_or(when);
        let n = turns.len();
        turns[n - 1] = json!([start, when.max(start)]);
    }
    turns.push(json!([when, Value::Null]));
    turns.retain(|t| t.get(1).map_or(true, |e| e.is_null() || e.as_f64().map_or(false, |x| x >= when - KEEP_SECS)));
    if turns.len() > MAX_TURNS { let cut = turns.len() - MAX_TURNS; turns.drain(0..cut); }
    state["turni"] = Value::Array(turns);
}

/// The turn that is open ends at `when` (no open turn: nothing to do).
pub fn record_end(state: &mut Value, when: f64) {
    let Some(arr) = state["turni"].as_array_mut() else { return };
    if let Some(last) = arr.last_mut() {
        if last.get(1).map_or(true, |e| e.is_null()) {
            let start = last[0].as_f64().unwrap_or(when);
            *last = json!([start, when.max(start)]);
        }
    }
}

/// Today's turns of one session. `working`: the session is working right now (its open turn is really running);
/// otherwise an open turn (Stop lost, crash) ends at `since` (the last state change) so it does not count forever.
pub fn today(state: &Value, midnight: f64, working: bool, since: f64) -> Value {
    let mut out: Vec<Value> = vec![];
    for t in state["turni"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let Some(start) = t.get(0).and_then(|x| x.as_f64()) else { continue };
        let end = t.get(1).and_then(|x| x.as_f64());
        let end = match end { Some(e) => Some(e.max(start)), None if working => None, None => Some(since.max(start)) };
        if let Some(e) = end { if e <= midnight { continue; } }
        out.push(json!([start.max(midnight), end.map(|e| json!(e)).unwrap_or(Value::Null)]));
    }
    Value::Array(out)
}

/// Midnight of today, local time, in Unix seconds.
pub fn local_midnight(now: f64) -> f64 {
    unsafe {
        let t = now as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() { return now - now.rem_euclid(86400.0); }
        tm.tm_hour = 0; tm.tm_min = 0; tm.tm_sec = 0;
        let m = libc::mktime(&mut tm);
        if m < 0 { now - now.rem_euclid(86400.0) } else { m as f64 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_turns_and_gives_today() {
        let mut s = json!({});
        record_start(&mut s, 1000.0);
        record_end(&mut s, 1500.0);
        record_start(&mut s, 2000.0);
        assert_eq!(today(&s, 0.0, true, 0.0), json!([[1000.0, 1500.0], [2000.0, null]]));
        // running turn but session no longer working: ends at the last state change
        assert_eq!(today(&s, 0.0, false, 2300.0), json!([[1000.0, 1500.0], [2000.0, 2300.0]]));
        // midnight cuts the turns: a turn that ended before midnight disappears, one across midnight is cut at midnight
        assert_eq!(today(&s, 1200.0, true, 0.0), json!([[1200.0, 1500.0], [2000.0, null]]));
        assert_eq!(today(&s, 1600.0, true, 0.0), json!([[2000.0, null]]));
        let mut c = json!({});
        record_start(&mut c, 1000.0); record_end(&mut c, 3000.0);
        assert_eq!(today(&c, 2000.0, false, 0.0), json!([[2000.0, 3000.0]]));
    }
    #[test]
    fn a_new_turn_closes_the_one_that_never_stopped() {
        let mut s = json!({});
        record_start(&mut s, 100.0);
        record_start(&mut s, 400.0);
        assert_eq!(s["turni"], json!([[100.0, 400.0], [400.0, null]]));
        record_end(&mut s, 450.0); record_end(&mut s, 999.0);
        assert_eq!(s["turni"], json!([[100.0, 400.0], [400.0, 450.0]]));
    }
    #[test]
    fn old_turns_are_dropped_and_midnight_is_before_now() {
        let mut s = json!({});
        record_start(&mut s, 100.0); record_end(&mut s, 200.0);
        record_start(&mut s, 100.0 + 4.0 * 86400.0);
        assert_eq!(s["turni"].as_array().unwrap().len(), 1);
        let now = 1_795_000_000.0;
        let m = local_midnight(now);
        assert!(m <= now && now - m < 25.0 * 3600.0);
    }
}
