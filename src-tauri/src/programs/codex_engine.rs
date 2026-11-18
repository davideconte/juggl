// engines (Nov 12): OpenAI's Codex CLI inside the keeper, like Claude Code.
//
// The keeper launches `codex app-server` (JSON-RPC on stdin/stdout, one process per conversation) and this translator
// sits in the middle in both directions:
//   Juggl -> Codex:  the same lines Juggl sends to Claude (stream-json) become Codex requests:
//                    user message -> turn/start (or turn/steer if already working), "interrupt" -> turn/interrupt,
//                    control_response (permissions) -> reply to Codex's approval request.
//   Codex -> Juggl:  Codex notifications become the lines Claude would send: text in pieces (stream_event),
//                    tools (assistant tool_use / user tool_result), end of turn ("result"), permission requests
//                    (control_request can_use_tool).
// In addition it writes the "mirror": the conversation in Claude's transcript format (~/tavolo/motori/<id>.jsonl).
// Juggl uses it as the chat's transcript ("transcript" field of the state): history, search, summaries, name,
// "what it is doing" work as for Claude without knowing anything about Codex.
//
// Configuration (a JSON, last argument): {"id", "cwd", "specchio", "thread" (resume), "model", "effort",
// "approvalPolicy", "sandbox", "istruzioni"}.
//
// Protocol verified with codex-cli 0.160.0 (`codex app-server generate-json-schema`).
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};

pub struct Started { pub figlio: Child, pub uscita: Box<dyn Read + Send>, pub entrata: Box<dyn Write + Send> }

fn lock_mutex<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }
fn str_of<'a>(v: &'a Value, k: &str) -> &'a str { v.get(k).and_then(|x| x.as_str()).unwrap_or("") }

/// What I was waiting for in reply to a request of own.
enum Pending { Init, Modelli, Thread, Turno(Vec<Value>), Steer(Vec<Value>), Interrompi }

#[derive(Default)]
struct State {
    thread: Option<String>,
    turno: Option<String>,
    /// messages that arrived before the conversation was ready (or while a turn is starting)
    coda: Vec<Vec<Value>>,
    avvio_turno: bool,
    prossimo: u64,
    attese: HashMap<u64, Pending>,
    /// permission requests passed on to Juggl: Juggl's request_id -> (Codex's JSON-RPC id, method, parameters)
    richieste: HashMap<String, (Value, String, Value)>,
    /// file changes in progress, to know which files it asks to touch
    modifiche: HashMap<String, Value>,
    /// last text reply not yet written to the mirror (waits to know whether it is the last of the turn)
    in_sospeso: Option<Value>,
    ultimo_testo: String,
    errore: Option<String>,
    modello: String,
    /// text arriving per item (for the deltas)
    aperto: Option<String>,
    /// models the account can use (model/list) and the default one: with a different ChatGPT account (e.g. free)
    /// the model in ~/.codex/config.toml may not exist, and every turn would end with "model is not supported"
    modelli: Option<(Vec<String>, String)>,
    /// unified diff of the whole turn (turn/diff/updated): at the end of the turn it becomes a "juggl_modifiche" line in the
    /// mirror, read by live_edits.rs (chat "live changes")
    diff: String,
    /// last juggl_modifiche line written during the turn: at most one every 3 seconds (+ the final one)
    diff_scritto_il: Option<std::time::Instant>,
}

struct Bridge {
    st: Mutex<State>,
    codex: Mutex<ChildStdin>,
    juggl: Mutex<Box<dyn Write + Send>>,
    specchio: Mutex<Option<std::fs::File>>,
    conf: Value,
}

/// JSON line with "type" first: the keeper and Juggl recognise the "result" from the start of the line, and serde_json
/// sorts the keys.
fn line(kind: &str, rest: Value) -> String {
    let body = rest.to_string();
    let inside = body.trim_start_matches('{').trim_end_matches('}');
    if inside.trim().is_empty() { format!("{{\"type\":\"{kind}\"}}") } else { format!("{{\"type\":\"{kind}\",{inside}}}") }
}

/// Time in ISO format (UTC, milliseconds), like the timestamps of Claude's transcripts.
fn now_iso() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let (sec, ms) = (d.as_secs() as i64, d.subsec_millis());
    let (days, rest) = (sec.div_euclid(86400), sec.rem_euclid(86400));
    // from days since 1970 to civil date (Howard Hinnant's algorithm)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let g = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{g:02}T{:02}:{:02}:{:02}.{ms:03}Z", rest / 3600, (rest % 3600) / 60, rest % 60)
}

