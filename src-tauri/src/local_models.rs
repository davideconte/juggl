// Chats with the local models of the studio PC (Ollama behind the bridge with the VRAM guardian).
// Chats live ONLY on the Mac (~/tavolo/chat-locali): the PC does the computing and saves nothing (the user's rule).
// Each chat appears in Juggl as a session ("locale:<id>"), with the same conversation and the same sending.
use crate::{base_dir, load_json, unix_now, str_of, write_state};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

// address of the PC and default model: in ~/.config/juggl/personale.json ("pc"), never in the program
fn ollama() -> Option<String> {
    // tests only: the test program lacks macOS's "local network" permission, it goes through an ssh tunnel
    #[cfg(test)]
    if let Ok(u) = std::env::var("JUGGL_PROVA_OLLAMA") { return Some(u); }
    if !crate::profile::is_enabled("pc") { return None; }
    // studio network if it answers, otherwise Tailscale (Nov 10: Mac away from the studio)
    crate::profile::pc_ollama().map(|p| crate::profile::pc_address(&p, crate::profile::pc_ollama_fallback().as_deref()))
}
fn default_model() -> String { crate::profile::pc_model() }

fn running() -> &'static Mutex<HashSet<String>> { static M: OnceLock<Mutex<HashSet<String>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(HashSet::new())) }
fn to_stop() -> &'static Mutex<HashSet<String>> { static M: OnceLock<Mutex<HashSet<String>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(HashSet::new())) }

/// Folder of the chats: readable only by this user, outside Spotlight and Time Machine (Nov 10: when the user
/// deletes them no trace must remain, not even in an index or a backup).
fn folder() -> std::path::PathBuf {
    let d = base_dir().join("chat-locali");
    let _ = std::fs::create_dir_all(&d);
    static PROTECTED: OnceLock<()> = OnceLock::new();
    PROTECTED.get_or_init(|| {
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)); }
        let _ = std::fs::write(d.join(".metadata_never_index"), b"");
        // opening (Nov 17): tmutil can take 10 s and more; here it held up the first chat list (12 s at startup).
        // Permissions and no-Spotlight stay immediate; the Time Machine exclusion arrives in the background.
        let dd = d.clone();
        std::thread::spawn(move || { let _ = crate::new_command("/usr/bin/tmutil").arg("addexclusion").arg(&dd).output(); });
    });
    d
}
fn chat_file(id: &str) -> std::path::PathBuf {
    let clean: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    folder().join(format!("{clean}.json"))
}

pub fn models() -> Value {
    let Some(ollama) = ollama() else { return json!({"ok": false, "msg": "nessun PC configurato", "modelli": []}) };
    match ureq::get(&format!("{ollama}/api/tags")).timeout(Duration::from_secs(4)).call() {
        Ok(r) => {
            let v: Value = r.into_json().unwrap_or(json!({}));
            let names: Vec<String> = v["models"].as_array().cloned().unwrap_or_default().iter()
                .map(|m| str_of(m, "name").trim_end_matches(":latest").to_string()).filter(|n| !n.contains("agente")).collect();
            json!({"ok": true, "modelli": names, "predefinito": default_model(), "agente": crate::pc_agent::is_available()})
        }
        Err(e) => json!({"ok": false, "msg": format!("PC dello studio non raggiungibile ({e})"), "modelli": []}),
    }
}

/// Can the model reason? From Ollama's capabilities list (/api/show), kept in memory. The models of the
/// llama-server behind the bridge have no list: the bridge turns reasoning on itself when it receives "think": true.
fn can_reason(model: &str) -> bool {
    static C: OnceLock<Mutex<std::collections::HashMap<String, bool>>> = OnceLock::new();
    let c = C.get_or_init(|| Mutex::new(Default::default()));
    if let Some(x) = c.lock().unwrap().get(model) { return *x; }
    let Some(ollama) = ollama() else { return false };
    let can = match ureq::post(&format!("{ollama}/api/show")).timeout(Duration::from_secs(6)).send_json(json!({"model": model})) {
        Ok(r) => match r.into_json::<Value>().ok().and_then(|v| v.get("capabilities").cloned()) {
            Some(Value::Array(a)) => a.iter().any(|x| x.as_str() == Some("thinking")),
            _ => true,   // no list: llama-server model, the bridge takes care of it
        },
        Err(_) => false,
    };
    c.lock().unwrap().insert(model.to_string(), can);
    can
}

