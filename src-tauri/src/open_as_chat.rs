// Open as chat (Nov 12, approved by the user): a sub-agent becomes a real Juggl chat.
//
// From the "Apri come chat" button (sub-agent row in the list and the sub-agent view; iPhone with
// POST /api/sottoagente/apri {id, agente, ferma}):
// 1. if the sub-agent is still working, confirmation is needed ("ferma": true). It is stopped cleanly (stop_task for
//    hosted chats, like the Ferma button) and we wait until it is stopped: never two writing the same files;
// 2. a hosted chat is created in the SAME folder as the parent chat (hence in the same group), named after the sub-agent's
//    task. If the parent's group came from the name and not from the folder, the name takes it over;
// 3. the first message carries the original task, what it did (last actions, steps, last thing written), the files
//    touched (Edit/Write), the state (finished / stopped halfway) and the transcript path. All derived here in Rust
//    from the transcript, without a model: zero consumption;
// 4. the parent chat receives "Spostato in chat <link>" as a Juggl message (it does not interrupt the turn: on hosted chats
//    Claude Code reads it at the next step, like `juggl manda`), so it does not resume that work;
// 5. permissions: if the parent has its own level (permessi_chat) the new chat inherits it, otherwise the usual computation.
// The parent's state keeps "sotto_spostati" {agent: {chat, t}}: a second click opens the chat already created.
use crate::{base_dir, log_juggl, unix_now, str_of};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::Emitter;

/// How long to wait at most for the sub-agent to be stopped.
const STOP_WAIT_S: u64 = 45;
/// Hosted chat: after the stop, a sub-agent transcript idle for long = stopped even if the notice has not arrived yet.
const QUIET_S: u64 = 8;

fn state_file(id: &str) -> PathBuf { base_dir().join("stato").join(format!("{id}.json")) }
fn truncate(t: &str, n: usize) -> String { let c: String = t.chars().take(n).collect(); if t.chars().count() > n { format!("{}…", c.trim_end()) } else { c } }
fn one_line(t: &str, n: usize) -> String { truncate(&t.split_whitespace().collect::<Vec<_>>().join(" "), n) }
/// Moves in progress (parent:agent): a double click does not create two chats.
fn running() -> &'static Mutex<HashSet<String>> { static M: OnceLock<Mutex<HashSet<String>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(HashSet::new())) }

/// What the move does outward: in the app these are hosted.rs and Juggl's send, in tests real processes.
pub struct Actions<'a> {
    /// stops the sub-agent (parent, agent)
    pub ferma: &'a dyn Fn(&str, &str) -> Result<(), String>,
    /// starts the new chat (folder, first message, already chosen id) and returns its id
    pub avvia: &'a dyn Fn(&str, &str, &str) -> Result<String, String>,
    /// Juggl message to the parent (parent, text)
    pub manda: &'a dyn Fn(&str, &str) -> Value,
    /// hosted chat: after the stop the transcript quiet period counts too
    pub quiete: bool,
}

// ---------- summary from the transcript, without a model ----------

#[derive(Default, Debug)]
pub struct Summary {
    pub compito: String,
    pub azioni: Vec<String>,
    pub passi: Vec<(String, String)>,
    pub ultimo_testo: String,
    pub file: Vec<String>,
    pub errori: usize,
    /// commands launched in the background (Bash with run_in_background): they may go on even after the sub-agent finished
    pub in_sfondo: Vec<String>,
}

/// A readable line for a sub-agent action ("Modifica /x/y.rs", "Comando: cargo build").
fn describe_action(name: &str, i: &Value) -> String {
    let fp = str_of(i, "file_path");
    match name {
        "Edit" | "MultiEdit" => format!("Modifica {fp}"),
        "Write" => format!("Scrive {fp}"),
        "NotebookEdit" => format!("Modifica {}", str_of(i, "notebook_path")),
        "Read" => format!("Legge {fp}"),
        "Bash" => { let d = str_of(i, "description"); format!("Comando: {}", one_line(if d.is_empty() { str_of(i, "command") } else { d }, 140)) }
        "Grep" | "Glob" => format!("Cerca {}", one_line(str_of(i, "pattern"), 100)),
        "WebFetch" => format!("Apre {}", str_of(i, "url")),
        "WebSearch" => format!("Cerca sul web: {}", one_line(str_of(i, "query"), 100)),
        "Agent" | "Task" => format!("Sotto-agente: {}", one_line(str_of(i, "description"), 100)),
        "TodoWrite" => "Aggiorna la lista dei passi".into(),
        _ => {
            let first = i.as_object().and_then(|o| o.values().find_map(|x| x.as_str())).unwrap_or("");
            if first.is_empty() { name.to_string() } else { format!("{name}: {}", one_line(first, 120)) }
        }
    }
}

