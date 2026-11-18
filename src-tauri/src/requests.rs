// The user's requests, understood by a model.
// The user writes many things at once, even while the agents are working, and then loses the answers or finds out something was not done.
// Each of their messages goes through a small model (haiku) with the session's open requests and the open titles of the other
// sessions (last 48 hours): out come new requests, repetitions (also across sessions: the new session goes into "anche_in") or nothing.
// At the end of a turn a more careful model (sonnet) reads the whole turn, the steps and the open requests and decides the state:
//   in_lavoro, fatta (with the answer, which stays "new" until the user opens the chat), serve_te (only real needs).
// Requests do not get lost on their own: an ignored request (non_presa) is sent back automatically to the session, once;
// if it is ignored again it becomes serve_te. One in progress with no news for 12 hours gets an "A che punto sei?", once;
// if it stays stuck another 12 hours it becomes serve_te.
// Registry: ~/tavolo/richieste-ai.json, saved atomically and under a single lock.
use crate::{base_dir, f64_of, local_models, unix_now, autopilot, str_of, write_state};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const STATES: [&str; 4] = ["non_presa", "in_lavoro", "serve_te", "fatta"];
const VERSION: u64 = 4;
const HOUR_S: f64 = 3600.0;
const DAY: f64 = 86400.0;
/// In progress with no news for more than 12 hours: stalled, and gets an "A che punto sei?".
const STALL_AFTER: f64 = 12.0 * HOUR_S;
/// Done items stay 14 days, then they are deleted.
const KEEP_DONE: f64 = 14.0 * DAY;
/// Duplicates: compared with the open requests of the other sessions in these last hours.
const DUPLICATES_WITHIN: f64 = 48.0 * HOUR_S;
/// A message already seen in this window (e.g. sent from the app and then re-read from the session state) is not re-analyzed.
const SEEN_FOR_S: f64 = 1800.0;
/// Between two model calls for the same session.
const PAUSE_S: f64 = 5.0;   // pause between any two jobs of the same chat; for extraction the settings pause applies (helpers.rs, 150 s)
/// Model for the states (more careful than the small one that extracts).
const STATE_MODEL: Option<&str> = Some("sonnet");
pub const ASK_PREFIX: &str = "A che punto sei con:";
pub const REMIND_PREFIX: &str = "Te lo avevo chiesto e non l'hai preso in carico: ";
const REMINDER_PREFIX: [&str; 2] = ["Mi era rimasta indietro questa richiesta di «NOME»", "Mi erano rimaste indietro queste richieste di «NOME»"];

static REG: Mutex<()> = Mutex::new(());

/// JUGGL_RICHIESTE_FILE: registry elsewhere (only for tests on real data, without touching the app's one).
fn registry_path() -> PathBuf { std::env::var("JUGGL_RICHIESTE_FILE").map(PathBuf::from).unwrap_or_else(|_| base_dir().join("richieste-ai.json")) }

fn normalize(mut r: Value) -> Value {
    if !r.is_object() { r = json!({}); }
    if !r["voci"].is_array() { r["voci"] = json!([]); }
    if !r["visti"].is_object() { r["visti"] = json!({}); }
    if !r["popolate"].is_array() { r["popolate"] = json!([]); }
    if !r["unite"].is_object() { r["unite"] = json!({}); }
    r
}

/// States of the earlier versions (nuova, in_corso, ferma, fatta; then da_controllare, chiusa) in the current model.
fn migrate(v: &mut Value, now: f64) {
    let old = str_of(v, "stato").to_string();
    let updated = f64_of(v, "aggiornata");
    let by_hand = v["chiusa_a_mano"].as_bool().unwrap_or(false) || str_of(v, "manuale") == "chiusa";
    let (state, stalled) = match old.as_str() {
        "ferma" => ("in_lavoro", true),
        "fatta" | "da_controllare" | "chiusa" => ("fatta", false),
        "serve_te" => ("serve_te", false),
        "non_presa" => ("non_presa", false),
        _ => ("in_lavoro", v["ferma"] == json!(true)),
    };
    if v.get("stato_v1").is_none() { v["stato_v1"] = json!(old); }
    v["stato"] = json!(state);
    v["ferma"] = json!(stalled);
    if str_of(v, "manuale") == "chiusa" { v["manuale"] = json!(""); }
    if state == "fatta" {
        let at = f64_of(v, "chiusa_il");
        v["fatta_il"] = json!(if at > 0.0 { at } else { updated });
        // answers already old or closed by hand: considered read; recent ones stay new
        v["letta"] = json!(old == "chiusa" || by_hand || now - updated > 6.0 * HOUR_S);
    }
    v["da_rivalutare"] = json!(state != "fatta" || now - updated < DAY);
}

/// Time rules: migration, stalled after 12 hours, serve te after the unanswered nudge, cleanup of done items.
fn apply_rules(r: &mut Value) {
    let now = unix_now();
    let version = r["versione"].as_u64().unwrap_or(1);
    let needs_migration = version < VERSION;
    if let Some(a) = r["voci"].as_array_mut() {
        for v in a.iter_mut() {
            if version < 3 { migrate(v, now); }
            // v4: new rules (linked oks, done when it answered, you do it): open ones and today's done ones are re-evaluated
            else if needs_migration && (is_open(v) || now - done_at(v) < DAY) && v["scartata"] != json!(true) { v["da_rivalutare"] = json!(true); }
            if !v["anche_in"].is_array() { v["anche_in"] = json!([]); }
            if !v["messaggi"].is_array() {
                let t = str_of(v, "testo").to_string();
                v["messaggi"] = if t.trim().is_empty() { json!([]) } else { json!([msg_key(str_of(v, "sessione"), &t)]) };
            }
            if str_of(v, "stato") == "in_lavoro" {
                if now - f64_of(v, "aggiornata") > STALL_AFTER && v["ferma"] != json!(true) { v["ferma"] = json!(true); }
                // nudged and still no news after another 12 hours: the work is HANGING. Before it moved to "serve te" and
                // "Scrivi la risposta" appeared on items that were not questions to the user (0.1.79: stays in progress, kind "appesa")
                let sol = f64_of(v, "sollecitata_t");
                if sol > 0.0 && f64_of(v, "aggiornata") < sol && now - sol > STALL_AFTER && v["appesa"] != json!(true) {
                    v["appesa"] = json!(true); v["ferma"] = json!(true); v["motivo_appesa"] = json!("ferma da piu' di un giorno, non risponde al sollecito");
                }
            } else if v["ferma"] == json!(true) { v["ferma"] = json!(false); }
            // requests born from chat-to-chat or automatic messages (not written by the user): closed by Juggl, reopenable (0.1.79)
            // (never those with a linked ok or "you do it": they are the user's own; and if they had already been closed, they reopen on their own)
            let users_own = !str_of(v, "ok_id").is_empty() || v["fai_tu"] == json!(true);
            if is_open(v) && !users_own && v["no_auto"] != json!(true) && v["scartata"] != json!(true) && v.get("chiusa_da_juggl").is_none() && !str_of(v, "testo").is_empty() && is_automatic(str_of(v, "testo")) {
                close_by_juggl(v, "nata da un messaggio tra chat o automatico, non da una tua richiesta", now);
            } else if users_own && str_of(v, "stato") == "fatta" && str_of(&v["chiusa_da_juggl"], "motivo").starts_with("nata da un messaggio tra chat") {
                undo_juggl(v);
            }
            // those already promoted to "serve te" by the system (not by a real question) go back to hanging work, reopenable by hand
            if str_of(v, "stato") == "serve_te" && str_of(v, "ok_id").is_empty() && v["fai_tu"] != json!(true) && promoted_by_system(str_of(v, "motivo")) {
                v["motivo_appesa"] = json!(str_of(v, "motivo")); v["motivo"] = json!(""); v["appesa"] = json!(true); v["ferma"] = json!(true);
                v["stato"] = json!(if str_of(v, "motivo_appesa").starts_with("ignorata") { "non_presa" } else { "in_lavoro" });
                v["promossa_dal_sistema"] = json!(true);
            }
        }
        a.retain(|v| !(str_of(v, "stato") == "fatta" && now - done_at(v) > KEEP_DONE && v.get("chiusa_da_juggl").is_none()));   // list (15/11): those closed by Juggl stay (they can be reopened)
    }
    if needs_migration {
        r["versione"] = json!(VERSION);
        // automatic nudges start only after the old data has been re-evaluated
        r["auto_da"] = json!(now + HOUR_S);
    }
}

/// The reasons with which the system (not the model, not the user) set "serve te": they are not real questions.
fn promoted_by_system(reason: &str) -> bool { reason.starts_with("ferma da piu' di un giorno") || reason.starts_with("ignorata due volte") }

fn done_at(v: &Value) -> f64 { let t = f64_of(v, "fatta_il"); if t > 0.0 { t } else { f64_of(v, "aggiornata") } }

/// Reads, modifies and saves the registry while holding the lock: no writes stepping on each other.
fn with_registry<R>(fun: impl FnOnce(&mut Value) -> R) -> R {
    let _g = REG.lock().unwrap_or_else(|e| e.into_inner());
    let p = registry_path();
    // file lock: richieste-ai.json is also touched by scripts (storage.rs protocol)
    let _l = crate::storage::lock(&p).map_err(|e| crate::log_juggl(&format!("richieste: {e}"))).ok();
    let (loaded, broken) = match crate::storage::read_json(&p) {
        crate::storage::ReadResult::Valida(v) => (v, false),
        crate::storage::ReadResult::Assente => (json!({}), false),
        crate::storage::ReadResult::Rotta => (json!({}), true),
    };
    let mut r = normalize(loaded);
    apply_rules(&mut r);
    let out = fun(&mut r);
    apply_rules(&mut r);
    let now = unix_now();
    if let Some(o) = r["visti"].as_object_mut() { o.retain(|_, t| now - t.as_f64().unwrap_or(0.0) < 2.0 * DAY); }
    // unreadable file: do not rewrite it from scratch (all requests would be lost)
    if broken { crate::log_juggl("richieste-ai.json illeggibile: non lo riscrivo"); } else { write_state(&p, &r); }
    out
}

fn registry() -> Value {
    let _g = REG.lock().unwrap_or_else(|e| e.into_inner());
    // fast chats (14/11): 3.8 MB re-read at every chat opening and every list: now once per file version
    let mut r = normalize(crate::fast_open::json(&registry_path()).map(|a| (*a).clone()).unwrap_or(json!({})));
    apply_rules(&mut r);
    r
}

fn all_items() -> Vec<Value> { registry()["voci"].as_array().cloned().unwrap_or_default() }

fn notify() { if let Some(a) = crate::APP.get() { let _ = a.emit("cambiato", ()); } }

fn key(sid: &str, text: &str) -> String { key_n(sid, text, 80) }
fn key_n(sid: &str, text: &str, n: usize) -> String {
    format!("{sid}:{}", text.chars().filter(|c| !c.is_whitespace()).take(n).collect::<String>())
}

/// Key of the message a request comes from: session + the first 60 characters without spaces.
/// The interface recomputes it from the bubble text (same rule) to put the labels under the right message.
fn msg_key(sid: &str, text: &str) -> String { key_n(sid, text.trim().trim_end_matches('…'), 60) }

fn new_id(r: &mut Value) -> String {
    let n = r["n"].as_u64().unwrap_or(0) + 1;
    r["n"] = json!(n);
    format!("r{n}")
}

/// Id of an item, following merges (a merged duplicate points to the item that stays).
fn resolve(r: &Value, id: &str) -> String {
    let mut id = id.to_string();
    for _ in 0..8 { match r["unite"].get(&id).and_then(|x| x.as_str()) { Some(n) => id = n.to_string(), None => break } }
    id
}
fn item_mut<'a>(r: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    let id = resolve(r, id);
    r["voci"].as_array_mut()?.iter_mut().find(|v| str_of(v, "id") == id)
}

fn also_in(v: &Value) -> Vec<String> {
    v["anche_in"].as_array().map(|a| a.iter().map(|x| str_of(x, "sessione").to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default()
}
/// The request concerns this session (it is its own or it arrived there too).
fn concerns(v: &Value, sid: &str) -> bool { str_of(v, "sessione") == sid || also_in(v).iter().any(|x| x == sid) }
fn is_open(v: &Value) -> bool { str_of(v, "stato") != "fatta" }
/// Answer the user has not seen yet.
fn new_reply(v: &Value) -> bool { str_of(v, "stato") == "fatta" && v["letta"] != json!(true) && v["scartata"] != json!(true) }

fn add_session(v: &mut Value, sid: &str, name: &str) {
    if concerns(v, sid) { return; }
    if let Some(a) = v["anche_in"].as_array_mut() { a.push(json!({"sessione": sid, "nome": name})); }
}
fn add_msg(v: &mut Value, k: &str) {
    if k.ends_with(':') { return; }
    let Some(a) = v["messaggi"].as_array_mut() else { return };
    if !a.iter().any(|x| x.as_str() == Some(k)) { a.push(json!(k)); }
    let n = a.len(); if n > 12 { a.drain(0..n - 12); }
}

/// User messages in the conversation (not system notices): they mark the start of a turn.
fn is_user_msg(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && !t.starts_with('<') && !t.starts_with("This session") && !t.starts_with("Caveat") && !t.starts_with("[Request interrupted")
}
/// Automatic messages from Juggl and the coordinator that are not the user's requests (0.1.79): cause of many "asked N times" requests.
/// Messages carrying an order from the user stay requests ("Da l'utente (via regia)", "[Da l'utente, via Juggl]", "[Da Juggl ... l'utente ...]").
fn is_automatic(t: &str) -> bool {
    let t = t.trim();
    if t.starts_with("Messaggio automatico di Juggl") || t.starts_with("[Da Juggl] Juggl e' stato riavviato") || t.starts_with("[Da Juggl] Juggl è stato riavviato") { return true; }
    // chat-to-chat messages (juggl manda): signed with the chat name and its short id, or from the coordinator, without being the user's orders
    if t.starts_with("Regia:") || t.starts_with("Regia,") { return true; }
    let head100: String = t.chars().take(100).collect();
    if let Some(i) = head100.find('(') { let rest = &head100[i + 1..]; if rest.len() > 9 && rest.as_bytes()[8] == b')' && rest.as_bytes()[..8].iter().all(|c| c.is_ascii_hexdigit()) && rest.as_bytes()[..8].iter().any(|c| c.is_ascii_digit()) { return true; } }
    if t.starts_with("[Da Juggl") {
        // from the coordinator or other chats: a request only if there is an order from the user in the first words
        let head: String = t.chars().take(120).collect::<String>().to_lowercase();
        let name = crate::profile::with_name("«NOME»").to_lowercase();   // the Juggl user's name, never written in the program
        return !(head.contains(&name) || head.contains("ordine") || head.contains("richiesta di"));
    }
    false
}

/// Messages really written by the user (not Juggl's questions or reminders).
fn from_user(t: &str) -> bool {
    let t = t.trim();
    is_user_msg(t) && !is_automatic(t) && !t.starts_with(ASK_PREFIX) && !t.starts_with(REMIND_PREFIX) && !REMINDER_PREFIX.iter().any(|p| t.starts_with(&crate::profile::with_name(p)))
}

/// One-word replies that ask nothing: no point bothering the model.
fn just_confirmation(t: &str) -> bool {
    let p: String = t.trim().to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '\'').collect();
    let p = p.trim();
    p.chars().count() <= 12 && ["ok", "okay", "si", "sì", "vai", "procedi", "grazie", "perfetto", "bene", "ottimo", "vedi tu", "fai tu", "certo", "va bene", "no", "continua", "top", "ciao", "esatto", "yes"].contains(&p)
}

/// Marks a message as already handled (sent by Juggl or the autopilot: it is not a request from the user).
pub fn mark_handled_text(sid: &str, text: &str) {
    if sid.starts_with("locale:") { return; }
    let k = key(sid, text);
    with_registry(|r| { r["visti"][k] = json!(unix_now()); });
}

fn seen_recently(sid: &str, text: &str) -> bool {
    if sid.starts_with("locale:") { return false; }
    let k = key(sid, text);
    registry()["visti"].get(&k).and_then(|x| x.as_f64()).map(|t| unix_now() - t < SEEN_FOR_S).unwrap_or(false)
}

// ---------- per-session job queue: one call at a time, at most one every 5 s ----------

enum Job { Estrai { testi: Vec<String>, quando: f64, completa: bool }, Aggiorna, Primo, Rivaluta }

#[derive(Default)]
struct Queues { occupate: HashSet<String>, code: HashMap<String, VecDeque<Job>>, ultimo: HashMap<String, f64> }

fn queues() -> &'static Mutex<Queues> { static C: OnceLock<Mutex<Queues>> = OnceLock::new(); C.get_or_init(|| Mutex::new(Queues::default())) }

fn enqueue(sid: &str, job: Job) {
    // local PC chats: their texts NEVER go to a cloud model (the user's rule, 10/11)
    if sid.starts_with("locale:") { return; }
    let mut c = queues().lock().unwrap_or_else(|e| e.into_inner());
    // an update already queued is enough: it reads the last turn anyway
    let q = c.code.entry(sid.to_string()).or_default();
    if matches!(job, Job::Aggiorna) && q.iter().any(|x| matches!(x, Job::Aggiorna)) { return; }
    q.push_back(job);
    if c.occupate.insert(sid.to_string()) {
        let id = sid.to_string();
        std::thread::spawn(move || worker(id));
    }
}

fn worker(sid: String) {
    loop {
        let (job, wait) = {
            let mut c = queues().lock().unwrap_or_else(|e| e.into_inner());
            let next = c.code.get_mut(&sid).and_then(|q| q.pop_front());
            let Some(l) = next else { c.occupate.remove(&sid); c.code.remove(&sid); return };
            let pause = if matches!(l, Job::Estrai { .. } | Job::Aggiorna) { crate::helpers::extractor_pause() } else { PAUSE_S };
            let wait = c.ultimo.get(&sid).map(|t| pause - (unix_now() - t)).unwrap_or(0.0);
            (l, wait)
        };
        if wait > 0.0 { std::thread::sleep(Duration::from_secs_f64(wait)); }
        // messages that arrived in the meantime go together in a single call
        let job = match job {
            Job::Estrai { testi: mut texts, quando: mut when, completa: mut complete } => {
                let mut c = queues().lock().unwrap_or_else(|e| e.into_inner());
                if let Some(q) = c.code.get_mut(&sid) {
                    while let Some(Job::Estrai { .. }) = q.front() {
                        if let Some(Job::Estrai { testi: t, quando: qu, completa: co }) = q.pop_front() { texts.extend(t); when = when.min(qu); complete = complete && co; }
                    }
                }
                // the same message arrived from two routes (app and session state): only once, the longest text
                let mut unique: Vec<String> = vec![];
                for x in texts {
                    let k = key_n(&sid, x.trim_end_matches('…'), 60);
                    match unique.iter().position(|u| key_n(&sid, u, 60) == k) {
                        Some(i) => if x.chars().count() > unique[i].chars().count() { unique[i] = x; },
                        None => unique.push(x),
                    }
                }
                Job::Estrai { testi: unique, quando: when, completa: complete }
            }
            x => x,
        };
        let id = sid.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
            Job::Estrai { testi: texts, quando: when, completa: complete } => extract(&id, texts, when, complete),
            Job::Aggiorna => update(&id),
            Job::Primo => first_run(&id),
            Job::Rivaluta => reevaluate(&id),
        }));
        queues().lock().unwrap_or_else(|e| e.into_inner()).ultimo.insert(sid.clone(), unix_now());
    }
}