/// Model text with the reasoning inside <think>...</think> (models and servers that do not separate it):
/// (reasoning, answer, still inside the reasoning).
pub(crate) fn split_think(raw: &str) -> (String, String, bool) {
    let t = raw.trim_start();
    let Some(rest_after) = t.strip_prefix("<think>") else { return (String::new(), raw.to_string(), false) };
    match rest_after.find("</think>") {
        Some(i) => (rest_after[..i].trim().to_string(), rest_after[i + 8..].trim_start().to_string(), false),
        None => (rest_after.trim().to_string(), String::new(), true),
    }
}

pub fn new_chat(model: &str, text: &str, app: &AppHandle) -> Value {
    let id = format!("{:x}", (unix_now() * 1000.0) as u64);
    let pre = default_model();
    let m = if model.is_empty() { pre.as_str() } else { model };
    // "agente-pc": the chat talks with the PC's OpenCode agent (does things: commands, files, web), not with a bare model
    let c = if m == "agente-pc" { json!({"id": id, "modello": "agente PC", "tipo": "agente", "nome": "", "messaggi": [], "creata": unix_now()}) }
        else { json!({"id": id, "modello": m, "nome": "", "messaggi": [], "creata": unix_now()}) };
    write_state(&chat_file(&id), &c);
    if !text.trim().is_empty() { send(&id, text, app); }
    json!({"ok": true, "id": format!("locale:{id}")})
}

/// The local chats in the shape of sessions, for Juggl's list.
pub fn as_sessions() -> Vec<Value> {
    let mut out = vec![];
    let working_set = running().lock().unwrap().clone();
    for e in std::fs::read_dir(folder()).into_iter().flatten().flatten() {
        let Some(c) = load_json(&e.path()) else { continue };
        let id = str_of(&c, "id").to_string();
        let msgs = c["messaggi"].as_array().cloned().unwrap_or_default();
        let last = msgs.last().cloned().unwrap_or(json!({}));
        let first_user = msgs.iter().find(|m| str_of(m, "ruolo") == "user").map(|m| str_of(m, "testo").chars().take(40).collect::<String>()).unwrap_or_default();
        let name = if !str_of(&c, "nome").is_empty() { str_of(&c, "nome").to_string() } else if !first_user.is_empty() { first_user } else { "Nuova chat".into() };
        let is_working = working_set.contains(&id);
        let asks = is_working && c.get("permesso").map(|x| !x.is_null()).unwrap_or(false);
        out.push(json!({
            "sessione": format!("locale:{id}"), "nome": name, "nome_scelto": !str_of(&c, "nome").is_empty(), "locale": true, "agente_pc": str_of(&c, "tipo") == "agente", "ospitata": true, "agente": str_of(&c, "modello"),
            "vista": if asks { "bloccata" } else if is_working { "lavora" } else { "ferma" },
            "da": last.get("t").and_then(|x| x.as_f64()).unwrap_or(c.get("creata").and_then(|x| x.as_f64()).unwrap_or(0.0)),
            "risposta": if str_of(&last, "ruolo") == "assistant" { str_of(&last, "testo").chars().take(300).collect::<String>() } else { String::new() },
            "chiesto": msgs.iter().rev().find(|m| str_of(m, "ruolo") == "user").map(|m| str_of(m, "testo").chars().take(200).collect::<String>()).unwrap_or_default(),
            "opzioni": c.get("opzioni").cloned().unwrap_or(json!({})),
            // agent at work: what it is doing, from its steps (summaries::adesso, all local, no model)
            "adesso_chiaro": if is_working && str_of(&c, "tipo") == "agente" {
                let mut o = serde_json::Map::new(); o.insert("vista".into(), json!("lavora")); o.insert("eventi".into(), c["in_corso"].clone());
                crate::summaries::now_line(&o).map(Value::from).unwrap_or(Value::Null)
            } else { Value::Null },
            "progetto": {"id": "locale", "nome": format!("PC studio · {}", str_of(&c, "modello")), "logo": ""},
            "cwd": "",
        }));
    }
    out
}