/// Reads the sub-agent's transcript line by line (even if large). `tool` = toolUseId of the call that launched it:
/// in forks the task is in the Agent call at the top of the transcript, in the others it is the first message.
pub fn summarize(jl: &Path, tool: &str) -> Summary {
    let mut r = Summary::default();
    let Ok(fh) = std::fs::File::open(jl) else { return r };
    let mut all_actions: Vec<String> = vec![];
    let mut first_user = String::new();
    for line in BufReader::new(fh).lines().map_while(Result::ok) {
        let Ok(d) = serde_json::from_str::<Value>(&line) else { continue };
        let c = d.get("message").and_then(|m| m.get("content")).cloned().unwrap_or(Value::Null);
        match str_of(&d, "type") {
            "user" => match &c {
                Value::String(t) => if first_user.is_empty() && !t.trim().is_empty() { first_user = t.clone(); },
                Value::Array(a) => for x in a {
                    match str_of(x, "type") {
                        "text" => { let t = str_of(x, "text"); if first_user.is_empty() && !t.trim_start().starts_with('<') && !t.trim().is_empty() { first_user = t.to_string(); } }
                        "tool_result" => {
                            if x.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) { r.errori += 1; }
                            let text = match &x["content"] { Value::String(t) => t.clone(), Value::Array(a) => a.iter().map(|y| str_of(y, "text")).collect::<Vec<_>>().join(" "), _ => String::new() };
                            if let Some(i) = text.find("running in background with ID: ") {
                                let bid: String = text[i + 31..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                                if !bid.is_empty() && !r.in_sfondo.contains(&bid) { r.in_sfondo.push(bid); }
                            }
                        }
                        _ => {}
                    }
                },
                _ => {}
            },
            "assistant" => if let Value::Array(a) = &c {
                for x in a {
                    match str_of(x, "type") {
                        "text" => { let t = str_of(x, "text").trim(); if !t.is_empty() { r.ultimo_testo = t.to_string(); } }
                        "tool_use" => {
                            let (name, i) = (str_of(x, "name"), x.get("input").cloned().unwrap_or(json!({})));
                            // the call that launched the fork: the task is there, it is not an action of its own
                            if !tool.is_empty() && str_of(x, "id") == tool { if r.compito.is_empty() { r.compito = str_of(&i, "prompt").to_string(); } continue; }
                            if matches!(name, "Edit" | "Write" | "MultiEdit" | "NotebookEdit") {
                                let p = [str_of(&i, "file_path"), str_of(&i, "notebook_path")].into_iter().find(|p| !p.is_empty()).unwrap_or("").to_string();
                                if !p.is_empty() && !r.file.contains(&p) { r.file.push(p); }
                            }
                            if name == "TodoWrite" {
                                r.passi = i["todos"].as_array().cloned().unwrap_or_default().iter()
                                    .map(|t| (str_of(t, "status").to_string(), str_of(t, "content").to_string())).filter(|t| !t.1.is_empty()).collect();
                            }
                            all_actions.push(describe_action(name, &i));
                        }
                        _ => {}
                    }
                }
            },
            _ => {}
        }
    }
    if r.compito.is_empty() { r.compito = first_user; }
    let n = all_actions.len();
    r.azioni = all_actions.into_iter().skip(n.saturating_sub(15)).collect();
    if n > 15 { r.azioni.insert(0, format!("(… {} azioni prima di queste)", n - 15)); }
    r
}

/// State in words for the first message.
fn state_in_words(state: &str, outcome: &str, stopped_now: bool) -> &'static str {
    if stopped_now { return "fermato a metà: l'utente l'ha fermato adesso per spostarlo in questa chat"; }
    match (state, outcome) {
        ("finito", "fallito") => "finito con un errore",
        ("finito", _) => "finito",
        (_, "fermato") => "fermato a metà (era già stato fermato prima)",
        (_, "fallito") => "interrotto da un errore, a metà",
        _ => "interrotto a metà (la chat madre si era chiusa o fermata)",
    }
}

/// First message of the new chat.
pub fn first_message(parent_name: &str, parent_id: &str, sa: &Value, r: &Summary, jl: &Path, stopped_now: bool) -> String {
    let pref = crate::profile::with_name(crate::approvals::FROM_USER_PREFIX);
    let state = state_in_words(str_of(sa, "stato"), str_of(sa, "esito"), stopped_now);
    let finished = str_of(sa, "stato") == "finito" && !stopped_now;
    let mut t = format!("{pref} Questa chat continua il lavoro di un sotto-agente della chat [{}](juggl://sessione/{parent_id}). \
L'utente l'ha spostato qui con \"Apri come chat\": da adesso questo lavoro va avanti solo qui, la chat madre lo sa e non lo riprende.\n\n",
        parent_name.replace(['[', ']'], ""));
    let compito = if r.compito.trim().is_empty() { str_of(sa, "descrizione").to_string() } else { r.compito.trim().to_string() };
    let long = compito.chars().count() > 8000;
    t += &format!("## Compito originale\n{}{}\n\n", truncate(&compito, 8000), if long { "\n(continua nel transcript qui sotto)" } else { "" });
    t += &format!("## Stato\n{state}{}\n\n", if r.errori > 0 { format!(" · {} comandi andati in errore", r.errori) } else { String::new() });
    if !r.passi.is_empty() {
        t += "## Passi (la sua lista)\n";
        for (st, text) in &r.passi { t += &format!("- [{}] {text}\n", match st.as_str() { "completed" => "x", "in_progress" => "~", _ => " " }); }
        t += "\n";
    }
    if !r.azioni.is_empty() {
        t += "## Cosa ha fatto (ultime azioni)\n";
        for a in &r.azioni { t += &format!("- {a}\n"); }
        t += "\n";
    }
    if !r.ultimo_testo.is_empty() {
        t += &format!("## {}\n{}\n\n", if finished { "Risultato" } else { "Ultima cosa che ha scritto" }, truncate(&r.ultimo_testo, 3000));
    }
    t += "## File toccati (Edit/Write)\n";
    if r.file.is_empty() { t += "nessuno con Edit/Write (eventuali file scritti con comandi Bash sono nelle azioni)\n\n"; }
    else { for f in r.file.iter().take(40) { t += &format!("- {f}\n"); } if r.file.len() > 40 { t += &format!("- (altri {})\n", r.file.len() - 40); } t += "\n"; }
    t += &format!("## Transcript del sotto-agente\n{} (JSONL, una riga per evento: leggilo solo se ti manca un dettaglio, a pezzi, mai tutto)\n\n", jl.display());
    t += "## Cosa fare adesso\n";
    t += if finished {
        "Il lavoro risulta finito: non rifarlo. Controlla in breve che i file siano come descritto, poi dimmi in poche righe a che punto siamo e cosa resta, e aspetta."
    } else {
        "Riprendi il compito da dove si è fermato. Prima guarda lo stato attuale dei file toccati e non rifare quello che è già fatto; se qualcosa è a metà, sistemalo."
    };
    t
}