// ---------- session data ----------

fn live_session(sid: &str) -> Option<Value> {
    if sid.starts_with("locale:") { return local_models::as_sessions().into_iter().find(|v| str_of(v, "sessione") == sid); }
    crate::find_session(sid)
}

fn detail_of(sid: &str, v: &Value) -> Value {
    match sid.strip_prefix("locale:") { Some(l) => local_models::detail(l), None => crate::session_detail_pub(v) }
}
fn events_of(sid: &str, v: &Value) -> Vec<Value> { detail_of(sid, v)["eventi"].as_array().cloned().unwrap_or_default() }

fn ev_time(e: &Value) -> f64 { from_iso(str_of(e, "t")) }
fn is_user_event(e: &Value) -> bool { str_of(e, "tipo") == "tu" && is_user_msg(str_of(e, "testo")) }

/// Where the turn to judge starts: after the user's last message, or earlier if there is a request
/// waiting for its first turn (messages sent in a row: everything is read from the request's one onwards).
fn turn_start(ev: &[Value], since_t: f64) -> usize {
    let last = ev.iter().rposition(is_user_event).map(|i| i + 1).unwrap_or(0);
    if since_t <= 0.0 || unix_now() - since_t > 6.0 * HOUR_S { return last; }
    ev.iter().position(|e| is_user_event(e) && ev_time(e) >= since_t).map(|i| (i + 1).min(last)).unwrap_or(last)
}

/// The request's message reached the agent and it has already answered at least once afterwards.
fn judgeable(ev: &[Value], waiting_since: f64) -> bool {
    let Some(j) = ev.iter().position(|e| is_user_event(e) && ev_time(e) >= waiting_since) else { return false };
    ev[j + 1..].iter().any(|e| str_of(e, "tipo") == "agente" && !str_of(e, "testo").trim().is_empty())
}

/// The reminder sent by Juggl reached the session and it has already written something afterwards.
fn reminder_read(ev: &[Value], since: f64) -> bool {
    let Some(j) = ev.iter().position(|e| str_of(e, "tipo") == "tu" && ev_time(e) >= since - 10.0 && {
        let t = str_of(e, "testo").trim(); t.starts_with(REMIND_PREFIX) || REMINDER_PREFIX.iter().any(|p| t.starts_with(&crate::profile::with_name(p)))
    }) else { return false };
    ev[j + 1..].iter().any(|e| str_of(e, "tipo") == "agente" && !str_of(e, "testo").trim().is_empty())
}

fn head_and_tail(t: &str, a: usize, b: usize) -> String {
    let n = t.chars().count();
    if n <= a + b { return t.to_string(); }
    format!("{}\n[...]\n{}", t.chars().take(a).collect::<String>(), t.chars().skip(n - b).collect::<String>())
}

/// Everything the agent wrote from `start` onwards, and the tools it used.
fn turn_from(ev: &[Value], start: usize) -> (String, String) {
    let ev = &ev[start.min(ev.len())..];
    let text = ev.iter().filter(|e| str_of(e, "tipo") == "agente").map(|e| str_of(e, "testo")).collect::<Vec<_>>().join("\n\n");
    let st: Vec<String> = ev.iter().filter(|e| str_of(e, "tipo") == "strumento").map(|e| format!("- {}: {}", str_of(e, "nome"), short_text(str_of(e, "testo"), 110))).collect();
    (head_and_tail(&text, 2500, 9000), st[st.len().saturating_sub(40)..].join("\n"))
}

fn steps_of(d: &Value) -> String {
    d["passi"].as_array().cloned().unwrap_or_default().iter().map(|p| format!("- [{}] {}", str_of(p, "stato"), str_of(p, "testo"))).collect::<Vec<_>>().join("\n")
}

/// Unix time from an ISO date "2026-11-07T10:20:30.123Z".
pub(crate) fn from_iso(t: &str) -> f64 {
    let b = t.as_bytes();
    if b.len() < 19 { return 0.0; }
    let n = |a: usize, z: usize| t.get(a..z).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    let (y, m, d) = (n(0, 4), n(5, 7), n(8, 10));
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400); let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1; let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let fraction = t.get(19..).and_then(|x| x.strip_prefix('.')).map(|x| x.trim_end_matches('Z')).and_then(|x| format!("0.{x}").parse::<f64>().ok()).unwrap_or(0.0);
    (days * 86400 + n(11, 13) * 3600 + n(14, 16) * 60 + n(17, 19)) as f64 + fraction
}

fn short_text(t: &str, n: usize) -> String {
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= n { t } else { format!("{}…", t.chars().take(n - 1).collect::<String>()) }
}

fn clean_title(t: &str) -> String {
    let t = t.replace(['\u{2014}', '\u{2013}'], ",").trim().trim_end_matches('.').to_string();
    t.split_whitespace().take(12).collect::<Vec<_>>().join(" ")
}

fn clean_sentence(t: &str) -> String { short_text(&t.replace(['\u{2014}', '\u{2013}'], ","), 260) }

const PREAMBLE: &str = "Sei l'assistente di «NOME». «NOME» lavora con molte sessioni di agenti AI di programmazione insieme, scrive tante richieste \
anche in un solo messaggio e mentre gli agenti lavorano, poi le dimentica e teme che si perdano. Tieni il registro delle sue richieste: cose che vuole fatte, controllate, decise o spiegate. \
Scrivi in italiano semplice, senza trattini lunghi e senza emoji.";

const TITLE_RULES: &str = "Titolo: massimo 10 parole, concreto, dice cosa vuole «NOME» come cosa da fare \
(es. \"Rifare il logo con il tulipano\", \"Capire perche' le email non partono\", \"Pubblicare il sito di prova\"). Niente nomi di sessione nel titolo.";

const STATE_RULES: &str = "Stati possibili:\n\
- \"fatta\": l'agente dice di averla finita (fatta, corretta, provata, pubblicata, installata, inviata) oppure, se era una domanda, ha dato la risposta. \
Vale anche se l'agente aggiunge \"fammi sapere\", propone extra facoltativi o dice a «NOME» di dare un'occhiata al risultato. \
Se la richiesta era studiare, capire, verificare o spiegare, e' fatta quando l'agente ha dato lo studio o la risposta, anche se chiude chiedendo a «NOME» delle decisioni: quelle sono ok a parte. \
E' fatta anche quando «NOME» stesso scrive che quella richiesta ha gia' avuto risposta o e' fatta.\n\
- \"in_lavoro\": ci sta ancora lavorando, l'ha fatta solo in parte, ha avviato qualcosa che deve ancora finire (build, deploy, sotto-agente in background), oppure dice \"sto facendo\", \"ora passo a\", \"prossimo passo\".\n\
- \"serve_te\": SOLO se senza «NOME» il lavoro resta fermo: una scelta che spetta a lui, un'approvazione prima di un'azione verso clienti, soldi o produzione, un'informazione che solo lui ha, un permesso, un'azione che solo lui puo' fare. \
«NOME» non vuole fare verifiche al posto degli agenti: se il lavoro e' finito e lui deve solo guardarlo, e' fatta. \
Una domanda gia' fatta e a cui l'agente ha poi risposto non e' serve_te. Se aspetta un'altra persona (un cliente, un collega, Apple) e' in_lavoro, non serve_te.\n\
- \"fai_tu\": true solo con serve_te, quando «NOME» deve fare qualcosa con le sue mani (installare o provare una build, generare un token, entrare in un sito); false se deve rispondere o decidere.\n\
- \"ok\": se c'e' l'elenco degli ok che la sessione ha gia' chiesto a «NOME» e la richiesta aspetta solo uno di quelli (decisione, approvazione, prova o installazione sua), metti qui il suo id (es. \"k41\") e lo stato \"in_lavoro\", MAI serve_te: «NOME» lo vede gia' tra gli ok, non va ripetuto. Altrimenti \"\".\n\
Esempi:\n\
1. Richiesta \"Verificare il vocale di Alberto e aggiornare Hub\". L'agente: \"Vocale trascritto: niente richieste ne' date, Hub invariato.\" Stato: fatta (ha controllato e risposto).\n\
2. Richiesta \"Capire la causa dello scroll bloccato e risolverla\". L'agente: \"La causa era il ricalcolo dell'altezza a ogni messaggio. Corretto, installato e provato su una chat lunga.\" Stato: fatta.\n\
3. Richiesta \"Stampare il portaplettro v5\". L'agente: \"La v5 non tiene il plettro. Sto preparando la v7 con un fermo, prossimo passo la stampa.\" Stato: in_lavoro.\n\
4. Richiesta \"Scegliere la copertina del cliente\". L'agente: \"Ho preparato tre copertine nella griglia: dimmi quale preferisci.\" Stato: serve_te (la scelta spetta a «NOME»).\n\
5. Richiesta \"Far funzionare l'app iPhone fuori dallo studio\". L'agente: \"Serve Tailscale sull'iPhone: va installato da te dall'App Store.\" Stato: serve_te (solo lui puo' farlo).\n\
6. Richiesta \"Quanti iscritti ci sono stati ieri?\". L'agente: \"Ieri 14 iscritti, 3 sul piano Pro.\" Stato: fatta (domanda con risposta).\n\
7. Richiesta \"Pubblicare il sito di prova\". L'agente: \"Ho lanciato il deploy, ci vogliono un paio di minuti.\" Stato: in_lavoro.\n\
8. Richiesta \"Rifare il logo\". L'agente: \"Fatto e provato a 16 px. Se vuoi posso preparare anche la versione scura.\" Stato: fatta (la versione scura e' un extra facoltativo).\n\
9. Richiesta \"Animare i messaggi\". L'agente: \"Animazione installata, ma non l'ho vista in movimento: guardala tu e dimmi se va.\" Stato: fatta (il lavoro e' finito).\n\
10. Richiesta \"Passare il lavoro delle box alla sessione del sito\". L'agente: \"Passato alla sessione Sito, che ci sta lavorando.\" Stato: in_lavoro (il lavoro vero non e' ancora finito).\n\
11. Richiesta \"Mandare il messaggio al cliente\". L'agente: \"Ho preparato il testo, lo mando solo con il tuo ok.\" Stato: serve_te (serve l'approvazione prima di scrivere a terzi). Se tra gli ok c'e' gia' \"k12 Mandare il messaggio al cliente\": in_lavoro, ok \"k12\".\n\
12. Richiesta \"Verificare se si puo' fermare un agente\". L'agente: \"Si': mentre la sessione lavora il tasto invia diventa un quadrato e la ferma.\" Stato: fatta (ha risposto), anche se prima l'aveva ignorata.\n\
13. Richiesta \"Studiare un piano di lancio\". L'agente: \"Piano pronto nel documento. Servono 4 decisioni tue.\" Stato: fatta (lo studio c'e'; le decisioni sono un ok a parte).\n\
14. Richiesta \"Far installare l'app a «NOME»\". L'agente: \"Build 123 su TestFlight: installala tu.\" Ok gia' chiesti: \"k43 Provare la build 123\". Stato: in_lavoro, ok \"k43\". Senza quell'ok: serve_te con fai_tu true.\n\
Nel dubbio tra fatta e in_lavoro: se l'agente dice di aver finito, fatta; se dice che manca qualcosa o che deve ancora verificare, in_lavoro.";

const REPLY_RULES: &str = "\"risposta\": una frase breve (massimo 20 parole) con l'ultima novita' per questa richiesta: cosa e' stato fatto, cosa manca o cosa serve da «NOME». Se c'e' un link o un file pronto, dillo.";

fn list_items(items: &[Value], sid: &str) -> String {
    if items.is_empty() { return "(nessuna)".into(); }
    items.iter().map(|v| {
        let mut notes = vec![];
        if str_of(v, "manuale") == "riaperta" { notes.push("riaperta a mano da «NOME» perche' non andava".to_string()); }
        if str_of(v, "sessione") != sid { notes.push(format!("nata nella sessione {}", str_of(v, "nome"))); }
        format!("- {} [{}] {}{}", str_of(v, "id"), str_of(v, "stato"), str_of(v, "titolo"), if notes.is_empty() { String::new() } else { format!(" ({})", notes.join(", ")) })
    }).collect::<Vec<_>>().join("\n")
}

/// Like `list_items`, with the last recorded news: old requests (outside the latest messages) are judged by it.
fn list_with_reply(items: &[Value], sid: &str) -> String {
    let base = list_items(items, sid);
    if items.is_empty() { return base; }
    base.lines().zip(items.iter()).map(|(r, v)| {
        let t = str_of(v, "risposta").trim();
        if t.is_empty() { r.to_string() } else { format!("{r}\n    ultima novita' registrata: {}", short_text(t, 240)) }
    }).collect::<Vec<_>>().join("\n")
}