/// List for the iPhone: the local chats are there (they open and you write in them as on the Mac) but without texts, because
/// the iPhone keeps the list in its disk cache: neutral name (if the user has not given one), no last
/// answer nor last request. The content arrives only by opening the chat, and the iPhone does not save it (Deposito.swift).
pub fn for_iphone(d: &mut Value) {
    let Some(a) = d.get_mut("sessioni").and_then(|x| x.as_array_mut()) else { return };
    for v in a.iter_mut().filter(|v| str_of(v, "sessione").starts_with("locale:")) {
        if !v.get("nome_scelto").and_then(|x| x.as_bool()).unwrap_or(false) { v["nome"] = json!(format!("Chat locale · {}", str_of(v, "agente"))); }
        v["risposta"] = json!(""); v["chiesto"] = json!("");
    }
}

/// Detail in the same shape as a Claude session ("tu" / "agente" events).
pub fn detail(id: &str) -> Value {
    let Some(c) = load_json(&chat_file(id)) else { return Value::Null };
    let is_working = running().lock().unwrap().contains(id);
    let mut events: Vec<Value> = vec![];
    for m in c["messaggi"].as_array().cloned().unwrap_or_default() {
        let t = chrono_iso(m.get("t").and_then(|x| x.as_f64()).unwrap_or(0.0));
        // model reasoning (Nov 10): a dropdown block before the answer, like "Sta ragionando..."
        if !str_of(&m, "ragionamento").is_empty() {
            events.push(json!({"tipo": "pensiero", "testo": str_of(&m, "ragionamento"), "durata": m.get("pensato_s").cloned().unwrap_or(json!(0)), "t": t}));
        }
        // steps of the PC agent (commands, files, reasoning) before its answer
        for e in m["eventi"].as_array().cloned().unwrap_or_default() { events.push(e); }
        events.push(json!({"tipo": if str_of(&m, "ruolo") == "user" { "tu" } else { "agente" }, "testo": str_of(&m, "testo"), "t": t}));
    }
    let mut sess = as_sessions().into_iter().find(|x| str_of(x, "sessione") == format!("locale:{id}")).unwrap_or(json!({}));
    // agent at work: the steps done so far, and the permission question if there is one
    if is_working { for e in c["in_corso"].as_array().cloned().unwrap_or_default() { events.push(e); } }
    let asks = is_working && c.get("permesso").map(|x| !x.is_null()).unwrap_or(false);
    if asks { sess["domanda_terminale"] = json!({"testo": str_of(&c["permesso"], "testo"), "opzioni": c["permesso"]["opzioni"].clone()}); }
    sess["eventi"] = json!(events);
    sess["vista"] = json!(if asks { "bloccata" } else if is_working { "lavora" } else { "ferma" });
    sess["file"] = json!([]);
    sess
}

/// ISO date from unix time (no dependencies: enough for the interface).
fn chrono_iso(t: f64) -> String {
    let secs = t as i64; let ms = ((t - secs as f64) * 1000.0) as i64;
    let days = secs.div_euclid(86400); let rem = secs.rem_euclid(86400);
    let (h, mi, se) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // from days since 1970 to civil date (Howard Hinnant's algorithm)
    let z = days + 719468; let era = z.div_euclid(146097); let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1; let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", if m <= 2 { y + 1 } else { y }, m, d, h, mi, se, ms)
}

pub fn adjust(id: &str, model: &str, temperature: f64, context: u64, think: bool) -> Value {
    let p = chat_file(id);
    let Some(mut c) = load_json(&p) else { return json!({"ok": false, "msg": "chat non trovata"}) };
    if !model.is_empty() { c["modello"] = json!(model); }
    // "think" belongs to Juggl (reasoning yes/no), it does not go among Ollama's options: invia() removes it
    c["opzioni"] = json!({"temperature": temperature, "num_ctx": context, "think": think});
    write_state(&p, &c);
    json!({"ok": true, "msg": "Applicato"})
}

/// Keys of the permission question of the PC agent.
pub fn answer_choice(id: &str, n: &str, text: Option<&str>) -> Value { crate::pc_agent::answer(&chat_file(id), n, text) }

pub fn stop(id: &str) { to_stop().lock().unwrap().insert(id.to_string()); }