// ---------- name and group ----------

/// Name of the new chat: the sub-agent's short task. If the parent was in its group by name (folder outside
/// the projects), the new chat takes a word of the project, so it stays in the same group.
pub fn chat_name(parent: &Value, sa: &Value, cwd: &str) -> String {
    let d = one_line(str_of(sa, "descrizione"), 60);
    let base_name = if d.is_empty() { format!("Sotto-agente di {}", one_line(str_of(parent, "nome"), 40)) } else { d };
    let pm = parent.get("progetto").cloned().unwrap_or(Value::Null);
    if pm.is_null() { return base_name; }
    let projects = crate::session_care::projects();
    let child = crate::session_care::project_of(&json!({"cwd": cwd, "cwd_origine": cwd}), &base_name, &projects);
    if str_of(&child, "id") == str_of(&pm, "id") { return base_name; }
    let word = pm.get("parole").and_then(|x| x.as_array()).and_then(|a| a.iter().find_map(|w| w.as_str().filter(|w| !w.is_empty())))
        .map(|w| w.to_string()).unwrap_or_else(|| str_of(&pm, "nome").to_string());
    if word.is_empty() { base_name } else { format!("{word} · {base_name}") }
}

// ---------- waiting for the stop ----------

fn byte_size(p: &Path) -> u64 { std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) }

fn sub_state(id: &str, agent: &str) -> Option<Value> {
    let v = crate::find_session(id)?;
    crate::list_subagents(&v).into_iter().find(|x| str_of(x, "id") == agent)
}

/// Waits for the sub-agent to be stopped. Returns how (e.g. "avviso di fine", "transcript fermo da 8 s") or None.
fn wait_stopped(id: &str, agent: &str, jl: &Path, quiete: bool) -> Option<&'static str> {
    let t0 = Instant::now();
    let (mut p, mut quiet) = (byte_size(jl), Instant::now());
    while t0.elapsed() < Duration::from_secs(STOP_WAIT_S) {
        std::thread::sleep(Duration::from_millis(700));
        if let Some(sa) = sub_state(id, agent) { if str_of(&sa, "stato") != "lavora" { return Some("stato del sotto-agente"); } }
        let now_p = byte_size(jl);
        if now_p != p { p = now_p; quiet = Instant::now(); }
        else if quiete && quiet.elapsed() >= Duration::from_secs(QUIET_S) { return Some("transcript fermo dopo lo stop"); }
    }
    None
}

/// Background commands of the sub-agent without the end notice in the parent's transcript. Only if the parent is open and the
/// sub-agent worked in the last 6 hours (after a chat restart the notice no longer arrives).
fn live_commands(parent: &Value, r: &Summary, sa: &Value) -> Vec<String> {
    if r.in_sfondo.is_empty() || unix_now() - sa.get("fine").and_then(|x| x.as_f64()).unwrap_or(0.0) > 6.0 * 3600.0 { return vec![]; }
    // hibernated or closed chat: its background commands ended with it
    if matches!(str_of(parent, "vista"), "ibernata" | "chiusa") { return vec![]; }
    let results = crate::find_transcript(parent).map(|t| crate::task_outcomes(&t)).unwrap_or_default();
    r.in_sfondo.iter().filter(|b| !results.contains_key(*b)).cloned().collect()
}

// ---------- move ----------

/// The core, the same in the app and in tests. `ferma` = the user confirmed stopping it if it is working.
pub fn move_to_chat(id: &str, agent: &str, ferma: bool, az: &Actions) -> Value {
    if !crate::storage::is_valid_id(id) || id.starts_with("locale:") || agent.is_empty() || !agent.chars().all(|c| c.is_ascii_alphanumeric()) {
        return json!({"ok": false, "msg": "id non valido"});
    }
    let key = format!("{id}:{agent}");
    if !running().lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone()) { return json!({"ok": false, "msg": "Lo sto già spostando"}); }
    let r = move_inner(id, agent, ferma, az);
    running().lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
    r
}