/// The oks the session already asked the user for (approvals.rs): requests waiting only for those do not become "serve te".
fn list_oks(oks: &[Value]) -> String {
    if oks.is_empty() { return String::new(); }
    format!("Ok che la sessione ha gia' chiesto a «NOME» (li vede gia' in un posto suo, con i tasti Ok/No o Fatto):\n{}\n",
        oks.iter().map(|o| format!("- {} {}{}{}", str_of(o, "id"), str_of(o, "titolo"), if o["fai_tu"] == json!(true) { " (lo fa lui)" } else { "" },
            if str_of(o, "stato") == "in_conferma" { " (ha gia' risposto, aspetta conferma)" } else { "" })).collect::<Vec<_>>().join("\n"))
}

/// State decided by the model, with the fixed rules above: a request waiting for an ok already asked is in progress
/// linked to that ok (no duplicate "serve te"); "you do it" only for serve_te.
fn set_state(v: &mut Value, x: &Value, oks: &[Value]) {
    let mut state = str_of(x, "stato").to_string();
    let ok_id = str_of(x, "ok").trim().to_string();
    let linked = state != "fatta" && !ok_id.is_empty() && oks.iter().any(|o| str_of(o, "id") == ok_id);
    if linked && (state == "serve_te" || state == "non_presa") { state = "in_lavoro".into(); }
    if STATES.contains(&state.as_str()) { v["stato"] = json!(state); }
    v["ok_id"] = json!(if linked { ok_id } else { String::new() });
    v["fai_tu"] = json!(str_of(v, "stato") == "serve_te" && x["fai_tu"].as_bool().unwrap_or(false));
    if str_of(v, "stato") != "serve_te" || linked { v["motivo"] = json!(""); }
}

/// Requests the model must know about for a session: open ones, plus those closed in the last 24 hours (if the user asks again, they reopen).
fn known_for(sid: &str) -> Vec<Value> {
    let now = unix_now();
    all_items().into_iter().filter(|v| concerns(v, sid) && (is_open(v) || now - f64_of(v, "aggiornata") < DAY)).collect()
}

/// Open requests of the other sessions (the most recent), to recognize duplicates.
fn others_open(sid: &str, within: f64) -> Vec<Value> {
    let now = unix_now();
    let mut a: Vec<Value> = all_items().into_iter().filter(|v| is_open(v) && !concerns(v, sid) && !str_of(v, "sessione").starts_with("locale:") && now - f64_of(v, "aggiornata").max(f64_of(v, "creata")) < within).collect();
    a.sort_by(|x, y| f64_of(y, "aggiornata").partial_cmp(&f64_of(x, "aggiornata")).unwrap_or(std::cmp::Ordering::Equal));
    a.truncate(40);
    a
}
fn list_others(a: &[Value]) -> String {
    if a.is_empty() { return "(nessuna)".into(); }
    a.iter().map(|v| format!("- {} [sessione {}] {}", str_of(v, "id"), str_of(v, "nome"), str_of(v, "titolo"))).collect::<Vec<_>>().join("\n")
}

fn new_item(r: &mut Value, sid: &str, name: &str, title: &str, text: &str, created: f64, state: &str, reply: &str, times: u64) -> Value {
    json!({"id": new_id(r), "sessione": sid, "nome": name, "titolo": title, "testo": short_text(text, 2000), "creata": created,
        "aggiornata": unix_now(), "stato": state, "risposta": reply, "volte": times.max(1), "anche_in": [], "messaggi": [],
        "manuale": "", "ferma": false, "attesa_da": 0})
}

/// The user asked for it again (even from another session): it is counted and goes back to in progress, waiting for the next turn.
fn repeat(v: &mut Value, sid: &str, name: &str, title: &str, k: &str, when: f64) {
    v["volte"] = json!(v["volte"].as_u64().unwrap_or(1) + 1);
    v["aggiornata"] = json!(unix_now());
    add_session(v, sid, name);
    add_msg(v, k);
    if !title.is_empty() { v["titolo"] = json!(title); }
    if str_of(v, "stato") != "in_lavoro" { v["stato"] = json!("in_lavoro"); }
    v["scartata"] = json!(false); v["ricordata"] = json!(false); v["sollecitata_t"] = json!(0); v["motivo"] = json!("");
    v["ferma"] = json!(false);
    v["attesa_da"] = json!(when);
    v["ultima_sessione"] = json!(sid);
}

/// Merges a duplicate into the older item: sessions, messages and counts together; state and answer from the most recent.
fn merge(r: &mut Value, a: &str, b: &str) {
    let (a, b) = (resolve(r, a), resolve(r, b));
    if a == b { return; }
    let Some(arr) = r["voci"].as_array() else { return };
    let (Some(ia), Some(ib)) = (arr.iter().position(|v| str_of(v, "id") == a), arr.iter().position(|v| str_of(v, "id") == b)) else { return };
    let (keep, drop_i) = if f64_of(&arr[ia], "creata") <= f64_of(&arr[ib], "creata") { (ia, ib) } else { (ib, ia) };
    let gone = arr[drop_i].clone();
    let newer = f64_of(&gone, "aggiornata") > f64_of(&arr[keep], "aggiornata");
    let gone_id = str_of(&gone, "id").to_string();
    let v = &mut r["voci"][keep];
    add_session(v, str_of(&gone, "sessione"), str_of(&gone, "nome"));
    for x in gone["anche_in"].as_array().cloned().unwrap_or_default() { add_session(v, str_of(&x, "sessione"), str_of(&x, "nome")); }
    for k in gone["messaggi"].as_array().cloned().unwrap_or_default() { if let Some(k) = k.as_str() { add_msg(v, k); } }
    v["volte"] = json!(v["volte"].as_u64().unwrap_or(1) + gone["volte"].as_u64().unwrap_or(1));
    if newer {
        for k in ["stato", "risposta", "aggiornata", "ferma", "attesa_da", "ultima_sessione", "letta", "fatta_il", "risposta_t", "risposta_sessione", "motivo"] { if gone.get(k).is_some() { v[k] = gone[k].clone(); } }
    }
    let keep_id = str_of(v, "id").to_string();
    if let Some(a) = r["voci"].as_array_mut() { a.retain(|x| str_of(x, "id") != gone_id); }
    r["unite"][gone_id] = json!(keep_id);
}

// ---------- extraction: the user wrote ----------

/// Cap exceeded: the analysis is postponed by 10 minutes (on a separate thread, the chat queue does not wait) and then re-queued.
/// The work is never lost: as long as the cap is exceeded it is postponed again.
fn postpone_extraction(sid: &str, texts: Vec<String>, when: f64) {
    let sid = sid.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(600));
        enqueue(&sid, Job::Estrai { testi: texts, quando: when, completa: true });
    });
}

fn extract(sid: &str, texts: Vec<String>, when: f64, complete: bool) {
    let v = live_session(sid).unwrap_or(json!({}));
    let name = str_of(&v, "nome").to_string();
    // the text from the session state is cut at 200 characters: if possible take it whole from the conversation
    let mut texts = texts;
    if !complete && !v.is_null() {
        let ev = events_of(sid, &v);
        for t in texts.iter_mut() {
            let k = key_n(sid, t.trim_end_matches('…'), 60);
            if let Some(e) = ev.iter().rev().find(|e| str_of(e, "tipo") == "tu" && key(sid, str_of(e, "testo")).starts_with(&k)) { *t = str_of(e, "testo").to_string(); }
        }
    }
    let texts: Vec<String> = texts.into_iter().filter(|t| from_user(t)).collect();
    if texts.is_empty() { return; }
    // 24-hour cap (0.1.70): when exceeded, the request is NOT discarded, it is postponed. Does not apply to turns waiting for the user.
    let urgent = str_of(&v, "stato") == "aspetta";
    if !urgent && crate::helpers::is_enabled("estrattore") && crate::helpers::over_cap("estrattore") {
        crate::helpers::record("estrattore", "rimandata");
        postpone_extraction(sid, texts, when);
        return;
    }
    let known = known_for(sid);
    let others = others_open(sid, DUPLICATES_WITHIN);
    // extractor (12/11): what was being discussed (agent's last reply before the message) and fixed rules from memory
    let context = context_of(sid, &v, &texts[0]);
    let rules = fixed_rules();
    let msgs_block = if texts.len() == 1 { format!("Messaggio nuovo di «NOME» alla sessione:\n\"\"\"\n{}\n\"\"\"", truncate_chars(&texts[0], 5000)) }
        else { format!("Messaggi nuovi di «NOME» alla sessione:\n\n{}", texts.iter().enumerate().map(|(i, t)| format!("Messaggio {}:\n\"\"\"\n{}\n\"\"\"", i + 1, truncate_chars(t, 2500))).collect::<Vec<_>>().join("\n\n")) };
    let question = format!("{PREAMBLE}\n\nSessione: {name}\nRichieste gia' registrate per questa sessione (id, stato, titolo):\n{}\n\n\
Richieste aperte in ALTRE sessioni (ultime 48 ore), per riconoscere i doppioni:\n{}\n\n{msgs_block}\n\n\
Regole:\n- Una richiesta e' qualcosa che «NOME» vuole che venga fatto, controllato, deciso o spiegato. Se il messaggio chiede piu' cose diverse, crea una richiesta per ognuna (al massimo 6).\n\
- Se il messaggio non chiede niente di nuovo (\"ok\", \"procedi\", \"vedi tu\", \"grazie\", saluti, commenti, sfoghi, conferme, la risposta a una domanda dell'agente), \"niente\" e' true.\n\
- Se il messaggio ripete, sollecita o precisa una richiesta gia' registrata in questa sessione, NON crearne una nuova: mettila in \"ripete\" con il suo id; se la precisa, dai anche un titolo aggiornato, altrimenti titolo vuoto.\n\
- Se chiede lo stesso lavoro di una richiesta aperta in un'altra sessione (es. un compito passato da una sessione all'altra, anche detto con parole diverse), mettila in \"ripete\" con l'id di quella. Solo se e' proprio lo stesso lavoro, non basta lo stesso cliente o progetto.\n\
- \"messaggio\": il numero del messaggio da cui viene (1 se ce n'e' uno solo).\n\
- Leggi il messaggio nel suo contesto (l'ultima risposta dell'agente prima del messaggio e' qui sotto): la richiesta riguarda quasi sempre la cosa di cui si stava parlando. \
Il titolo dice su che cosa e' (es. \"Scheda App Store e pagine in inglese\"), MAI una regola generale su come l'agente deve lavorare o rispondere \
(es. NON \"Rispondere sempre in inglese\") se il messaggio parlava di una cosa precisa.\n\
- Regole fisse di «NOME» (dalla sua memoria):\n{rules}\nSe una richiesta, letta nel suo contesto, va contro una di queste regole, aggiungi alla voce \"dubbia\": true e \"perche\": \"<quale regola, in poche parole>\": \
non verra' mandata alla sessione ma chiesta a «NOME».\n\
- {TITLE_RULES}\n\n\
Rispondi SOLO con un oggetto JSON, senza altro testo: {{\"nuove\": [{{\"titolo\": \"...\", \"messaggio\": 1, \"dubbia\": false, \"perche\": \"\"}}], \"ripete\": [{{\"id\": \"r1\", \"titolo\": \"\", \"messaggio\": 1}}], \"niente\": true oppure false}}",
        list_items(&known, sid), list_others(&others));
    let question = if context.is_empty() { question } else { format!("{question}\n\nUltima risposta dell'agente prima del messaggio (il contesto):\n\"\"\"\n{context}\n\"\"\"") };
    let Some(r) = autopilot::ask_for("estrattore", &question, None, autopilot::Cache::SoloUna, urgent) else { return };
    let msg_idx = |x: &Value| x["messaggio"].as_u64().map(|m| (m as usize).clamp(1, texts.len()) - 1).unwrap_or(0);
    let doubtful: Vec<(String, String)> = r["nuove"].as_array().cloned().unwrap_or_default().iter()
        .filter(|x| x["dubbia"] == json!(true)).map(|x| (clean_title(str_of(x, "titolo")).to_lowercase(), clean_sentence(str_of(x, "perche")))).collect();
    let new_titles: Vec<(String, usize)> = r["nuove"].as_array().cloned().unwrap_or_default().iter()
        .map(|x| (clean_title(if x.is_string() { x.as_str().unwrap_or("") } else { str_of(x, "titolo") }), msg_idx(x))).filter(|(t, _)| !t.is_empty()).take(6).collect();
    let repeats: Vec<(String, String, usize)> = r["ripete"].as_array().cloned().unwrap_or_default().iter()
        .map(|x| (str_of(x, "id").to_string(), clean_title(str_of(x, "titolo")), msg_idx(x))).filter(|(i, _, _)| !i.is_empty()).collect();
    if new_titles.is_empty() && repeats.is_empty() { return; }
    let now = unix_now();
    with_registry(|reg| {
        let mut added = vec![];
        for (t, m) in &new_titles {
            let k = msg_key(sid, &texts[*m]);
            // same title already open (here, or recently in another session): it is a repetition
            let existing = reg["voci"].as_array().and_then(|a| a.iter().position(|v| is_open(v) && str_of(v, "titolo").to_lowercase() == t.to_lowercase()
                && (concerns(v, sid) || now - f64_of(v, "aggiornata") < DUPLICATES_WITHIN)));
            if let Some(i) = existing { repeat(&mut reg["voci"][i], sid, &name, "", &k, when); continue; }
            let mut item = new_item(reg, sid, &name, t, &texts[*m], now, "in_lavoro", "", 1);
            item["attesa_da"] = json!(when);
            // extractor (12/11): goes against a fixed rule -> to clarify with «NOME», no reminder to the session
            if let Some((_, why)) = doubtful.iter().find(|(d, _)| *d == t.to_lowercase()) {
                item["stato"] = json!("serve_te"); item["dubbia"] = json!(true); item["attesa_da"] = json!(0);
                item["motivo"] = json!(format!("Da chiarire: {}", if why.is_empty() { "sembra andare contro una regola fissa" } else { why.as_str() }));
            }
            item["messaggi"] = json!([k]);
            added.push(item);
        }
        for (id, title, m) in &repeats {
            let k = msg_key(sid, &texts[*m]);
            if let Some(v) = item_mut(reg, id) { repeat(v, sid, &name, title, &k, when); }
        }
        if let Some(a) = reg["voci"].as_array_mut() { a.extend(added); }
    });
    notify();
}

fn truncate_chars(t: &str, n: usize) -> String { t.chars().take(n).collect() }

/// extractor (12/11): the agent's last reply before the user's message (the context), at most 900 characters.
fn context_of(sid: &str, v: &Value, text: &str) -> String {
    if v.is_null() { return String::new(); }
    let ev = events_of(sid, v);
    let k = key_n(sid, text.trim_end_matches('…'), 60);
    let i = ev.iter().rposition(|e| str_of(e, "tipo") == "tu" && key(sid, str_of(e, "testo")).starts_with(&k)).unwrap_or(ev.len());
    ev[..i].iter().rev().find(|e| str_of(e, "tipo") == "agente" && !str_of(e, "testo").trim().is_empty())
        .map(|e| head_and_tail(str_of(e, "testo"), 500, 400)).unwrap_or_default()
}

/// extractor (12/11): fixed rules from the user's memory (MEMORY.md index, only the entries with SEMPRE or MAI), line by line.
/// Without memory (other users): no rules. Re-read only if the file changes.
fn fixed_rules() -> String {
    static CACHE: Mutex<Option<(std::time::SystemTime, String)>> = Mutex::new(None);
    let p = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude/projects/-/memory/MEMORY.md");
    let Ok(m) = std::fs::metadata(&p).and_then(|m| m.modified()) else { return "(nessuna)".into() };
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, r)) = c.as_ref() { if *t == m { return r.clone(); } }
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let lines: Vec<String> = text.lines().filter_map(|l| {
        let a = l.find('[')?; let b = l[a..].find("](")? + a;
        let title = &l[a + 1..b];
        let after = l.split(" — ").nth(1).unwrap_or("");
        let whole = format!("{title} {after}");
        (whole.contains("SEMPRE") || whole.contains("MAI")).then(|| format!("- {}", short_text(title, 100)))
    }).take(30).collect();
    let r = if lines.is_empty() { "(nessuna)".to_string() } else { lines.join("\n") };
    *c = Some((m, r.clone()));
    r
}

// ---------- update: the agent finished a turn ----------

/// The user is looking at this chat right now (window in the foreground): the answer is already seen.
fn watched_now(sid: &str) -> bool {
    autopilot::on_screen(sid) && crate::APP.get().and_then(|a| a.get_webview_window("main")).and_then(|w| w.is_focused().ok()).unwrap_or(false)
}

