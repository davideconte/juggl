// Compaction while idle (Nov 16, "smart autocompact").
// Claude Code compacts an idle chat by itself before the 1-hour cache expires ("Compacted while idle"), but ONLY in the
// interactive terminal: checked on Nov 16 on the transcripts (about 45 idle compactions, all with entrypoint "cli";
// zero in the chats inside Juggl, entrypoint "sdk-cli", out of 367 pauses over one hour with context >= 200k and 1-hour cache).
// Here Juggl does the equivalent for hosted chats:
//  - chat idle for `ferma_min` minutes (default 45: the 1-hour cache is still warm, compacting is cheap) and not beyond
//    `fredda_min` (57: after that the cache has expired and compacting would cost as much as resuming it);
//  - context (last usage in the transcript: input + cache read + cache written) >= threshold (default 150k, at most
//    60% of the model's window);
//  - "urgent": context >= `urgente_pct` of the window (85%), compacts even with a warm cache, but only once the turn is over;
//  - NEVER if: the chat is working, waiting for you or has a question/permission, has background commands or sub-agents, queued
//    messages, you are looking at it on the Mac, it was just resumed, Claude's week or 5 hours almost used up, it is already compacted.
// Sends `/compact <ISTRUZIONI>` as a Juggl message (not yours: no requests), stores the previous state in the chat
// ("compattando") and at end of turn puts it back (hosted.rs): compaction does not make "ti aspetta" appear.
// Log of every decision in compattazione.jsonl (7 days): one line when the reason changes for a chat, one for
// every send and one for the outcome, with the REAL before/after tokens (Claude Code's compactMetadata), not estimates.
// General switch off by default; per chat: "compattazione": true/false in the state (true = on even with the general switch off).
use crate::{storage, base_dir, f64_of, load_json, unix_now, str_of};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const INSTRUCTIONS: &str = "Compattazione automatica di Juggl: la chat e' ferma. Nel riassunto tieni: le decisioni prese e il perche'; \
i file creati o modificati, con il percorso completo; le richieste dell'utente ancora aperte e cosa sta aspettando; comandi, id, link, \
porte, rami e versioni che servono per riprendere; i problemi ancora da risolvere. Togli gli output lunghi e i tentativi gia' superati.";

const LOG_DAYS: f64 = 7.0;
const EVERY_S: f64 = 60.0;
/// After a send with no outcome within this time, the compaction is considered failed (it can be retried).
const OUTCOME_WAIT_S: f64 = 15.0 * 60.0;
const MAX_24H_PER_CHAT: usize = 6;

fn settings_file() -> PathBuf { base_dir().join("impostazioni.json") }
fn log_file() -> PathBuf { base_dir().join("compattazione.jsonl") }
fn log_lock() -> &'static Mutex<()> { static L: OnceLock<Mutex<()>> = OnceLock::new(); L.get_or_init(|| Mutex::new(())) }

/// Settings with default values.
pub fn settings() -> Value {
    let v = load_json(&settings_file()).map(|v| v["compattazione"].clone()).filter(|v| v.is_object()).unwrap_or(json!({}));
    let n = |k: &str, d: f64| v[k].as_f64().unwrap_or(d);
    json!({
        "attiva": v["attiva"].as_bool().unwrap_or(false),
        "ferma_min": n("ferma_min", 45.0),
        "fredda_min": n("fredda_min", 57.0),
        "soglia_k": n("soglia_k", 150.0),
        "urgente_pct": n("urgente_pct", 85.0),
        "settimana_max": n("settimana_max", 95.0),
        "cinque_ore_max": n("cinque_ore_max", 90.0),
    })
}

pub fn set(new_values: &Value) -> Value {
    let r = storage::modify(&settings_file(), Some(json!({})), |v| {
        if !v["compattazione"].is_object() { v["compattazione"] = json!({}); }
        let c = &mut v["compattazione"];
        if let Some(b) = new_values["attiva"].as_bool() { c["attiva"] = json!(b); }
        let limits = [("ferma_min", 10.0, 58.0), ("fredda_min", 15.0, 59.0), ("soglia_k", 50.0, 900.0), ("urgente_pct", 50.0, 95.0),
                   ("settimana_max", 50.0, 100.0), ("cinque_ore_max", 50.0, 100.0)];
        for (k, a, b) in limits { if let Some(x) = new_values[k].as_f64() { c[k] = json!(x.clamp(a, b)); } }
        true
    });
    if let Err(e) = r { return json!({"ok": false, "msg": e}); }
    let imp = settings();
    crate::log_juggl(&format!("compattazione: impostazioni {imp}"));
    json!({"ok": true, "impostazioni": imp})
}