fn move_inner(id: &str, agent: &str, ferma: bool, az: &Actions) -> Value {
    let Some(parent) = crate::find_session(id) else { return json!({"ok": false, "msg": "chat non trovata"}) };
    // already moved: open the chat created then (if it still exists)
    if let Some(c) = parent.get("sotto_spostati").and_then(|x| x.get(agent)).map(|x| str_of(x, "chat").to_string()) {
        if !c.is_empty() && state_file(&c).exists() { return json!({"ok": true, "id": c, "gia": true, "msg": "Era già spostato: apro la sua chat"}); }
    }
    let Some(dir) = crate::subagents_dir(&parent) else { return json!({"ok": false, "msg": "sotto-agenti non trovati"}) };
    let Some(mut sa) = crate::list_subagents(&parent).into_iter().find(|x| str_of(x, "id") == agent) else { return json!({"ok": false, "msg": "sotto-agente non trovato"}) };
    let jl = dir.join(format!("agent-{agent}.jsonl"));
    // folder: the one where the parent was born (its group), otherwise the current one
    let cwd = [str_of(&parent, "cwd_origine"), str_of(&parent, "cwd")].into_iter().find(|c| !c.is_empty() && *c != "/" && Path::new(c).is_dir()).unwrap_or("").to_string();
    if cwd.is_empty() { return json!({"ok": false, "msg": "la cartella della chat non esiste più"}); }
    let mut stopped_now = false;
    // background commands of the sub-agent without the end notice yet: they may write even after the sub-agent finished
    let bg_live = live_commands(&parent, &summarize(&jl, str_of(&sa, "tool")), &sa);
    let working = str_of(&sa, "stato") == "lavora";
    if working || !bg_live.is_empty() {
        if !ferma {
            let msg = if working { "Il sotto-agente sta lavorando: va fermato prima di spostarlo" } else { "Il sotto-agente ha comandi in sfondo forse ancora attivi: vanno fermati prima di spostarlo" };
            return json!({"ok": false, "conferma": true, "msg": msg});
        }
        if working {
            if let Err(e) = (az.ferma)(id, agent) { return json!({"ok": false, "msg": format!("Non fermato: {e}")}); }
            match wait_stopped(id, agent, &jl, az.quiete) {
                Some(how) => log_juggl(&format!("{id}: sotto-agente {agent} fermato per Apri come chat ({how})")),
                None => return json!({"ok": false, "msg": "Il sotto-agente non risulta ancora fermo: nessuna chat creata, riprova tra poco"}),
            }
        }
        // background commands: after the agent (which, finishing, may launch others), then the end notice or 15 s
        let bg_live = live_commands(&parent, &summarize(&jl, str_of(&sa, "tool")), &sa);
        for b in &bg_live { if let Err(e) = (az.ferma)(id, b) { log_juggl(&format!("{id}: comando in sfondo {b} non fermato: {e}")); } }
        if !bg_live.is_empty() {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(15) && crate::find_session(id).map(|m| !live_commands(&m, &summarize(&jl, str_of(&sa, "tool")), &sa).is_empty()).unwrap_or(false) {
                std::thread::sleep(Duration::from_millis(700));
            }
            log_juggl(&format!("{id}: comandi in sfondo del sotto-agente {agent} fermati per Apri come chat: {}", bg_live.join(", ")));
        }
        stopped_now = true;
        if let Some(x) = sub_state(id, agent) { sa = x; }
    }
    let r = summarize(&jl, str_of(&sa, "tool"));
    let name = chat_name(&parent, &sa, &cwd);
    let text = first_message(str_of(&parent, "nome"), id, &sa, &r, &jl, stopped_now);
    // state of the new chat written before the start: hosted::avvia_con reads it (name, permission level)
    let new_id = crate::new_uuid();
    let mut st = json!({"sessione": new_id, "nome_manuale": name, "da_sottoagente": {"madre": id, "agente": agent, "transcript": jl.to_string_lossy()}});
    let level = str_of(&parent, "permessi_chat");
    if crate::permissions::LEVELS.contains(&level) { st["permessi_chat"] = json!(level); }
    crate::write_state(&state_file(&new_id), &st);
    // written by Juggl, not by the user: the Sentinella derives no requests from it
    crate::requests::mark_handled_text(&new_id, &text);
    let new_id = match (az.avvia)(&cwd, &text, &new_id) {
        Ok(x) => x,
        Err(e) => { let _ = std::fs::remove_file(state_file(&new_id)); return json!({"ok": false, "msg": format!("Chat non avviata: {e}")}); }
    };
    // the parent knows: Juggl message, it does not interrupt the turn
    let pref = crate::profile::with_name(crate::approvals::FROM_USER_PREFIX);
    let note = format!("{pref} Spostato in chat [{}](juggl://sessione/{new_id}): il sotto-agente \"{}\" (id {agent}) ora continua lì. \
Non riprendere e non rilanciare quel lavoro: adesso è di quella chat.", name.replace(['[', ']'], ""), str_of(&sa, "descrizione"));
    let sent = (az.manda)(id, &note);
    crate::update_json(&state_file(id), |v| {
        if !v.get("sotto_spostati").map(|x| x.is_object()).unwrap_or(false) { v["sotto_spostati"] = json!({}); }
        v["sotto_spostati"][agent] = json!({"chat": new_id, "t": unix_now()});
        true
    });
    log_juggl(&format!("{id}: sotto-agente {agent} spostato nella chat {new_id} ({name}); avviso alla madre: {}", str_of(&sent, "esito")));
    json!({"ok": true, "id": new_id, "nome": name, "fermato": stopped_now, "avviso_madre": sent})
}