/// Text to send again for a request left behind: the original message if it asked only for that, otherwise the title.
fn original_text(v: &Value, all: &[Value]) -> String {
    let id = str_of(v, "id");
    let own = v["messaggi"].as_array().cloned().unwrap_or_default();
    let shared = all.iter().any(|x| str_of(x, "id") != id && x["messaggi"].as_array().map(|a| a.iter().any(|k| own.contains(k))).unwrap_or(false));
    let t = str_of(v, "testo").trim();
    if shared || t.is_empty() { return str_of(v, "titolo").to_string(); }
    // extractor (12/11): a short sentence on its own loses the context ("Fai tutto in inglese però"): title + sentence
    if t.chars().count() < 120 && !str_of(v, "titolo").is_empty() { return format!("{} (avevi scritto: «{}»)", str_of(v, "titolo"), t); }
    short_text(t, 700)
}

fn reminder_text(things: &[String]) -> String {
    if things.len() == 1 {
        format!("{}, fatta mentre lavoravi ad altro: {}. Fallo ora o dimmi se c'è un motivo per non farlo.", crate::profile::with_name(REMINDER_PREFIX[0]), things[0].trim_end_matches('.'))
    } else {
        format!("{}, fatte mentre lavoravi ad altro:\n{}\nFalle ora o dimmi se c'è un motivo per non farle.", crate::profile::with_name(REMINDER_PREFIX[1]), things.iter().map(|c| format!("- {}", c.trim_end_matches('.'))).collect::<Vec<_>>().join("\n"))
    }
}

fn update(sid: &str) {
    let open: Vec<Value> = all_items().into_iter().filter(|v| concerns(v, sid) && is_open(v)).collect();
    if open.is_empty() { return; }
    let Some(v) = live_session(sid) else { return };
    // 24-hour cap for the end-of-turn update too (0.1.86): it skipped the cap because it was always urgent. Only turns
    // waiting for the user always go through; the others, with the cap exceeded, are postponed by 10 minutes and not lost.
    let urgent = str_of(&v, "stato") == "aspetta";
    if !urgent && crate::helpers::is_enabled("estrattore") && crate::helpers::over_cap("estrattore") {
        crate::helpers::record("estrattore", "rimandata");
        let sid = sid.to_string();
        std::thread::spawn(move || { std::thread::sleep(Duration::from_secs(600)); enqueue(&sid, Job::Aggiorna); });
        return;
    }
    let d = detail_of(sid, &v);
    let ev = d["eventi"].as_array().cloned().unwrap_or_default();
    let min_wait = open.iter().map(|v| f64_of(v, "attesa_da")).filter(|t| *t > 0.0).fold(f64::INFINITY, f64::min);
    let start = turn_start(&ev, if min_wait.is_finite() { min_wait } else { 0.0 });
    let t_start = if start > 0 { ev_time(&ev[start - 1]) } else { 0.0 };
    let (turn_text, tools) = turn_from(&ev, start);
    if turn_text.trim().is_empty() && tools.is_empty() { return; }
    // the agent's message closing the turn: the answer to open with one click
    let reply_t = ev[start.min(ev.len())..].iter().rev().find(|e| str_of(e, "tipo") == "agente" && !str_of(e, "testo").trim().is_empty()).map(|e| str_of(e, "t").to_string()).unwrap_or_default();
    let steps = steps_of(&d);
    let fresh: Vec<&str> = open.iter().filter(|v| f64_of(v, "attesa_da") > 0.0).map(|v| str_of(v, "id")).collect();
    let oks = crate::approvals::oks_of(sid);
    let question = format!("{PREAMBLE}\n\nSessione: {}\nRichieste aperte di «NOME» che riguardano questa sessione (id, stato attuale, titolo):\n{}\n{}{}\n\
L'agente ha appena finito un turno di lavoro. Ecco tutto quello che ha scritto nel turno, dal primo all'ultimo messaggio:\n\"\"\"\n{}\n\"\"\"\n{}{}\n\
Per OGNI richiesta dell'elenco decidi:\n\
- \"toccata\": true se in questo turno l'agente ci ha lavorato, ne ha parlato o ha risposto; false se non l'ha toccata in nessun modo. \
Per le richieste riaperte a mano da «NOME», true solo se in questo turno l'agente ci ha lavorato di nuovo in modo esplicito.\n\
- \"stato\" (solo se toccata): fatta, in_lavoro oppure serve_te.\n{STATE_RULES}\n\
- {REPLY_RULES} Se e' serve_te, scrivi cosa deve fare «NOME». Vuota se non toccata.\n\n\
Rispondi SOLO con un oggetto JSON, senza altro testo, con tutte le richieste dell'elenco: {{\"aggiornate\": [{{\"id\": \"r1\", \"toccata\": true, \"stato\": \"fatta\", \"risposta\": \"...\", \"ok\": \"\", \"fai_tu\": false}}]}}",
        str_of(&v, "nome"), list_with_reply(&open, sid),
        if fresh.is_empty() { String::new() } else { format!("Appena chieste, questo e' il primo turno dopo la richiesta: {}\n", fresh.join(", ")) },
        list_oks(&oks),
        turn_text,
        if tools.is_empty() { String::new() } else { format!("\nStrumenti usati nel turno (gli ultimi):\n{tools}\n") },
        if steps.is_empty() { String::new() } else { format!("\nLista dei passi dell'agente (TodoWrite/Task):\n{steps}\n") });
    let Some(r) = autopilot::ask_for("estrattore", &question, STATE_MODEL, autopilot::Cache::SoloUna, urgent) else { return };
    let updates: HashMap<String, Value> = r["aggiornate"].as_array().cloned().unwrap_or_default().into_iter().map(|x| (str_of(&x, "id").to_string(), x)).collect();
    let ids: Vec<String> = open.iter().map(|v| str_of(v, "id").to_string()).collect();
    let watched = watched_now(sid);
    let reminders: Vec<String> = with_registry(|reg| {
        let now = unix_now();
        let all = reg["voci"].as_array().cloned().unwrap_or_default();
        let mut to_remind = vec![];
        for id in &ids {
            let x = updates.get(id).cloned();
            let Some(v) = item_mut(reg, id) else { continue };
            if !is_open(v) { continue; }   // closed by hand in the meantime
            let touched = x.as_ref().map(|x| x["toccata"].as_bool().unwrap_or(true)).unwrap_or(false);
            let att = f64_of(v, "attesa_da");
            let judged = att > 0.0 && judgeable(&ev, att);
            // reopened by hand: the agent changes it only with new work, after the reopening
            if str_of(v, "manuale") == "riaperta" {
                if t_start + 5.0 < f64_of(v, "manuale_t") { continue; }
                if touched { v["manuale"] = json!(""); }
            }
            if touched {
                let x = x.unwrap_or(json!({}));
                let before = str_of(v, "stato").to_string();
                if ["in_lavoro", "fatta", "serve_te"].contains(&str_of(&x, "stato")) { set_state(v, &x, &oks); }
                else if before == "non_presa" { v["stato"] = json!("in_lavoro"); }
                let state = str_of(v, "stato").to_string();
                let reply = clean_sentence(str_of(&x, "risposta"));
                if !reply.is_empty() { v["risposta"] = json!(reply); }
                if state == "fatta" && before != "fatta" {
                    v["fatta_il"] = json!(now); v["letta"] = json!(watched);
                    v["risposta_t"] = json!(reply_t); v["risposta_sessione"] = json!(sid);
                }
                if state != "non_presa" { v["motivo"] = json!(""); v["ricordata"] = json!(false); }
                v["toccata_il"] = json!(now);
                v["aggiornata"] = json!(now); v["ferma"] = json!(false); v["ultima_sessione"] = json!(sid); v["sollecitata_t"] = json!(0);
                if judged { v["attesa_da"] = json!(0); }
            } else if v["dubbia"] == json!(true) {
                continue;   // extractor (12/11): to clarify with «NOME», never sent back on its own
            } else if judged || (str_of(v, "stato") == "non_presa" && v["ricordata"] != json!(true)) {
                // already reminded but the reminder has not arrived yet or got no answer (queued behind another turn):
                // this turn says nothing, wait for the next one
                if v["ricordata"] == json!(true) && !reminder_read(&ev, f64_of(v, "ricordata_il")) { continue; }
                // the agent already worked on it or answered after the last time the user asked: a turn on something else is not ignoring it
                if f64_of(v, "toccata_il") > att && att > 0.0 { v["attesa_da"] = json!(0); continue; }
                // ignored: the first time Juggl sends it back on its own, the second time it is up to the user
                v["attesa_da"] = json!(0); v["aggiornata"] = json!(now); v["ferma"] = json!(false); v["non_presa_in"] = json!(sid);
                if v["ricordata"] == json!(true) {
                    // 0.1.79: ignored twice is NOT a decision for the user: stays "non presa", marked hanging (kind "appesa")
                    v["stato"] = json!("non_presa"); v["appesa"] = json!(true); v["motivo_appesa"] = json!("ignorata due volte, anche dopo il promemoria");
                } else {
                    v["stato"] = json!("non_presa"); v["ricordata"] = json!(true); v["ricordata_il"] = json!(now);
                    v["attesa_da"] = json!(now - 5.0);   // the turn after the reminder tells whether it was taken
                    to_remind.push(original_text(v, &all));
                }
            }
        }
        to_remind
    });
    // the reminder goes out as an internal message (it does not become a new request); if the session is working, it is queued
    if !reminders.is_empty() { let _ = crate::send_internal(sid.to_string(), reminder_text(&reminders)); }
    notify();
}

/// Every 10 minutes: a request in progress with no news for 12 hours gets, once, "A che punto sei con: ...?".
fn nudges() {
    let now = unix_now();
    if now < f64_of(&registry(), "auto_da") { return; }
    let mut ss = crate::list_sessions();
    let active: HashSet<String> = ss.iter().filter(|v| str_of(v, "vista") != "ibernata").map(|v| str_of(v, "sessione").to_string()).collect();
    let by_session: HashMap<String, Vec<String>> = with_registry(|reg| {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        let Some(a) = reg["voci"].as_array_mut() else { return m };
        for v in a.iter_mut() {
            if str_of(v, "stato") != "in_lavoro" || v["scartata"] == json!(true) || is_automatic(str_of(v, "testo")) || now - f64_of(v, "aggiornata") < STALL_AFTER || f64_of(v, "sollecitata_t") > 0.0 { continue; }
            v["sollecitata_t"] = json!(now);   // only once: after another 12 hours with no news it becomes serve_te
            let mut target = str_of(v, "ultima_sessione").to_string();
            if target.is_empty() || !concerns(v, &target) || !active.contains(&target) { target = str_of(v, "sessione").to_string(); }
            if active.contains(&target) { m.entry(target).or_default().push(str_of(v, "titolo").to_string()); }
        }
        m
    });
    for (sid, titles) in by_session {
        let text = if titles.len() == 1 { format!("{ASK_PREFIX} {}? Rispondi in breve: fatto, in corso (cosa manca) o bloccato (cosa ti serve).", titles[0]) }
            else { format!("{ASK_PREFIX} {}? Rispondi in breve per ognuna: fatto, in corso (cosa manca) o bloccato (cosa ti serve).", titles.join("? E con: ")) };
        let _ = crate::send_internal(sid, text);
    }
    notify();
}

// ---------- conversation history (first start and re-evaluation) ----------

/// The user's last `n` messages, each with what the agent wrote afterwards (the last one longer).
fn history(ev: &[Value], n: usize) -> (Vec<usize>, String) {
    let idx: Vec<usize> = ev.iter().enumerate().filter(|(_, e)| str_of(e, "tipo") == "tu" && from_user(str_of(e, "testo")) && !just_confirmation(str_of(e, "testo"))).map(|(i, _)| i).collect();
    let idx: Vec<usize> = idx.iter().skip(idx.len().saturating_sub(n)).copied().collect();
    let mut lines = vec![];
    for (k, &i) in idx.iter().enumerate() {
        let end = idx.get(k + 1).copied().unwrap_or(ev.len());
        let is_last = k + 1 == idx.len();
        let all_text = ev[i + 1..end].iter().filter(|x| str_of(x, "tipo") == "agente").map(|x| str_of(x, "testo")).collect::<Vec<_>>().join("\n\n");
        let reply = if all_text.trim().is_empty() { "(nessuna risposta ancora)".to_string() } else if is_last { head_and_tail(&all_text, 1200, 4000) } else { head_and_tail(&all_text, 300, 900) };
        lines.push(format!("Messaggio {} di «NOME» ({}): \"\"\"{}\"\"\"\nCosa ha scritto l'agente dopo: \"\"\"{}\"\"\"", k + 1, str_of(&ev[i], "t").get(..16).unwrap_or(""), short_text(str_of(&ev[i], "testo"), 900), reply));
    }
    (idx, lines.join("\n\n"))
}

// ---------- first start: the latest requests of each session ----------

fn slots() -> &'static (Mutex<usize>, Condvar) { static P: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new(); P.get_or_init(|| (Mutex::new(0), Condvar::new())) }

struct Slot;
impl Drop for Slot {
    fn drop(&mut self) { let (m, c) = slots(); *m.lock().unwrap_or_else(|e| e.into_inner()) -= 1; c.notify_all(); }
}
/// At most 2 calls together between first start and re-evaluation.
fn take_slot() {
    let (m, c) = slots();
    let mut n = m.lock().unwrap_or_else(|e| e.into_inner());
    while *n >= 2 { n = c.wait(n).unwrap_or_else(|e| e.into_inner()); }
    *n += 1;
}

fn first_run(sid: &str) {
    let _slot = Slot;
    let mark_done = |r: &mut Value| { if let Some(a) = r["popolate"].as_array_mut() { a.push(json!(sid)); } };
    let Some(v) = live_session(sid) else { return with_registry(mark_done) };
    let ev = events_of(sid, &v);
    let (idx, lines) = history(&ev, 8);
    if idx.is_empty() { return with_registry(mark_done); }
    let working = crate::turn_view(&v) == "lavora";
    let others = others_open(sid, DUPLICATES_WITHIN);
    let question = format!("{PREAMBLE}\n\nSessione: {}{}\nEcco gli ultimi messaggi di «NOME» a questa sessione, ognuno con quello che l'agente ha scritto dopo, dal piu' vecchio al piu' recente:\n\n{}\n\n\
Richieste aperte in ALTRE sessioni, per riconoscere i doppioni:\n{}\n\n\
Ricava le richieste di «NOME». Se piu' messaggi chiedono la stessa cosa, e' UNA richiesta: elenca in \"messaggi\" i numeri dei messaggi che la chiedono. \
Ignora i messaggi che non chiedono niente (ok, procedi, vedi tu, saluti, commenti, risposte a domande dell'agente). Al massimo 8 richieste.\n\
Se una richiesta e' proprio lo stesso lavoro di una delle altre sessioni, metti il suo id in \"uguale_a\", altrimenti \"\".\n\
{TITLE_RULES}\n{STATE_RULES}\n\
- \"non_presa\": dopo la richiesta l'agente ha finito almeno un turno senza mai parlarne ne' lavorarci.\n\
{REPLY_RULES} Vuota se l'agente non ha ancora risposto.\n\n\
Rispondi SOLO con un oggetto JSON, senza altro testo: {{\"richieste\": [{{\"titolo\": \"...\", \"messaggi\": [1, 3], \"stato\": \"in_lavoro\", \"risposta\": \"...\", \"uguale_a\": \"\"}}]}}",
        str_of(&v, "nome"), if working { " (sta lavorando adesso sull'ultimo messaggio)" } else { "" }, lines, list_others(&others));
    let r = autopilot::ask_for("estrattore", &question, STATE_MODEL, autopilot::Cache::No, true);
    let name = str_of(&v, "nome").to_string();
    with_registry(|reg| {
        for &i in &idx { reg["visti"][key(sid, str_of(&ev[i], "testo"))] = json!(unix_now()); }
        mark_done(reg);
        // the session already has items (arrived live in the meantime): do not duplicate
        if reg["voci"].as_array().map(|a| a.iter().any(|x| concerns(x, sid))).unwrap_or(false) { return; }
        let Some(r) = &r else { return };
        let mut added = vec![];
        for x in r["richieste"].as_array().cloned().unwrap_or_default().iter().take(8) {
            let title = clean_title(str_of(x, "titolo"));
            if title.is_empty() { continue; }
            let msg: Vec<usize> = x["messaggi"].as_array().cloned().unwrap_or_default().iter().filter_map(|m| m.as_u64()).map(|m| m as usize)
                .filter(|m| *m >= 1 && *m <= idx.len()).collect();
            let first_m = msg.iter().min().copied().unwrap_or(idx.len());
            let keys: Vec<String> = (if msg.is_empty() { vec![first_m] } else { msg.clone() }).iter().map(|m| msg_key(sid, str_of(&ev[idx[m - 1]], "testo"))).collect();
            let same_as = str_of(x, "uguale_a");
            if !same_as.is_empty() {
                if let Some(v) = item_mut(reg, same_as) {
                    if is_open(v) {
                        add_session(v, sid, &name);
                        for k in &keys { add_msg(v, k); }
                        v["volte"] = json!(v["volte"].as_u64().unwrap_or(1) + msg.len().max(1) as u64);
                        continue;
                    }
                }
            }
            let e = &ev[idx[first_m - 1]];
            let created = { let t = from_iso(str_of(e, "t")); if t > 0.0 { t } else { unix_now() } };
            let mut state = str_of(x, "stato").to_string();
            if !STATES.contains(&state.as_str()) { state = "in_lavoro".into(); }
            let reply = clean_sentence(str_of(x, "risposta"));
            let mut item = new_item(reg, sid, &name, &title, str_of(e, "testo"), created, &state, &reply, msg.len() as u64);
            item["messaggi"] = json!(keys);
            let last_m = msg.iter().max().copied().unwrap_or(first_m);
            let t_last = from_iso(str_of(&ev[idx[last_m - 1]], "t"));
            item["aggiornata"] = json!(if state == "fatta" && t_last > 0.0 { t_last } else { unix_now() });
            if state == "fatta" { item["fatta_il"] = item["aggiornata"].clone(); item["letta"] = json!(unix_now() - f64_of(&item, "aggiornata") > 6.0 * HOUR_S); item["risposta_sessione"] = json!(sid); }
            added.push(item);
        }
        if let Some(a) = reg["voci"].as_array_mut() { a.extend(added); }
    });
    notify();
}