/// Deletes a chat: the file, its entries among the notifications and any leftover in the search index.
pub fn delete_chat(id: &str) -> Value {
    to_stop().lock().unwrap().insert(id.to_string());
    let _ = std::fs::remove_file(chat_file(id));
    let sid = format!("locale:{id}");
    forget(move |s| s == sid);
    json!({"ok": true})
}

/// "Delete all local chats": files, notifications, index, in one go. Answers in progress stop.
pub fn delete_all() -> Value {
    let mut n = 0;
    for e in std::fs::read_dir(folder()).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        if let Some(id) = p.file_stem().and_then(|x| x.to_str()) { to_stop().lock().unwrap().insert(id.to_string()); }
        if std::fs::remove_file(&p).is_ok() { n += 1; }
    }
    // temporary write files left half-done (storage::scrivi_json)
    for e in std::fs::read_dir(folder()).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().ends_with(".tmp") { let _ = std::fs::remove_file(e.path()); }
    }
    forget(|s| s.starts_with("locale:"));
    crate::log_juggl(&format!("chat locali: cancellate tutte ({n})"));
    json!({"ok": true, "n": n, "msg": if n == 1 { "1 chat cancellata".to_string() } else { format!("{n} chat cancellate") }})
}

/// Removes the traces of deleted chats: entries in notifiche.json (title and session) and, in a separate thread, any leftover
/// in the search index with the file compacted (VACUUM).
fn forget(session: impl Fn(&str) -> bool) {
    crate::update_json(&base_dir().join("notifiche.json"), |l| {
        let Some(a) = l.as_array_mut() else { return false };
        let before_len = a.len();
        a.retain(|v| !session(str_of(v, "sessione")));
        a.len() != before_len
    });
    std::thread::spawn(crate::search_index::forget_local);
}

pub fn send(id: &str, text: &str, app: &AppHandle) -> Value {
    let p = chat_file(id);
    let Some(mut c) = load_json(&p) else { return json!({"ok": false, "esito": "errore", "msg": "chat non trovata"}) };
    if running().lock().unwrap().contains(id) { return json!({"ok": false, "esito": "errore", "msg": "sta ancora rispondendo"}) }
    let mut msgs = c["messaggi"].as_array().cloned().unwrap_or_default();
    msgs.push(json!({"ruolo": "user", "testo": text, "t": unix_now()}));
    c["messaggi"] = json!(msgs);
    write_state(&p, &c);
    running().lock().unwrap().insert(id.to_string());
    to_stop().lock().unwrap().remove(id);
    let _ = app.emit("cambiato", ());
    if str_of(&c, "tipo") == "agente" {
        let (id2, app2, text2) = (id.to_string(), app.clone(), text.to_string());
        std::thread::spawn(move || {
            let idf = id2.clone();
            crate::pc_agent::turn(id2.clone(), chat_file(&id2), text2, app2, move || to_stop().lock().unwrap().contains(&idf));
            to_stop().lock().unwrap().remove(&id2);
            running().lock().unwrap().remove(&id2);
        });
        return json!({"ok": true, "esito": "inviato", "msg": ""});
    }
    let (id2, app2, model) = (id.to_string(), app.clone(), str_of(&c, "modello").to_string());
    std::thread::spawn(move || reply_with(id2, c, msgs, model, move |ev, x| { let _ = app2.emit(ev, x); }));
    json!({"ok": true, "esito": "inviato", "msg": ""})
}