fn fake_uuid() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let h = format!("{:032x}", t ^ ((std::process::id() as u128) << 64) ^ (n as u128));
    format!("{}-{}-4{}-8{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
}

pub fn start(codex: &str, conf: Value) -> Result<Started, String> {
    let mut cmd = Command::new(codex);
    cmd.arg("app-server").current_dir(str_of(&conf, "cwd"))
        // the user's hooks (guards, image saver) stay active; TAVOLO_INTERNO only tells Juggl's hook not to
        // create a second chat with Codex's id: the state is written by Juggl from the stream
        .env("TAVOLO_INTERNO", "1")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    #[cfg(unix)] { use std::os::unix::process::CommandExt; cmd.process_group(0); }
    #[cfg(windows)] { use super::sys::CommandExtJuggl; cmd.process_group(0); }
    let mut figlio = cmd.spawn().map_err(|e| format!("Codex non parte: {e}"))?;
    let stdin = figlio.stdin.take().ok_or("stdin di Codex")?;
    let stdout = figlio.stdout.take().ok_or("stdout di Codex")?;
    let (out_r, out_w) = std::io::pipe().map_err(|e| e.to_string())?;
    let (in_r, in_w) = std::io::pipe().map_err(|e| e.to_string())?;
    let specchio = str_of(&conf, "specchio").to_string();
    let file = if specchio.is_empty() { None } else {
        if let Some(d) = std::path::Path::new(&specchio).parent() { let _ = std::fs::create_dir_all(d); }
        std::fs::OpenOptions::new().create(true).append(true).open(&specchio).ok()
    };
    let p = Arc::new(Bridge { st: Mutex::new(State { modello: str_of(&conf, "model").to_string(), ..Default::default() }), codex: Mutex::new(stdin), juggl: Mutex::new(Box::new(out_w)), specchio: Mutex::new(file), conf });
    p.ask("initialize", json!({"clientInfo": {"name": "juggl", "title": "Juggl", "version": "1"}}), Pending::Init);
    let p1 = p.clone();
    std::thread::spawn(move || {
        for r in BufReader::new(stdout).lines() {
            let Ok(r) = r else { break };
            let Ok(d) = serde_json::from_str::<Value>(&r) else { continue };
            p1.from_codex(&d);
        }
        // Codex finished: I close the output, so the keeper sees the end
        p1.close_open_turn("Codex si e' fermato");
    });
    let p2 = p.clone();
    std::thread::spawn(move || {
        for r in BufReader::new(in_r).lines() {
            let Ok(r) = r else { break };
            let Ok(d) = serde_json::from_str::<Value>(r.trim()) else { continue };
            p2.from_juggl(&d);
        }
    });
    Ok(Started { figlio, uscita: Box::new(out_r), entrata: Box::new(in_w) })
}

impl Bridge {
    fn write_codex(&self, v: &Value) {
        let mut w = lock_mutex(&self.codex);
        let _ = writeln!(w, "{v}").and_then(|_| w.flush());
    }
    fn ask(&self, method: &str, params: Value, a: Pending) {
        let id = { let mut st = lock_mutex(&self.st); st.prossimo += 1; let n = st.prossimo; st.attese.insert(n, a); n };
        self.write_codex(&json!({"id": id, "method": method, "params": params}));
    }
    fn to_juggl(&self, r: &str) {
        let mut w = lock_mutex(&self.juggl);
        let _ = writeln!(w, "{r}").and_then(|_| w.flush());
    }
    fn event(&self, ev: Value) { self.to_juggl(&line("stream_event", json!({"event": ev, "session_id": str_of(&self.conf, "id")}))); }