// ---------- re-evaluation: once, the open items with the new rules (and duplicates merged) ----------

fn reevaluate(sid: &str) {
    let _slot = Slot;
    let own: Vec<Value> = all_items().into_iter().filter(|v| str_of(v, "sessione") == sid && v["da_rivalutare"] == json!(true)).collect();
    if own.is_empty() { return; }
    let ids: Vec<String> = own.iter().map(|v| str_of(v, "id").to_string()).collect();
    let mark = |reg: &mut Value| { for id in &ids { if let Some(v) = item_mut(reg, id) { v["da_rivalutare"] = json!(false); } } };
    let Some(v) = live_session(sid) else { return with_registry(mark) };
    let d = detail_of(sid, &v);
    let ev = d["eventi"].as_array().cloned().unwrap_or_default();
    let (_, lines) = history(&ev, 10);
    if lines.is_empty() { return with_registry(mark); }
    let steps = steps_of(&d);
    let others = others_open(sid, 7.0 * DAY);
    let oks = crate::approvals::oks_of(sid);
    let question = format!("{PREAMBLE}\n\nSessione: {}{}\nRichieste di «NOME» registrate per questa sessione (id, stato attuale, titolo). Lo stato attuale puo' essere sbagliato:\n{}\n\n\
Ecco gli ultimi messaggi di «NOME» a questa sessione, ognuno con quello che l'agente ha scritto dopo, dal piu' vecchio al piu' recente:\n\n{}\n{}\n{}\
Richieste aperte in ALTRE sessioni:\n{}\n\n\
Per OGNI richiesta di questa sessione decidi lo stato giusto guardando tutta la conversazione, in particolare l'ultima cosa che l'agente ha detto su di essa. \
Se una richiesta non compare negli ultimi messaggi qui sopra, giudicala dalla sua ultima novita' registrata: se dice che e' fatta o da' la risposta, e' fatta; se aspetta un ok dell'elenco, mettine l'id in \"ok\".\n{STATE_RULES}\n\
- \"non_presa\": dopo il messaggio con la richiesta l'agente ha finito almeno un turno senza mai parlarne ne' lavorarci.\n\
{REPLY_RULES}\n\
\"uguale_a\": l'id di una richiesta delle altre sessioni se e' proprio lo stesso lavoro (es. un compito passato da una sessione all'altra), altrimenti \"\". Non basta lo stesso cliente o progetto.\n\n\
Rispondi SOLO con un oggetto JSON, senza altro testo, con tutte le richieste di questa sessione: {{\"richieste\": [{{\"id\": \"r1\", \"stato\": \"fatta\", \"risposta\": \"...\", \"ok\": \"\", \"fai_tu\": false, \"uguale_a\": \"\"}}]}}",
        str_of(&v, "nome"), if crate::turn_view(&v) == "lavora" { " (sta lavorando adesso sull'ultimo messaggio)" } else { "" }, list_with_reply(&own, sid), lines,
        if steps.is_empty() { String::new() } else { format!("\nLista dei passi dell'agente (TodoWrite/Task):\n{steps}\n") }, list_oks(&oks), list_others(&others));
    let Some(r) = autopilot::ask_for("estrattore", &question, STATE_MODEL, autopilot::Cache::No, true) else { return };   // retries at the next start
    // the user's last message in this chat: answers that arrived before it were already seen
    let last_user_msg = ev.iter().rev().find(|e| str_of(e, "tipo") == "tu" && from_user(str_of(e, "testo"))).map(ev_time).unwrap_or(0.0);
    with_registry(|reg| {
        let now = unix_now();
        if let Some(a) = reg["voci"].as_array_mut() {
            for v in a.iter_mut() {
                if ids.contains(&str_of(v, "id").to_string()) && new_reply(v) && done_at(v) < last_user_msg { v["letta"] = json!(true); }
            }
        }
        for x in r["richieste"].as_array().cloned().unwrap_or_default() {
            let id = str_of(&x, "id").to_string();
            if !ids.contains(&id) { continue; }
            let Some(v) = item_mut(reg, &id) else { continue };
            if str_of(v, "manuale") == "riaperta" || v["scartata"] == json!(true) { continue; }
            if STATES.contains(&str_of(&x, "stato")) {
                let before = (str_of(v, "stato").to_string(), str_of(v, "ok_id").to_string(), v["fai_tu"].clone());
                set_state(v, &x, &oks);
                let state = str_of(v, "stato").to_string();
                // just became done: the answer ended up in the conversation without the user seeing it
                // (if the user wrote in this chat after the last news on the request, they already saw the answer)
                if state == "fatta" && before.0 != "fatta" { v["letta"] = json!(f64_of(v, "aggiornata") < last_user_msg); v["fatta_il"] = json!(now); v["risposta_sessione"] = json!(sid); }
                if (state.clone(), str_of(v, "ok_id").to_string(), v["fai_tu"].clone()) != before { v["aggiornata"] = json!(now); v["ferma"] = json!(false); }
                // the whole conversation tells the state: the earlier automatic notes ("ignorata due volte") no longer apply
                if state != "non_presa" { v["motivo"] = json!(""); v["ricordata"] = json!(false); }
            }
            let reply = clean_sentence(str_of(&x, "risposta"));
            if !reply.is_empty() { v["risposta"] = json!(reply); }
            v["rivalutata"] = json!(now);
            let same_as = str_of(&x, "uguale_a").to_string();
            if !same_as.is_empty() && same_as != id {
                let other = resolve(reg, &same_as);
                let ok = reg["voci"].as_array().map(|a| a.iter().any(|y| str_of(y, "id") == other && is_open(y) && !concerns(y, sid))).unwrap_or(false);
                if ok { merge(reg, &id, &other); }
            }
        }
        mark(reg);
    });
    notify();
}

/// Puts all open items back into re-evaluation (from the bridge, for tests or after a rules change).
pub fn reevaluate_all() -> Value {
    let n = with_registry(|r| {
        let mut n = 0;
        let now = unix_now();
        if let Some(a) = r["voci"].as_array_mut() { for v in a.iter_mut() { if is_open(v) || now - done_at(v) < DAY { v["da_rivalutare"] = json!(true); n += 1; } } }
        n
    });
    std::thread::spawn(start_reevaluation);
    json!({"ok": true, "voci": n})
}

fn start_reevaluation() {
    let mut sids: Vec<String> = vec![];
    for v in all_items() { if v["da_rivalutare"] == json!(true) && !sids.contains(&str_of(&v, "sessione").to_string()) { sids.push(str_of(&v, "sessione").to_string()); } }
    for sid in sids {
        take_slot();
        enqueue(&sid, Job::Rivaluta);
    }
}

/// At startup: the sessions not analyzed yet (not hibernated) and the one-off re-evaluation, at most 2 calls together.
pub fn populate() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(8));
        let done: HashSet<String> = registry()["popolate"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).collect();
        let mut ss = crate::list_sessions();
        for v in ss {
            let sid = str_of(&v, "sessione").to_string();
            if str_of(&v, "vista") == "ibernata" || done.contains(&sid) || sid.is_empty() { continue; }
            take_slot();
            enqueue(&sid, Job::Primo);
        }
        start_reevaluation();
    });
}

// ---------- inputs ----------

/// The user wrote from the app (or from the iPhone): the message goes out or is queued.
pub fn on_send(sid: &str, text: &str, _queued: bool) {
    if sid.starts_with("locale:") { return; }   // local PC chats: the text is not saved here either (10/11)
    if !from_user(text) { return; }
    mark_handled_text(sid, text);
    if just_confirmation(text) { return; }
    enqueue(sid, Job::Estrai { testi: vec![text.to_string()], quando: unix_now() - 5.0, completa: true });
}

struct Snapshot { chiesto: String, stato: String, vista: String, da: f64 }

fn snapshots() -> &'static Mutex<Option<HashMap<String, Snapshot>>> { static P: OnceLock<Mutex<Option<HashMap<String, Snapshot>>>> = OnceLock::new(); P.get_or_init(|| Mutex::new(None)) }

/// Every 3 s from the Sentinel: new messages (also written in the terminal) and turns just finished.
pub fn tick(_app: &AppHandle, ss: &[Value]) {
    // every 10 minutes the time rules (stalled, automatic closings, cleanup) end up in the file
    static LAST_RUN: Mutex<f64> = Mutex::new(0.0);
    {
        let mut u = LAST_RUN.lock().unwrap_or_else(|e| e.into_inner());
        if *u == 0.0 { *u = unix_now(); }
        if unix_now() - *u > 600.0 { *u = unix_now(); std::thread::spawn(|| { with_registry(|_| ()); nudges(); mark_superseded(); }); }   // list (15/11): superate
    }
    let mut all: Vec<Value> = ss.to_vec();
    let mut g = snapshots().lock().unwrap_or_else(|e| e.into_inner());
    let first = g.is_none();
    let map = g.get_or_insert_with(HashMap::new);
    for v in &all {
        let sid = str_of(v, "sessione").to_string();
        if sid.is_empty() { continue; }
        let current = Snapshot { chiesto: str_of(v, "chiesto").to_string(), stato: str_of(v, "stato").to_string(), vista: str_of(v, "vista").to_string(), da: f64_of(v, "da") };
        let p = map.remove(&sid);
        let cur = map.entry(sid.clone()).or_insert(current);
        if first { continue; }
        // session just appeared: its first message counts as new
        let p = p.unwrap_or(Snapshot { chiesto: String::new(), stato: String::new(), vista: String::new(), da: 0.0 });
        // new message
        if cur.chiesto != p.chiesto && !cur.chiesto.trim().is_empty() && !cur.chiesto.starts_with('<') && from_user(&cur.chiesto) {
            if !seen_recently(&sid, cur.chiesto.trim_end_matches('…')) && !just_confirmation(&cur.chiesto) {
                mark_handled_text(&sid, cur.chiesto.trim_end_matches('…'));
                enqueue(&sid, Job::Estrai { testi: vec![cur.chiesto.clone()], quando: unix_now() - 30.0, completa: false });
            }
        }
        // turn finished
        let local = sid.starts_with("locale:");
        let finished = if local { p.vista == "lavora" && cur.vista != "lavora" }
            else { cur.stato == "aspetta" && (p.stato != "aspetta" || p.da != cur.da) };
        if finished { enqueue(&sid, Job::Aggiorna); }
    }
}

// ---------- list (15/11): automatic cleanup of requests (old duplicates, vanished chats) ----------
// The user's request (12/11): requests per project "in a smart way", duplicates merged, old ones closed automatically.
// Coordinator conditions (15/11): the user's requests are NEVER lost. So here nothing is deleted and everything can be undone:
// 1) Duplicates: two OPEN requests of the same project with almost the same title (meaningful words, numbers included:
//    0.1.60 is not 0.1.61; Jaccard >= 0.75 with at least 2 words, or equal). They are NOT merged by deleting: the one in the
//    less urgent state gets "doppione_di" (with reason and date) and the interface shows them together (asked N times, linked
//    chats, most urgent state). All texts stay in both items. "Separa" removes the mark for good ("separata").
// 2) Vanished chats: an OPEN request in progress (or not taken), with no open ok, no "you do it", created AND updated more
//    than 7 days ago, with ALL its chats no longer existing (closed, archived, deleted) becomes "fatta" with
//    chiusa_da_juggl = {motivo, il, stato_prima, ferma_prima}. Never serve_te. Those closed by Juggl are not deleted after 14 days.
//    "Riapri" restores the previous state and it is never closed automatically again ("no_auto").
// 3) Every automatic action writes a line in the Juggl log; the interface has the "Chiuse o unite da Juggl" section.
// 4) Switch: setting "pulizia_richieste" (on if missing), in Settings > Requests.
// The deciding part is applica_superate (no files or network): tested in tests_superseded with fake data.
const STOP_WORDS: [&str; 34] = ["il", "lo", "la", "le", "gli", "un", "uno", "una", "di", "da", "del", "della", "dei", "delle", "al", "alla",
    "che", "con", "per", "nel", "nella", "sul", "sulla", "non", "anche", "poi", "come", "piu", "più", "the", "and", "for", "to", "of"];

pub(crate) fn title_words(t: &str) -> Vec<String> {
    // numbers always count (0.1.60 is not 0.1.61); short words and stop words do not
    let keep = |x: &&str| !x.is_empty() && (x.chars().all(|c| c.is_ascii_digit()) || (x.chars().count() > 2 && !STOP_WORDS.contains(x)));
    let low = t.to_lowercase();
    let mut w: Vec<String> = low.split(|c: char| !c.is_alphanumeric()).filter(keep).map(String::from).collect();
    w.sort(); w.dedup(); w
}
pub(crate) fn similar_titles(a: &str, b: &str) -> bool {
    let (x, y) = (title_words(a), title_words(b));
    if x.is_empty() || y.is_empty() { return false; }
    if x == y { return true; }
    if x.len() < 2 || y.len() < 2 { return false; }
    let common = x.iter().filter(|w| y.contains(w)).count() as f64;
    let union = (x.len() + y.len()) as f64 - common;
    common / union >= 0.75
}
/// How urgent it is for the user: serve_te first of all.
fn urgency(v: &Value) -> u8 { match str_of(v, "stato") { "serve_te" => 3, "non_presa" => 2, "in_lavoro" => 1, _ => 0 } }

fn extended_cleanup_on() -> bool { crate::session_care::settings().get("pulizia_estesa").and_then(|x| x.as_bool()).unwrap_or(false) }
fn cleanup_on() -> bool { crate::session_care::settings().get("pulizia_richieste").and_then(|x| x.as_bool()).unwrap_or(true) }