/// In the app: stop like the Ferma button, start with hosted::avvia_con, notice with Juggl's send.
pub fn open(id: String, agent: String, ferma: bool) -> Value {
    let Some(app) = crate::APP.get() else { return json!({"ok": false, "msg": "app non pronta"}) };
    // sub-agent or background command: stop_task straight to hosted chats (like the Ferma button), in the terminal TaskStop asked of the session
    let stop_fn = |m: &str, t: &str| -> Result<(), String> {
        if crate::hosted::is_hosted(m) { return crate::hosted::stop_task(m, t); }
        let r = crate::send_internal(m.to_string(), format!("Ferma subito con TaskStop il task {t} (sotto-agente o comando in sfondo): l'utente lo sposta in un'altra chat con \"Apri come chat\" di Juggl. Non rilanciarlo."));
        if r["ok"].as_bool().unwrap_or(false) { Ok(()) } else { Err(str_of(&r, "msg").to_string()) }
    };
    // model: like a new chat (Sonnet with Eco mode, otherwise the usual one)
    let start_fn = |cwd: &str, text: &str, new_id: &str| crate::hosted::start_with(app, cwd, text, Some(new_id), crate::eco::model_for_new());
    let send_fn = |m: &str, t: &str| crate::send_internal(m.to_string(), t.to_string());
    let r = move_to_chat(&id, &agent, ferma, &Actions { ferma: &stop_fn, avvia: &start_fn, manda: &send_fn, quiete: crate::hosted::is_hosted(&id) });
    let _ = app.emit("cambiato", ());
    r
}

#[tauri::command(async)]
pub fn sottoagente_in_chat(id: String, agente: String, ferma: Option<bool>) -> Value { open(id, agente, ferma.unwrap_or(false)) }

#[cfg(test)]
mod tests {
    use super::*;

    fn write_lines(p: &Path, rows: &[Value]) { std::fs::write(p, rows.iter().map(|r| r.to_string()).collect::<Vec<_>>().join("\n")).unwrap(); }