    /// Line in the mirror (Claude transcript format) and, for assistant/user, also to Juggl (updates the view).
    fn mirror(&self, kind: &str, message: Value, extra: Value) {
        let mut v = json!({"message": message, "timestamp": now_iso(), "uuid": fake_uuid(), "sessionId": str_of(&self.conf, "id"),
            "cwd": str_of(&self.conf, "cwd"), "isSidechain": false, "motore": "codex"});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) { for (k, x) in e { o.insert(k.clone(), x.clone()); } }
        let r = line(kind, v);
        if let Some(f) = lock_mutex(&self.specchio).as_mut() { let _ = writeln!(f, "{r}").and_then(|_| f.flush()); }
        self.to_juggl(&r);
    }
    fn assistant(&self, content: Value, stop: Option<&str>) -> Value {
        json!({"role": "assistant", "model": lock_mutex(&self.st).modello.clone(), "content": content, "stop_reason": stop})
    }
    /// Writes the pending text reply (stop_reason: tool_use if there is more after, end_turn at end of turn).
    fn flush_pending(&self, stop: &str) {
        let m = lock_mutex(&self.st).in_sospeso.take();
        if let Some(mut m) = m { m["stop_reason"] = json!(stop); self.mirror("assistant", m, json!({})); }
    }

    // ---------- from Juggl ----------

    fn from_juggl(&self, d: &Value) {
        match str_of(d, "type") {
            "user" => {
                let c = d.get("message").and_then(|m| m.get("content")).cloned().unwrap_or(Value::Null);
                let input = input_codex(&c);
                if input.is_empty() { return; }
                // in the mirror as Juggl sees it (text and photos), as Claude would do
                self.mirror("user", json!({"role": "user", "content": c}), json!({}));
                self.send_input(input);
            }
            "control_request" => {
                let rid = str_of(d, "request_id").to_string();
                let req = d.get("request").cloned().unwrap_or(Value::Null);
                match str_of(&req, "subtype") {
                    "interrupt" => {
                        let (t, u) = { let st = lock_mutex(&self.st); (st.thread.clone(), st.turno.clone()) };
                        if let (Some(t), Some(u)) = (t, u) { self.ask("turn/interrupt", json!({"threadId": t, "turnId": u}), Pending::Interrompi); }
                        else { lock_mutex(&self.st).coda.clear(); }
                        self.to_juggl(&line("control_response", json!({"response": {"subtype": "success", "request_id": rid}})));
                    }
                    _ => self.to_juggl(&line("control_response", json!({"response": {"subtype": "error", "request_id": rid, "error": "non disponibile con Codex"}}))),
                }
            }
            "control_response" => {
                let r = d.get("response").cloned().unwrap_or(Value::Null);
                let rid = str_of(&r, "request_id").to_string();
                let Some((id, method, params)) = lock_mutex(&self.st).richieste.remove(&rid) else { return };
                let inside = r.get("response").cloned().unwrap_or(Value::Null);
                let allow = str_of(&r, "subtype") == "success" && str_of(&inside, "behavior") == "allow";
                let always = inside.get("updatedPermissions").and_then(|x| x.as_array()).map(|a| !a.is_empty()).unwrap_or(false);
                let outcome = match method.as_str() {
                    "item/permissions/requestApproval" => if allow {
                        json!({"permissions": params.get("permissions").cloned().unwrap_or(json!({})), "scope": if always { "session" } else { "turn" }})
                    } else { json!({"permissions": {}}) },
                    _ => json!({"decision": if !allow { "decline" } else if always { "acceptForSession" } else { "accept" }}),
                };
                self.write_codex(&json!({"id": id, "result": outcome}));
            }
            _ => {}
        }
    }

    fn send_input(&self, input: Vec<Value>) {
        let (thread, turno, starting) = { let st = lock_mutex(&self.st); (st.thread.clone(), st.turno.clone(), st.avvio_turno) };
        let Some(t) = thread else { lock_mutex(&self.st).coda.push(input); return };
        if let Some(u) = turno {
            // it is working: the message enters the current turn (like writing to Claude while it works)
            self.ask("turn/steer", json!({"threadId": t, "expectedTurnId": u, "input": input}), Pending::Steer(input.clone()));
        } else if starting {
            lock_mutex(&self.st).coda.push(input);
        } else {
            lock_mutex(&self.st).avvio_turno = true;
            let mut p = json!({"threadId": t, "input": input});
            // model not available for this account: use the account's default (only for this chat)
            { let mut st = lock_mutex(&self.st);
              if let Some((names, default_model)) = st.modelli.clone() { if !st.modello.is_empty() && !names.contains(&st.modello) { st.modello = default_model.clone(); } if names.contains(&st.modello) { p["model"] = json!(st.modello.clone()); } } }
            let e = str_of(&self.conf, "effort");
            if !e.is_empty() { p["effort"] = json!(e); }
            self.ask("turn/start", p, Pending::Turno(input));
        }
    }

    /// Queued messages: all together in a single turn.
    fn drain_queue(&self) {
        let c: Vec<Value> = { let mut st = lock_mutex(&self.st); if st.turno.is_some() || st.avvio_turno { return; } st.coda.drain(..).flatten().collect() };
        if !c.is_empty() { self.send_input(c); }
    }

    // ---------- from Codex ----------

    fn from_codex(&self, d: &Value) {
        let method = str_of(d, "method");
        let id = d.get("id").cloned();
        // reply to a request of own
        if method.is_empty() {
            let Some(n) = id.as_ref().and_then(|x| x.as_u64()) else { return };
            let a = lock_mutex(&self.st).attese.remove(&n);
            let errore = d.get("error").map(|e| str_of(e, "message").to_string());
            match a {
                Some(Pending::Init) => {
                    self.write_codex(&json!({"method": "initialized"}));
                    self.ask("model/list", json!({}), Pending::Modelli);
                    self.open_thread();
                }
                Some(Pending::Modelli) => if errore.is_none() {
                    let list = d["result"]["data"].as_array().cloned().unwrap_or_default();
                    let names: Vec<String> = list.iter().map(|m| str_of(m, "model").to_string()).filter(|x| !x.is_empty()).collect();
                    let default_model = list.iter().find(|m| m.get("isDefault").and_then(|x| x.as_bool()).unwrap_or(false)).map(|m| str_of(m, "model").to_string())
                        .or_else(|| names.first().cloned()).unwrap_or_default();
                    if !default_model.is_empty() { lock_mutex(&self.st).modelli = Some((names, default_model)); }
                },
                Some(Pending::Thread) => match errore {
                    Some(e) => { self.fatal_error(&format!("Codex non apre la conversazione: {e}")); }
                    None => {
                        let th = d["result"]["thread"].clone();
                        let tid = str_of(&th, "id").to_string();
                        let modello = d["result"].get("model").and_then(|x| x.as_str()).or_else(|| th.get("model").and_then(|x| x.as_str())).unwrap_or("").to_string();
                        { let mut st = lock_mutex(&self.st); st.thread = Some(tid.clone()); if !modello.is_empty() { st.modello = modello.clone(); } }
                        // first line of the mirror: which Codex conversation it matches (needed to resume it)
                        let is_new = str_of(&self.conf, "thread").is_empty();
                        if is_new { if let Some(f) = lock_mutex(&self.specchio).as_mut() { let _ = writeln!(f, "{}", line("motore", json!({"motore": "codex", "codex_thread": tid, "cwd": str_of(&self.conf, "cwd"), "sessionId": str_of(&self.conf, "id"), "timestamp": now_iso()}))); } }
                        self.to_juggl(&line("system", json!({"subtype": "init", "session_id": str_of(&self.conf, "id"), "motore": "codex", "codex_thread": tid, "model": modello, "cwd": str_of(&self.conf, "cwd")})));
                        self.drain_queue();
                    }
                },
                Some(Pending::Turno(input)) => {
                    let mut st = lock_mutex(&self.st);
                    st.avvio_turno = false;
                    match errore {
                        None => { if let Some(u) = d["result"]["turn"].get("id").and_then(|x| x.as_str()) { st.turno = Some(u.to_string()); } }
                        Some(e) => { drop(st); let _ = input; self.end_turn_error(&format!("Codex non ha preso il messaggio: {e}")); }
                    }
                }
                Some(Pending::Steer(input)) => if errore.is_some() {
                    // the turn had just finished: the message starts as a new turn
                    lock_mutex(&self.st).turno = None;
                    self.send_input(input);
                },
                Some(Pending::Interrompi) | None => {}
            }
            return;
        }
        // request from Codex waiting for an answer (permissions, questions)
        if let Some(rid) = id {
            self.request(&rid, method, d.get("params").cloned().unwrap_or(Value::Null));
            return;
        }
        let p = d.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "turn/started" => {
                let mut st = lock_mutex(&self.st);
                st.turno = p["turn"].get("id").and_then(|x| x.as_str()).map(String::from);
                st.avvio_turno = false; st.errore = None; st.ultimo_testo.clear();
            }
            "item/started" => self.item(&p["item"], false),
            "item/completed" => self.item(&p["item"], true),
            "item/agentMessage/delta" => {
                let t = str_of(&p, "delta");
                if !t.is_empty() { self.event(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": t}})); }
            }
            "turn/diff/updated" => {
                let should_write = {
                    let mut st = lock_mutex(&self.st);
                    st.diff = str_of(&p, "diff").to_string();
                    let now_inst = std::time::Instant::now();
                    let ok = st.diff_scritto_il.map(|t| now_inst.duration_since(t).as_secs_f64() >= 3.0).unwrap_or(true);
                    if ok { st.diff_scritto_il = Some(now_inst); }
                    ok
                };
                if should_write { self.write_changes(); }
            }
            // preview while it writes: the incoming change as an Edit tool with the input in pieces
            "item/fileChange/patchUpdated" => for c in p["changes"].as_array().cloned().unwrap_or_default() {
                let tid = format!("{}-anteprima", str_of(&p, "itemId"));
                self.event(json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": tid, "name": "Edit", "input": {}}}));
                self.event(json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": json!({"file_path": str_of(&c, "path"), "diff": str_of(&c, "diff")}).to_string()}}));
                self.event(json!({"type": "content_block_stop", "index": 1}));
            },
            "turn/plan/updated" => {
                // Codex's plan becomes the list of steps (like Claude's TodoWrite)
                let todos: Vec<Value> = p["plan"].as_array().cloned().unwrap_or_default().iter().map(|x| {
                    let status = match str_of(x, "status") { "completed" => "completed", "inProgress" | "in_progress" => "in_progress", _ => "pending" };
                    json!({"content": str_of(x, "step"), "status": status, "activeForm": str_of(x, "step")})
                }).collect();
                self.flush_pending("tool_use");
                let tid = format!("piano-{}", fake_uuid());
                self.mirror("assistant", self.assistant(json!([{"type": "tool_use", "id": tid, "name": "TodoWrite", "input": {"todos": todos}}]), Some("tool_use")), json!({}));
                self.mirror("user", json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": tid, "content": "ok"}]}), json!({}));
            }
            "error" => {
                let will_retry = p.get("willRetry").and_then(|x| x.as_bool()).unwrap_or(false);
                if !will_retry { lock_mutex(&self.st).errore = Some(str_of(&p["error"], "message").to_string()); }
            }
            "turn/completed" => {
                let status = str_of(&p["turn"], "status").to_string();
                let errore = p["turn"]["error"].get("message").and_then(|x| x.as_str()).map(String::from).or_else(|| lock_mutex(&self.st).errore.take());
                match status.as_str() {
                    "interrupted" => {
                        self.flush_pending("end_turn");
                        // like Claude when you stop it: the turn is closed
                        self.mirror("user", json!({"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]}), json!({}));
                        self.close_turn(false, "");
                    }
                    "failed" => self.end_turn_error(&errore.unwrap_or_else(|| "Codex si e' fermato con un errore".into())),
                    _ => { self.flush_pending("end_turn"); self.close_turn(false, ""); }
                }
            }
            "thread/name/updated" => {
                let n = str_of(&p, "threadName");
                if !n.is_empty() { self.to_juggl(&line("system", json!({"subtype": "nome", "session_id": str_of(&self.conf, "id"), "motore": "codex", "nome": n}))); }
            }
            _ => {}
        }
    }

    fn open_thread(&self) {
        let c = &self.conf;
        let mut p = json!({"cwd": str_of(c, "cwd"), "approvalPolicy": str_of(c, "approvalPolicy"), "sandbox": str_of(c, "sandbox")});
        for (k, n) in [("model", "model"), ("istruzioni", "developerInstructions")] { if !str_of(c, k).is_empty() { p[n] = json!(str_of(c, k)); } }
        let t = str_of(c, "thread");
        if t.is_empty() { self.ask("thread/start", p, Pending::Thread); }
        else { p["threadId"] = json!(t); p["excludeTurns"] = json!(true); self.ask("thread/resume", p, Pending::Thread); }
    }

    /// A Codex step (started or finished) in the mirror and in the view.
    fn item(&self, it: &Value, finished: bool) {
        let iid = str_of(it, "id").to_string();
        match str_of(it, "type") {
            "agentMessage" => {
                if !finished {
                    self.flush_pending("tool_use");
                    lock_mutex(&self.st).aperto = Some(iid);
                    self.event(json!({"type": "message_start", "message": {}}));
                } else {
                    let t = str_of(it, "text").to_string();
                    self.event(json!({"type": "content_block_stop", "index": 0}));
                    if t.trim().is_empty() { return; }
                    let m = self.assistant(json!([{"type": "text", "text": t}]), None);
                    let mut st = lock_mutex(&self.st);
                    st.ultimo_testo = t; st.aperto = None;
                    // I keep it until I know whether it is the last of the turn
                    let previous = st.in_sospeso.replace(m);
                    drop(st);
                    if let Some(mut x) = previous { x["stop_reason"] = json!("tool_use"); self.mirror("assistant", x, json!({})); }
                }
            }
            "commandExecution" => self.tool_item(&iid, finished, "Bash", json!({"command": str_of(it, "command"), "description": ""}),
                it.get("aggregatedOutput").and_then(|x| x.as_str()).unwrap_or(""), it.get("exitCode").and_then(|x| x.as_i64()).map(|c| c != 0).unwrap_or(false) || str_of(it, "status") == "failed"),
            "fileChange" => {
                let changes = it["changes"].as_array().cloned().unwrap_or_default();
                if !finished { lock_mutex(&self.st).modifiche.insert(iid.clone(), it.clone()); }
                else { lock_mutex(&self.st).modifiche.remove(&iid); }
                for (n, c) in changes.iter().enumerate() {
                    let name = if str_of(&c["kind"], "type") == "add" { "Write" } else { "Edit" };
                    let failed = matches!(str_of(it, "status"), "failed" | "declined");
                    self.tool_item(&format!("{iid}-{n}"), finished, name, json!({"file_path": str_of(c, "path"), "diff": str_of(c, "diff")}), if failed { "modifica non applicata" } else { "ok" }, failed);
                }
            }
            "mcpToolCall" => {
                let name = format!("mcp__{}__{}", str_of(it, "server"), str_of(it, "tool"));
                let out = if it.get("error").map(|e| !e.is_null()).unwrap_or(false) { it["error"].to_string() } else { it.get("result").map(|r| r.to_string()).unwrap_or_default() };
                self.tool_item(&iid, finished, &name, it.get("arguments").cloned().unwrap_or(json!({})), &out, str_of(it, "status") == "failed");
            }
            "webSearch" => self.tool_item(&iid, finished, "WebSearch", json!({"query": str_of(it, "query")}), "ok", false),
            "imageView" => self.tool_item(&iid, finished, "Read", json!({"file_path": str_of(it, "path")}), "ok", false),
            _ => {}
        }
    }

    /// Tool: at the start tool_use (what it is doing now), at the end tool_result.
    fn tool_item(&self, tid: &str, finished: bool, name: &str, input: Value, uscita: &str, errore: bool) {
        if !finished {
            self.flush_pending("tool_use");
            self.event(json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": tid, "name": name, "input": {}}}));
            self.event(json!({"type": "content_block_stop", "index": 1}));
            self.mirror("assistant", self.assistant(json!([{"type": "tool_use", "id": tid, "name": name, "input": input}]), Some("tool_use")), json!({}));
        } else {
            let text: String = uscita.chars().take(20_000).collect();
            self.mirror("user", json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": tid, "content": text, "is_error": errore}]}), json!({}));
        }
    }

    /// Codex request: permissions -> control_request to Juggl (same buttons as Claude); anything else is refused at once.
    fn request(&self, id: &Value, method: &str, p: Value) {
        let (strumento, input) = match method {
            "item/commandExecution/requestApproval" => ("Bash", json!({"command": command_text(&p["command"]), "description": str_of(&p, "reason")})),
            "item/fileChange/requestApproval" => {
                let it = lock_mutex(&self.st).modifiche.get(str_of(&p, "itemId")).cloned().unwrap_or(Value::Null);
                let first = it["changes"].as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null);
                let name = if str_of(&first["kind"], "type") == "add" { "Write" } else { "Edit" };
                let file = if str_of(&first, "path").is_empty() { str_of(&p, "grantRoot").to_string() } else { str_of(&first, "path").to_string() };
                (name, json!({"file_path": file, "description": str_of(&p, "reason")}))
            }
            "item/permissions/requestApproval" => ("Permessi di Codex", json!({"description": str_of(&p, "reason"), "permissions": p.get("permissions").cloned().unwrap_or(Value::Null)})),
            _ => {
                self.write_codex(&json!({"id": id, "error": {"code": -32601, "message": "non gestito da Juggl"}}));
                return;
            }
        };
        let rid = format!("codex-{}", fake_uuid());
        lock_mutex(&self.st).richieste.insert(rid.clone(), (id.clone(), method.to_string(), p));
        self.to_juggl(&line("control_request", json!({"request_id": rid, "request": {"subtype": "can_use_tool", "tool_name": strumento, "input": input}})));
    }

    /// "juggl_modifiche" line with the turn's diff so far (live_edits.rs reads the last one of the reply).
    fn write_changes(&self) {
        let diff = lock_mutex(&self.st).diff.clone();
        if diff.trim().is_empty() { return; }
        let r = line("juggl_modifiche", json!({"timestamp": now_iso(), "cwd": str_of(&self.conf, "cwd"), "diff": diff, "sessionId": str_of(&self.conf, "id"), "motore": "codex"}));
        if let Some(f) = lock_mutex(&self.specchio).as_mut() { let _ = writeln!(f, "{r}").and_then(|_| f.flush()); }
    }

    fn close_turn(&self, errore: bool, msg: &str) {
        // the final one of the turn, always
        self.write_changes();
        { let mut st = lock_mutex(&self.st); st.diff.clear(); st.diff_scritto_il = None; }
        let text = { let mut st = lock_mutex(&self.st); st.turno = None; st.avvio_turno = false; std::mem::take(&mut st.ultimo_testo) };
        let r = if errore { msg.to_string() } else { text };
        self.to_juggl(&line("result", json!({"subtype": if errore { "error_during_execution" } else { "success" }, "is_error": errore, "result": r, "session_id": str_of(&self.conf, "id"), "motore": "codex"})));
        self.drain_queue();
    }

    /// Turn ended badly: the error shows in the chat as a reply (so it does not look stuck for no reason).
    fn end_turn_error(&self, e: &str) {
        self.flush_pending("tool_use");
        let text = format!("Codex si e' fermato: {e}");
        self.mirror("assistant", self.assistant(json!([{"type": "text", "text": text}]), Some("end_turn")), json!({"isApiErrorMessage": true}));
        self.close_turn(true, &text);
    }

    fn fatal_error(&self, e: &str) {
        lock_mutex(&self.st).coda.clear();
        self.end_turn_error(e);
    }

    fn close_open_turn(&self, e: &str) {
        let aperto = { let st = lock_mutex(&self.st); st.turno.is_some() || st.avvio_turno };
        if aperto { self.end_turn_error(e); }
        // close the output towards the keeper
        *lock_mutex(&self.juggl) = Box::new(std::io::sink());
    }
}

/// Command of an approval: string or list of arguments.
fn command_text(c: &Value) -> String {
    match c {
        Value::String(t) => t.clone(),
        Value::Array(a) => a.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

/// Content of a Juggl message (string or blocks as for Claude) -> Codex input.
pub fn input_codex(c: &Value) -> Vec<Value> {
    let mut out = vec![];
    match c {
        Value::String(t) if !t.trim().is_empty() => out.push(json!({"type": "text", "text": t, "text_elements": []})),
        Value::Array(a) => for x in a {
            match str_of(x, "type") {
                "text" if !str_of(x, "text").trim().is_empty() => out.push(json!({"type": "text", "text": str_of(x, "text"), "text_elements": []})),
                "image" => {
                    let src = &x["source"];
                    if str_of(src, "type") == "base64" { out.push(json!({"type": "image", "url": format!("data:{};base64,{}", str_of(src, "media_type"), str_of(src, "data"))})); }
                }
                _ => {}
            }
        },
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn line_has_type_first() {
        let r = line("result", json!({"is_error": false, "session_id": "x"}));
        assert!(r.starts_with("{\"type\":\"result\""));
        assert!(serde_json::from_str::<Value>(&r).is_ok());
        assert_eq!(line("motore", json!({})), "{\"type\":\"motore\"}");
    }
    #[test]
    fn iso_time() {
        let t = now_iso();
        assert_eq!(t.len(), 24);
        assert!(t.ends_with('Z') && t.starts_with("20"));
    }
    #[test]
    fn input_from_juggl() {
        let c = json!([{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "QUJD"}}, {"type": "text", "text": "ciao"}]);
        let i = input_codex(&c);
        assert_eq!(i.len(), 2);
        assert_eq!(i[0]["url"], "data:image/png;base64,QUJD");
        assert_eq!(i[1]["text"], "ciao");
        assert!(input_codex(&json!("  ")).is_empty());
    }
}