/// Live sessions and working ones, projects and open oks: what is needed to decide the closings.
fn cleanup_data() -> Option<(HashSet<String>, HashSet<String>, HashMap<String, String>, HashSet<String>)> {
    let mut ss = crate::list_sessions();
    ss.extend(local_models::as_sessions());
    if ss.is_empty() { return None; }   // empty list (startup, error): touch nothing
    let alive: HashSet<String> = ss.iter().map(|v| str_of(v, "sessione").to_string()).collect();
    let working: HashSet<String> = ss.iter().filter(|v| str_of(v, "vista") == "lavora").map(|v| str_of(v, "sessione").to_string()).collect();
    let mut prog: HashMap<String, String> = ss.iter().map(|v| (str_of(v, "sessione").to_string(), str_of(&v["progetto"], "id").to_string())).collect();
    for v in crate::archived_sessions() { prog.entry(str_of(&v, "sessione").to_string()).or_insert_with(|| str_of(&v["progetto"], "id").to_string()); }
    let oks: HashSet<String> = crate::approvals::open_oks().into_keys().collect();
    Some((alive, working, prog, oks))
}

pub fn mark_superseded() {
    if !cleanup_on() { return; }
    let Some((alive, working, prog, oks)) = cleanup_data() else { return };
    let extended = extended_cleanup_on();
    let lines = with_registry(|r| apply_cleanup(r, &alive, &working, &prog, &oks, unix_now(), extended));
    for l in &lines { crate::log_juggl(&format!("richieste: {l}")); }
    if !lines.is_empty() { notify(); }
}

/// Decides and marks (without deleting anything). Returns the lines for the log.
pub(crate) fn apply_superseded(r: &mut Value, alive: &HashSet<String>, prog: &HashMap<String, String>, oks: &HashSet<String>, now: f64) -> Vec<String> {
    apply_cleanup(r, alive, &HashSet::new(), prog, oks, now, false)
}

/// Like `apply_superseded`, with the chats working right now and the extended cleanup (off by default).
pub(crate) fn apply_cleanup(r: &mut Value, alive: &HashSet<String>, working: &HashSet<String>, prog: &HashMap<String, String>, oks: &HashSet<String>, now: f64, extended: bool) -> Vec<String> {
    let mut lines = vec![];
    let Some(a) = r["voci"].as_array_mut() else { return lines };
    // duplicate of an item that is no longer open (or no longer exists): it comes back on its own, the mark stays only as history
    let open_ids: HashSet<String> = a.iter().filter(|v| is_open(v)).map(|v| str_of(v, "id").to_string()).collect();
    for v in a.iter_mut() {
        let d = str_of(v, "doppione_di").to_string();
        if !d.is_empty() && (!is_open(v) || !open_ids.contains(&d)) { if let Some(o) = v.as_object_mut() { o.remove("doppione_di"); } }
    }
    // 1) duplicates in the same project
    // same project; without a project, the same chat (0.1.79): two almost equal items in the same chat are the same request
    let project_of = |v: &Value| { let p = prog.get(str_of(v, "sessione")).cloned().unwrap_or_default(); if p.is_empty() && !str_of(v, "sessione").is_empty() { format!("chat:{}", str_of(v, "sessione")) } else { p } };
    let cand: Vec<usize> = (0..a.len()).filter(|&i| is_open(&a[i]) && str_of(&a[i], "doppione_di").is_empty() && a[i]["separata"] != json!(true) && !project_of(&a[i]).is_empty()).collect();
    let mut marked: HashSet<usize> = HashSet::new();
    for (x, &i) in cand.iter().enumerate() {
        if marked.contains(&i) { continue; }
        for &j in &cand[x + 1..] {
            if marked.contains(&j) || marked.contains(&i) || project_of(&a[i]) != project_of(&a[j]) || !similar_titles(str_of(&a[i], "titolo"), str_of(&a[j], "titolo")) { continue; }
            // main one: the most urgent; at equal urgency the oldest
            let (main, dup) = if urgency(&a[j]) > urgency(&a[i]) || (urgency(&a[j]) == urgency(&a[i]) && f64_of(&a[j], "creata") < f64_of(&a[i], "creata")) { (j, i) } else { (i, j) };
            let main_id = str_of(&a[main], "id").to_string();
            lines.push(format!("doppione: \"{}\" segnata come doppione di \"{}\" (stesso progetto, titolo quasi uguale)", str_of(&a[dup], "titolo"), str_of(&a[main], "titolo")));
            a[dup]["doppione_di"] = json!(main_id);
            a[dup]["doppione_juggl"] = json!({"il": now, "motivo": "stesso progetto, titolo quasi uguale"});
            marked.insert(dup);
        }
    }
    // 2) vanished chats (archived, closed, deleted): request in progress or not taken, with no open ok and no "you do it".
    //    After 1 day (0.1.79; before, 7 days on the last change, which the system however touches every day: it never fired).
    //    Never serve_te: a real question to the user is never closed on its own.
    let mut closed_now: Vec<String> = vec![];
    for v in a.iter_mut() {
        if !is_open(v) || !matches!(str_of(v, "stato"), "in_lavoro" | "non_presa") || v["no_auto"] == json!(true) || v["fai_tu"] == json!(true) { continue; }
        if now - f64_of(v, "creata") < DAY { continue; }
        if !str_of(v, "ok_id").is_empty() && oks.contains(str_of(v, "ok_id")) { continue; }
        let mut chat = vec![str_of(v, "sessione").to_string()]; chat.extend(also_in(v));
        if chat.iter().any(|c| alive.contains(c)) { continue; }
        close_by_juggl(v, "la chat di origine è archiviata o chiusa e la richiesta non ha novità", now);
        closed_now.push(str_of(v, "id").to_string());
        lines.push(format!("chiusa da Juggl: \"{}\" (chat di origine archiviata o chiusa)", str_of(v, "titolo")));
    }
    // 3) EXTENDED CLEANUP (switch off by default): idle chat and request older than 3 days with no question to the user.
    //    By default these stay open as kind "vecchia" (in "Mostra altre"): they contain real unfinished work.
    if extended {
        for v in a.iter_mut() {
            if !is_open(v) || !matches!(str_of(v, "stato"), "in_lavoro" | "non_presa") || v["no_auto"] == json!(true) || v["fai_tu"] == json!(true) { continue; }
            if now - f64_of(v, "creata") < 3.0 * DAY || (!str_of(v, "ok_id").is_empty() && oks.contains(str_of(v, "ok_id"))) { continue; }
            let mut chat = vec![str_of(v, "sessione").to_string()]; chat.extend(also_in(v));
            if chat.iter().any(|c| working.contains(c)) { continue; }
            close_by_juggl(v, "aperta da più di 3 giorni, la chat è ferma e non c'è nessuna domanda per te", now);
            closed_now.push(str_of(v, "id").to_string());
            lines.push(format!("chiusa da Juggl (pulizia estesa): \"{}\"", str_of(v, "titolo")));
        }
    }
    if !closed_now.is_empty() { r["ultimo_lotto"] = json!({"il": now, "ids": closed_now}); }
    lines
}

fn close_by_juggl(v: &mut Value, reason: &str, now: f64) {
    v["chiusa_da_juggl"] = json!({"il": now, "motivo": reason, "stato_prima": str_of(v, "stato"), "ferma_prima": v["ferma"].as_bool().unwrap_or(false), "chiusa_da": "auto"});
    v["stato"] = json!("fatta"); v["letta"] = json!(true); v["fatta_il"] = json!(now); v["ferma"] = json!(false);
}

/// "Riapri" / "Separa" of an item closed or marked by Juggl: it goes back to how it was and Juggl no longer touches it.
fn undo_juggl(x: &mut Value) -> bool {
    let mut changed = false;
    if let Some(c) = x.get("chiusa_da_juggl").cloned().filter(|c| c.is_object()) {
        x["stato"] = json!(if str_of(&c, "stato_prima").is_empty() { "in_lavoro" } else { str_of(&c, "stato_prima") });
        x["ferma"] = json!(c["ferma_prima"].as_bool().unwrap_or(false));
        x["fatta_il"] = json!(0); x["no_auto"] = json!(true); x["riaperta_juggl_il"] = json!(unix_now());
        if let Some(o) = x.as_object_mut() { o.remove("chiusa_da_juggl"); }
        changed = true;
    }
    if !str_of(x, "doppione_di").is_empty() || x.get("doppione_juggl").is_some() {
        if let Some(o) = x.as_object_mut() { o.remove("doppione_di"); o.remove("doppione_juggl"); }
        x["separata"] = json!(true); changed = true;
    }
    changed
}

/// "Pulizia automatica delle richieste" switch (Settings > Requests).
#[tauri::command]
pub fn richieste_pulizia_imposta(acceso: bool) -> Value {
    let r = crate::storage::modify(&base_dir().join("impostazioni.json"), Some(json!({})), |d| { d["pulizia_richieste"] = json!(acceso); true });
    match r { Ok(_) => json!({"ok": true, "acceso": acceso}), Err(e) => json!({"ok": false, "msg": e}) }
}

/// "Pulizia estesa" switch (off by default): also closes requests older than 3 days with an idle chat and no questions.
#[tauri::command]
pub fn richieste_pulizia_estesa_imposta(acceso: bool) -> Value {
    let r = crate::storage::modify(&base_dir().join("impostazioni.json"), Some(json!({})), |d| { d["pulizia_estesa"] = json!(acceso); true });
    match r { Ok(_) => json!({"ok": true, "acceso": acceso}), Err(e) => json!({"ok": false, "msg": e}) }
}

/// "Annulla ultima pulizia": reopens the requests closed by the last round that closed any (and never closes them again).
#[tauri::command(async)]
pub fn richieste_annulla_pulizia() -> Value {
    let n = with_registry(|r| {
        let ids: HashSet<String> = r["ultimo_lotto"]["ids"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let mut n = 0;
        if let Some(a) = r["voci"].as_array_mut() { for v in a.iter_mut() { if ids.contains(str_of(v, "id")) && undo_juggl(v) { n += 1; } } }
        r["ultimo_lotto"] = json!(null);
        n
    });
    notify();
    json!({"ok": true, "riaperte": n})
}

/// Cleanup state for the Settings: switches, last batch and how many open requests per kind.
#[tauri::command(async)]
pub fn richieste_pulizia_stato() -> Value {
    let reg = registry();
    let mut by_kind: HashMap<String, u32> = HashMap::new();
    let mut vv: Vec<Value> = reg["voci"].as_array().cloned().unwrap_or_default();
    with_ok(&mut vv);
    with_kind(&mut vv, None);
    for v in &vv { let t = str_of(v, "tipo"); if !t.is_empty() { *by_kind.entry(t.to_string()).or_default() += 1; } }
    let l = &reg["ultimo_lotto"];
    json!({"ok": true, "accesa": cleanup_on(), "estesa": extended_cleanup_on(), "per_tipo": by_kind,
        "ultimo_lotto": if l.is_object() { json!({"il": l["il"], "n": l["ids"].as_array().map(|a| a.len()).unwrap_or(0)}) } else { Value::Null }})
}

/// Sample without writing anything: how many requests the cleanup would close now (basic and with the extended cleanup) and some examples.
#[tauri::command(async)]
pub fn richieste_campione_pulizia() -> Value {
    let Some((alive, working, prog, oks)) = cleanup_data() else { return json!({"ok": false, "msg": "sessioni non disponibili"}) };
    let reg = registry();
    let open_count = reg["voci"].as_array().map(|a| a.iter().filter(|v| is_open(v)).count()).unwrap_or(0);
    let closable = |extended: bool| -> Vec<Value> {
        let mut copy = reg.clone();
        let before: HashSet<String> = copy["voci"].as_array().map(|a| a.iter().filter(|v| is_open(v)).map(|v| str_of(v, "id").to_string()).collect()).unwrap_or_default();
        apply_cleanup(&mut copy, &alive, &working, &prog, &oks, unix_now(), extended);
        copy["voci"].as_array().map(|a| a.iter().filter(|v| before.contains(str_of(v, "id")) && !is_open(v)).map(|v| json!({"titolo": str_of(v, "titolo"), "nome": str_of(v, "nome"), "motivo": str_of(&v["chiusa_da_juggl"], "motivo"), "stato_prima": str_of(&v["chiusa_da_juggl"], "stato_prima")})).collect()).unwrap_or_default()
    };
    let basic = closable(false);
    let extended = closable(true);
    json!({"ok": true, "aperte": open_count, "chiuderebbe": basic.len(), "chiuderebbe_con_estesa": extended.len(),
        "esempi": basic.iter().take(20).cloned().collect::<Vec<_>>(), "esempi_estesa": extended.iter().skip(basic.len()).take(20).cloned().collect::<Vec<_>>()})
}

// ---------- commands ----------

/// Current name of the live sessions, for the items and for "anche in".
fn with_names(items: &mut [Value], alive: &HashMap<String, Value>) {
    for x in items.iter_mut() {
        if let Some(v) = alive.get(str_of(x, "sessione")) {
            x["nome"] = json!(str_of(v, "nome"));
            x["progetto"] = v.get("progetto").cloned().unwrap_or(Value::Null);
            x["vista"] = json!(str_of(v, "vista"));
            x["viva"] = json!(true);
        } else { x["viva"] = json!(false); x["progetto"] = Value::Null; x["vista"] = json!("chiusa"); }
        if let Some(a) = x["anche_in"].as_array_mut() {
            for y in a.iter_mut() {
                if let Some(v) = alive.get(str_of(y, "sessione")) { y["nome"] = json!(str_of(v, "nome")); y["viva"] = json!(true); } else { y["viva"] = json!(false); }
            }
        }
    }
}

/// The ok a request is linked to (ok_id), if still open: the interface puts only a pointer to it.
fn with_ok(items: &mut [Value]) {
    let oks = crate::approvals::open_oks();
    for x in items.iter_mut() {
        let o = if is_open(x) { oks.get(str_of(x, "ok_id")) } else { None };
        x["aspetta_ok"] = match o { Some(o) => json!({"id": str_of(o, "id"), "titolo": str_of(o, "titolo"), "fai_tu": o["fai_tu"].as_bool().unwrap_or(false), "stato": str_of(o, "stato")}), None => Value::Null };
    }
}

/// What kind of request it is (0.1.79), for the bar: what to show and what to leave in "Mostra altre".
///  decisione: a real question to the user (serve te not promoted by the system, or an open linked ok);
///  lavoro_in_corso: work that is going on (or that is the user's own: "you do it");
///  appesa: stalled work or promoted by the system, with no question to the user (no "Scrivi la risposta");
///  vecchia: open for more than 3 days (7 for a decision) with an idle chat: goes into "Mostra altre".
/// Empty for done ones. `chat_working`: the origin chat is working right now (it is not old, it is long).
pub(crate) fn kind_of(v: &Value, now: f64, chat_working: bool) -> &'static str {
    if !is_open(v) { return ""; }
    let age = if f64_of(v, "creata") > 0.0 { (now - f64_of(v, "creata")) / DAY } else { 0.0 };
    if v["aspetta_ok"].is_object() { return "decisione"; }
    if v["fai_tu"] == json!(true) { return "lavoro_in_corso"; }   // unfinished work of the user's own: never "vecchia", never closed
    let is_decision = str_of(v, "stato") == "serve_te" && v["appesa"] != json!(true) && !promoted_by_system(str_of(v, "motivo"));
    if is_decision { return if age > 7.0 { "vecchia" } else { "decisione" }; }
    if age > 3.0 && !chat_working { return "vecchia"; }
    if v["appesa"] == json!(true) || v["ferma"] == json!(true) { return "appesa"; }
    "lavoro_in_corso"
}

fn with_kind(items: &mut [Value], alive: Option<&HashMap<String, Value>>) {
    let now = unix_now();
    for x in items.iter_mut() {
        let working = alive.map(|m| { let mut ch = vec![str_of(x, "sessione").to_string()]; ch.extend(also_in(x)); ch.iter().any(|c| m.get(c).map(|v| str_of(v, "vista") == "lavora").unwrap_or(false)) }).unwrap_or(false);
        let t = kind_of(x, now, working);
        x["tipo"] = json!(t);
    }
}