    #[test]
    fn summary_of_fork_and_normal_agent() {
        let d = std::env::temp_dir().join(format!("juggl-sposta-r-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        // fork: the task is in the Agent call at the top
        let f = d.join("fork.jsonl");
        write_lines(&f, &[
            json!({"type": "fork-context-ref"}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_X", "name": "Agent", "input": {"description": "Prova", "prompt": "Fai la cosa A"}}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}, {"type": "text", "text": "<fork-boilerplate>..."}]}}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Edit", "input": {"file_path": "/a/b.rs"}}]}}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t2", "name": "Write", "input": {"file_path": "/a/c.rs"}}]}}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t3", "name": "Edit", "input": {"file_path": "/a/b.rs"}}]}}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t4", "name": "TodoWrite", "input": {"todos": [{"content": "uno", "status": "completed"}, {"content": "due", "status": "in_progress"}]}}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "is_error": true, "content": "no"}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": [{"type": "text", "text": "Command running in background with ID: bb638eto4. Output is being written to: /tmp/x"}]}]}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Fatto quasi tutto"}]}}),
        ]);
        let r = summarize(&f, "toolu_X");
        assert_eq!(r.compito, "Fai la cosa A");
        assert_eq!(r.file, vec!["/a/b.rs", "/a/c.rs"]);
        assert_eq!(r.passi.len(), 2);
        assert_eq!(r.errori, 1);
        assert_eq!(r.in_sfondo, vec!["bb638eto4"]);
        assert_eq!(r.ultimo_testo, "Fatto quasi tutto");
        assert!(!r.azioni.iter().any(|a| a.contains("Prova")), "la chiamata del fork non e' un'azione: {:?}", r.azioni);
        // normal agent: the task is the first message
        let g = d.join("gp.jsonl");
        write_lines(&g, &[
            json!({"type": "user", "message": {"content": "Compito B"}}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls", "description": "Elenco"}}]}}),
        ]);
        let r = summarize(&g, "toolu_Y");
        assert_eq!(r.compito, "Compito B");
        assert_eq!(r.azioni, vec!["Comando: Elenco"]);
        let t = first_message("Madre", "m-1", &json!({"stato": "finito", "descrizione": "B"}), &r, &g, false);
        assert!(t.contains("## Compito originale\nCompito B") && t.contains("## Stato\nfinito") && t.contains("nessuno con Edit/Write") && t.contains(&g.to_string_lossy().to_string()));
        let t = first_message("Madre", "m-1", &json!({"stato": "lavora"}), &r, &g, true);
        assert!(t.contains("fermato a metà") && t.contains("Riprendi il compito"));
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Tests with real Claude (cargo test sposta -- --ignored --nocapture): test parent chat in a temporary folder,
/// with its own Juggl base (JUGGL_BASE), launching a real sub-agent with the small model.
#[cfg(test)]
mod real_tests {
    use super::*;
    use crate::load_json;
    use std::collections::HashMap;
    use std::io::Write;
    use std::process::{Child, ChildStdin, Command, Stdio};

    fn stdin() -> &'static Mutex<HashMap<String, ChildStdin>> { static M: OnceLock<Mutex<HashMap<String, ChildStdin>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(HashMap::new())) }
    fn children() -> &'static Mutex<Vec<Child>> { static M: OnceLock<Mutex<Vec<Child>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(vec![])) }
    fn init() -> &'static Mutex<HashMap<String, Value>> { static M: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(HashMap::new())) }
    fn asked() -> &'static Mutex<HashMap<String, usize>> { static M: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(HashMap::new())) }
    fn state(id: &str) -> Value { load_json(&state_file(id)).unwrap_or(json!({})) }

    fn write_line(id: &str, r: &Value) -> Result<(), String> {
        let mut m = stdin().lock().unwrap();
        let w = m.get_mut(id).ok_or("stdin mancante")?;
        writeln!(w, "{r}").and_then(|_| w.flush()).map_err(|e| e.to_string())
    }
    fn send_msg(id: &str, t: &str) -> Value {
        match write_line(id, &json!({"type": "user", "message": {"role": "user", "content": t}})) { Ok(_) => json!({"ok": true, "esito": "inviato"}), Err(e) => json!({"ok": false, "msg": e}) }
    }
    /// Starts a chat like hosted::avvia_con: state already written, same permission arguments, small model.
    fn start_chat(id: &str, cwd: &str, text: &str) -> Result<String, String> {
        let st = state(id);
        let mut a: Vec<String> = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
            "--model", "haiku", "--setting-sources", "project,local", "--strict-mcp-config", "--session-id", id].iter().map(|x| x.to_string()).collect();
        let (x, liv) = crate::permissions::cli_args(&st, cwd, &a);
        a.extend(x);
        let mut c = crate::new_command(crate::profile::claude_cmd()); crate::NoConsoleWindow::no_console_window(&mut c);
        c.args(&a).current_dir(cwd).env("TAVOLO_INTERNO", "1").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        for (k, _) in std::env::vars() { if k.starts_with("CLAUDE") || k.starts_with("CMUX") { c.env_remove(k); } }
        let mut f = c.spawn().map_err(|e| e.to_string())?;
        let pid = f.id();
        crate::update_json(&state_file(id), |v| {
            v["pid"] = json!(pid); v["cwd"] = json!(cwd); v["cwd_origine"] = json!(cwd); v["ospite"] = json!("tavolo");
            v["permessi_livello"] = json!(liv); v["stato"] = json!("lavora"); v["da"] = json!(unix_now()); v["agg"] = json!(unix_now()); true
        });
        stdin().lock().unwrap().insert(id.to_string(), f.stdin.take().unwrap());
        let out = f.stdout.take().unwrap();
        let id2 = id.to_string();
        std::thread::spawn(move || {
            for r in BufReader::new(out).lines().map_while(Result::ok) {
                let Ok(d) = serde_json::from_str::<Value>(&r) else { continue };
                if str_of(&d, "type") == "system" && str_of(&d, "subtype") == "init" { init().lock().unwrap().insert(id2.clone(), d.clone()); }
                // permissions asked (parent on "chiedi"): in tests we always allow, and count them
                if str_of(&d, "type") == "control_request" && str_of(&d["request"], "subtype") == "can_use_tool" {
                    *asked().lock().unwrap().entry(id2.clone()).or_insert(0) += 1;
                    let _ = write_line(&id2, &crate::permissions::response(str_of(&d, "request_id"), &d["request"], "consenti", ""));
                }
                if str_of(&d, "type") == "result" { crate::update_json(&state_file(&id2), |v| { v["stato"] = json!("aspetta"); v["agg"] = json!(unix_now()); true }); }
            }
        });
        children().lock().unwrap().push(f);
        if !text.is_empty() { write_line(id, &json!({"type": "user", "message": {"role": "user", "content": text}}))?; }
        Ok(id.to_string())
    }
    fn stop_chat(id: &str, agent: &str) -> Result<(), String> {
        write_line(id, &json!({"type": "control_request", "request_id": crate::hosted::uuid(), "request": {"subtype": "stop_task", "task_id": agent}}))
    }
    /// Waits for the parent's first sub-agent with the wanted state (or "qualsiasi").
    fn wait_sub(id: &str, wanted_state: &str, max: u64) -> Value {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(max) {
            if let Some(v) = crate::find_session(id) {
                if let Some(x) = crate::list_subagents(&v).into_iter().find(|x| wanted_state == "qualsiasi" || str_of(x, "stato") == wanted_state) { return x; }
            }
            std::thread::sleep(Duration::from_millis(800));
        }
        panic!("nessun sotto-agente {wanted_state} in {max} s");
    }
    fn wait_init(id: &str) -> Value {
        for _ in 0..240 { if let Some(v) = init().lock().unwrap().get(id).cloned() { return v; } std::thread::sleep(Duration::from_millis(250)); }
        panic!("niente init per {id}");
    }
    /// User messages the parent received (from its transcript), to see the notice.
    fn parent_texts(id: &str) -> String {
        let Some(v) = crate::find_session(id) else { return String::new() };
        crate::find_transcript(&v).and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default()
    }
    fn new_uid(n: u8) -> String { format!("00000000-0000-4000-9000-{:012}", std::process::id() as u64 * 100 + n as u64) }

    #[test]
    #[ignore]
    fn open_as_chat_with_real_claude() {
        let root = crate::home().join(format!(".cache/juggl-sposta-prova/e2e-{}", std::process::id()));
        let test_base = root.join("juggl");
        std::fs::create_dir_all(test_base.join("stato")).unwrap();
        let _base_lock = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (a whole-process variable)
        std::env::set_var("JUGGL_BASE", &test_base);
        let cart = root.join("progetto-prova");
        std::fs::create_dir_all(&cart).unwrap();
        let cwd = cart.to_string_lossy().to_string();
        // general "mai", project with the test folder: the parent has its own "chiedi" exception
        std::fs::write(test_base.join("impostazioni.json"), json!({"permessi": "mai", "pilota": false}).to_string()).unwrap();
        std::fs::write(test_base.join("progetti.json"), json!({"progetti": [{"id": "prova", "nome": "Prova", "cartella": cwd}]}).to_string()).unwrap();
        let actions_ = Actions { ferma: &stop_chat, avvia: &|c: &str, t: &str, n: &str| start_chat(n, c, t), manda: &send_msg, quiete: true };

        // ---- 1. finished sub-agent ----
        let m1 = new_uid(1);
        crate::write_state(&state_file(&m1), &json!({"sessione": m1, "nome_manuale": "Madre di prova", "permessi_chat": "chiedi"}));
        start_chat(&m1, &cwd, "Usa lo strumento Agent (subagent_type general-purpose, model haiku, in primo piano) con description \"Scrivo il file finito\" e questo prompt: \"Con lo strumento Write crea il file finito.txt nella cartella attuale con dentro la parola ciao. Poi rispondi solo FATTO.\". Quando ha finito rispondi solo OK.").unwrap();
        assert_eq!(str_of(&wait_init(&m1), "permissionMode"), "default", "madre su chiedi");
        let sa = wait_sub(&m1, "finito", 240);
        println!("1a. sotto-agente finito: {} ({}); permessi chiesti dalla madre su chiedi: {}", str_of(&sa, "descrizione"), str_of(&sa, "id"), asked().lock().unwrap().get(&m1).copied().unwrap_or(0));
        // the parent must have finished its turn (so the notice arrives as a new message)
        let t0 = Instant::now(); while str_of(&state(&m1), "stato") != "aspetta" && t0.elapsed() < Duration::from_secs(120) { std::thread::sleep(Duration::from_millis(500)); }
        let r = move_to_chat(&m1, str_of(&sa, "id"), false, &actions_);
        assert_eq!(r["ok"], json!(true), "{r}");
        let f1 = str_of(&r, "id").to_string();
        let st1 = state(&f1);
        println!("1b. chat nuova {f1}: nome «{}», cartella {}", str_of(&st1, "nome_manuale"), str_of(&st1, "cwd"));
        assert_eq!(str_of(&st1, "cwd"), cwd, "stessa cartella");
        let v1 = crate::find_session(&f1).expect("chat nuova nell'elenco");
        let vm = crate::find_session(&m1).unwrap();
        assert_eq!(str_of(&v1["progetto"], "id"), "prova"); assert_eq!(str_of(&vm["progetto"], "id"), "prova");
        println!("1c. stesso gruppo: {} = {}", str_of(&v1["progetto"], "id"), str_of(&vm["progetto"], "id"));
        // permissions inherited: the parent has "chiedi" with the general on "mai"
        assert_eq!(str_of(&st1, "permessi_chat"), "chiedi");
        assert_eq!(str_of(&state(&f1), "permessi_livello"), "chiedi");
        assert_eq!(str_of(&wait_init(&f1), "permissionMode"), "default", "figlia su chiedi");
        println!("1d. permessi: figlia permessi_chat=chiedi, livello di partenza chiedi, permissionMode default (generale: mai)");
        // the whole first message: from the daughter's transcript
        std::thread::sleep(Duration::from_secs(3));
        let tf = crate::find_session(&f1).and_then(|v| crate::find_transcript(&v)).and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        for piece in ["## Compito originale", "finito.txt", "## Stato\\nfinito", "## File toccati", "finito.txt", "## Transcript del sotto-agente", "agent-"] {
            assert!(tf.contains(piece), "manca «{piece}» nel primo messaggio");
        }
        println!("1e. primo messaggio con compito, stato finito, file toccati, transcript");
        let t0 = Instant::now();
        while !parent_texts(&m1).contains(&format!("juggl://sessione/{f1}")) && t0.elapsed() < Duration::from_secs(30) { std::thread::sleep(Duration::from_millis(500)); }
        assert!(parent_texts(&m1).contains(&format!("Spostato in chat [")), "la madre non ha ricevuto la riga");
        println!("1f. la madre ha ricevuto «Spostato in chat [..](juggl://sessione/{f1})»");
        let r2 = move_to_chat(&m1, str_of(&sa, "id"), false, &actions_);
        assert_eq!(str_of(&r2, "id"), f1); assert_eq!(r2["gia"], json!(true));
        println!("1g. secondo clic: apre la stessa chat");

        // ---- 2. sub-agent at work: confirmation needed, then stopped, then chat; no double writing ----
        let m2 = new_uid(2);
        crate::write_state(&state_file(&m2), &json!({"sessione": m2, "nome_manuale": "Madre due", "permessi_chat": "mai"}));
        start_chat(&m2, &cwd, "Usa lo strumento Agent (subagent_type general-purpose, model haiku, run_in_background true) con description \"Conto lento\" e questo prompt: \"Esegui con lo strumento Bash, cosi' com'e', questo comando (dura circa 2 minuti, usa timeout 300000): for i in $(seq 1 60); do echo $i >> conta.txt; sleep 2; done. Poi rispondi FATTO.\". Poi rispondi solo PARTITO e non fare altro.").unwrap();
        // at work, or already finished with the command still in the background: in both cases it must be stopped
        let sa = wait_sub(&m2, "qualsiasi", 180);
        let aid = str_of(&sa, "id").to_string();
        // wait until it has written something
        let t0 = Instant::now(); while std::fs::read_to_string(cart.join("conta.txt")).unwrap_or_default().lines().count() < 3 && t0.elapsed() < Duration::from_secs(150) { std::thread::sleep(Duration::from_millis(500)); }
        println!("2a. sotto-agente {}: {} ({aid}), conta.txt sta crescendo", str_of(&sub_state(&m2, &aid).unwrap(), "stato"), str_of(&sa, "descrizione"));
        let r = move_to_chat(&m2, &aid, false, &actions_);
        assert_eq!(r["conferma"], json!(true), "{r}");
        println!("2b. senza conferma: {}", str_of(&r, "msg"));
        // start of the new chat delayed by 8 s: meanwhile conta.txt must not grow (the stopped sub-agent does not write)
        static COUNT: Mutex<(usize, usize)> = Mutex::new((0, 0));
        let conta = cart.join("conta.txt");
        let n_lines = |p: &Path| std::fs::read_to_string(p).unwrap_or_default().lines().count();
        let start_after = |c: &str, t: &str, n: &str| { let a = n_lines(&conta); std::thread::sleep(Duration::from_secs(8)); *COUNT.lock().unwrap() = (a, n_lines(&conta)); start_chat(n, c, t) };
        let actions2 = Actions { ferma: &stop_chat, avvia: &start_after, manda: &send_msg, quiete: true };
        let r = move_to_chat(&m2, &aid, true, &actions2);
        assert_eq!(r["ok"], json!(true), "{r}");
        let f2 = str_of(&r, "id").to_string();
        let rows = |p: &Path| std::fs::read_to_string(p).unwrap_or_default().lines().count();
        let n0 = rows(&cart.join("conta.txt"));
        let sa2 = sub_state(&m2, &aid).unwrap();
        let (before, after) = *COUNT.lock().unwrap();
        println!("2c. fermato e spostato in {f2}: stato del sotto-agente {} (esito {}); conta.txt al fermo {before} righe, 8 s dopo {after} righe", str_of(&sa2, "stato"), str_of(&sa2, "esito"));
        assert!(before > 0, "il sotto-agente non aveva ancora scritto");
        assert_eq!(before, after, "dopo il fermo conta.txt e' cresciuto: scrive ancora");
        let _ = n0;
        assert_ne!(str_of(&sa2, "stato"), "lavora");
        // the stopped sub-agent no longer writes: the transcript does not grow and conta.txt grows only for the new chat
        let jl = crate::subagents_dir(&crate::find_session(&m2).unwrap()).unwrap().join(format!("agent-{aid}.jsonl"));
        let p0 = byte_size(&jl);
        std::thread::sleep(Duration::from_secs(12));
        assert_eq!(byte_size(&jl), p0, "il sotto-agente fermato ha scritto ancora");
        println!("2d. dopo 12 s il transcript del sotto-agente non e' cresciuto ({p0} byte): nessuna scrittura doppia; conta.txt ora {} righe (la chat nuova riprende)", rows(&cart.join("conta.txt")));
        assert_eq!(str_of(&state(&f2), "permessi_livello"), "mai");
        let t0 = Instant::now();
        while !parent_texts(&m2).contains(&format!("juggl://sessione/{f2}")) && t0.elapsed() < Duration::from_secs(30) { std::thread::sleep(Duration::from_millis(500)); }
        assert!(parent_texts(&m2).contains(&format!("juggl://sessione/{f2}")), "la madre due non ha ricevuto la riga");
        println!("2e. la madre due ha ricevuto la riga; figlia con permessi mai come la madre");
        let tf = crate::find_session(&f2).and_then(|v| crate::find_transcript(&v)).and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        assert!(tf.contains("fermato a metà"), "stato fermato nel primo messaggio");

        // cleanup: processes, test conversations in ~/.claude/projects, folders
        for mut f in children().lock().unwrap().drain(..) { let _ = f.kill(); let _ = f.wait(); }
        for id in [&m1, &m2, &f1, &f2] {
            if let Some(p) = crate::find_transcript(&json!({"sessione": id})) {
                let _ = std::fs::remove_dir_all(p.with_extension(""));
                let _ = std::fs::remove_file(&p);
            }
        }
        // the test conversations folder (name with juggl-sposta-prova: ours only), even with empty memory
        let project = crate::home().join(".claude/projects").join(crate::hosted::claude_project(&cwd));
        if project.to_string_lossy().contains("juggl-sposta-prova") { let _ = std::fs::remove_dir_all(&project); }
        let _ = std::fs::remove_dir_all(&root);
        println!("tutto ok, prove cancellate");
    }
}