/// Per-chat switch: Some(true) on even with the general switch off, Some(false) off, None = follows the general switch.
pub fn set_chat(id: &str, on: Option<bool>) -> Value {
    if !storage::is_valid_id(id) { return json!({"ok": false, "msg": "id non valido"}); }
    let p = base_dir().join("stato").join(format!("{id}.json"));
    let ok = crate::update_json(&p, |v| {
        match on { Some(b) => v["compattazione"] = json!(b), None => { if let Some(o) = v.as_object_mut() { o.remove("compattazione"); } } }
        true
    });
    record(json!({"sessione": id, "esito": "interruttore", "motivo": match on { Some(true) => "accesa per la chat", Some(false) => "spenta per la chat", None => "segue il generale" }}));
    json!({"ok": ok})
}

// ---------- context usage, read from the transcript ----------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    /// context of the last answer (input + cache read + cache written)
    pub token: u64,
    pub finestra: u64,
    pub modello: String,
    /// time of the last model call (end of the last answer)
    pub t_ultimo: f64,
    /// cache written with a 1-hour TTL (false = 5 minutes: the 45-minute rule does not apply)
    pub ttl_1h: bool,
    /// after the last answer there is already a compaction (nothing to do)
    pub gia_compattata: bool,
    /// last compaction in the chunk read: (time, before, after, trigger)
    pub ultima_compattazione: Option<(f64, u64, u64, String)>,
}

fn ts(x: &str) -> f64 {
    // "2026-11-16T10:00:00.000Z" -> seconds (no dependencies: days since 1970 with the civil-from-days algorithm)
    let b = x.as_bytes();
    if b.len() < 19 { return 0.0; }
    let n = |a: usize, l: usize| x.get(a..a + l).and_then(|t| t.parse::<i64>().ok()).unwrap_or(0);
    let (mut y, m, d) = (n(0, 4), n(5, 2), n(8, 2));
    let (hh, mm, ss) = (n(11, 2), n(14, 2), n(17, 2));
    let frac = x.get(19..).and_then(|t| t.strip_prefix('.')).map(|t| t.trim_end_matches('Z')).and_then(|t| format!("0.{t}").parse::<f64>().ok()).unwrap_or(0.0);
    if m <= 2 { y -= 1; }
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    (days * 86400 + hh * 3600 + mm * 60 + ss) as f64 + frac
}

fn window_of(model: &str) -> u64 {
    if model.contains("[1m]") || model.contains("opus-5") || model.contains("sonnet-5") || model.contains("fable") { 1_000_000 } else { 200_000 }
}

/// Context usage from the transcript lines (in order).
pub fn usage_from_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Option<Usage> {
    let mut u = Usage::default();
    let mut seen = false;
    for r in lines {
        let is_boundary = r.contains("\"compact_boundary\"");
        if !is_boundary && !(r.contains("\"usage\"") && r.contains("\"assistant\"")) { continue; }
        let Ok(d) = serde_json::from_str::<Value>(r) else { continue };
        if is_boundary && str_of(&d, "subtype") == "compact_boundary" {
            let m = &d["compactMetadata"];
            u.ultima_compattazione = Some((ts(str_of(&d, "timestamp")), m["preTokens"].as_u64().unwrap_or(0), m["postTokens"].as_u64().unwrap_or(0), str_of(m, "trigger").to_string()));
            u.gia_compattata = true;
            continue;
        }
        if str_of(&d, "type") != "assistant" { continue; }
        let us = &d["message"]["usage"];
        let k = |n: &str| us[n].as_u64().unwrap_or(0);
        let total = k("input_tokens") + k("cache_creation_input_tokens") + k("cache_read_input_tokens");
        if total == 0 { continue; }
        seen = true;
        u.token = total;
        u.modello = str_of(&d["message"], "model").to_string();
        u.finestra = window_of(&u.modello);
        u.t_ultimo = ts(str_of(&d, "timestamp"));
        let cc = &us["cache_creation"];
        if cc.is_object() { u.ttl_1h = cc["ephemeral_1h_input_tokens"].as_u64().unwrap_or(0) > 0 || cc["ephemeral_5m_input_tokens"].as_u64().unwrap_or(0) == 0; }
        else { u.ttl_1h = true; }
        u.gia_compattata = false;
    }
    if seen || u.ultima_compattazione.is_some() { Some(u) } else { None }
}