/// All items, with name, project and logo taken from the live sessions.
pub fn all_requests() -> Value {
    let mut ss = crate::list_sessions();
    ss.extend(local_models::as_sessions());
    let alive: HashMap<String, Value> = ss.into_iter().map(|v| (str_of(&v, "sessione").to_string(), v)).collect();
    let mut items = all_items();
    with_names(&mut items, &alive);
    with_ok(&mut items);
    with_kind(&mut items, Some(&alive));
    // archived sessions: their requests stay, with name and project, marked "archiviata" (no reminders)
    let arch: HashMap<String, Value> = crate::archived_sessions().into_iter().map(|v| (str_of(&v, "sessione").to_string(), v)).collect();
    for x in items.iter_mut() {
        if x["viva"] == json!(true) { continue; }
        if let Some(v) = arch.get(str_of(x, "sessione")) { x["nome"] = json!(str_of(v, "nome")); x["progetto"] = v["progetto"].clone(); x["vista"] = json!("archiviata"); }
    }
    items.sort_by(|a, b| f64_of(b, "aggiornata").partial_cmp(&f64_of(a, "aggiornata")).unwrap_or(std::cmp::Ordering::Equal));
    let analyzing: Vec<String> = queues().lock().unwrap_or_else(|e| e.into_inner()).occupate.iter().cloned().collect();
    json!({"voci": items, "ora": unix_now(), "in_analisi": analyzing})
}

/// The items of a session, also those that arrived from other sessions (for the strip and the labels under the messages).
pub fn of_session(sid: &str) -> Value { of_session_since(sid, 0.0) }

/// fast chats (14/11): the chat's items without copying the whole registry (and without the lock: the file is replaced
/// whole, the read is always of a complete version). `since` > 0: only open ones, unseen answers and those
/// touched since that time (the messages of the window sent to the interface).
pub fn of_session_since(sid: &str, since: f64) -> Value {
    let reg = crate::fast_open::json(&registry_path()).unwrap_or_else(|| std::sync::Arc::new(json!({})));
    let chosen: Vec<Value> = reg["voci"].as_array().map(|a| a.iter().filter(|v| concerns(v, sid)).cloned().collect()).unwrap_or_default();
    // the same rules as registry() (states expiring over time, migrations), on the chat's items only
    let mut r = json!({"versione": reg["versione"].clone(), "voci": chosen});
    apply_rules(&mut r);
    let mut items: Vec<Value> = r["voci"].as_array().cloned().unwrap_or_default();
    if since > 0.0 {
        items.retain(|v| is_open(v) || new_reply(v) || f64_of(v, "creata").max(f64_of(v, "aggiornata")).max(done_at(v)) >= since);
    }
    items.sort_by(|a, b| f64_of(b, "creata").partial_cmp(&f64_of(a, "creata")).unwrap_or(std::cmp::Ordering::Equal));
    with_ok(&mut items);
    with_kind(&mut items, None);
    json!(items)
}

/// For each session: new answers (done and not yet seen) and requests waiting for the user.
pub fn counts() -> HashMap<String, (usize, usize)> {
    let mut m: HashMap<String, (usize, usize)> = HashMap::new();
    for v in all_items() {
        if new_reply(&v) {
            let target = if str_of(&v, "risposta_sessione").is_empty() { str_of(&v, "sessione") } else { str_of(&v, "risposta_sessione") };
            m.entry(target.to_string()).or_default().0 += 1;
        } else if str_of(&v, "stato") == "serve_te" {
            let mut ss = vec![str_of(&v, "sessione").to_string()];
            ss.extend(also_in(&v));
            for x in ss { m.entry(x).or_default().1 += 1; }
        }
    }
    m
}

/// The user opened the chat: its new answers become seen.
pub fn mark_read(sid: &str) -> Value {
    let n = with_registry(|r| {
        let mut n = 0;
        if let Some(a) = r["voci"].as_array_mut() {
            for v in a.iter_mut() {
                if !new_reply(v) { continue; }
                let target = if str_of(v, "risposta_sessione").is_empty() { str_of(v, "sessione").to_string() } else { str_of(v, "risposta_sessione").to_string() };
                if target == sid { v["letta"] = json!(true); v["letta_il"] = json!(unix_now()); n += 1; }
            }
        }
        n
    });
    if n > 0 { notify(); }
    json!({"ok": true, "lette": n})
}

/// The session to address for a request: the last one that worked on it (or ignored it), otherwise its own.
fn session_for(v: &Value) -> String {
    for k in ["ultima_sessione", "non_presa_in"] {
        let x = str_of(v, k);
        if !x.is_empty() && concerns(v, x) && live_session(x).is_some() { return x.to_string(); }
    }
    str_of(v, "sessione").to_string()
}

/// The user's actions on a request: letta, non_va, riapri, ricordaglielo, chiedi, chiudi.
pub fn action(id: &str, action: &str) -> Value {
    let Some(v) = ({ let r = registry(); let id = resolve(&r, id); r["voci"].as_array().and_then(|a| a.iter().find(|v| str_of(v, "id") == id).cloned()) })
        else { return json!({"ok": false, "msg": "richiesta non trovata"}) };
    let id = str_of(&v, "id").to_string();
    let target = session_for(&v);
    let change = |fun: &dyn Fn(&mut Value)| {
        with_registry(|r| { if let Some(x) = item_mut(r, &id) { fun(x); x["aggiornata"] = json!(unix_now()); } });
        notify();
    };
    let change_no_date = |id: &str, fun: &dyn Fn(&mut Value)| {
        with_registry(|r| { if let Some(x) = item_mut(r, id) { fun(x); } });
        notify();
    };
    match action {
        "letta" => { change_no_date(&id, &|x| { x["letta"] = json!(true); x["letta_il"] = json!(unix_now()); }); json!({"ok": true}) }
        "chiudi" => {
            // discarded: no longer needed, leaves everything without disturbing
            change(&|x| { x["stato"] = json!("fatta"); x["scartata"] = json!(true); x["letta"] = json!(true); x["fatta_il"] = json!(unix_now());
                x["ferma"] = json!(false); x["attesa_da"] = json!(0); x["manuale"] = json!(""); });
            json!({"ok": true})
        }
        "fatto" => {
            // "you do it": the user did it. The session knows (internal message, like the oks' Fatto button) and the request is closed.
            let Some(msg) = crate::approvals::text_for("ok", str_of(&v, "titolo"), true, "") else { return json!({"ok": false}) };
            let r = crate::send_internal(target.clone(), msg);
            if r["ok"].as_bool().unwrap_or(false) {
                change(&|x| { x["stato"] = json!("fatta"); x["letta"] = json!(true); x["fatta_il"] = json!(unix_now()); x["fai_tu"] = json!(false);
                    x["ferma"] = json!(false); x["attesa_da"] = json!(0); x["risposta"] = json!("L'hai fatto tu."); });
            }
            r
        }
        "non_va" | "riapri" => {
            change(&|x| { x["stato"] = json!("in_lavoro"); x["manuale"] = json!("riaperta"); x["manuale_t"] = json!(unix_now()); x["ferma"] = json!(false);
                x["scartata"] = json!(false); x["letta"] = json!(true); x["sollecitata_t"] = json!(0); x["ricordata"] = json!(false); });
            json!({"ok": true, "sessione": target, "titolo": str_of(&v, "titolo")})
        }
        "ricordaglielo" => {
            // the message goes out as "internal": it does not become a new request
            let shared_count = all_items().iter().filter(|x| str_of(x, "id") != id && x["messaggi"].as_array().map(|a| a.iter().any(|k| v["messaggi"].as_array().map(|b| b.contains(k)).unwrap_or(false))).unwrap_or(false)).count();
            let text = if shared_count > 0 || str_of(&v, "testo").trim().is_empty() {
                format!("{REMIND_PREFIX}{}.\n\nDal mio messaggio: \"{}\"", str_of(&v, "titolo"), str_of(&v, "testo").trim())
            } else { format!("{REMIND_PREFIX}{}", str_of(&v, "testo").trim()) };
            let r = crate::send_internal(target.clone(), text);
            if r["ok"].as_bool().unwrap_or(false) {
                let d = target.clone();
                change(&|x| { x["stato"] = json!("in_lavoro"); x["attesa_da"] = json!(unix_now() - 5.0); x["ricordata_il"] = json!(unix_now()); x["ferma"] = json!(false);
                    x["ultima_sessione"] = json!(d.clone()); });
            }
            r
        }
        "riapri_juggl" => {
            // list (15/11): undoes a closing or a "duplicate" decided by Juggl
            let mut undone = false;
            with_registry(|r| { if let Some(x) = item_mut(r, &id) { undone = undo_juggl(x); if undone { x["aggiornata"] = json!(unix_now()); } } });
            if undone { crate::log_juggl(&format!("richieste: l'utente ha annullato la scelta di Juggl su \"{}\"", str_of(&v, "titolo"))); notify(); }
            json!({"ok": undone, "msg": if undone { "" } else { "niente da annullare" }})
        }
        "chiedi" => {
            let text = format!("{ASK_PREFIX} {}? Rispondi in breve: fatto, in corso (cosa manca) o bloccato (cosa ti serve).", str_of(&v, "titolo"));
            let r = crate::send_internal(target, text);
            if r["ok"].as_bool().unwrap_or(false) {
                with_registry(|reg| { if let Some(x) = item_mut(reg, &id) { x["chiesto_il"] = json!(unix_now()); } });
                notify();
            }
            r
        }
        _ => json!({"ok": false, "msg": "azione sconosciuta"}),
    }
}

/// Delete chat: its requests go away too, and the references in "anche in" (if the request also lives elsewhere, it moves there).
pub fn remove_session(sid: &str) {
    with_registry(|r| {
        if let Some(a) = r["voci"].as_array_mut() {
            for v in a.iter_mut() {
                if let Some(ai) = v["anche_in"].as_array_mut() { ai.retain(|x| str_of(x, "sessione") != sid); }
                if let Some(m) = v["messaggi"].as_array_mut() { m.retain(|k| !k.as_str().unwrap_or("").starts_with(&format!("{sid}:"))); }
                for k in ["ultima_sessione", "non_presa_in", "risposta_sessione"] { if str_of(v, k) == sid { v[k] = json!(""); } }
                if str_of(v, "sessione") == sid {
                    let next = v["anche_in"].as_array_mut().filter(|a| !a.is_empty()).map(|a| a.remove(0));
                    match next {
                        Some(n) => { v["sessione"] = json!(str_of(&n, "sessione")); v["nome"] = json!(str_of(&n, "nome")); }
                        None => v["via"] = json!(true),
                    }
                }
            }
            a.retain(|v| v["via"] != json!(true));
        }
        if let Some(o) = r["visti"].as_object_mut() { o.retain(|k, _| !k.starts_with(&format!("{sid}:"))); }
        if let Some(a) = r["popolate"].as_array_mut() { a.retain(|x| x.as_str() != Some(sid)); }
    });
}