/// Answer of the local model (separate thread): streaming with reasoning, saving into the chat. Without the app: the
/// tests call it with a fake `emit_fn`.
fn reply_with(id2: String, c: Value, msgs: Vec<Value>, model: String, emit_fn: impl Fn(&str, Value)) {
    {
        let messages: Vec<Value> = msgs.iter().map(|m| json!({"role": str_of(m, "ruolo"), "content": str_of(m, "testo")})).collect();
        let mut body = json!({"model": model, "messages": messages, "stream": true, "keep_alive": "10m"});
        if let Some(o) = c.get("opzioni").filter(|o| o.is_object()) {
            let mut o = o.clone();
            if let Some(m) = o.as_object_mut() { m.remove("think"); }
            body["options"] = o;
        }
        // reasoning: on if the model can do it and it was not turned off in the chat's settings
        let wants = c["opzioni"].get("think").and_then(|x| x.as_bool()).unwrap_or(true);
        if wants && can_reason(&model) { body["think"] = json!(true); }
        let mut answer = String::new();
        let (mut thinking, mut raw) = (String::new(), String::new());
        let (mut t_think, mut thought_s) = (None::<Instant>, 0.0_f64);
        let mut final_thinking = String::new();
        let mut error_ = None;
        match ureq::post(&format!("{}/api/chat", ollama().unwrap_or_default())).timeout(Duration::from_secs(600)).send_json(body) {
            Ok(r) => {
                let mut last = Instant::now();
                for line in BufReader::new(r.into_reader()).lines() {
                    let Ok(line) = line else { break };
                    if to_stop().lock().unwrap().remove(&id2) { break; }
                    let Ok(d) = serde_json::from_str::<Value>(&line) else { continue };
                    let m = d.get("message");
                    if let Some(t) = m.and_then(|m| m.get("thinking")).and_then(|x| x.as_str()) { thinking.push_str(t); }
                    if let Some(t) = m.and_then(|m| m.get("content")).and_then(|x| x.as_str()) { raw.push_str(t); }
                    let (p_tag, r, inside) = split_think(&raw);
                    answer = r;
                    let all_text = if p_tag.is_empty() { thinking.clone() } else if thinking.is_empty() { p_tag } else { format!("{thinking}\n{p_tag}") };
                    let is_thinking = answer.is_empty() && (inside || !all_text.is_empty());
                    if is_thinking && t_think.is_none() { t_think = Some(Instant::now()); }
                    if !is_thinking { if let Some(t0) = t_think.take() { thought_s += t0.elapsed().as_secs_f64(); } }
                    // while reasoning one update every 250 ms is enough; the answer text every 50 ms
                    if last.elapsed().as_millis() > if is_thinking { 250 } else { 50 } {
                        last = Instant::now();
                        emit_fn("parziale", json!({"id": format!("locale:{id2}"), "testo": answer, "pensiero": if is_thinking { all_text.clone() } else { String::new() }}));
                    }
                    if !all_text.is_empty() { final_thinking = all_text; }
                    if d.get("done").and_then(|x| x.as_bool()).unwrap_or(false) { break; }
                }
            }
            Err(e) => error_ = Some(format!("Il PC dello studio non risponde: {e}")),
        }
        if let Some(t0) = t_think.take() { thought_s += t0.elapsed().as_secs_f64(); }
        let p = chat_file(&id2);
        if let Some(mut c) = load_json(&p) {
            let mut msgs = c["messaggi"].as_array().cloned().unwrap_or_default();
            let mut m = json!({"ruolo": "assistant", "testo": error_.unwrap_or(if answer.is_empty() { "(nessuna risposta)".into() } else { answer }), "t": unix_now()});
            // the reasoning stays in the chat (only on the Mac) but does not go back to the model in later turns
            if !final_thinking.trim().is_empty() { m["ragionamento"] = json!(final_thinking.trim()); m["pensato_s"] = json!(thought_s.round()); }
            msgs.push(m);
            c["messaggi"] = json!(msgs);
            write_state(&p, &c);
        }
        running().lock().unwrap().remove(&id2);
        emit_fn("parziale", json!({"id": format!("locale:{id2}"), "testo": ""}));
        emit_fn("cambiato", Value::Null);
    }
}

#[cfg(test)]
mod privacy_tests {
    use super::*;