pub fn usage_of(transcript: &Path) -> Option<Usage> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(transcript).ok()?;
    let n = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(n.saturating_sub(2_000_000)));
    let mut b = Vec::new();
    let _ = file.read_to_end(&mut b);
    let t = String::from_utf8_lossy(&b);
    let mut lines = t.lines();
    if n > 2_000_000 { lines.next(); }   // the first line is cut in half
    usage_from_lines(lines)
}

// ---------- decision (pure, covered by tests) ----------

pub struct Inputs<'a> {
    pub chat: &'a Value,
    pub uso: Option<&'a Usage>,
    pub imp: &'a Value,
    pub adesso: f64,
    /// the chat is open on screen and the user is at the Mac
    pub guardata: bool,
    pub in_coda: usize,
    /// used percentages (week, 5 hours) if known
    pub settimana: Option<f64>,
    pub cinque_ore: Option<f64>,
    /// Juggl sends to this chat in the last 24 hours and the last one (time)
    pub invii_24h: usize,
    pub ultimo_invio: f64,
    pub juggl_id: &'a str,
}

#[derive(Debug, PartialEq)]
pub enum Decision { Compatta(&'static str), Salta(String) }

fn skip(m: impl Into<String>) -> Decision { Decision::Salta(m.into()) }

pub fn decide(d: &Inputs) -> Decision {
    let v = d.chat;
    let id = str_of(v, "sessione");
    let per_chat = v.get("compattazione").and_then(|x| x.as_bool());
    if per_chat == Some(false) { return skip("spenta per questa chat"); }
    if !d.imp["attiva"].as_bool().unwrap_or(false) && per_chat != Some(true) { return skip("interruttore generale spento"); }
    if id.is_empty() || id.starts_with("locale:") || id == d.juggl_id { return skip("chat esclusa"); }
    if str_of(v, "ospite") != "tavolo" { return skip("nel terminale ci pensa Claude Code"); }
    if !str_of(v, "motore").is_empty() && str_of(v, "motore") != "claude" { return skip("non e' una chat di Claude"); }
    let view = str_of(v, "vista");
    if !matches!(view, "ferma" | "fatta") { return skip(format!("stato {view}")); }
    if !str_of(v, "domanda").is_empty() { return skip("ha una domanda per te"); }
    if v.get("in_background").and_then(|x| x.as_array()).map(|a| !a.is_empty()).unwrap_or(false) { return skip("comandi in background"); }
    if f64_of(v, "sotto_attivi") > 0.0 { return skip("sotto-agenti al lavoro"); }
    if d.in_coda > 0 { return skip("messaggi in coda"); }
    if v.get("compattando").map(|x| x.is_object()).unwrap_or(false) { return skip("compattazione gia' in corso"); }
    if d.guardata { return skip("la stai guardando"); }
    if d.adesso - f64_of(v, "ripresa_il") < 15.0 * 60.0 { return skip("appena ripresa"); }
    let Some(u) = d.uso else { return skip("contesto non noto") };
    if u.gia_compattata { return skip("gia' compattata"); }
    if d.adesso - d.ultimo_invio < OUTCOME_WAIT_S { return skip("aspetto l'esito dell'ultima compattazione"); }
    if d.invii_24h >= MAX_24H_PER_CHAT { return skip("troppe compattazioni oggi"); }
    if let Some(p) = d.settimana { if p >= d.imp["settimana_max"].as_f64().unwrap_or(95.0) { return skip(format!("settimana di Claude al {p:.0}%")); } }
    if let Some(p) = d.cinque_ore { if p >= d.imp["cinque_ore_max"].as_f64().unwrap_or(90.0) { return skip(format!("5 ore di Claude al {p:.0}%")); } }
    let window = if u.finestra > 0 { u.finestra } else { 200_000 } as f64;
    let idle_min = (d.adesso - u.t_ultimo) / 60.0;
    // urgent: almost full, turn over (2 quiet minutes), even with a warm cache
    if u.token as f64 >= window * d.imp["urgente_pct"].as_f64().unwrap_or(85.0) / 100.0 {
        return if idle_min >= 2.0 { Decision::Compatta("urgente") } else { skip("quasi piena, aspetto la fine del turno") };
    }
    let threshold = (d.imp["soglia_k"].as_f64().unwrap_or(150.0) * 1000.0).min(window * 0.6);
    if (u.token as f64) < threshold { return skip(format!("contesto {}k sotto la soglia {}k", u.token / 1000, (threshold / 1000.0) as u64)); }
    if !u.ttl_1h { return skip("cache da 5 minuti"); }
    if idle_min < d.imp["ferma_min"].as_f64().unwrap_or(45.0) { return skip("ferma da poco"); }
    if idle_min > d.imp["fredda_min"].as_f64().unwrap_or(57.0) { return skip("cache gia' scaduta: compattare costerebbe quanto riprenderla"); }
    Decision::Compatta("ferma")
}

// ---------- log ----------

pub fn record(mut r: Value) {
    let _g = log_lock().lock().unwrap_or_else(|e| e.into_inner());
    r["t"] = json!(unix_now());
    let p = log_file();
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; o.mode(0o600); }
    if let Ok(mut file) = o.open(&p) { let _ = writeln!(file, "{r}"); }
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 200 == 199 {
        let cutoff = unix_now() - LOG_DAYS * 86400.0;
        let kept: Vec<String> = log_lines().into_iter().filter(|x| f64_of(x, "t") >= cutoff).map(|x| x.to_string()).collect();
        let _ = storage::write(&p, (kept.join("\n") + "\n").as_bytes());
    }
}

fn log_lines() -> Vec<Value> {
    std::fs::read_to_string(log_file()).unwrap_or_default().lines().filter_map(|r| serde_json::from_str(r).ok()).collect()
}

/// Last reason written for each chat: a line is written only when it changes (no log bloating every minute).
fn last_reasons() -> &'static Mutex<HashMap<String, String>> { static U: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new(); U.get_or_init(|| Mutex::new(HashMap::new())) }

// ---------- loop (every minute, from the Sentinel loop) ----------

pub fn tick(ss: &[Value]) {
    static LAST_RUN: Mutex<f64> = Mutex::new(0.0);
    {
        let mut u = LAST_RUN.lock().unwrap_or_else(|e| e.into_inner());
        if unix_now() - *u < EVERY_S { return; }
        *u = unix_now();
    }
    let imp = settings();
    let adesso = unix_now();
    let at_mac = crate::user_at_mac();
    let watched: Vec<String> = {
        let mut g = crate::autopilot::WATCHED_ALL.lock().map(|x| x.clone()).unwrap_or_default();
        if let Ok(x) = crate::autopilot::WATCHED.lock() { if !x.is_empty() { g.push(x.clone()); } }
        g
    };
    let limits = crate::quota::valid_limits();
    let settimana = limits["seven_day"]["used_percentage"].as_f64();
    let cinque_ore = limits["five_hour"]["used_percentage"].as_f64();
    let log = log_lines();
    let juggl = crate::juggl_id();
    for v in ss {
        let id = str_of(v, "sessione").to_string();
        // outcome of a compaction sent by Juggl (even if the chat changed meanwhile)
        check_outcome(&id, v, &log);
        let per_chat = v.get("compattazione").and_then(|x| x.as_bool());
        if !imp["attiva"].as_bool().unwrap_or(false) && per_chat != Some(true) { continue; }   // off: no transcript reads
        let uso = if matches!(str_of(v, "vista"), "ferma" | "fatta") { crate::find_transcript(v).and_then(|t| usage_of(&t)) } else { None };
        let own: Vec<&Value> = log.iter().filter(|r| str_of(r, "sessione") == id && str_of(r, "esito") == "mandata").collect();
        let data = Inputs {
            chat: v, uso: uso.as_ref(), imp: &imp, adesso,
            guardata: at_mac && watched.iter().any(|g| g == &id),
            in_coda: crate::read_queue(&id).len(),
            settimana, cinque_ore,
            invii_24h: own.iter().filter(|r| adesso - f64_of(r, "t") < 86400.0).count(),
            ultimo_invio: own.iter().map(|r| f64_of(r, "t")).fold(0.0, f64::max),
            juggl_id: &juggl,
        };
        let decision = decide(&data);
        let reason = match &decision { Decision::Compatta(m) => format!("compatta: {m}"), Decision::Salta(m) => m.clone() };
        let changed = { let mut u = last_reasons().lock().unwrap_or_else(|e| e.into_inner()); u.insert(id.clone(), reason.clone()).as_deref() != Some(reason.as_str()) };
        let tok = uso.as_ref().map(|u| u.token).unwrap_or(0);
        match decision {
            Decision::Compatta(m) => send_compact(&id, v, m, tok, uso.as_ref().map(|u| (adesso - u.t_ultimo) / 60.0).unwrap_or(0.0)),
            Decision::Salta(m) => if changed {
                record(json!({"sessione": id, "nome": str_of(v, "nome"), "esito": "saltata", "motivo": m, "token": tok}));
            },
        }
    }
}

fn send_compact(id: &str, v: &Value, kind: &str, token: u64, idle_min: f64) {
    let p = base_dir().join("stato").join(format!("{id}.json"));
    // previous state: at end of turn hosted.rs puts it back (no "ti aspetta" for a compaction)
    let ok = crate::update_json(&p, |x| {
        x["compattando"] = json!({"t": unix_now(), "stato": str_of(x, "stato"), "da": f64_of(x, "da"), "letta_da": x.get("letta_da").cloned().unwrap_or(Value::Null),
            "serve_te": x.get("serve_te").cloned().unwrap_or(Value::Null), "token": token});
        true
    });
    if !ok { return; }
    let r = crate::send_internal(id.to_string(), format!("/compact {INSTRUCTIONS}"));
    let sent = r["ok"].as_bool().unwrap_or(false);
    if !sent { crate::update_json(&p, |x| { if let Some(o) = x.as_object_mut() { o.remove("compattando"); } true }); }
    record(json!({"sessione": id, "nome": str_of(v, "nome"), "esito": if sent { "mandata" } else { "non_mandata" }, "motivo": kind,
        "token": token, "ferma_min": (idle_min * 10.0).round() / 10.0, "invio": str_of(&r, "esito"), "msg": str_of(&r, "msg")}));
    crate::log_juggl(&format!("compattazione {kind} di {id}: {}k token, {}", token / 1000, if sent { "mandata" } else { "non mandata" }));
}

/// After a send: when the compaction shows up in the transcript, writes the outcome with the real before/after tokens.
fn check_outcome(id: &str, v: &Value, log: &[Value]) {
    let Some(sent_entry) = log.iter().rev().find(|r| str_of(r, "sessione") == id && matches!(str_of(r, "esito"), "mandata" | "fatta" | "non_riuscita")) else { return };
    if str_of(sent_entry, "esito") != "mandata" { return; }
    let sent_at = f64_of(sent_entry, "t");
    let Some(u) = crate::find_transcript(v).and_then(|t| usage_of(&t)) else { return };
    match &u.ultima_compattazione {
        Some((t, pre, post, trig)) if *t >= sent_at - 5.0 => {
            record(json!({"sessione": id, "nome": str_of(v, "nome"), "esito": "fatta", "prima": pre, "dopo": post, "tolti": pre.saturating_sub(*post),
                "trigger": trig, "secondi": (t - sent_at).round()}));
            crate::helpers::record("compattazione", "ok");
        }
        _ if unix_now() - sent_at > OUTCOME_WAIT_S && !matches!(str_of(v, "vista"), "lavora") => {
            record(json!({"sessione": id, "nome": str_of(v, "nome"), "esito": "non_riuscita", "motivo": "nessuna compattazione nel transcript dopo 15 minuti"}));
            let p = base_dir().join("stato").join(format!("{id}.json"));
            crate::update_json(&p, |x| { if let Some(o) = x.as_object_mut() { o.remove("compattando").is_some() } else { false } });
        }
        _ => {}
    }
}

/// At end of turn (hosted.rs, "result"): if the turn was a Juggl compaction, the chat goes back to how it was.
pub fn end_of_turn(v: &mut Value) {
    let Some(c) = v.get("compattando").filter(|x| x.is_object()).cloned() else { return };
    let st = str_of(&c, "stato");
    v["stato"] = json!(if st.is_empty() || st == "lavora" { "ferma" } else { st });
    if f64_of(&c, "da") > 0.0 { v["da"] = c["da"].clone(); }
    for k in ["letta_da", "serve_te"] {
        if c[k].is_null() { if let Some(o) = v.as_object_mut() { o.remove(k); } } else { v[k] = c[k].clone(); }
    }
    if let Some(o) = v.as_object_mut() { o.remove("compattando"); }
}

/// Sentence for the notice in the chat instead of the raw summary: "Compattata mentre eri via: da 310k a 40k token".
pub fn compacted_notice(session: &str, d: &Value) -> (String, bool) {
    let m = &d["compactMetadata"];
    let k = |n: &str| m[n].as_u64().map(|x| format!("{}k", (x + 500) / 1000));
    let when = ts(str_of(d, "timestamp"));
    let from_juggl = !session.is_empty() && log_lines().iter().any(|r| str_of(r, "sessione") == session && str_of(r, "esito") == "mandata" && (when - f64_of(r, "t")).abs() < OUTCOME_WAIT_S);
    let head = if from_juggl { "Compattata mentre eri via" } else if str_of(m, "trigger") == "auto" { "Chat compattata da Claude Code (contesto pieno)" } else { "Chat compattata" };
    let text = match (k("preTokens"), k("postTokens")) {
        (Some(a), Some(b)) => format!("{head}: da {a} a {b} token"),
        (Some(a), None) => format!("{head}: erano {a} token"),
        _ => head.to_string(),
    };
    (text, from_juggl)
}

/// Summary for Settings > Compaction: today (24 h) and latest log lines.
pub fn status() -> Value {
    let log = log_lines();
    let since = unix_now() - 86400.0;
    let done: Vec<&Value> = log.iter().filter(|r| f64_of(r, "t") >= since && str_of(r, "esito") == "fatta").collect();
    let removed: u64 = done.iter().map(|r| r["tolti"].as_u64().unwrap_or(0)).sum();
    let latest: Vec<Value> = log.iter().rev().filter(|r| str_of(r, "esito") != "saltata").take(20).cloned().collect();
    json!({"impostazioni": settings(), "oggi": done.len(), "tolti_oggi": removed, "ultime": latest})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imp(attiva: bool) -> Value { json!({"attiva": attiva, "ferma_min": 45.0, "fredda_min": 57.0, "soglia_k": 150.0, "urgente_pct": 85.0, "settimana_max": 95.0, "cinque_ore_max": 90.0}) }
    fn chat() -> Value { json!({"sessione": "abc", "ospite": "tavolo", "vista": "ferma", "ripresa_il": 0.0}) }
    fn usage_at(token: u64, idle_min: f64, adesso: f64) -> Usage { Usage { token, finestra: 1_000_000, modello: "claude-opus-5-5".into(), t_ultimo: adesso - idle_min * 60.0, ttl_1h: true, ..Default::default() } }
    fn decide_for(c: &Value, u: Option<&Usage>, i: &Value, adesso: f64) -> Decision {
        decide(&Inputs { chat: c, uso: u, imp: i, adesso, guardata: false, in_coda: 0, settimana: Some(10.0), cinque_ore: Some(10.0), invii_24h: 0, ultimo_invio: 0.0, juggl_id: "juggl" })
    }

    #[test]
    fn compacts_only_idle_big_and_warm() {
        let a = 2_000_000_000.0;
        let (c, i) = (chat(), imp(true));
        assert_eq!(decide_for(&c, Some(&usage_at(300_000, 50.0, a)), &i, a), Decision::Compatta("ferma"));
        assert!(matches!(decide_for(&c, Some(&usage_at(100_000, 50.0, a)), &i, a), Decision::Salta(m) if m.contains("sotto la soglia")));
        assert!(matches!(decide_for(&c, Some(&usage_at(300_000, 20.0, a)), &i, a), Decision::Salta(m) if m == "ferma da poco"));
        assert!(matches!(decide_for(&c, Some(&usage_at(300_000, 70.0, a)), &i, a), Decision::Salta(m) if m.contains("scaduta")));
        // urgent: almost full, even warm, but not in the middle of the turn
        assert_eq!(decide_for(&c, Some(&usage_at(900_000, 3.0, a)), &i, a), Decision::Compatta("urgente"));
        assert!(matches!(decide_for(&c, Some(&usage_at(900_000, 0.5, a)), &i, a), Decision::Salta(_)));
        // 200k window: threshold at 60% (120k)
        let mut u = usage_at(130_000, 50.0, a); u.finestra = 200_000;
        assert_eq!(decide_for(&c, Some(&u), &i, a), Decision::Compatta("ferma"));
    }

    #[test]
    fn never_with_questions_jobs_or_switches_off() {
        let a = 2_000_000_000.0;
        let u = usage_at(300_000, 50.0, a);
        let i = imp(true);
        for (field, val) in [("vista", json!("aspetta")), ("vista", json!("lavora")), ("vista", json!("bloccata")), ("domanda", json!("ok?")),
                             ("in_background", json!([{"id": "b1"}])), ("sotto_attivi", json!(1)), ("ospite", json!("")), ("compattazione", json!(false)),
                             ("ripresa_il", json!(a - 60.0)), ("compattando", json!({"t": a}))] {
            let mut c = chat(); c[field] = val.clone();
            assert!(matches!(decide_for(&c, Some(&u), &i, a), Decision::Salta(_)), "{field}={val}");
        }
        // general switch off: skipped, but the chat turned on by hand gets compacted
        assert!(matches!(decide_for(&chat(), Some(&u), &imp(false), a), Decision::Salta(m) if m.contains("generale")));
        let mut c = chat(); c["compattazione"] = json!(true);
        assert_eq!(decide_for(&c, Some(&u), &imp(false), a), Decision::Compatta("ferma"));
        // already compacted, quota almost used up, being watched
        let mut g = u.clone(); g.gia_compattata = true;
        assert!(matches!(decide_for(&chat(), Some(&g), &i, a), Decision::Salta(_)));
        let d = Inputs { chat: &chat(), uso: Some(&u), imp: &i, adesso: a, guardata: false, in_coda: 0, settimana: Some(97.0), cinque_ore: None, invii_24h: 0, ultimo_invio: 0.0, juggl_id: "" };
        assert!(matches!(decide(&d), Decision::Salta(m) if m.contains("settimana")));
        let d = Inputs { guardata: true, settimana: None, ..d };
        assert_eq!(decide(&d), Decision::Salta("la stai guardando".into()));
        let d = Inputs { guardata: false, in_coda: 1, ..d };
        assert_eq!(decide(&d), Decision::Salta("messaggi in coda".into()));
    }

    #[test]
    fn usage_from_transcript() {
        let r1 = json!({"type": "assistant", "timestamp": "2026-11-16T10:00:00.000Z", "message": {"model": "claude-opus-5-5", "usage": {"input_tokens": 10, "cache_read_input_tokens": 300_000, "cache_creation_input_tokens": 2000, "cache_creation": {"ephemeral_1h_input_tokens": 2000, "ephemeral_5m_input_tokens": 0}}}}).to_string();
        let u = usage_from_lines([r1.as_str()].into_iter()).unwrap();
        assert_eq!((u.token, u.finestra, u.ttl_1h, u.gia_compattata), (302_010, 1_000_000, true, false));
        assert_eq!(u.t_ultimo, 1794823200.0);
        let cb = json!({"type": "system", "subtype": "compact_boundary", "timestamp": "2026-11-16T10:50:00.000Z", "compactMetadata": {"trigger": "manual", "preTokens": 302010, "postTokens": 15000}}).to_string();
        let u = usage_from_lines([r1.as_str(), cb.as_str()].into_iter()).unwrap();
        assert!(u.gia_compattata);
        assert_eq!(u.ultima_compattazione, Some((1794826200.0, 302010, 15000, "manual".into())));
    }

    #[test]
    fn end_of_turn_restores_state() {
        let mut v = json!({"stato": "aspetta", "da": 999.0, "compattando": {"t": 1.0, "stato": "ferma", "da": 500.0, "letta_da": 500.0, "serve_te": null}, "serve_te": true});
        end_of_turn(&mut v);
        assert_eq!(v, json!({"stato": "ferma", "da": 500.0, "letta_da": 500.0}));
        let mut w = json!({"stato": "aspetta"});
        end_of_turn(&mut w);
        assert_eq!(w, json!({"stato": "aspetta"}));
    }

    #[test]
    fn chat_shows_the_line_not_the_summary() {
        let d = std::env::temp_dir().join(format!("juggl-compatta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap();
        let t = d.join("abc.jsonl");
        let lines = [
            json!({"type": "user", "timestamp": "2026-11-16T10:00:00.000Z", "message": {"role": "user", "content": "ciao"}}),
            json!({"type": "assistant", "timestamp": "2026-11-16T10:00:05.000Z", "message": {"role": "assistant", "model": "claude-opus-5-5", "content": [{"type": "text", "text": "fatto"}], "usage": {"input_tokens": 1, "cache_read_input_tokens": 300000}}}),
            json!({"type": "system", "subtype": "compact_boundary", "timestamp": "2026-11-16T10:50:00.000Z", "content": "Conversation compacted", "compactMetadata": {"trigger": "manual", "preTokens": 300001, "postTokens": 15000}}),
            json!({"type": "user", "isCompactSummary": true, "timestamp": "2026-11-16T10:50:01.000Z", "message": {"role": "user", "content": "This session is being continued from a previous conversation. Summary: ..."}}),
        ];
        std::fs::write(&t, lines.iter().map(|r| r.to_string()).collect::<Vec<_>>().join("\n") + "\n").unwrap();
        let v = crate::detail_tail(Some(t), false, 1_000_000, "");
        let ev = v["eventi"].as_array().unwrap();
        assert!(ev.iter().any(|e| e["tipo"] == "compattata" && str_of(e, "testo").contains("da 300k a 15k")), "{ev:?}");
        assert!(!ev.iter().any(|e| str_of(e, "testo").contains("being continued")), "{ev:?}");
        assert_eq!(ev.iter().filter(|e| e["tipo"] == "tu").count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// "/" commands (Nov 17): a /compact rejected by Claude Code (real lines taken from a test with a clean user)
    /// shows up in the chat; a /model sent by Juggl with a normal outcome does not.
    #[test]
    fn rejected_compact_is_not_silent() {
        let d = std::env::temp_dir().join(format!("juggl-compatta-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap();
        let t = d.join("abc.jsonl");
        let lines = [
            json!({"type": "user", "timestamp": "2026-11-17T10:00:00.000Z", "message": {"role": "user", "content": "<command-name>/compact</command-name>\n            <command-message>compact</command-message>\n            <command-args></command-args>"}}),
            json!({"type": "system", "subtype": "local_command", "timestamp": "2026-11-17T10:00:01.000Z", "content": "<local-command-stdout>Not enough messages to compact.</local-command-stdout>"}),
            json!({"type": "user", "timestamp": "2026-11-17T10:01:00.000Z", "message": {"role": "user", "content": "<command-name>/model</command-name>"}}),
            json!({"type": "system", "subtype": "local_command", "timestamp": "2026-11-17T10:01:01.000Z", "content": "<local-command-stdout>Set model to Sonnet</local-command-stdout>"}),
            json!({"type": "system", "subtype": "local_command", "timestamp": "2026-11-17T10:02:01.000Z", "content": "<local-command-stderr>Error: No messages to compact</local-command-stderr>"}),
        ];
        std::fs::write(&t, lines.iter().map(|r| r.to_string()).collect::<Vec<_>>().join("\n") + "\n").unwrap();
        let v = crate::detail_tail(Some(t), false, 1_000_000, "");
        let notes: Vec<&str> = v["eventi"].as_array().unwrap().iter().filter(|e| e["tipo"] == "nota_comando").map(|e| str_of(e, "testo")).collect();
        assert_eq!(notes, vec!["Niente da compattare: la chat è ancora troppo corta.", "Niente da compattare: la chat è ancora troppo corta."]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn notice_sentence() {
        let d = json!({"timestamp": "2026-11-16T10:50:00.000Z", "compactMetadata": {"trigger": "auto", "preTokens": 967_815, "postTokens": 19_630}});
        assert_eq!(compacted_notice("", &d), ("Chat compattata da Claude Code (contesto pieno): da 968k a 20k token".to_string(), false));
    }
}