#[cfg(test)]
mod tests_superseded {
    use super::*;
    fn item(id: &str, sid: &str, title: &str, state: &str, age_days: f64, now: f64) -> Value {
        json!({"id": id, "sessione": sid, "titolo": title, "testo": format!("testo di {title}"), "stato": state, "volte": 1, "anche_in": [],
            "messaggi": [], "creata": now - age_days * DAY - 60.0, "aggiornata": now - age_days * DAY})
    }
    fn maps() -> (HashSet<String>, HashMap<String, String>) {
        let alive: HashSet<String> = ["s1", "s2", "s3"].iter().map(|x| x.to_string()).collect();
        let prog: HashMap<String, String> = [("s1", "juggl"), ("s2", "juggl"), ("s9", "juggl"), ("s3", "alfa")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        (alive, prog)
    }
    #[test]
    fn nearly_equal_titles() {
        assert!(similar_titles("Logo Rossi nuovo", "Nuovo logo Rossi"));
        assert!(similar_titles("Raggruppare richieste progetto smart", "Raggruppare richieste progetto"));
        assert!(!similar_titles("Logo Rossi", "Sito Rossi"));
        assert!(!similar_titles("Pubblica la 0.1.60", "Pubblica la 0.1.61"));
    }
    #[test]
    fn different_versions_never_duplicates() {
        let t = 1_000_000_000.0; let (alive, prog) = maps();
        let mut r = json!({"voci": [item("a", "s1", "Pubblica la 0.1.60", "in_lavoro", 1.0, t), item("b", "s2", "Pubblica la 0.1.61", "in_lavoro", 0.5, t)]});
        let lines = apply_superseded(&mut r, &alive, &prog, &HashSet::new(), t);
        assert!(lines.is_empty());
        assert!(r["voci"].as_array().unwrap().iter().all(|v| v.get("doppione_di").is_none()));
    }
    #[test]
    fn duplicate_marked_without_losing_anything_and_serve_te_stays() {
        let t = 1_000_000_000.0; let (alive, prog) = maps();
        let mut r = json!({"voci": [item("a", "s1", "Logo Rossi nuovo", "in_lavoro", 3.0, t), item("b", "s2", "Nuovo logo Rossi", "serve_te", 1.0, t), item("c", "s3", "Nuovo logo Rossi", "in_lavoro", 1.0, t)]});
        let lines = apply_superseded(&mut r, &alive, &prog, &HashSet::new(), t);
        let v = r["voci"].as_array().unwrap();
        assert_eq!(v.len(), 3, "niente cancellato");
        assert_eq!(str_of(&v[0], "doppione_di"), "b", "la principale e' quella in serve_te");
        assert!(v[1].get("doppione_di").is_none() && str_of(&v[1], "stato") == "serve_te");
        assert!(v[2].get("doppione_di").is_none(), "progetto diverso: non e' un doppione");
        assert_eq!(str_of(&v[0], "testo"), "testo di Logo Rossi nuovo");
        assert_eq!(lines.len(), 1);
        // Separa: comes back on its own and is never marked again
        let mut x = v[0].clone(); assert!(undo_juggl(&mut x)); r["voci"][0] = x;
        assert!(apply_superseded(&mut r, &alive, &prog, &HashSet::new(), t).is_empty());
    }
    #[test]
    fn vanished_chat_closed_only_if_old_and_reopenable() {
        let t = 1_000_000_000.0; let (alive, prog) = maps();
        let mut open_ok = item("d", "s9", "Approvare il logo", "in_lavoro", 10.0, t); open_ok["ok_id"] = json!("ok1");
        let mut r = json!({"voci": [
            item("a", "s9", "Vecchia cosa di una chat sparita", "in_lavoro", 10.0, t),
            item("b", "s9", "Recente di una chat sparita", "in_lavoro", 0.3, t),
            item("c", "s9", "Serve te in chat sparita", "serve_te", 10.0, t),
            open_ok,
            item("e", "s1", "Vecchia ma la chat c'e'", "in_lavoro", 10.0, t)]});
        let oks: HashSet<String> = ["ok1".to_string()].into_iter().collect();
        let lines = apply_superseded(&mut r, &alive, &prog, &oks, t);
        let v = r["voci"].as_array().unwrap();
        assert_eq!(v.len(), 5, "niente cancellato");
        assert_eq!(str_of(&v[0], "stato"), "fatta"); assert!(v[0]["chiusa_da_juggl"].is_object());
        for i in 1..5 { assert_ne!(str_of(&v[i], "stato"), "fatta", "voce {i} non doveva chiudersi"); }
        assert_eq!(lines.len(), 1);
        // those closed by Juggl stay even after the 14 days of done items
        let mut r2 = r.clone(); r2["versione"] = json!(VERSION); r2["voci"][0]["fatta_il"] = json!(1.0); apply_rules(&mut r2);
        assert!(r2["voci"].as_array().unwrap().iter().any(|x| str_of(x, "id") == "a"));
        // Riapri: previous state, and it does not close itself again
        let mut x = v[0].clone(); assert!(undo_juggl(&mut x));
        assert_eq!(str_of(&x, "stato"), "in_lavoro"); assert!(x.get("chiusa_da_juggl").is_none());
        r["voci"][0] = x;
        assert!(apply_superseded(&mut r, &alive, &prog, &oks, t).is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_ok_is_not_serve_te() {
        let oks = vec![json!({"id": "k41", "titolo": "Dare l'ok a git init", "stato": "aperta"}), json!({"id": "k43", "titolo": "Provare la build", "fai_tu": true, "stato": "aperta"})];
        let mut v = json!({"stato": "serve_te", "motivo": "ignorata due volte, anche dopo il promemoria"});
        set_state(&mut v, &json!({"stato": "serve_te", "ok": "k41"}), &oks);
        assert_eq!(str_of(&v, "stato"), "in_lavoro");
        assert_eq!(str_of(&v, "ok_id"), "k41");
        assert_eq!(str_of(&v, "motivo"), "");
        // ok that does not exist: stays serve te, and "you do it" if the model says so
        set_state(&mut v, &json!({"stato": "serve_te", "ok": "k99", "fai_tu": true}), &oks);
        assert_eq!(str_of(&v, "stato"), "serve_te");
        assert_eq!(str_of(&v, "ok_id"), "");
        assert_eq!(v["fai_tu"], json!(true));
        // done wins over the link
        set_state(&mut v, &json!({"stato": "fatta", "ok": "k41"}), &oks);
        assert_eq!(str_of(&v, "stato"), "fatta");
        assert_eq!(str_of(&v, "ok_id"), "");
        assert_eq!(v["fai_tu"], json!(false));
    }

    #[test]
    fn reminder_only_if_arrived() {
        let ev = vec![json!({"tipo": "tu", "testo": "fai X", "t": "2026-11-08T10:00:00.000Z"}), json!({"tipo": "agente", "testo": "lavoro su Y", "t": "2026-11-08T10:05:00.000Z"})];
        let since = from_iso("2026-11-08T10:06:00.000Z");
        assert!(!reminder_read(&ev, since));
        let mut ev2 = ev.clone();
        ev2.push(json!({"tipo": "tu", "testo": format!("{}, fatta mentre lavoravi ad altro: X.", crate::profile::with_name(REMINDER_PREFIX[0])), "t": "2026-11-08T10:07:00.000Z"}));
        assert!(!reminder_read(&ev2, since));
        ev2.push(json!({"tipo": "agente", "testo": "X era gia' fatta", "t": "2026-11-08T10:08:00.000Z"}));
        assert!(reminder_read(&ev2, since));
    }

    /// On real data, without touching the app's registry and without writing to sessions: re-evaluates with the new rules
    /// the items of the sessions in RIVALUTA (comma-separated ids) on the registry in JUGGL_RICHIESTE_FILE (a copy).
    /// cargo test --release requests::tests::real_reevaluation -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_reevaluation() {
        let p = std::env::var("JUGGL_RICHIESTE_FILE").expect("serve JUGGL_RICHIESTE_FILE (una copia)");
        assert!(!p.ends_with("tavolo/richieste-ai.json"), "non sul registro dell'app");
        let sids: Vec<String> = std::env::var("RIVALUTA").expect("serve RIVALUTA").split(',').map(String::from).collect();
        let full = |sid: &str| crate::find_session(sid).map(|v| str_of(&v, "sessione").to_string()).unwrap_or_else(|| sid.to_string());
        let sids: Vec<String> = sids.iter().map(|x| full(x)).collect();
        with_registry(|r| { if let Some(a) = r["voci"].as_array_mut() { for v in a.iter_mut() { if sids.contains(&str_of(v, "sessione").to_string()) && (is_open(v) || unix_now() - done_at(v) < DAY) { v["da_rivalutare"] = json!(true); } } } });
        for sid in &sids { take_slot(); let t = unix_now(); reevaluate(sid); eprintln!("rivalutata {} in {:.0}s", &sid[..8], unix_now() - t); }
        let mut vv: Vec<Value> = all_items().into_iter().filter(|v| sids.contains(&str_of(v, "sessione").to_string()) && (is_open(v) || unix_now() - done_at(v) < 3.0 * HOUR_S)).collect();
        with_ok(&mut vv);
        // for the strip screenshots: the chat detail as the interface receives it (requests, oks, steps)
        if let Ok(dir) = std::env::var("FASCIA_DIR") {
            for sid in &sids {
                let Some(v) = crate::find_session(sid) else { continue };
                let d = crate::session_detail_pub(&v);
                let out = json!({"sessione": sid, "nome": str_of(&v, "nome"), "vista": str_of(&v, "vista"), "passi": d["passi"], "coda": [], "richieste_ai": of_session(sid), "ok": crate::approvals::of_session(sid)});
                let _ = std::fs::write(format!("{dir}/fascia-{}.json", &sid[..8]), serde_json::to_string_pretty(&out).unwrap());
            }
        }
        for v in vv { eprintln!("{} [{}{}{}] {} :: {}", str_of(&v, "id"), str_of(&v, "stato"), if v["aspetta_ok"].is_object() { format!(" -> {}", str_of(&v["aspetta_ok"], "id")) } else { String::new() }, if v["fai_tu"] == json!(true) { " tu" } else { "" }, str_of(&v, "titolo"), str_of(&v, "risposta")); }
    }
}

#[cfg(test)]
mod tests_kinds_and_cleanup {
    use super::*;
    const T: f64 = 1_000_000_000.0;
    fn item(id: &str, sid: &str, title: &str, state: &str, created_age: f64) -> Value {
        json!({"id": id, "sessione": sid, "titolo": title, "testo": format!("testo di {title}"), "stato": state, "volte": 1, "anche_in": [],
            "messaggi": [], "creata": T - created_age * DAY, "aggiornata": T - 0.1 * DAY})
    }
    #[test]
    fn automatic_messages_are_not_requests() {
        let name = crate::profile::with_name("«NOME»");
        assert!(!from_user("Messaggio automatico di Juggl: la chat e' stata riavviata"));
        assert!(!from_user("[Da Juggl] Juggl è stato riavviato e la tua chat si è fermata con un lavoro a metà"));
        assert!(!from_user("[Da Juggl · regia] La 0.1.64 è installata"));
        assert!(from_user(&format!("[Da Juggl · regia, ordine di {name} 14/11: fase 2 della revisione]")), "un ordine relayato resta una richiesta");
        assert!(from_user("Aggiungi una pagina al sito"));
        assert!(!from_user("A che punto sei con: logo?"));
    }

    #[test]
    fn chat_to_chat_messages_closed_unless_ok_or_user_task() {
        let mut r = json!({"versione": VERSION, "voci": [
            {"id": "a", "sessione": "s1", "titolo": "Da chat", "stato": "in_lavoro", "testo": "Regia: ok al campione", "creata": unix_now() - DAY, "aggiornata": unix_now(), "anche_in": [], "messaggi": []},
            {"id": "b", "sessione": "s1", "titolo": "Con ok", "stato": "in_lavoro", "testo": "Regia: ok al campione", "ok_id": "k1", "creata": unix_now() - DAY, "aggiornata": unix_now(), "anche_in": [], "messaggi": []},
            {"id": "c", "sessione": "s1", "titolo": "Lo fai tu", "stato": "serve_te", "fai_tu": true, "testo": "Da elenco chiaro (4209182d): pronto", "creata": unix_now() - DAY, "aggiornata": unix_now(), "anche_in": [], "messaggi": []},
            {"id": "d", "sessione": "s1", "titolo": "Gia chiusa per errore", "stato": "fatta", "fai_tu": true, "testo": "Regia: ok", "ok_id": "k2", "creata": unix_now() - DAY, "aggiornata": unix_now(), "anche_in": [], "messaggi": [],
             "chiusa_da_juggl": {"il": 1.0, "motivo": "nata da un messaggio tra chat o automatico, non da una tua richiesta", "stato_prima": "in_lavoro", "ferma_prima": false}},
        ]});
        apply_rules(&mut r);
        let v = r["voci"].as_array().unwrap();
        assert_eq!(str_of(&v[0], "stato"), "fatta", "messaggio tra chat: chiusa");
        assert_eq!(str_of(&v[1], "stato"), "in_lavoro", "con un ok collegato: resta");
        assert_eq!(str_of(&v[2], "stato"), "serve_te", "lo fai tu: resta");
        assert_eq!(str_of(&v[3], "stato"), "in_lavoro", "chiusa per errore: riaperta da sola");
    }

    #[test]
    fn nudge_no_longer_promotes_to_serve_te() {
        let mut r = json!({"versione": VERSION, "voci": [{"id": "a", "sessione": "s1", "titolo": "Lavoro", "stato": "in_lavoro", "creata": unix_now() - 2.0 * DAY,
            "aggiornata": unix_now() - 2.0 * DAY, "sollecitata_t": unix_now() - 1.5 * DAY, "anche_in": [], "messaggi": []}]});
        apply_rules(&mut r);
        let v = &r["voci"][0];
        assert_eq!(str_of(v, "stato"), "in_lavoro", "resta in lavoro");
        assert_eq!(v["appesa"], json!(true)); assert_eq!(v["ferma"], json!(true));
    }

    #[test]
    fn already_promoted_become_hanging_work_and_real_questions_stay() {
        let mut r = json!({"versione": VERSION, "voci": [
            {"id": "p", "sessione": "s1", "titolo": "Promossa", "stato": "serve_te", "motivo": "ferma da piu' di un giorno, non risponde al sollecito", "creata": unix_now() - DAY, "aggiornata": unix_now(), "anche_in": [], "messaggi": []},
            {"id": "i", "sessione": "s1", "titolo": "Ignorata", "stato": "serve_te", "motivo": "ignorata due volte, anche dopo il promemoria", "creata": unix_now() - DAY, "aggiornata": unix_now(), "anche_in": [], "messaggi": []},
            {"id": "v", "sessione": "s1", "titolo": "Domanda vera", "stato": "serve_te", "motivo": "", "creata": unix_now() - DAY, "aggiornata": unix_now(), "anche_in": [], "messaggi": []},
            {"id": "o", "sessione": "s1", "titolo": "Con ok", "stato": "serve_te", "motivo": "ferma da piu' di un giorno, non risponde al sollecito", "ok_id": "k1", "creata": unix_now() - DAY, "aggiornata": unix_now(), "anche_in": [], "messaggi": []},
        ]});
        apply_rules(&mut r);
        let v = r["voci"].as_array().unwrap();
        assert_eq!(str_of(&v[0], "stato"), "in_lavoro"); assert_eq!(v[0]["appesa"], json!(true)); assert_eq!(str_of(&v[0], "motivo"), "");
        assert_eq!(str_of(&v[1], "stato"), "non_presa");
        assert_eq!(str_of(&v[2], "stato"), "serve_te", "la domanda vera a l'utente non si tocca");
        assert_eq!(str_of(&v[3], "stato"), "serve_te", "con un ok collegato resta com'e'");
    }

    #[test]
    fn kinds() {
        let mut ok = item("a", "s", "t", "serve_te", 1.0); ok["aspetta_ok"] = json!({"id": "k"});
        assert_eq!(kind_of(&ok, T, false), "decisione");
        assert_eq!(kind_of(&item("b", "s", "t", "serve_te", 1.0), T, false), "decisione");
        assert_eq!(kind_of(&item("b", "s", "t", "serve_te", 9.0), T, false), "vecchia", "una domanda vera di 9 giorni: vecchia, mai chiusa");
        let mut by_user = item("c", "s", "t", "serve_te", 9.0); by_user["fai_tu"] = json!(true);
        assert_eq!(kind_of(&by_user, T, false), "lavoro_in_corso");
        assert_eq!(kind_of(&item("d", "s", "t", "in_lavoro", 1.0), T, false), "lavoro_in_corso");
        let mut hanging = item("e", "s", "t", "in_lavoro", 1.0); hanging["appesa"] = json!(true);
        assert_eq!(kind_of(&hanging, T, false), "appesa");
        assert_eq!(kind_of(&item("f", "s", "t", "in_lavoro", 5.0), T, false), "vecchia");
        assert_eq!(kind_of(&item("f", "s", "t", "in_lavoro", 5.0), T, true), "lavoro_in_corso", "chat al lavoro adesso: non e' vecchia");
        assert_eq!(kind_of(&item("g", "s", "t", "fatta", 5.0), T, false), "");
        let mut promoted = item("h", "s", "t", "serve_te", 1.0); promoted["motivo"] = json!("ferma da piu' di un giorno, non risponde al sollecito");
        assert_ne!(kind_of(&promoted, T, false), "decisione", "una serve_te promossa dal sistema non e' una decisione");
    }

    #[test]
    fn basic_close_only_vanished_chats_and_never_serve_te() {
        let alive: HashSet<String> = ["viva"].iter().map(|x| x.to_string()).collect();
        let prog: HashMap<String, String> = HashMap::new();
        let mut by_user = item("e", "morta", "Lo fai tu", "in_lavoro", 5.0);
        by_user["fai_tu"] = json!(true);
        let mut r = json!({"voci": [
            item("a", "morta", "Lavoro di chat archiviata", "in_lavoro", 2.0),
            item("b", "morta", "Domanda vera di chat archiviata", "serve_te", 20.0),
            item("c", "viva", "Lavoro con chat viva ferma da giorni", "in_lavoro", 6.0),
            item("d", "morta", "Appena nata", "in_lavoro", 0.3),
            by_user,
        ]});
        let lines = apply_cleanup(&mut r, &alive, &HashSet::new(), &prog, &HashSet::new(), T, false);
        let v = r["voci"].as_array().unwrap();
        assert_eq!(str_of(&v[0], "stato"), "fatta"); assert_eq!(str_of(&v[0]["chiusa_da_juggl"], "chiusa_da"), "auto");
        assert_eq!(str_of(&v[1], "stato"), "serve_te", "una domanda vera a l'utente non si chiude mai");
        assert_eq!(str_of(&v[2], "stato"), "in_lavoro", "senza pulizia estesa la chat viva resta");
        assert_eq!(str_of(&v[3], "stato"), "in_lavoro", "meno di un giorno");
        assert_eq!(str_of(&v[4], "stato"), "in_lavoro", "lo fai tu");
        assert_eq!(lines.len(), 1);
        assert_eq!(r["ultimo_lotto"]["ids"], json!(["a"]));
    }

    #[test]
    fn extended_cleanup_off_by_default_and_skips_working_chats() {
        let alive: HashSet<String> = ["viva", "lavora"].iter().map(|x| x.to_string()).collect();
        let working: HashSet<String> = ["lavora"].iter().map(|x| x.to_string()).collect();
        let mk = || json!({"voci": [item("c", "viva", "Ferma da giorni", "in_lavoro", 6.0), item("d", "lavora", "Chat al lavoro", "in_lavoro", 6.0)]});
        let mut r = mk();
        apply_cleanup(&mut r, &alive, &working, &HashMap::new(), &HashSet::new(), T, false);
        assert_eq!(str_of(&r["voci"][0], "stato"), "in_lavoro");
        let mut r = mk();
        apply_cleanup(&mut r, &alive, &working, &HashMap::new(), &HashSet::new(), T, true);
        assert_eq!(str_of(&r["voci"][0], "stato"), "fatta", "estesa accesa: chiusa");
        assert_eq!(str_of(&r["voci"][1], "stato"), "in_lavoro", "chat al lavoro: mai");
    }

    #[test]
    fn undo_reopens_and_does_not_reclose() {
        let alive: HashSet<String> = HashSet::new();
        let mut r = json!({"voci": [item("a", "morta", "Lavoro", "in_lavoro", 2.0)]});
        apply_cleanup(&mut r, &alive, &HashSet::new(), &HashMap::new(), &HashSet::new(), T, false);
        assert_eq!(str_of(&r["voci"][0], "stato"), "fatta");
        assert!(undo_juggl(&mut r["voci"][0]));
        assert_eq!(str_of(&r["voci"][0], "stato"), "in_lavoro"); assert_eq!(r["voci"][0]["no_auto"], json!(true));
        apply_cleanup(&mut r, &alive, &HashSet::new(), &HashMap::new(), &HashSet::new(), T + 3600.0, false);
        assert_eq!(str_of(&r["voci"][0], "stato"), "in_lavoro", "riaperta a mano: non si richiude");
    }

    #[test]
    fn duplicates_in_the_same_chat_even_without_project() {
        let alive: HashSet<String> = ["s1", "s2"].iter().map(|x| x.to_string()).collect();
        let mut r = json!({"voci": [item("a", "s1", "Sistemare scroll in fondo alla chat", "in_lavoro", 0.5), item("b", "s1", "Sistemare scroll in fondo alla chat adesso", "in_lavoro", 0.2),
            item("c", "s2", "Sistemare scroll in fondo alla chat", "in_lavoro", 0.2)]});
        apply_cleanup(&mut r, &alive, &HashSet::new(), &HashMap::new(), &HashSet::new(), T, false);
        let v = r["voci"].as_array().unwrap();
        assert!(!str_of(&v[0], "doppione_di").is_empty() || !str_of(&v[1], "doppione_di").is_empty(), "stessa chat: doppione");
        assert!(str_of(&v[2], "doppione_di").is_empty(), "un'altra chat non si tocca");
    }
}