    /// Test chat (never the real chats): after "Cancella tutte" the secret word is nowhere any more,
    /// not even in the bytes of the index file and its WAL. Runs in a temporary folder (JUGGL_BASE).
    #[test]
    #[ignore]
    fn delete_all_without_traces() {
        let d = std::env::temp_dir().join(format!("juggl-privacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let _base_lock = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (a whole-process variable)
        std::env::set_var("JUGGL_BASE", &d);
        const SECRET: &str = "ZAFFERANOPROVA7731";
        // test chat + entry in the notifications
        write_state(&chat_file("abc123"), &json!({"id": "abc123", "modello": "prova", "nome": "", "creata": unix_now(),
            "messaggi": [{"ruolo": "user", "testo": format!("la parola e' {SECRET}"), "t": unix_now()}]}));
        std::fs::write(d.join("notifiche.json"), json!([{"t": unix_now(), "titolo": format!("{SECRET} ha finito"), "testo": "", "sessione": "locale:abc123", "tipo": "fine"},
            {"t": unix_now(), "titolo": "altra", "testo": "", "sessione": "02ced450-9036-453a-897c-818b366c2ea0", "tipo": "fine"}]).to_string()).unwrap();
        // as old versions did: the local chat inside the search index
        crate::search_index::test_insert_local("locale:abc123", &format!("la parola e' {SECRET}"));
        let inside = |p: &std::path::Path| std::fs::read(p).map(|b| b.windows(SECRET.len()).any(|w| w == SECRET.as_bytes())).unwrap_or(false);
        assert!(inside(&d.join("indice.db")) || inside(&d.join("indice.db-wal")), "la prova deve partire con la parola nell'indice");
        // folder protections
        let c = folder();
        #[cfg(unix)] use std::os::unix::fs::PermissionsExt;
        #[cfg(unix)] assert_eq!(std::fs::metadata(&c).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(c.join(".metadata_never_index").exists());
        // delete all
        let r = delete_all();
        assert_eq!(r["n"], 1);
        std::thread::sleep(std::time::Duration::from_secs(3));   // the VACUUM runs in a separate thread
        crate::search_index::forget_local();                         // and here I wait for it for sure (same lock)
        assert!(std::fs::read_dir(&c).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".json")));
        let n = std::fs::read_to_string(d.join("notifiche.json")).unwrap();
        assert!(!n.contains(SECRET) && n.contains("altra"), "notifiche: {n}");
        for f in ["indice.db", "indice.db-wal", "indice.db-shm"] { assert!(!inside(&d.join(f)), "{f} contiene ancora la parola"); }
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod reasoning_tests {
    use super::*;
    /// A real question to the PC's small model with reasoning on (test chat in a temporary folder).
    /// `cargo test --bin tavolo real_reasoning -- --ignored --nocapture --test-threads=1`
    #[test]
    #[ignore]
    fn real_reasoning() {
        let d = std::env::temp_dir().join(format!("juggl-pensa-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let _base_lock = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (a whole-process variable)
        std::env::set_var("JUGGL_BASE", &d);
        let model = "qwen3.5-4b-uncensored".to_string();
        let msgs = vec![json!({"ruolo": "user", "testo": "Quanto fa 17*23? Rispondi solo con il numero.", "t": unix_now()})];
        let c = json!({"id": "pensa1", "modello": model, "nome": "", "creata": unix_now(), "messaggi": msgs});
        write_state(&chat_file("pensa1"), &c);
        let live = std::sync::Mutex::new((0, 0));   // updates with reasoning, with answer
        let t0 = Instant::now();
        reply_with("pensa1".into(), c, msgs, model, |ev, x| {
            if ev == "parziale" { let mut v = live.lock().unwrap(); if !str_of(&x, "pensiero").is_empty() { v.0 += 1; } if !str_of(&x, "testo").is_empty() { v.1 += 1; } }
        });
        let c = load_json(&chat_file("pensa1")).unwrap();
        let m = c["messaggi"].as_array().unwrap().last().unwrap().clone();
        let (vp, vr) = *live.lock().unwrap();
        println!("{:.0} s · aggiornamenti dal vivo: {vp} col ragionamento, {vr} con la risposta · ragionato {} s", t0.elapsed().as_secs_f64(), m["pensato_s"]);
        println!("RAGIONAMENTO: {}", str_of(&m, "ragionamento").chars().take(300).collect::<String>());
        println!("RISPOSTA: {}", str_of(&m, "testo"));
        assert!(str_of(&m, "testo").contains("391"), "risposta: {}", str_of(&m, "testo"));
        assert!(!str_of(&m, "ragionamento").is_empty(), "nessun ragionamento salvato");
        let det = detail("pensa1");
        let kinds: Vec<&str> = det["eventi"].as_array().unwrap().iter().map(|e| str_of(e, "tipo")).collect();
        assert_eq!(kinds, ["tu", "pensiero", "agente"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
