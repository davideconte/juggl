// Tavolo: native app (Tauri). The core reads the state written by the hooks (~/tavolo/stato) and the session
// transcripts; it updates by itself when a state file changes (no constant polling). The terminal that hosts
// the sessions (today cmux) is only used for "Apri" and "Rispondi".
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod storage;
mod session_care;
mod local_models;
mod helpers;
mod background_jobs;
mod autopilot;
mod requests;
mod approvals;
mod bridge;
mod hosted;
mod keeper;
mod migrate;
mod voice_notes;
mod messages;
mod media_strip;
mod documents;
mod search_index;
mod microphone;
mod push;
mod summaries;
mod quota;
mod claude_account;
mod in_progress;
mod eco;
mod profile;
mod updates;
mod import;
mod license;
// automatic coordinator (12/11): a coordinator for every chat group (src/coordinator.rs)
mod coordinator;
// idle compaction (16/11): equivalent of Claude Code's idle compaction for chats inside Juggl (src/compaction.rs)
mod compaction;
// live activity (12/11): iPhone Live Activity updates via APNs (src/live.rs)
mod live;
// permissions (12/11): chat permission levels and permission requests with buttons (src/permissions.rs)
mod permissions;
// live edits (12/11): diff per answer, preview of what it writes, Undo
mod live_edits;
mod fast_open;   // fast (14/11): cache of chats and logs, git that does not block
// open as chat (12/11): a subagent becomes a real chat in the same folder (src/open_as_chat.rs)
mod open_as_chat;
mod sys;
mod hook;
mod fallback;   // fallback (12/11): Eco above 90%, Continue on Codex, cautious chats
mod engines;   // engines (12/11): chats with Codex (ChatGPT account)
mod logos;   // list (15/11): automatic icons and project colors
mod disk_space;   // disk space for everyone (14/11): built-in Space engine, for those without the personal engine
mod commands;   // commands (13/11): "/" menu in chats, Juggl and Claude Code commands
mod settings_chat;   // settings as a chat (16/11): night alerts, model for new chats, change log, phrases not understood
mod support;   // help and reports (17/11): fixed help chat, assistant with the guide, reports to juggl.dev
mod snap;   // snap (16/11): global shortcut that captures an area of the screen and attaches it to the chat
mod claude_login;   // switch account (12/11): Claude login/logout from Juggl and "/..." commands that do not exist in -p
// Personal features (cargo feature "personale", on by default): messaging, agent and local model on the PC.
// Without the feature the same commands exist as placeholders (src/personal_stubs.rs) and answer "non disponibile".
#[cfg(not(feature = "personale"))]
mod personal_stubs;
#[cfg(not(feature = "personale"))]
use personal_stubs::{pc_agent, pc_local_engine, wa};
#[cfg(windows)] use sys as libc;

/// Windows: no black console window for programs launched by the app (claude, git...). Does nothing elsewhere.
pub(crate) trait NoConsoleWindow { fn no_console_window(&mut self) -> &mut Self; }
impl NoConsoleWindow for std::process::Command {
    fn no_console_window(&mut self) -> &mut Self {
        #[cfg(windows)] { use std::os::windows::process::CommandExt; self.creation_flags(sys::CREATE_NO_WINDOW); }
        self
    }
}

/// The ONLY way to create a command in the app (12/11, popup reported by a tester): on Windows it already starts without a console window.
/// Whoever needs to change flags (process_group, DETACHED) sets them afterwards, always with CREATE_NO_WINDOW inside.
/// The test `no_command_with_window` fails the build if a `Command::new` appears in src/ outside of here.
pub(crate) fn new_command<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    let mut c = std::process::Command::new(program);
    c.no_console_window();
    c
}

#[cfg(test)]
mod window_tests {
    /// Every program launched by the app goes through `comando()` (src/*.rs) or, in the standalone programs (src/programs,
    /// src/sys.rs), sets the Windows flags within 8 lines: process_group (which sets CREATE_NO_WINDOW) or creation_flags.
    #[test]
    fn no_command_with_window() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![];
        for d in [root.clone(), root.join("programs")] {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                if e.path().extension().map(|x| x == "rs").unwrap_or(false) { files.push(e.path()); }
            }
        }
        assert!(files.len() > 20, "non trovo i sorgenti in {}", root.display());
        let needle = ["Command", "::new("].concat();
        let mut errors = vec![];
        for f in &files {
            let text = std::fs::read_to_string(f).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            let standalone = f.ends_with("sys.rs") || f.parent().map(|p| p.ends_with("programs")).unwrap_or(false);
            for (i, r) in lines.iter().enumerate() {
                if !r.contains(&needle) || r.trim_start().starts_with("//") { continue; }
                if f.ends_with("main.rs") && r.contains("let mut c = std::process::") { continue; }   // inside comando()
                let following = lines[i..(i + 8).min(lines.len())].join("\n");
                if standalone && (following.contains("process_group(") || following.contains("creation_flags(")) { continue; }
                errors.push(format!("{}:{}: {}", f.display(), i + 1, r.trim()));
            }
        }
        assert!(errors.is_empty(), "comandi che su Windows aprirebbero una finestra (usa crate::comando):\n{}", errors.join("\n"));
    }
}

use notify::{RecursiveMode, Watcher};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};

const CMUX: &str = "/Applications/cmux.app/Contents/Resources/bin/cmux";
const MAX_WAIT_HOURS: f64 = 12.0;
const DIALOGS: [&str; 9] = ["Do you want", "Esc to cancel", "Enter to confirm", "[y/n]", "Yes, and", "\u{276f} 1.", "Would you like", "Select ", "Press Enter"];
const PLACEHOLDERS: [&str; 2] = ["Try \"", "Press up to edit queued messages"];

fn home() -> PathBuf { PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into())) }
/// Tests: JUGGL_BASE applies to the whole process and tests run on different threads, so whoever changes it takes this
/// lock at the start and holds it until the end (14/11, stable tests). A panicked thread does not block the others.
#[cfg(test)]
pub fn test_data_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}
/// Tests that change environment variables (HOME, JUGGL_BASE, ...) hold this guard: on drop, even after a panic, the
/// variables go back to what they were, so tests that run later (alphabetical order of modules) see the real ones.
#[cfg(test)]
pub struct EnvGuard(Vec<(String, Option<std::ffi::OsString>)>);
#[cfg(test)]
impl EnvGuard {
    pub fn new(names: &[&str]) -> Self { EnvGuard(names.iter().map(|n| (n.to_string(), std::env::var_os(n))).collect()) }
}
#[cfg(test)]
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (n, v) in self.0.drain(..) { match v { Some(v) => std::env::set_var(&n, v), None => std::env::remove_var(&n) } }
    }
}
/// Juggl data folder (JUGGL_BASE for tests). rename (14/11): see dati_dir().
fn base_dir() -> PathBuf { data_dir() }

/// rename (14/11): the data folder. Whoever has used Juggl from before (the user, the testers) has the data in ~/tavolo and it stays there;
/// new users have it in ~/.juggl, hidden like ~/.claude and ~/.codex. Any ~/tavolo (without the Juggl
/// files) does not count. The choice is remembered for the current home: it is called often, and tests change HOME.
pub(crate) fn data_dir() -> PathBuf {
    if let Some(b) = std::env::var("JUGGL_BASE").ok().filter(|x| Path::new(x).is_absolute()) { return PathBuf::from(b); }
    static CHOICE: std::sync::Mutex<Option<(PathBuf, PathBuf)>> = std::sync::Mutex::new(None);
    let h = home();
    let mut g = CHOICE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((home_dir, d)) = g.as_ref() { if *home_dir == h { return d.clone(); } }
    let d = choose_data_dir(&h);
    *g = Some((h, d.clone()));
    d
}

/// rename (14/11): ~/tavolo if it already contains Juggl data, otherwise ~/.juggl.
fn choose_data_dir(h: &Path) -> PathBuf {
    let old = h.join("tavolo");
    if ["ponte.json", "impostazioni.json", "stato"].iter().any(|x| old.join(x).exists()) { old } else { h.join(".juggl") }
}

/// rename (14/11): the data folder written for people and for commands (~/.juggl, ~/tavolo).
pub(crate) fn data_dir_short() -> String {
    let (d, h) = (base_dir(), home());
    match d.strip_prefix(&h) { Ok(r) if h != Path::new("/") => format!("~/{}", r.to_string_lossy().replace('\\', "/")), _ => d.to_string_lossy().to_string() }
}
fn unix_now() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
fn load_json(p: &Path) -> Option<Value> { serde_json::from_str(&fs::read_to_string(p).ok()?).ok() }
fn str_of<'a>(v: &'a Value, k: &str) -> &'a str { v.get(k).and_then(|x| x.as_str()).unwrap_or("") }
fn f64_of(v: &Value, k: &str) -> f64 { v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) }

/// Full write of a shared JSON file (unique temp file, fsync, rename: see storage.rs).
/// For "read, change, write" use `update_json`, which holds the lock.
fn write_state(p: &Path, v: &Value) {
    if let Err(e) = storage::write_json(p, v) { log_juggl(&format!("scrittura non riuscita: {e}")); }
}

/// Read, change, write an existing JSON file under lock (nothing if it is missing or unreadable).
/// The function returns false to not write. Returns true if the file was there.
pub(crate) fn update_json(p: &Path, f: impl FnOnce(&mut Value) -> bool) -> bool {
    match storage::modify(p, None, f) {
        Ok(_) => true,
        Err(e) => { if p.exists() { log_juggl(&format!("aggiornamento non riuscito: {e}")); } false }
    }
}

/// Juggl technical log (~/tavolo/juggl.log): write errors, loop panics.
pub(crate) fn log_juggl(r: &str) {
    use std::io::Write;
    let p = base_dir().join("juggl.log");
    // cap: above 2 MB it starts from zero
    if fs::metadata(&p).map(|m| m.len() > 2_000_000).unwrap_or(false) { let _ = fs::remove_file(&p); }
    if let Ok(mut fh) = fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(fh, "{:.0} {r}", unix_now());
    }
}

/// One round of the background threads (Sentinel, queue, Keeper): a panic no longer stops them silently.
fn run_guarded(name: &str, f: impl FnOnce()) {
    if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        let msg = e.downcast_ref::<&str>().map(|x| x.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default();
        log_juggl(&format!("panic nel giro {name}: {msg}"));
    }
}

fn is_alive(pid: i64) -> bool { pid > 0 && unsafe { libc::kill(pid as i32, 0) == 0 } }

/// Last piece of a path: "/Users/<name>/Progetto" -> "Progetto"; on Windows also "D:\\lavori\\Progetto".
fn path_tail(p: &str) -> String { p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("").to_string() }

fn name_of(v: &Value, names: &Value) -> String {
    let sid = str_of(v, "sessione");
    if !str_of(v, "nome_manuale").is_empty() { return str_of(v, "nome_manuale").into(); }
    if let Some(n) = names.get(sid).and_then(|x| x.get("name")).and_then(|x| x.as_str()) { if !n.is_empty() { return n.into(); } }
    if !str_of(v, "titolo").is_empty() { return str_of(v, "titolo").into(); }
    // the name comes from the folder the session started in, not the current one: a `cd` into a subfolder
    // (Claude Code records it as the session folder) renamed it, e.g. CLIENTE -> "2026-10-03" (09/11)
    let origin = str_of(v, "cwd_origine");
    let cwd = if !origin.is_empty() && origin != "/" && !origin.contains("/scratchpad") { origin } else { str_of(v, "cwd") };
    if cwd.contains("/scratchpad") || cwd.is_empty() || cwd == "/" {
        let c = str_of(v, "chiesto");
        return if c.is_empty() { sid.chars().take(8).collect() } else { c.chars().take(40).collect() };
    }
    path_tail(cwd)
}

fn list_sessions() -> Vec<Value> {
    let mut v = list_sessions_with_pid();
    for x in v.iter_mut() { x.as_object_mut().unwrap().remove("pid"); }
    v
}

/// Terminal tab that REALLY hosts each session, according to cmux (a tab can be handed to another
/// conversation: without this check Juggl would read or write in the wrong session).
fn real_tabs() -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Some(v) = load_json(&home().join(".cmuxterm/claude-hook-sessions.json")) {
        if let Some(Value::Object(a)) = v.get("activeSessionsBySurface") {
            for (sf, x) in a { m.insert(str_of(x, "sessionId").to_string(), sf.clone()); }
        }
    }
    m
}

fn list_sessions_with_pid() -> Vec<Value> { list_sessions_of(None) }

/// The sessions (all, or only `only`): for a single one only its state file is read, so reloading the chat on
/// screen does not recompute all the others (it was the slow part of every update).
fn list_sessions_of(only: Option<&str>) -> Vec<Value> {
    let names = load_json(&home().join(".claude/state/nomi-sessioni.json")).unwrap_or(json!({}));
    let pp = session_care::projects();
    // fast (14/11): for a single chat the 2 s old `ps` is enough (it cost 30 ms on every open)
    let proc = if only.is_some() { fast_open::processes() } else { std::sync::Arc::new(session_care::processes()) };
    let tabs = real_tabs();
    let mut out = vec![];
    let now = unix_now();
    let paths: Vec<PathBuf> = match only {
        Some(id) => vec![base_dir().join("stato").join(format!("{id}.json"))],
        None => match fs::read_dir(base_dir().join("stato")) { Ok(rd) => rd.flatten().map(|e| e.path()).collect(), Err(_) => return out },
    };
    for p in paths {
        if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        let Some(mut v) = load_json(&p) else { continue };
        if let Some(id) = only { if str_of(&v, "sessione") != id { continue; } }
        // archived: out of the list, the autopilot, alerts and reminders (they live in `archived_sessions()`)
        if str_of(&v, "stato") == "archiviata" { continue; }
        // processes found at startup with no title or name: not sessions you are working on
        if v.get("importata").and_then(|x| x.as_bool()).unwrap_or(false) && str_of(&v, "titolo").is_empty() && str_of(&v, "nome_manuale").is_empty()
            && (str_of(&v, "surface").is_empty() || str_of(&v, "stato") == "ibernata") { continue; }
        // verified tab: the one cmux says hosts this session, otherwise none
        if !str_of(&v, "surface").is_empty() || tabs.contains_key(str_of(&v, "sessione")) {
            match tabs.get(str_of(&v, "sessione")) {
                Some(sf) => { v["surface"] = json!(sf); }
                None => { if let Some(o) = v.as_object_mut() { o.remove("surface"); } }
            }
        }
        let pid = v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
        let hibernated = str_of(&v, "stato") == "ibernata";
        let just_resumed = now - f64_of(&v, "ripresa_il") < 600.0;
        if str_of(&v, "stato") == "chiusa" || (!hibernated && !just_resumed && !is_alive(pid)) { continue; }
        let mut st = str_of(&v, "stato").to_string();
        if !hibernated && is_alive(pid) {
            let (mb, cpu) = proc.tree_usage(pid);
            v["memoria_mb"] = json!(mb.round());
            v["cpu"] = json!((cpu * 10.0).round() / 10.0);
        }
        // "lavora" but stuck for a while on a question or a permission in the terminal: it is actually waiting for you
        if st == "lavora" && !str_of(&v, "surface").is_empty() && f64_of(&v, "cpu") < 5.0 && now - f64_of(&v, "agg") > 12.0 {
            let dt = terminal_question(str_of(&v, "surface"));
            if !dt.is_null() { st = "bloccata".into(); v["domanda"] = json!(str_of(&dt, "testo")); }
        }
        // permissions (12/11): hosted chat stuck on a permission request: waiting for you
        if !hibernated && is_alive(pid) && permissions::is_waiting(&v) {
            st = "bloccata".into();
            if let Some(d) = permissions::question(&v) { v["domanda"] = json!(str_of(&d, "testo")); }
        }
        if st == "aspetta" && now - f64_of(&v, "da") > MAX_WAIT_HOURS * 3600.0 { st = "ferma".into(); }
        if st == "aspetta" && v.get("serve_te").and_then(|x| x.as_bool()) == Some(false) {
            st = if f64_of(&v, "letta_da") == f64_of(&v, "da") { "ferma".into() } else { "fatta".into() };
        }
        if st == "aspetta" && f64_of(&v, "letta_da") == f64_of(&v, "da") && v.get("serve_te").is_none() && now - f64_of(&v, "da") > 3600.0 { st = "ferma".into(); }
        let name = name_of(&v, &names);
        if st == "lavora" { v["ora_fa"] = quick_current(&v); }
        let prog = session_care::project_of(&v, &name, &pp);
        let sa = list_subagents(&v);
        let active = sa.iter().filter(|x| str_of(x, "stato") == "lavora").count();
        let recent: Vec<Value> = sa.into_iter().filter(|x| str_of(x, "stato") == "lavora" || now - f64_of(x, "fine") < 1800.0).take(4).collect();
        v["sotto_attivi"] = json!(active);
        // jobs launched with run_in_background that do not have the end notice yet (0.1.74): optional field, the state text does not change
        if matches!(st.as_str(), "lavora" | "ferma" | "fatta" | "aspetta") && now - f64_of(&v, "da") < 86400.0 { v["in_background"] = json!(background_jobs::in_background(&v)); }
        v["sotto_recenti"] = json!(recent);
        // main turn finished but subagents still working: for the user the session is working.
        // "vista_turno" keeps the real turn state (message queue, Sentinel, sending). "aspetta" and "bloccata" stay.
        if active > 0 && matches!(st.as_str(), "ferma" | "fatta") {
            v["vista_turno"] = json!(st);
            v["lavora_sotto"] = json!(active);
            st = "lavora".into();
        }
        let o = v.as_object_mut().unwrap();
        o.insert("vista".into(), json!(st));
        o.insert("nome".into(), json!(name));
        o.insert("progetto".into(), prog);
        out.push(v);
    }
    // merge (15/11): a sub-chat (sotto_di) takes the project of its main chat, if the main chat is open
    inherit_project(&mut out);
    // the ready ones (Import projects) at the bottom, all together: you have not used them yet
    let rank = |v: &Value| match str_of(v, "vista") { "bloccata" => 0, "aspetta" => 1, "fatta" => 2, "lavora" => 3,
        "ibernata" if v.get("pronta").and_then(|x| x.as_bool()).unwrap_or(false) => 6, "ibernata" => 5, _ => 4 };
    out.sort_by(|a, b| {
        let (ra, rb) = (rank(a), rank(b));
        if ra != rb { return ra.cmp(&rb); }
        let (ta, tb) = (f64_of(a, "da"), f64_of(b, "da"));
        if ra <= 2 { ta.partial_cmp(&tb).unwrap() } else { tb.partial_cmp(&ta).unwrap() }
    });
    out
}

// ---------- presence at the Mac (same rule as bin/tavolo: under 5 min at the Mac, over 10 away) ----------

fn idle_secs() -> f64 {
    // Linux: no idle-time reading (depends on X11/Wayland): always "at the computer"
    #[cfg(target_os = "linux")] { return 0.0; }
    #[cfg(windows)] { return sys::idle_secs(); }
    #[allow(unreachable_code)]
    let Ok(o) = crate::new_command("ioreg").args(["-c", "IOHIDSystem"]).output() else { return 0.0 };
    let t = String::from_utf8_lossy(&o.stdout);
    for r in t.lines() {
        if let Some(i) = r.find("\"HIDIdleTime\" = ") {
            return r[i + 16..].trim().parse::<f64>().unwrap_or(0.0) / 1e9;
        }
    }
    0.0
}

fn user_at_mac() -> bool {
    let i = idle_secs();
    if i < 300.0 { return true; }
    if i > 600.0 { return false; }
    load_json(&base_dir().join("presenza.json")).and_then(|v| v.get("al_mac").and_then(|x| x.as_bool())).unwrap_or(true)
}

// ---------- transcript ----------

fn find_transcript(v: &Value) -> Option<PathBuf> {
    let t = str_of(v, "transcript");
    if !t.is_empty() && Path::new(t).exists() { return Some(t.into()); }
    let name = format!("{}.jsonl", str_of(v, "sessione"));
    for d in fs::read_dir(home().join(".claude/projects")).ok()?.flatten() {
        let p = d.path().join(&name);
        if p.exists() { return Some(p); }
    }
    None
}

fn tail_file(p: &Path, amount: u64) -> (Vec<String>, u64) {
    let Ok(mut fh) = fs::File::open(p) else { return (vec![], 0) };
    let n = fh.metadata().map(|m| m.len()).unwrap_or(0);
    let from = n.saturating_sub(amount);
    let _ = fh.seek(SeekFrom::Start(from));
    let mut buf = vec![];
    let _ = fh.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut lines: Vec<String> = text.lines().map(|x| x.to_string()).collect();
    if from > 0 && !lines.is_empty() { lines.remove(0); }
    (lines, n)
}

/// Text of one of your messages, to recognize it between the Claude Code queue and the transcript (ignoring spaces and system blocks).
fn normalize_queue_text(t: &str) -> String { clean_text(t).split_whitespace().collect::<Vec<_>>().join(" ") }

/// "/" commands (17/11): "/compact" from the <command-name>/compact</command-name> message Claude Code writes in the transcript.
fn command_in(c: &Value) -> Option<String> {
    let t = match c { Value::String(t) => t.clone(), Value::Array(a) => a.iter().map(|x| str_of(x, "text")).collect::<Vec<_>>().join(" "), _ => return None };
    let i = t.find("<command-name>")? + "<command-name>".len();
    let j = t[i..].find("</command-name>")?;
    Some(t[i..i + j].trim().to_lowercase())
}

/// "/" commands (17/11): line to show for the outcome of a command run by Claude Code, or None if not needed.
/// Errors always; the normal outcome only for /compact (the others, like /model sent by Juggl, would just be noise).
fn command_note(content: &str, command: &str) -> Option<String> {
    let inner = |tag: &str| -> Option<String> {
        let (a, b) = (format!("<{tag}>"), format!("</{tag}>"));
        let i = content.find(&a)? + a.len();
        let j = content[i..].find(&b).map(|j| i + j).unwrap_or(content.len());
        Some(content[i..j].trim().to_string()).filter(|x| !x.is_empty())
    };
    let (text, is_error) = match (inner("local-command-stderr"), inner("local-command-stdout")) {
        (Some(e), _) => (e, true),
        (None, Some(o)) if command == "/compact" => (o, false),
        _ => return None,
    };
    let l = text.to_lowercase();
    if l.contains("no messages to compact") || l.contains("not enough messages to compact") {
        return Some("Niente da compattare: la chat è ancora troppo corta.".into());
    }
    let name = if command.is_empty() { "il comando".to_string() } else { command.to_string() };
    Some(format!("{}{}: {}", if is_error { "Errore di " } else { "" }, name, truncate_chars(&text, 400)))
}

/// Strips the blocks the user did not write (<system-reminder>... and similar).
fn clean_text(t: &str) -> String {
    let mut s = t.to_string();
    for tag in ["system-reminder", "command-name", "command-message", "command-args", "local-command-stdout", "local-command-caveat"] {
        let (a, b) = (format!("<{tag}>"), format!("</{tag}>"));
        while let Some(i) = s.find(&a) {
            match s[i..].find(&b) { Some(j) => s.replace_range(i..i + j + b.len(), ""), None => { s.truncate(i); break; } }
        }
    }
    s.trim().to_string()
}

fn truncate_chars(t: &str, n: usize) -> String { t.chars().take(n).collect() }

const MEDIA: [&str; 11] = ["png", "jpg", "jpeg", "gif", "webp", "heic", "svg", "mp4", "mov", "m4v", "webm"];

fn is_media(p: &str) -> bool {
    p.rsplit('.').next().map(|e| MEDIA.contains(&e.to_lowercase().as_str())).unwrap_or(false) && p.contains('.')
}

/// Image/video paths that really exist, inside a text (answer, command).
fn media_paths(t: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for tok in t.split(|c: char| c.is_whitespace() || "()[]`'\"<>,;|".contains(c)) {
        let tok = tok.trim_end_matches(|c: char| c == '.' || c == ':' || c == '*');
        let p = if let Some(r) = tok.strip_prefix("~/") { home().join(r).to_string_lossy().to_string() } else { tok.to_string() };
        if p.starts_with('/') && is_media(&p) && !out.contains(&p) && !fast_open::is_network(&p) && Path::new(&p).is_file() { out.push(p); }   // fast (15/11): never checks on network drives
        if out.len() >= 6 { break; }
    }
    out
}

/// Images and videos touched by a tool: files read or written, paths in commands (conversation and strip).
fn tool_media(name: &str, inp: &Value) -> Vec<String> {
    let mut fm = vec![];
    let fp = str_of(inp, "file_path");
    if matches!(name, "Read" | "Write") && is_media(fp) && !fast_open::is_network(fp) && Path::new(fp).is_file() { fm.push(fp.to_string()); }   // fast (15/11)
    if matches!(name, "Bash" | "bash") { fm.extend(media_paths(str_of(inp, "command"))); }
    fm
}

/// Images inside transcript blocks (tool screenshots, pasted photos): data URIs if not too large.
fn block_images(x: &Value, out: &mut Vec<String>) {
    match str_of(x, "type") {
        "image" => {
            let src = x.get("source").cloned().unwrap_or(Value::Null);
            let d = str_of(&src, "data");
            if str_of(&src, "type") == "base64" && !d.is_empty() && d.len() < 3_000_000 {
                let mt = if str_of(&src, "media_type").is_empty() { "image/png" } else { str_of(&src, "media_type") };
                out.push(format!("data:{mt};base64,{d}"));
            }
        }
        "tool_result" => if let Some(Value::Array(a)) = x.get("content") { for y in a { block_images(y, out); } },
        _ => {}
    }
}

fn tool_summary(name: &str, inp: &Value) -> (String, String) {
    let g = |k: &str| inp.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    match name {
        "Bash" | "bash" => { let d = g("description"); let c = g("command"); (if d.is_empty() { truncate_chars(&c, 160) } else { d }, c) }
        "Edit" | "Write" | "Read" | "MultiEdit" => (g("file_path"), g("file_path")),
        "NotebookEdit" => { let p = if g("notebook_path").is_empty() { g("file_path") } else { g("notebook_path") }; (p.clone(), p) }
        "Agent" => (g("description"), truncate_chars(&g("prompt"), 800)),
        "Skill" => (g("skill"), String::new()),
        _ if name.starts_with("mcp__") => (name.splitn(3, "__").last().unwrap_or(name).to_string(), truncate_chars(&inp.to_string(), 500)),
        _ => (truncate_chars(&inp.to_string(), 140), String::new()),
    }
}

/// Photos of messages queued by Claude Code ("queued_command" attachment): the gallery goes right before your
/// message with the same text that arrived after sending (read within the turn). If that message went out as a real
/// message (dequeue), the photos are already inside it: the queue copy is removed. If the text is not found, it stays where it is.
fn move_queued_photos(events: &mut Vec<Value>) {
    if !events.iter().any(|e| e.get("coda_k").is_some()) { return; }
    let n = events.len();
    let mut before: Vec<Vec<usize>> = vec![vec![]; n];   // for each of your messages: the galleries to put before it
    let mut removed = vec![false; n];                          // galleries moved or removed
    let mut taken = vec![false; n];                        // messages already matched to a queue gallery
    for i in 0..n {
        let Some(k) = events[i].get("coda_k").and_then(|x| x.as_str()).map(String::from) else { continue };
        let t0 = str_of(&events[i], "t").to_string();
        let lo = i.saturating_sub(300);
        let hi = (i + 300).min(n);
        // your first message with that text that arrived after sending (the transcript line can be before or after)
        let mut best: Option<usize> = None;
        for j in lo..hi {
            if j == i || taken[j] || str_of(&events[j], "tipo") != "tu" || str_of(&events[j], "t") < t0.as_str() { continue; }
            if normalize_queue_text(str_of(&events[j], "testo")) != k { continue; }
            if best.map(|m| str_of(&events[j], "t") < str_of(&events[m], "t")).unwrap_or(true) { best = Some(j); }
        }
        let Some(j) = best else { continue };
        taken[j] = true;
        removed[i] = true;
        // already with its photos (real message with the images inside, right before): no duplicate
        let already = j > 0 && str_of(&events[j - 1], "tipo") == "media" && str_of(&events[j - 1], "da") == "tu" && events[j - 1].get("coda_k").is_none();
        if !already { before[j].push(i); }
    }
    let mut old: Vec<Option<Value>> = std::mem::take(events).into_iter().map(Some).collect();
    for i in 0..n {
        if removed[i] { continue; }
        let Some(mut e) = old[i].take() else { continue };
        for &g in &before[i] {
            let Some(mut m) = old[g].take() else { continue };
            if let Some(o) = m.as_object_mut() { o.remove("coda_k"); }
            m["t"] = e["t"].clone();
            events.push(m);
        }
        if let Some(o) = e.as_object_mut() { o.remove("coda_k"); }
        events.push(e);
    }
}

fn session_detail(v: &Value) -> Value { detail_from(find_transcript(v), false) }
pub(crate) fn session_detail_pub(v: &Value) -> Value { session_detail(v) }

fn detail_from(transcript: Option<PathBuf>, sub: bool) -> Value { detail_since(transcript, sub, "") }

fn detail_since(transcript: Option<PathBuf>, sub: bool, since: &str) -> Value { detail_since_n(transcript, sub, since, 6, 60_000_000) }

/// fast (14/11): the chat for the UI (app and bridge). First open: at most 3 MB read from the end (a ~500 MB chat
/// of screenshots read up to 60 cold: 8 s), with "piu_vecchi" if there is more before. Older messages:
/// `since` = time of the first message on screen and `before` = how many more before (120 when scrolling up). Cached per file.
const FIRST_READ: u64 = 3_000_000;
fn detail_view(v: &Value, since: &str, before: usize) -> Value {
    let p = find_transcript(v);
    let cap = if since.is_empty() { FIRST_READ } else { 60_000_000 };
    fast_open::cached_detail(p.as_deref(), since, before, || detail_since_n(p.clone(), false, since, before, cap))
}

fn detail_since_n(transcript: Option<PathBuf>, sub: bool, since: &str, before: usize, cap: u64) -> Value {
    // huge transcripts (base64 screenshots): if the tail read has few messages, read further back
    // (and, with `since`, until the first event read has reached that time and there are `before` before it)
    let mut amount = 700_000u64.min(cap);
    loop {
        let d = detail_tail_n(transcript.clone(), sub, amount, since, before);
        let ev = d["eventi"].as_array();
        let n = ev.map(|a| a.len()).unwrap_or(0);
        let first = ev.and_then(|a| a.iter().find_map(|e| e["t"].as_str().filter(|t| !t.is_empty()))).unwrap_or("");
        let size = d["peso"].as_u64().unwrap_or(0);
        let before_count = if since.is_empty() { 0 } else { ev.map(|a| a.iter().filter(|e| e["t"].as_str().map(|t| !t.is_empty() && t < since).unwrap_or(false)).count()).unwrap_or(0) };
        let reached = since.is_empty() || (!first.is_empty() && first <= since && (before <= 6 || before_count >= before));
        if (n >= 40 && reached) || amount >= size || amount >= cap { return d; }
        amount = (amount * 4).min(cap);
    }
}

fn detail_tail(transcript: Option<PathBuf>, sub: bool, amount: u64, since: &str) -> Value { detail_tail_n(transcript, sub, amount, since, 6) }

fn detail_tail_n(transcript: Option<PathBuf>, sub: bool, amount: u64, since: &str, before: usize) -> Value {
    let mut events: Vec<Value> = vec![];
    let mut files: HashMap<String, String> = HashMap::new();
    let mut size = 0;
    // commands launched and not yet finished (for "what it is doing now") and Claude's step list
    let mut open_tools: Vec<(String, Value)> = vec![];
    let mut steps: Vec<Value> = vec![];
    let mut turn_files: Vec<Value> = vec![];
    // tokens in the context of the last call (input + cache + output): how much the conversation "weighs" now
    let mut token = 0u64;
    // your messages in the Claude Code queue (enqueue) not yet read: normalized text, position in events
    let mut queued: Vec<(String, usize)> = vec![];
    // the Claude Code queue as it is now (enqueue, dequeue, remove): None = message with no text in the transcript
    // (sessions inside the app, stream-json). The remaining ones already reached Claude and wait for the next step.
    let mut queue: Vec<Option<String>> = vec![];
    let mut last_t = String::new();
    let mut truncated = false;
    let mut edits = live_edits::Collector::new();   // live edits (12/11)
    let mut last_command = String::new();   // "/" commands (17/11): the last command run by Claude Code
    if let Some(p) = transcript {
        let (lines, n) = tail_file(&p, amount);
        size = n;
        truncated = n > amount;   // fast (14/11): there is more before the piece read
        for r in lines {
            let Ok(d) = serde_json::from_str::<Value>(&r) else { continue };
            if (!sub && d.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false)) || d.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
            let when = str_of(&d, "timestamp").to_string();
            edits.see(&d);   // live edits (12/11)
            // last real step of Claude (not queue operations): "Ragiona" counts from there when there is no open action
            if matches!(str_of(&d, "type"), "assistant" | "user") && when > last_t { last_t = when.clone(); }
            let c = d.get("message").and_then(|m| m.get("content")).cloned().unwrap_or(Value::Null);
            match str_of(&d, "type") {
                // message written while the session was working: Claude reads it within the turn ("absorbed_mid_turn")
                // and in the transcript it only shows up as a queue operation, not as a message. It is still one of your messages.
                // "enqueue" = Claude Code has already received it (it is in its queue): for Juggl it is delivered right away, even if
                // it will only read it at the end of a long command. Then it becomes "remove absorbed_mid_turn" (read in the turn) or
                // "dequeue" + user message (read at the start of the next turn): a single bubble remains, in the right place.
                "queue-operation" => {
                    let t = str_of(&d, "content").trim().to_string();
                    let k = normalize_queue_text(&t);
                    match str_of(&d, "operation") {
                        "enqueue" => queue.push(if t.is_empty() { None } else { Some(k.clone()) }),
                        "dequeue" => if !queue.is_empty() { queue.remove(0); },
                        "remove" => if let Some(p) = queue.iter().position(|x| x.as_deref() == Some(k.as_str())).or_else(|| queue.iter().position(|x| x.is_none())) { queue.remove(p); },
                        "popAll" => queue.clear(),
                        _ => {}
                    }
                    match str_of(&d, "operation") {
                        "enqueue" if !t.is_empty() && !t.starts_with('<') => {
                            events.push(json!({"tipo": "tu", "testo": truncate_chars(&t, 4000), "t": when, "durante": true}));
                            queued.push((k, events.len() - 1));
                        }
                        "remove" if !t.is_empty() && !t.starts_with('<') => {
                            if let Some(p) = queued.iter().position(|x| x.0 == k) { queued.remove(p); }
                            else if str_of(&d, "reason") == "absorbed_mid_turn" { events.push(json!({"tipo": "tu", "testo": truncate_chars(&t, 4000), "t": when, "durante": true})); }
                        }
                        // taken back into the input box to edit them (up arrow): they were not sent
                        "popAll" => for (_, i) in queued.drain(..) { events[i]["tolto"] = json!(true); },
                        _ => {}
                    }
                }
                // compaction (16/11): a short notice instead of the raw summary ("This session is being continued...")
                // "/" commands run by Claude Code (17/11): the outcome is only here (<local-command-stderr|stdout>) and before it was not
                // visible: a rejected /compact ("Not enough messages to compact.") left the chat silent.
                // Errors of every command and the outcome of /compact are shown; the rest (e.g. /model sent by Juggl) is not.
                "system" if str_of(&d, "subtype") == "local_command" => {
                    if let Some(n) = command_note(str_of(&d, "content"), &last_command) { events.push(json!({"tipo": "nota_comando", "testo": n, "t": when})); }
                }
                "system" if str_of(&d, "subtype") == "compact_boundary" => {
                    let sid = p.file_stem().and_then(|x| x.to_str()).unwrap_or("");
                    let (text, by_juggl) = compaction::compacted_notice(sid, &d);
                    events.push(json!({"tipo": "compattata", "testo": text, "da_juggl": by_juggl, "t": when}));
                }
                "user" if d.get("isCompactSummary").and_then(|x| x.as_bool()).unwrap_or(false) => {}
                "user" => {
                    if let Some(c) = command_in(&c) { last_command = c; }
                    let mut texts = vec![];
                    match &c {
                        Value::String(t) => texts.push(t.clone()),
                        Value::Array(a) => for x in a {
                            let mut im = vec![];
                            block_images(x, &mut im);
                            if !im.is_empty() {
                                let who = if str_of(x, "type") == "image" { "tu" } else { "strumento" };
                                events.push(json!({"tipo": "media", "da": who, "dati": im, "file": [], "t": when}));
                            }
                            if str_of(x, "type") == "tool_result" { let tid = str_of(x, "tool_use_id"); open_tools.retain(|(i, _)| i != tid); }
                            match str_of(x, "type") {
                                "text" => texts.push(str_of(x, "text").to_string()),
                                "tool_result" if x.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) => {
                                    let t = match x.get("content") { Some(Value::String(t)) => t.clone(), Some(Value::Array(a)) => a.iter().map(|y| str_of(y, "text")).collect::<Vec<_>>().join(" "), _ => String::new() };
                                    events.push(json!({"tipo": "errore", "nome": "errore", "testo": truncate_chars(&t, 600), "t": when}));
                                }
                                _ => {}
                            }
                        },
                        _ => {}
                    }
                    for t in texts {
                        let t = clean_text(&t);
                        if t.is_empty() || t.starts_with("[Request interrupted") { continue; }
                        if t.starts_with("<task-notification") {
                            let sm = t.split("<summary>").nth(1).and_then(|x| x.split("</summary>").next()).unwrap_or(&t);
                            events.push(json!({"tipo": "strumento", "nome": "avviso", "testo": truncate_chars(sm, 300), "extra": truncate_chars(&t, 2000), "t": when}));
                        } else {
                            // it was in the Claude Code queue (enqueue) and goes out now: the bubble moves here, it is not duplicated
                            let k = normalize_queue_text(&t);
                            if let Some(p) = queued.iter().position(|x| x.0 == k) { let (_, i) = queued.remove(p); events[i]["tolto"] = json!(true); }
                            if let Some(m) = edits.close(&when) { events.push(m); }   // live edits (12/11): end of the previous answer
                            events.push(json!({"tipo": "tu", "testo": truncate_chars(&t, 4000), "t": when}));
                            turn_files.clear();   // new turn
                        }
                    }
                }
                // message with photos sent while the session was working: Claude Code queues it and the photos are
                // only here (the "remove absorbed_mid_turn" only carries the text). Without this the photo disappeared from the chat
                // (09/11). It becomes one of your galleries, then moved before the right message (move_queued_photos).
                "attachment" => {
                    let a = d.get("attachment").cloned().unwrap_or(Value::Null);
                    if str_of(&a, "type") == "queued_command" {
                        if let Some(Value::Array(p)) = a.get("prompt") {
                            let mut im = vec![];
                            for x in p { if str_of(x, "type") == "image" { block_images(x, &mut im); } }
                            if !im.is_empty() {
                                let t = p.iter().filter(|x| str_of(x, "type") == "text").map(|x| str_of(x, "text")).collect::<Vec<_>>().join("\n");
                                events.push(json!({"tipo": "media", "da": "tu", "dati": im, "file": [], "t": when, "coda_k": normalize_queue_text(&t)}));
                            }
                        }
                    }
                }
                "assistant" => if let Value::Array(a) = &c {
                    if let Some(u) = d.get("message").and_then(|m| m.get("usage")) {
                        let k = |n: &str| u.get(n).and_then(|x| x.as_u64()).unwrap_or(0);
                        let tot = k("input_tokens") + k("cache_creation_input_tokens") + k("cache_read_input_tokens") + k("output_tokens");
                        if tot > 0 { token = tot; }
                    }
                    for x in a {
                        match str_of(x, "type") {
                            "text" if !str_of(x, "text").trim().is_empty() => {
                                let t = str_of(x, "text");
                                events.push(json!({"tipo": "agente", "testo": truncate_chars(t, 8000), "t": when, "file": media_paths(t)}));
                            }
                            "tool_use" => {
                                let name = str_of(x, "name");
                                let inp = x.get("input").cloned().unwrap_or(json!({}));
                                let (short_text, long_text) = tool_summary(name, &inp);
                                let fp = str_of(&inp, "file_path").to_string();
                                if !fp.is_empty() {
                                    let action = match name { "Write" => "creato", "Edit" | "NotebookEdit" => "modificato", _ => "letto" };
                                    let rank = |a: &str| match a { "creato" => 3, "modificato" => 2, _ => 1 };
                                    if let Some(x) = turn_files.iter_mut().find(|x| str_of(x, "path") == fp) {
                                        if rank(action) > rank(str_of(x, "azione")) { x["azione"] = json!(action); }
                                        x["t"] = json!(when.clone());
                                    } else { turn_files.push(json!({"path": fp, "azione": action, "t": when.clone()})); }
                                }
                                open_tools.push((str_of(x, "id").to_string(), json!({"nome": name, "testo": short_text, "t": when, "input": if name == "AskUserQuestion" { inp.clone() } else { Value::Null }})));
                                // step list: TodoWrite (whole list) or TaskCreate / TaskUpdate (one item at a time)
                                match name {
                                    "TodoWrite" => if let Some(Value::Array(t)) = inp.get("todos") {
                                        steps = t.iter().map(|x| json!({"testo": str_of(x, "content"), "stato": str_of(x, "status"), "ora": str_of(x, "activeForm")})).collect();
                                    },
                                    "TaskCreate" => steps.push(json!({"id": (steps.len() + 1).to_string(), "testo": str_of(&inp, "subject"), "stato": "pending", "ora": str_of(&inp, "activeForm")})),
                                    "TaskUpdate" => {
                                        let tid = inp.get("taskId").map(|x| x.as_str().map(String::from).unwrap_or(x.to_string())).unwrap_or_default();
                                        if let Some(p) = steps.iter_mut().find(|p| str_of(p, "id") == tid) { if !str_of(&inp, "status").is_empty() { p["stato"] = json!(str_of(&inp, "status")); } }
                                    }
                                    _ => {}
                                }
                                if matches!(name, "Edit" | "Write" | "NotebookEdit") {
                                    let fp = str_of(&inp, "file_path");
                                    if !fp.is_empty() { files.insert(fp.to_string(), when.clone()); }
                                }
                                events.push(json!({"tipo": "strumento", "nome": name, "testo": short_text, "extra": long_text, "t": when, "id": str_of(x, "id")}));
                                // images it is working on: files read, shown, created or passed to commands
                                let fm = tool_media(name, &inp);
                                if !fm.is_empty() { events.push(json!({"tipo": "media", "da": "strumento", "dati": [], "file": fm, "t": when})); }
                            }
                            _ => {}
                        }
                    }
                },
                _ => {}
            }
        }
    }
    if let Some(m) = edits.close("") { events.push(m); }   // live edits (12/11): answer in progress or the last one
    events.retain(|e| e.get("tolto").is_none());
    move_queued_photos(&mut events);
    let n = events.len();
    // usually the last 120 events; with `since` from some events before that time (at most 800)
    let mut from = n.saturating_sub(120);
    if !since.is_empty() {
        let i = events.iter().position(|e| str_of(e, "t") >= since).unwrap_or(n);
        // fast (14/11): `before` (6, or 120 when scrolling up); the window reaches at most 1500 messages
        from = from.min(i.saturating_sub(before)).max(n.saturating_sub(800.max((n - i.min(n)) + before).min(1500)));
    }
    let older = from > 0 || truncated;
    let mut events: Vec<Value> = events.into_iter().skip(from).collect();
    // files created, modified or mentioned in answers: cards under the answer (documents.rs)
    if !sub { documents::enrich(&mut events); }
    // embedded images are heavy: keep only the last 10 from tools. Your photos have a separate count (40):
    // before they were counted together with screenshots and after about ten screenshots your photo disappeared from the chat (09/11)
    let (mut kept, mut kept_yours) = (0, 0);
    for e in events.iter_mut().rev() {
        let yours = str_of(e, "tipo") == "media" && str_of(e, "da") == "tu";
        if let Some(Value::Array(d)) = e.get_mut("dati") {
            let (n, max) = if yours { (&mut kept_yours, 40) } else { (&mut kept, 10) };
            if *n >= max { d.clear(); } else { *n += d.len(); }
        }
    }
    events.retain(|e| str_of(e, "tipo") != "media" || e.get("dati").and_then(|d| d.as_array()).map(|d| !d.is_empty()).unwrap_or(false)
        || e.get("file").and_then(|d| d.as_array()).map(|d| !d.is_empty()).unwrap_or(false));
    let mut fl: Vec<(String, String)> = files.into_iter().collect();
    fl.sort_by(|a, b| b.1.cmp(&a.1));
    let n_files = fl.len();
    let current = open_tools.iter().rev().find(|(_, v)| str_of(v, "nome") != "AskUserQuestion").map(|(_, v)| json!({"nome": str_of(v, "nome"), "testo": str_of(v, "testo"), "t": str_of(v, "t")})).unwrap_or(Value::Null);
    let question = open_tools.iter().rev().find(|(_, v)| str_of(v, "nome") == "AskUserQuestion").map(|(_, v)| v["input"].clone()).unwrap_or(Value::Null);
    let steps: Vec<Value> = steps.into_iter().filter(|p| str_of(p, "stato") != "deleted").collect();
    let n = turn_files.len();
    let turn_files: Vec<Value> = turn_files.into_iter().skip(n.saturating_sub(12)).collect();
    json!({"eventi": events, "file": fl.into_iter().take(20).map(|x| x.0).collect::<Vec<_>>(), "peso": size, "adesso": current, "passi": steps, "domanda": question, "file_turno": turn_files, "n_file": n_files, "token": token, "ricevuti_in_coda": queue.iter().filter(|x| x.is_none()).count(), "ultimo_t": last_t, "piu_vecchi": older})
}

// ---------- subagents: <transcript>/subagents/agent-<id>.jsonl + .meta.json ----------

fn subagents_dir(v: &Value) -> Option<PathBuf> {
    let t = find_transcript(v)?;
    let d = t.with_extension("").join("subagents");
    if d.is_dir() { Some(d) } else { None }
}

/// Outcome of subagents from the <task-notification> of the main transcript: agent id (= task-id) ->
/// (status, when). Claude writes "finished" in .meta.json only when the agent ends by itself: stopped with
/// TaskStop ("killed") or crashed ("failed") it stayed "working" for 10 minutes (10/11). Read in pieces: only
/// the new part of the transcript is read again.
fn task_outcomes(t: &Path) -> HashMap<String, (String, f64)> {
    // fast (14/11): after a restart the first read loaded the WHOLE transcript into memory (482 MB in one chat:
    // 33 s and ~1 GB), holding the shared lock: the other chats with helpers waited. Now: the first time only the
    // last 8 MB (older helpers with no outcome stay "interrotto" after 10 minutes, as before), then only the
    // new part, in 4 MB chunks, and the lock is held only to take and put back the state.
    const FIRST_READ_BYTES: u64 = 8 * 1024 * 1024;
    const CHUNK: u64 = 4 * 1024 * 1024;
    static CACHE: std::sync::Mutex<Option<HashMap<PathBuf, (u64, HashMap<String, (String, f64)>)>>> = std::sync::Mutex::new(None);
    let size = fs::metadata(t).map(|m| m.len()).unwrap_or(0);
    let (mut from, mut outcomes) = {
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let m = c.get_or_insert_with(HashMap::new);
        if m.len() > 64 { m.clear(); }
        m.get(t).cloned().unwrap_or((0, HashMap::new()))
    };
    if size < from { from = 0; outcomes.clear(); }   // transcript rewritten or truncated
    if size == from { return outcomes; }
    let mut skip_partial = false;
    if from == 0 && size > FIRST_READ_BYTES { from = size - FIRST_READ_BYTES; skip_partial = true; }
    if let Ok(mut fh) = fs::File::open(t) {
        while from < size {
            let mut b = vec![];
            if fh.seek(SeekFrom::Start(from)).is_err() || (&mut fh).take(CHUNK.min(size - from)).read_to_end(&mut b).is_err() || b.is_empty() { break; }
            let mut start = 0;
            if skip_partial {   // started in the middle of a line: skip it
                match b.iter().position(|x| *x == b'\n') { Some(k) => start = k + 1, None => { from += b.len() as u64; continue; } }
                skip_partial = false;
            }
            // whole lines only: the last half line is read again next round (or with the next chunk)
            let Some(end) = b.iter().rposition(|x| *x == b'\n').map(|k| k + 1).filter(|&f| f > start) else {
                if b.len() as u64 >= CHUNK { from += b.len() as u64; skip_partial = true; continue; }   // line longer than a chunk
                break;
            };
            for r in String::from_utf8_lossy(&b[start..end]).lines() {
                if !r.contains("<task-notification") { continue; }
                let when = r.find("\"timestamp\":\"").map(|i| &r[i + 13..]).and_then(|x| x.split('"').next()).map(requests::from_iso).unwrap_or(0.0);
                for n in r.split("<task-notification>").skip(1) {
                    let n = n.split("</task-notification>").next().unwrap_or(n);
                    let between = |a: &str, b: &str| n.split(a).nth(1).and_then(|x| x.split(b).next()).map(|x| x.trim().to_string());
                    if let (Some(id), Some(st)) = (between("<task-id>", "</task-id>"), between("<status>", "</status>")) {
                        // lines without a date (api-request-blob repeats the notice): keep the time already seen
                        let q = if when > 0.0 { when } else { outcomes.get(&id).map(|p| p.1).unwrap_or(0.0) };
                        if !id.is_empty() { outcomes.insert(id, (st, q)); }
                    }
                }
            }
            from += end as u64;
            if (end as u64) < b.len() as u64 && (b.len() as u64) < CHUNK { break; }   // a half line remains at the end
        }
    }
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    c.get_or_insert_with(HashMap::new).insert(t.to_path_buf(), (from, outcomes.clone()));
    outcomes
}

fn list_subagents(v: &Value) -> Vec<Value> {
    let Some(dir) = subagents_dir(v) else { return vec![] };
    let outcomes = find_transcript(v).map(|t| task_outcomes(&t)).unwrap_or_default();
    let mut out = vec![];
    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
        if !name.ends_with(".meta.json") { continue; }
        let id = name.trim_start_matches("agent-").trim_end_matches(".meta.json").to_string();
        let m = load_json(&p).unwrap_or(json!({}));
        let jl = dir.join(format!("agent-{id}.jsonl"));
        let (start, end) = fs::metadata(&jl).map(|md| (
            md.created().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0),
            md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0))).unwrap_or((0.0, 0.0));
        let finished = m.get("finished").and_then(|x| x.as_bool()).unwrap_or(false);
        // end notice reached the session and the agent has not written since (with SendMessage it can restart)
        let outcome = outcomes.get(&id).filter(|(_, q)| *q > 0.0 && end <= q + 5.0).map(|(st, _)| st.as_str());
        // not finished but idle for 10 minutes: interrupted (the session was closed or stopped)
        let state = if finished { "finito" } else { match outcome {
            Some("completed") => "finito",
            Some("killed") | Some("failed") | Some("stopped") => "interrotto",
            _ => if unix_now() - end > 600.0 { "interrotto" } else { "lavora" },
        } };
        let mut x = json!({"id": id, "tipo": str_of(&m, "agentType"), "descrizione": str_of(&m, "description"), "tool": str_of(&m, "toolUseId"),
            "stato": state, "inizio": start, "fine": end, "sfondo": str_of(&m, "requestShape") == "background"});
        // why it ended: "fermato" (TaskStop), "fallito" (error), "finito"
        if let Some(e) = outcome { x["esito"] = json!(match e { "killed" | "stopped" => "fermato", "failed" => "fallito", _ => "finito" }); }
        if state == "lavora" { if let (Value::Object(o), Value::Object(r)) = (&mut x, subagent_summary(&jl)) { o.extend(r); } }
        out.push(x);
    }
    out.sort_by(|a, b| f64_of(b, "inizio").partial_cmp(&f64_of(a, "inizio")).unwrap());
    out
}

/// Summary of a working subagent, from the tail of its transcript (at most 400 KB):
/// what it is doing now (open command, otherwise the last action or sentence), steps done/total, files changed, tokens.
/// Kept in memory until the file grows: the list asks for it often.
fn subagent_summary(jl: &Path) -> Value {
    static CACHE: std::sync::Mutex<Option<HashMap<PathBuf, (u64, Value)>>> = std::sync::Mutex::new(None);
    let size = fs::metadata(jl).map(|m| m.len()).unwrap_or(0);
    if let Some((p, v)) = CACHE.lock().ok().and_then(|c| c.as_ref().and_then(|m| m.get(jl).cloned())) { if p == size { return v; } }
    let d = detail_tail(Some(jl.to_path_buf()), true, 400_000, "");
    let ev = d["eventi"].as_array().cloned().unwrap_or_default();
    let current = if !d["adesso"].is_null() { d["adesso"].clone() } else {
        ev.iter().rev().find(|e| matches!(str_of(e, "tipo"), "agente" | "strumento") && str_of(e, "nome") != "avviso").map(|e| {
            if str_of(e, "tipo") == "agente" {
                // the last sentence written, on one line
                let t = str_of(e, "testo").lines().map(str::trim).filter(|r| !r.is_empty()).last().unwrap_or("").trim_start_matches(|c: char| c == '#' || c == '-' || c == '*' || c == ' ');
                json!({"nome": "", "testo": truncate_chars(t, 200), "t": str_of(e, "t")})
            } else { json!({"nome": str_of(e, "nome"), "testo": truncate_chars(str_of(e, "testo"), 200), "t": str_of(e, "t"), "finito": true}) }
        }).unwrap_or(Value::Null)
    };
    let steps = d["passi"].as_array().cloned().unwrap_or_default();
    let steps = if steps.is_empty() { Value::Null } else {
        let current_step = steps.iter().find(|p| str_of(p, "stato") == "in_progress");
        json!({"fatti": steps.iter().filter(|p| str_of(p, "stato") == "completed").count(), "totale": steps.len(),
            "in_corso": current_step.is_some(), "ora": current_step.map(|p| if str_of(p, "ora").is_empty() { str_of(p, "testo") } else { str_of(p, "ora") }).unwrap_or("")})
    };
    let v = json!({"ora": current, "passi": steps, "file": d["n_file"].as_u64().unwrap_or(0), "token": d["token"].as_u64().unwrap_or(0)});
    if let Ok(mut c) = CACHE.lock() {
        let m = c.get_or_insert_with(HashMap::new);
        if m.len() > 64 { m.clear(); }
        m.insert(jl.to_path_buf(), (size, v.clone()));
    }
    v
}

/// "Ferma" button on a working subagent (Mac and iPhone). Hosted: stop_task straight to Claude Code, no turns
/// spent. In the terminal (cmux, rare): internal message to the session that stops it with TaskStop.
#[tauri::command(async)]
fn ferma_sottoagente(id: String, agente: String) -> Value {
    if !storage::is_valid_id(&id) || agente.is_empty() || !agente.chars().all(|c| c.is_ascii_alphanumeric()) { return json!({"ok": false, "msg": "id non valido"}) }
    let Some(v) = find_session(&id) else { return json!({"ok": false, "msg": "sessione non trovata"}) };
    let Some(sa) = list_subagents(&v).into_iter().find(|x| str_of(x, "id") == agente) else { return json!({"ok": false, "msg": "sotto-agente non trovato"}) };
    if str_of(&sa, "stato") != "lavora" { return json!({"ok": true, "msg": "Era gia' fermo"}) }
    let name = str_of(&sa, "descrizione").to_string();
    let r = if hosted::is_hosted(&id) {
        match hosted::stop_task(&id, &agente) {
            Ok(()) => { log_juggl(&format!("{id}: sotto-agente {agente} ({name}) fermato dal tasto Ferma")); json!({"ok": true, "msg": "Fermato"}) }
            Err(e) => json!({"ok": false, "msg": e}),
        }
    } else {
        let r = send_internal(id.clone(), format!("Ferma subito con TaskStop il sotto-agente \"{name}\" (id {agente}): l'utente l'ha chiesto dal tasto Ferma di Juggl. Non rilanciarlo."));
        if r["ok"].as_bool().unwrap_or(false) { json!({"ok": true, "msg": "Chiesto alla sessione"}) } else { r }
    };
    if let Some(app) = APP.get() { let _ = app.emit("cambiato", ()); }
    r
}

#[tauri::command(async)]
fn sottoagente(id: String, agente: String) -> Value {
    let Some(v) = find_session(&id) else { return Value::Null };
    let Some(dir) = subagents_dir(&v) else { return Value::Null };
    let mut info = list_subagents(&v).into_iter().find(|x| str_of(x, "id") == agente).unwrap_or(json!({}));
    // finished or interrupted: the summary (files, tokens, last action) is needed in the panel too
    if info.get("ora").is_none() && info.get("id").is_some() {
        if let (Value::Object(o), Value::Object(r)) = (&mut info, subagent_summary(&dir.join(format!("agent-{agente}.jsonl")))) { o.extend(r); }
    }
    let mut d = detail_from(Some(dir.join(format!("agent-{agente}.jsonl"))), true);
    d["info"] = info;
    bridge::lighten_for_mac(&mut d);
    d
}

fn git(cwd: &str) -> Value {
    // on every chat update: git status can cost hundreds of ms on big folders, keep it for 10 s
    static G: std::sync::OnceLock<std::sync::Mutex<HashMap<String, (f64, Value)>>> = std::sync::OnceLock::new();
    let g = G.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    // fast (14/11): never git status in the middle of opening a chat (110-230 ms on an external disk). Cached value
    // right away; if it is old a separate thread redoes it. The first time wait at most 40 ms, then it arrives next round.
    static IN_PROGRESS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    let c = g.lock().unwrap_or_else(|e| e.into_inner()).get(cwd).cloned();
    if let Some((t, v)) = &c { if unix_now() - t < 10.0 { return v.clone(); } }
    let ic = IN_PROGRESS.get_or_init(|| std::sync::Mutex::new(Default::default()));
    let (tx, rx) = std::sync::mpsc::channel();
    if ic.lock().unwrap_or_else(|e| e.into_inner()).insert(cwd.to_string()) {
        let cw = cwd.to_string();
        std::thread::spawn(move || {
            let v = git_now(&cw);
            g.lock().unwrap_or_else(|e| e.into_inner()).insert(cw.clone(), (unix_now(), v.clone()));
            IN_PROGRESS.get().map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).remove(&cw));
            let _ = tx.send(v);
        });
    }
    match c { Some((_, v)) => v, None => rx.recv_timeout(std::time::Duration::from_millis(40)).unwrap_or(Value::Null) }
}

fn git_now(cwd: &str) -> Value {
    if cwd.is_empty() || !Path::new(cwd).is_dir() { return Value::Null; }
    let Ok(o) = crate::new_command("git").no_console_window().args(["-C", cwd, "status", "--porcelain=v1", "--branch"]).output() else { return Value::Null };
    if !o.status.success() { return Value::Null; }
    let t = String::from_utf8_lossy(&o.stdout);
    let mut lines = t.lines();
    let branch = lines.next().unwrap_or("").trim_start_matches("## ").split("...").next().unwrap_or("").to_string();
    json!({"ramo": branch, "modifiche": lines.count()})
}

/// Last lines Claude is writing in the terminal, without the graphics (borders, input box, tips).
fn clean_screen(sf: &str) -> String {
    let Some(o) = cmux(&["read-screen", "--surface", sf, "--lines", "30"]) else { return String::new() };
    let t = String::from_utf8_lossy(&o.stdout).to_string();
    let lines: Vec<String> = t.lines().map(|r| r.trim_end().to_string()).collect();
    let end = lines.iter().rposition(|r| r.trim_start().starts_with('╭') || r.trim_start().starts_with('❯') || r.contains("────")).unwrap_or(lines.len());
    let useful: Vec<&String> = lines[..end].iter().filter(|r| {
        let x = r.trim();
        !x.is_empty() && !x.chars().all(|c| "─│╭╮╰╯ ".contains(c)) && !x.contains("esc to interrupt")
            && !x.contains("tokens)") && !x.contains("Tip:") && !x.contains("ctrl+b") && !x.contains("timeout ") && !x.contains("ctrl+o")
            && !x.starts_with('·') && !x.starts_with('✻') && !x.starts_with('✶') && !x.starts_with('✳') && !x.starts_with('✢') && !x.starts_with('*')
    }).collect();
    useful.iter().rev().take(8).rev().map(|r| r.trim_start_matches(|c: char| c == '⏺' || c == '●' || c == ' ').to_string()).collect::<Vec<_>>().join("\n")
}

/// Multiple-choice question or permission open in the terminal: text and numbered options read from the screen.
fn terminal_question(sf: &str) -> Value {
    let Some(o) = cmux(&["read-screen", "--surface", sf, "--lines", "40"]) else { return Value::Null };
    let sc = String::from_utf8_lossy(&o.stdout).to_string();
    let lines: Vec<&str> = sc.lines().collect();
    // a real Claude Code menu: at the bottom of the screen, consecutive options from 1, one with the ❯ cursor,
    // and the menu instructions (Esc / Enter / arrows). A numbered list written in the text has none of this.
    let n = lines.len();
    let bottom = &lines[n.saturating_sub(22)..];
    let has_instructions = bottom.iter().any(|r| { let l = r.to_lowercase(); l.contains("esc to") || l.contains("enter to") || l.contains("to navigate") || l.contains("to select") || l.contains("do you want") || l.contains("would you like") });
    if !has_instructions { return Value::Null; }
    let offset = n.saturating_sub(22);
    let mut options: Vec<Value> = vec![];
    let mut first_option = None;
    let mut expected = 1;
    let mut has_cursor = false;
    for (k, r) in bottom.iter().enumerate() {
        let with_cursor = r.contains('❯');
        let t = r.trim_start_matches(|c: char| c == '│' || c == ' ' || c == '❯' || c == '>').trim();
        let mut it = t.splitn(2, ". ");
        let (Some(num), Some(text)) = (it.next(), it.next()) else { continue };
        let Ok(v) = num.parse::<usize>() else { continue };
        if v != expected { if v == 1 { options.clear(); expected = 1; has_cursor = false; first_option = None; } else { continue; } }
        if first_option.is_none() { first_option = Some(offset + k); }
        has_cursor |= with_cursor;
        options.push(json!({"n": num, "testo": text.trim_end_matches('│').trim(), "scelta": with_cursor}));
        expected += 1;
    }
    if options.len() < 2 || !has_cursor { return Value::Null; }
    let p = first_option.unwrap_or(0);
    let question: Vec<String> = lines[p.saturating_sub(6)..p].iter()
        .map(|r| r.trim_matches(|c: char| c == '│' || c == '╭' || c == '╰' || c == '─' || c == ' ').to_string())
        .filter(|r| !r.is_empty() && !r.chars().all(|c| c == '─' || c == '╌')).collect();
    json!({"testo": question.join(" "), "opzioni": options})
}

/// Answers a terminal question: option number, or free text.
#[tauri::command(async)]
fn rispondi_scelta(id: String, n: String, testo: Option<String>) -> Value {
    // chat with the PC agent: the question is an OpenCode permission (src/pc_agent.rs)
    if let Some(l) = id.strip_prefix("locale:") { if !storage::is_valid_id(&id) { return json!({"ok": false, "msg": "id non valido"}) } return local_models::answer_choice(l, &n, testo.as_deref()); }
    // permissions (12/11): permission request of a hosted chat: 1 Allow, 2 Always allow in this chat, 3 Deny
    if storage::is_valid_id(&id) { if let Some(r) = permissions::answer_choice(&id, &n, testo.as_deref()) { return r; } }
    let Some(v) = find_session(&id) else { return json!({"ok": false, "msg": "sessione non trovata"}) };
    let sf = str_of(&v, "surface").to_string();
    if sf.is_empty() { return json!({"ok": false, "msg": "sessione non raggiungibile"}) }
    cmux(&["send-key", "--surface", &sf, &n]);
    std::thread::sleep(Duration::from_millis(350));
    if let Some(t) = testo.filter(|t| !t.trim().is_empty()) {
        cmux(&["send", "--surface", &sf, t.trim()]);
        std::thread::sleep(Duration::from_millis(200));
        cmux(&["send-key", "--surface", &sf, "Enter"]);
    } else if !terminal_question(&sf).is_null() {
        cmux(&["send-key", "--surface", &sf, "Enter"]);   // some menus also want Enter
    }
    json!({"ok": true})
}

/// Current action, lightweight: reads only the tail of the transcript (for the "In corso" strip of all projects).
fn quick_current(v: &Value) -> Value {
    let Some(p) = find_transcript(v) else { return Value::Null };
    let (lines, _) = tail_file(&p, 120_000);
    let mut open_tools: Vec<(String, Value)> = vec![];
    let mut files: Vec<String> = vec![];
    for r in lines {
        let Ok(d) = serde_json::from_str::<Value>(&r) else { continue };
        if d.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
        let Some(Value::Array(c)) = d.get("message").and_then(|m| m.get("content")) else { continue };
        for x in c {
            match str_of(x, "type") {
                "tool_use" => {
                    let inp = x.get("input").cloned().unwrap_or(json!({}));
                    let (short_text, _) = tool_summary(str_of(x, "name"), &inp);
                    let fp = str_of(&inp, "file_path"); if !fp.is_empty() { files.retain(|f| f != fp); files.push(fp.to_string()); }
                    open_tools.push((str_of(x, "id").to_string(), json!({"nome": str_of(x, "name"), "testo": short_text, "t": str_of(&d, "timestamp")})));
                }
                "tool_result" => { let t = str_of(x, "tool_use_id"); open_tools.retain(|(i, _)| i != t); }
                _ => {}
            }
        }
    }
    let n = files.len();
    json!({"azione": open_tools.last().map(|x| x.1.clone()).unwrap_or(Value::Null), "file": files.into_iter().skip(n.saturating_sub(3)).collect::<Vec<_>>()})
}

fn find_session(id: &str) -> Option<Value> {
    // fast: only this session's file; if the file name does not match, all of them as before
    let single = if storage::is_valid_id(id) { list_sessions_of(Some(id)).into_iter().next() } else { None };
    let v = single.or_else(|| list_sessions_with_pid().into_iter().find(|v| str_of(v, "sessione") == id))?;
    let mut v = v; if let Some(o) = v.as_object_mut() { o.remove("pid"); }
    Some(v)
}

/// Main turn state: like "vista", except when only subagents are working (then the turn is over).
pub(crate) fn turn_view(v: &Value) -> &str { let t = str_of(v, "vista_turno"); if t.is_empty() { str_of(v, "vista") } else { t } }

// ---------- commands for the UI ----------

fn desk_data() -> Value {
    let mut ss = list_sessions();
    ss.extend(local_models::as_sessions());
    // the ready ones (Import projects) at the bottom, all together: you have not used them yet
    let rank = |v: &Value| match str_of(v, "vista") { "bloccata" => 0, "aspetta" => 1, "fatta" => 2, "lavora" => 3,
        "ibernata" if v.get("pronta").and_then(|x| x.as_bool()).unwrap_or(false) => 6, "ibernata" => 5, _ => 4 };
    ss.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| if rank(a) <= 1 { f64_of(a, "da").partial_cmp(&f64_of(b, "da")).unwrap() } else { f64_of(b, "da").partial_cmp(&f64_of(a, "da")).unwrap() }));
    // new answers (done and not yet seen) and requests waiting for the user: indicator in the row
    let rq = requests::counts();
    for v in ss.iter_mut() { if let Some((new_count, for_you)) = rq.get(str_of(v, "sessione")).copied() { v["rq_nuove"] = json!(new_count); v["rq_te"] = json!(for_you); } }
    in_progress::enrich(&mut ss);   // "In corso" view: "Adesso" row and steps of whoever is working (src/in_progress.rs)
    // limits of the connected account (quota.rs): Null if not fresh or from another account = "not available"
    let limits = quota::valid_limits();
    let eco = eco::status();
    let ok_n = approvals::n_ok(&ss);
    // automatic coordinator (12/11): summary row per group, also in the bridge for the iPhone (/api/tavolo)
    let coordinators = coordinator::for_list(&ss);
    json!({"juggl": juggl_id(), "ok_n": ok_n, "sessioni": ss, "archiviate": archived_sessions(), "al_mac": user_at_mac(), "ora": unix_now(), "home": home(), "mac": session_care::mac_memory(), "disco_libero_gb": session_care::free_disk_gb(), "imp": session_care::settings(), "limiti": limits, "funzioni": profile::enabled_map(), "nome": profile::user_name(), "eco": eco, "regie": coordinators})
}

#[tauri::command(async)]
async fn media_recenti(quanti: Option<usize>) -> Value {
    // off the main thread: reading transcripts and making thumbnails must not stall the app's scrolling
    let q = quanti.unwrap_or(40);
    tauri::async_runtime::spawn_blocking(move || media_strip::recent_media_n(q)).await.unwrap_or_else(|_| json!([]))
}

#[tauri::command(async)]
fn tavolo(window: tauri::WebviewWindow) -> Value {
    let d = desk_data();
    // dock: the oks waiting for the user (Requests view) and the sessions stuck on a permission, which are oks too
    let n = d["sessioni"].as_array().map(|a| a.iter().filter(|v| str_of(v, "vista") == "bloccata").count()).unwrap_or(0) + d["ok_n"].as_u64().unwrap_or(0) as usize;
    let _ = window.set_badge_count(if n > 0 { Some(n as i64) } else { None });
    d
}

#[tauri::command(async)]
fn ponte_info() -> Value {

    // Tailscale from the command line or from the Mac App Store app
    let ts = ["/usr/local/bin/tailscale", "/opt/homebrew/bin/tailscale", "/Applications/Tailscale.app/Contents/MacOS/Tailscale"].iter()
        .find_map(|c| crate::new_command(c).args(["ip", "-4"]).output().ok().and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().map(|x| x.trim().to_string())).filter(|x| x.starts_with("100.")));
    // all local network addresses (Wi-Fi, cable, adapters)
    let lan: Vec<String> = crate::new_command("ifconfig").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).lines()
        .filter_map(|r| r.trim().strip_prefix("inet ").and_then(|x| x.split_whitespace().next()).map(String::from))
        .filter(|x| x.starts_with("192.168.") || x.starts_with("10.") || x.starts_with("172.")).collect()).unwrap_or_default();
    // code for the iPhone app (Collega.swift): juggl://collega?d=<base64url of {v,nome,hosts,porta,token}>
    let output = |c: &str, a: &[&str]| crate::new_command(c).args(a).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|x| !x.is_empty());
    // device name for the iPhone app: the computer's real one (Mac: scutil; Windows/Linux: host name)
    let name = if cfg!(target_os = "macos") { output("scutil", &["--get", "ComputerName"]).unwrap_or_else(license::device_name) } else { license::device_name() };
    let mut hosts = lan.clone();
    if let Some(l) = output("scutil", &["--get", "LocalHostName"]) { hosts.push(format!("{l}.local")); }
    if let Some(t) = &ts { hosts.push(t.clone()); }
    // tailscale qr (12/11): also the MagicDNS name (e.g. mac.tailnet.ts.net), second route away from home if the 100.x IP changes
    let magicdns = ts.as_ref().and_then(|_| ["/usr/local/bin/tailscale", "/opt/homebrew/bin/tailscale", "/Applications/Tailscale.app/Contents/MacOS/Tailscale"].iter()
        .find_map(|c| crate::new_command(c).args(["status", "--json"]).output().ok().and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok()))
        .and_then(|v| v["Self"]["DNSName"].as_str().map(|x| x.trim_end_matches('.').to_string())).filter(|x| x.ends_with(".ts.net")));
    if let Some(m) = &magicdns { hosts.push(m.clone()); }
    let mut data = json!({"v": 1, "nome": name, "hosts": hosts, "porta": bridge::port(), "token": bridge::key()});
    if let Some(m) = &magicdns { data["magicdns"] = json!(m); }   // without Tailscale the field is absent (format agreed with the iPhone)
    use base64::Engine;
    let link = format!("juggl://collega?d={}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data.to_string()));
    let qr = qrcode::QrCode::new(link.as_bytes()).map(|c| c.render::<qrcode::render::svg::Color>().min_dimensions(200, 200)
        .dark_color(qrcode::render::svg::Color("#000000")).light_color(qrcode::render::svg::Color("#ffffff")).build()).unwrap_or_default();
    json!({"porta": bridge::port(), "token": bridge::key(), "lan": lan, "tailscale": ts, "magicdns": magicdns, "nome": name, "link": link, "qr": qr,
           "accesa": profile::is_enabled("iphone"), "in_rete": bridge::ON_NETWORK.load(std::sync::atomic::Ordering::Relaxed)})
}

#[tauri::command(async)]
fn rinomina(id: String, nome: String) -> Value {
    if !storage::is_valid_id(&id) { return json!({"ok": false, "msg": "id non valido"}) }
    let p = base_dir().join("stato").join(format!("{id}.json"));
    // renamed by hand: the name is the user's, no longer "automatic" (no title replaces it)
    if !update_json(&p, |v| { v["nome_manuale"] = json!(nome.trim()); if let Some(o) = v.as_object_mut() { o.remove("nome_auto"); } true }) { return json!({"ok": false, "msg": "sessione non trovata"}) }
    json!({"ok": true})
}

/// Pins a session to the top of the list (like Finder Favorites). A pinned one is not archived and not cleaned up by itself.
#[tauri::command(async)]
fn fissa_sessione(id: String, on: bool) -> Value {
    if !storage::is_valid_id(&id) || id.starts_with("locale:") || id == juggl_id() { return json!({"ok": false, "msg": "questa sessione non si fissa"}) }
    let p = base_dir().join("stato").join(format!("{id}.json"));
    if !update_json(&p, |v| { v["fissata"] = json!(on); v["fissata_il"] = json!(if on { unix_now() } else { 0.0 }); true }) { return json!({"ok": false, "msg": "sessione non trovata"}) }
    json!({"ok": true})
}

#[tauri::command(async)]
fn pilota_annulla(id: String, spegni: bool) -> Value { autopilot::cancel(&id, spegni) }
#[tauri::command(async)]
fn pilota_imposta(id: String, acceso: bool) -> Value { autopilot::set(&id, acceso) }
#[tauri::command(async)]
fn pilota_ora(id: String) -> Value { autopilot::send_now(&id) }
#[tauri::command(async)]
fn guardando(id: String) {
    *autopilot::WATCHED.lock().unwrap() = id.clone();
    // opening the chat counts as having seen the new answers to its requests
    requests::mark_read(&id);
    if !storage::is_valid_id(&id) || id.starts_with("locale:") { return; }
    let p = base_dir().join("stato").join(format!("{id}.json"));
    if p.exists() { update_json(&p, |v| { if str_of(v, "stato") != "aspetta" { return false; } v["letta_da"] = v.get("da").cloned().unwrap_or(json!(0)); true }); }
}

/// list (15/11): "Segna come letta" / "Segna tutto come letto" in the To read section of the sidebar (like opening it from the iPhone).
#[tauri::command]
fn segna_letta(id: String) -> Value { mark_seen_by_user(&id) }

/// Chat opened from the iPhone: counts as read (new answers and "Da leggere"), without touching the session watched on the Mac.
pub(crate) fn mark_seen_by_user(id: &str) -> Value {
    if !storage::is_valid_id(id) || id.starts_with("locale:") { return json!({"ok": false}); }
    requests::mark_read(id);
    let p = base_dir().join("stato").join(format!("{id}.json"));
    if p.exists() { update_json(&p, |v| { if str_of(v, "stato") != "aspetta" { return false; } v["letta_da"] = v.get("da").cloned().unwrap_or(json!(0)); true }); }
    if let Some(a) = APP.get() { let _ = a.emit("cambiato", ()); }
    json!({"ok": true})
}

/// The sessions open in side-by-side columns: the watcher follows their transcripts, alerts skip them.
#[tauri::command(async)]
fn guardando_tutte(ids: Vec<String>) {
    let mut v: Vec<String> = Vec::new();
    for id in ids { if !id.is_empty() && !v.contains(&id) { v.push(id); } }
    v.truncate(8);
    *autopilot::WATCHED_ALL.lock().unwrap() = v;
}

#[tauri::command(async)]
fn progetti() -> Value { json!(session_care::projects()) }
#[tauri::command(async)]
fn iberna(id: String) -> Value { if !storage::is_valid_id(&id) { return json!({"ok": false, "msg": "id non valido"}) } session_care::hibernate(&id, "a mano") }
#[tauri::command(async)]
fn chiudi(id: String) -> Value {
    if !storage::is_valid_id(&id) { return json!({"ok": false, "msg": "id non valido"}) }
    if let Some(l) = id.strip_prefix("locale:") { return local_models::delete_chat(l); }
    hosted::close(&id, false); session_care::close(&id)
}
/// Deletes a file or folder. (Overwriting with zeros on APFS is pointless: copy-on-write, the old blocks stay.)
fn remove_fully(p: &Path) {
    if p.is_dir() { let _ = fs::remove_dir_all(p); return; }
    storage::remove(p);
}

/// Removes the entries containing the id from a JSON file (list or object) without touching the rest.
fn remove_from_json(p: &Path, id: &str) {
    if id.is_empty() || !p.exists() { return; }
    update_json(p, |v| {
        let keeps = |x: &Value| !x.to_string().contains(id);
        let updated = match v.clone() {
            Value::Array(a) => Value::Array(a.into_iter().filter(|x| keeps(x)).collect()),
            Value::Object(o) => Value::Object(o.into_iter().filter(|(k, x)| !k.contains(id) && keeps(x)).map(|(k, x)| {
                // one level below (e.g. entries inside "voci" or nested maps)
                let x = match x { Value::Array(a) => Value::Array(a.into_iter().filter(|y| keeps(y)).collect()),
                    Value::Object(o2) => Value::Object(o2.into_iter().filter(|(k2, y)| !k2.contains(id) && keeps(y)).collect()), y => y };
                (k, x)
            }).collect()),
            x => x,
        };
        if updated == *v { return false; }
        *v = updated;
        true
    });
}

/// Deletes a chat without leaving traces: conversation, state, queue, notifications, requests, names, history.
#[tauri::command(async)]
fn elimina_chat(id: String) -> Value {
    // SECURITY: only a valid session id (uuid) or local chat id ("locale:" + hex).
    // With an empty or odd id the prefix searches would touch files of other sessions.
    if !storage::is_valid_id(&id) { return json!({"ok": false, "msg": "id non valido"}) }
    let h = home();
    if let Some(l) = id.strip_prefix("locale:") {
        local_models::stop(l);
        let clean: String = l.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        remove_fully(&base_dir().join("chat-locali").join(format!("{clean}.json")));
        let _ = local_models::delete_chat(l);   // notifications and leftovers in the search index (VACUUM), 10/11
    } else {
        let v = find_session(&id);
        hosted::close(&id, false);
        if let Some(v) = &v { session_care::terminate(v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0)); }
        // look for the transcript in all project folders (the state may not have it)
        let mut found: Vec<PathBuf> = v.as_ref().and_then(find_transcript).into_iter().collect();
        for e in fs::read_dir(h.join(".claude/projects")).into_iter().flatten().flatten() {
            let t = e.path().join(format!("{id}.jsonl")); if t.exists() && !found.contains(&t) { found.push(t); }
        }
        for t in found { remove_fully(&t.with_extension("")); remove_fully(&t); }
        for p in [h.join(format!(".claude/file-history/{id}")), h.join(format!(".claude/session-env/{id}")), h.join(format!(".claude/tasks/{id}"))] { if p.exists() { remove_fully(&p); } }
        for e in fs::read_dir(h.join(".claude/todos")).into_iter().flatten().flatten() { if e.file_name().to_string_lossy().starts_with(&id) { remove_fully(&e.path()); } }
        // prompt history: remove only the lines of this session
        let hist = h.join(".claude/history.jsonl");
        if let Ok(t) = fs::read_to_string(&hist) { if t.contains(&id) { let r: String = t.lines().filter(|l| !l.contains(&id)).map(|l| format!("{l}\n")).collect(); let _ = storage::write(&hist, r.as_bytes()); } }
        remove_fully(&base_dir().join("stato").join(format!("{id}.json")));
        remove_from_json(&h.join(".claude/state/nomi-sessioni.json"), &id);
    }
    remove_fully(&queue_file(&id));
    for f in ["notifiche.json", "richieste.json"] { remove_from_json(&base_dir().join(f), &id); }
    requests::remove_session(&id);
    approvals::remove_session(&id);
    if let Some(app) = APP.get() { let _ = app.emit("cambiato", ()); }
    json!({"ok": true})
}

/// Settings of a Claude session: model and reasoning level.
/// Hosted: restarts with the same conversation (right away if idle, at the end of the turn if working). Terminal: /model and /effort.
#[tauri::command(async)]
fn regola(id: String, modello: String, effort: String) -> Value {
    if !storage::is_valid_id(&id) { return json!({"ok": false, "msg": "id non valido"}) }
    let p = base_dir().join("stato").join(format!("{id}.json"));
    let is_hosted = hosted::is_hosted(&id);
    let mut working = false;
    let existed = update_json(&p, |v| {
        v["modello"] = json!(modello); v["effort"] = json!(effort);
        working = str_of(v, "stato") == "lavora";
        if is_hosted && working { v["regole_da_applicare"] = json!(true); }
        true
    });
    if !existed { return json!({"ok": false, "msg": "sessione non trovata"}) }
    if is_hosted {
        if working { return json!({"ok": true, "msg": "Dal prossimo turno: la sessione sta lavorando"}) }
        hosted::restart(&id);
        return json!({"ok": true, "msg": "Applicato"});
    }
    let Some(vs) = find_session(&id) else { return json!({"ok": true, "msg": "Salvato"}) };
    if str_of(&vs, "surface").is_empty() { return json!({"ok": true, "msg": "Salvato: vale quando la riprendi"}) }
    let mut results = vec![];
    if !modello.is_empty() { results.push(reply_now(&vs, &format!("/model {modello}"))); std::thread::sleep(Duration::from_millis(900)); }
    if !effort.is_empty() { results.push(reply_now(&vs, &format!("/effort {effort}"))); }
    if results.iter().all(|r| r["ok"].as_bool().unwrap_or(false)) { json!({"ok": true, "msg": "Applicato nel terminale"}) }
    else { json!({"ok": false, "msg": format!("Nel terminale non e' andato: {}", results.iter().map(|r| str_of(r, "msg").to_string()).collect::<Vec<_>>().join(", "))}) }
}

/// Settings of a chat with the studio PC: model, temperature, memory (context).
#[tauri::command(async)]
fn regola_locale(id: String, modello: String, temperatura: f64, contesto: u64, think: Option<bool>) -> Value {
    local_models::adjust(id.trim_start_matches("locale:"), &modello, temperatura, contesto, think.unwrap_or(true))
}

/// Claude usage: 5-hour and weekly limits (from the status line) and, for each session,
/// context used, model and equivalent cost. Terminal sessions write them in ~/tavolo/contesto/;
/// for the ones inside the app the context is derived from the last usage in the transcript.
#[tauri::command(async)]
fn consumi() -> Value {
    let limits = { let mut v = load_json(&home().join(".claude/regista/usage/limiti.json")).unwrap_or(json!({})); v["rate_limits"] = quota::valid_limits(); v };
    let mut out = vec![];
    for v in list_sessions() {
        let sid = str_of(&v, "sessione").to_string();
        if sid.starts_with("locale:") { continue; }
        let c = load_json(&base_dir().join("contesto").join(format!("{sid}.json")));
        let (mut pct, mut token, mut window, mut model, mut cost) = (Value::Null, 0u64, 0u64, String::new(), Value::Null);
        if let Some(c) = &c {
            pct = c["context_window"]["used_percentage"].clone();
            window = c["context_window"]["context_window_size"].as_u64().unwrap_or(0);
            let u = &c["context_window"]["current_usage"];
            token = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().map(|k| u[*k].as_u64().unwrap_or(0)).sum();
            model = str_of(&c["model"], "display_name").to_string();
            cost = c["cost"]["total_cost_usd"].clone();
        }
        // sessions inside the app (no status line): last usage in the transcript
        if c.is_none() || hosted::is_hosted(&sid) {
            if let Some(t) = find_transcript(&v) {
                if let Ok(mut f) = fs::File::open(&t) {
                    use std::io::{Read, Seek, SeekFrom};
                    let n = f.metadata().map(|m| m.len()).unwrap_or(0);
                    let _ = f.seek(SeekFrom::Start(n.saturating_sub(1_500_000)));
                    let mut b = String::new(); let _ = f.read_to_string(&mut b);
                    if let Some(r) = b.lines().rev().filter(|l| l.contains("\"usage\"") && l.contains("\"assistant\"")).find_map(|l| serde_json::from_str::<Value>(l).ok()) {
                        let u = &r["message"]["usage"];
                        token = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().map(|k| u[*k].as_u64().unwrap_or(0)).sum();
                        let model_id = str_of(&r["message"], "model").to_string();
                        if model.is_empty() {
                            // "claude-opus-5-5" -> "Opus 5.5"
                            let p: Vec<&str> = model_id.trim_start_matches("claude-").split('-').collect();
                            model = match p.as_slice() { [n, rest @ ..] if !n.is_empty() => format!("{}{} {}", n[..1].to_uppercase(), &n[1..], rest.iter().take(2).copied().collect::<Vec<_>>().join(".")).trim().to_string(), _ => model_id.clone() };
                        }
                        if window == 0 { window = if model_id.contains("opus-5") || model_id.contains("sonnet-5") { 1_000_000 } else { 200_000 }; }
                        pct = json!(((token as f64 / window as f64) * 100.0).round());
                    }
                }
            }
        }
        if token == 0 && pct.is_null() { continue; }
        out.push(json!({"sessione": sid, "nome": str_of(&v, "nome"), "vista": str_of(&v, "vista"), "progetto": v.get("progetto").cloned().unwrap_or(Value::Null),
            "contesto_pct": pct, "token": token, "finestra": window, "modello": model, "costo_usd": cost, "surface": str_of(&v, "surface"), "ospitata": hosted::is_hosted(&sid)}));
    }
    json!({"limiti": limits["rate_limits"], "aggiornati": limits["at"], "sessioni": out})
}

/// Full usage (0.1.76): per chat, project, hour, model and thread type, from the transcripts. See quota::dettaglio.
#[tauri::command(async)]
fn consumi_dettaglio() -> Value {
    let mut ss = list_sessions();
    ss.extend(archived_sessions());
    quota::breakdown(ss)
}

/// Weekly quota: forecast, spend per day and per session (last 7 days, from all transcripts).
#[tauri::command(async)]
fn consumi_analisi() -> Value {
    let mut ss = list_sessions();
    ss.extend(archived_sessions());
    quota::analysis(ss)
}

// ---------- merge (15/11): a chat under another one (sub-chat) ----------
// Optional field "sotto_di" = id of the main chat, in the sub-chat state. No chains: if the main chat
// is itself a sub-chat its main chat is used; merging a chat that has sub-chats moves them all under the new one.
// If the main chat is gone (closed, archived, deleted) the sidebar shows the sub-chat on its own.
fn inherit_project(out: &mut [Value]) {
    let prog: HashMap<String, Value> = out.iter().map(|v| (str_of(v, "sessione").to_string(), v["progetto"].clone())).collect();
    for v in out.iter_mut() {
        let p = str_of(v, "sotto_di").to_string();
        if p.is_empty() { continue; }
        if let Some(pr) = prog.get(&p).filter(|x| x.is_object()) { v["progetto"] = pr.clone(); }
    }
}
fn state_path(id: &str) -> PathBuf { base_dir().join("stato").join(format!("{id}.json")) }

/// Camera, microphone, location and notifications are never granted to a page; the rest stays as it was.
fn webview_permission(kind: &tauri::webview::PermissionKind) -> tauri::webview::PermissionResponse {
    use tauri::webview::{PermissionKind as K, PermissionResponse as R};
    match kind { K::Microphone | K::Camera | K::Geolocation | K::Notifications => R::Deny, _ => R::Default }
}

#[cfg(test)]
mod webview_permission_tests {
    use super::*;
    use tauri::webview::{PermissionKind as K, PermissionResponse as R};
    #[test]
    fn camera_mic_location_notifications_always_denied() {
        for k in [K::Microphone, K::Camera, K::Geolocation, K::Notifications] {
            assert!(matches!(webview_permission(&k), R::Deny), "{k:?} deve essere rifiutato");
        }
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    #[test]
    fn sub_chat_takes_main_project() {
        let mut v = vec![
            json!({"sessione": "a", "progetto": {"id": "p1", "nome": "Progetto uno"}}),
            json!({"sessione": "b", "progetto": null, "sotto_di": "a"}),
            json!({"sessione": "c", "progetto": {"id": "p2", "nome": "Progetto due"}, "sotto_di": "manca"}),
            json!({"sessione": "d", "progetto": {"id": "p2", "nome": "Progetto due"}, "sotto_di": "a"}),
        ];
        inherit_project(&mut v);
        assert_eq!(str_of(&v[1]["progetto"], "id"), "p1", "senza progetto: prende quello della principale");
        assert_eq!(str_of(&v[2]["progetto"], "id"), "p2", "principale assente: resta com'era");
        assert_eq!(str_of(&v[3]["progetto"], "id"), "p1", "sotto una principale: segue la principale");
    }
}

#[tauri::command]
fn unisci_chat(figlia: String, principale: String) -> Value {
    if !storage::is_valid_id(&figlia) || !storage::is_valid_id(&principale) || figlia == principale { return json!({"ok": false, "msg": "chat non valide"}); }
    let (pf, pp) = (state_path(&figlia), state_path(&principale));
    if !pf.exists() || !pp.exists() { return json!({"ok": false, "msg": "chat non trovata"}); }
    // no chains: the real main chat is the main chat's own main, if it has one
    let real_main = load_json(&pp).map(|v| str_of(&v, "sotto_di").to_string()).filter(|x| !x.is_empty() && x != &figlia && state_path(x).exists()).unwrap_or(principale);
    if real_main == figlia { return json!({"ok": false, "msg": "una chat non puo' stare sotto se stessa"}); }
    // the child's sub-chats move under the real main chat
    for e in fs::read_dir(base_dir().join("stato")).into_iter().flatten().flatten() {
        let pe = e.path();
        if pe == pf || pe.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        if load_json(&pe).map(|v| str_of(&v, "sotto_di") == figlia).unwrap_or(false) {
            update_json(&pe, |v| { v["sotto_di"] = json!(real_main.clone()); true });
        }
    }
    let ok = update_json(&pf, |v| { v["sotto_di"] = json!(real_main.clone()); v["sotto_di_il"] = json!(unix_now()); true });
    log_juggl(&format!("unisci: {figlia} sotto {real_main}"));
    if let Some(a) = APP.get() { let _ = a.emit("cambiato", ()); }
    json!({"ok": ok, "principale": real_main})
}

#[tauri::command]
fn separa_chat(figlia: String) -> Value {
    if !storage::is_valid_id(&figlia) { return json!({"ok": false, "msg": "chat non valida"}); }
    let ok = update_json(&state_path(&figlia), |v| { let c = v.get("sotto_di").is_some(); if let Some(o) = v.as_object_mut() { o.remove("sotto_di"); o.remove("sotto_di_il"); } c });
    if ok { log_juggl(&format!("unisci: {figlia} tolta da sotto")); }
    if let Some(a) = APP.get() { let _ = a.emit("cambiato", ()); }
    json!({"ok": true})
}

#[tauri::command(async)]
fn non_serve(id: String) -> Value { let r = autopilot::not_needed(&id); if let Some(a) = APP.get() { let _ = a.emit("cambiato", ()); } r }
/// "Manda a un'altra chat": the message was for another chat and Juggl sent it there. In the original chat
/// this notice arrives, written by Juggl and not by the user (no new requests), with the same sending path.
fn moved_note_text() -> String { format!("{} Ignora il messaggio precedente: era per un'altra chat.", profile::with_name(approvals::FROM_USER_PREFIX)) }
#[tauri::command(async)]
fn nota_spostato(id: String) -> Value { send_internal(id, moved_note_text()) }
#[tauri::command(async)]
fn modelli_locali() -> Value { local_models::models() }
/// Screen of the PC agent for the "Agente PC" chat (in-memory image, as a data URL; never on disk).
#[tauri::command(async)]
fn schermo_agente() -> Value {
    use base64::Engine;
    match pc_agent::screen() {
        Ok(b) => json!({"ok": true, "img": format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(b))}),
        Err(e) => json!({"ok": false, "msg": e}),
    }
}

/// "Cancella tutte le chat locali" button: files, notifications, search index, in one go (10/11).
#[tauri::command(async)]
fn cancella_chat_locali() -> Value { let r = local_models::delete_all(); if let Some(a) = APP.get() { let _ = a.emit("cambiato", ()); } r }
/// View of a search result that is not among the open, closed or archived chats.
fn view_not_open(id: &str) -> String {
    let _ = id;
    "chiusa".into()
}
/// Search in all conversations (closed ones too): ripgrep on the transcripts, most recent first.
/// Search in all messages (FTS5 index): filters session, folder, who, img, days, limit, order.
#[tauri::command(async)]
fn cerca_tutto(q: String, filtri: Option<Value>) -> Value { search_index::search(&q, &filtri.unwrap_or(json!({}))) }
/// Search inside one conversation (all of it, subagents included), most recent first.
#[tauri::command(async)]
fn cerca_in_sessione(id: String, q: String) -> Value { search_index::search(&q, &json!({"sessione": id, "ordine": "tempo", "limite": 1000})) }
#[tauri::command(async)]
fn indice_stato() -> Value { search_index::status() }

#[tauri::command(async)]
fn cerca_ovunque(q: String) -> Value {
    let q = q.trim().to_string();
    if q.chars().count() < 3 { return json!([]); }
    let root = home().join(".claude/projects");
    // ripgrep if available (Homebrew), otherwise the system grep
    let rg = ["/opt/homebrew/bin/rg", "/usr/local/bin/rg"].into_iter().find(|p| Path::new(p).is_file());
    let out = match rg {
        Some(rg) => crate::new_command(rg).args(["-l", "-i", "-F", "--glob", "*.jsonl", "--max-depth", "2", &q]).arg(&root).output(),
        None => crate::new_command("/usr/bin/grep").args(["-r", "-l", "-i", "-F", "--include=*.jsonl", "--exclude-dir=subagents", "-e", &q]).arg(&root).output(),
    };
    let Ok(out) = out else { return json!([]) };
    let mut files: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout).lines().map(PathBuf::from).collect();
    files.sort_by_key(|p| std::cmp::Reverse(fs::metadata(p).and_then(|m| m.modified()).ok()));
    let alive: HashMap<String, Value> = list_sessions().into_iter().map(|v| (str_of(&v, "sessione").to_string(), v)).collect();
    let ql = q.to_lowercase();
    let mut results = vec![];
    for p in files.into_iter().take(25) {
        let sid = p.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string();
        let title = title_from(&p).unwrap_or_default();
        // the phrase found: search in the texts (yours and the agent's), not in technical data
        let mut phrase = String::new(); let mut when = String::new(); let mut cwd = String::new();
        if let Ok(f) = fs::File::open(&p) {
            for r in BufReader::new(f).lines().map_while(Result::ok) {
                if cwd.is_empty() { if let Some(i) = r.find("\"cwd\":\"") { cwd = r[i + 7..].split('"').next().unwrap_or("").to_string(); } }
                if !r.to_lowercase().contains(&ql) || !(r.contains("\"type\":\"user\"") || r.contains("\"type\":\"assistant\"")) { continue; }
                let Ok(d) = serde_json::from_str::<Value>(&r) else { continue };
                let c = d.get("message").and_then(|m| m.get("content")).cloned().unwrap_or(Value::Null);
                let text = match &c { Value::String(t) => t.clone(), Value::Array(a) => a.iter().filter(|x| str_of(x, "type") == "text").map(|x| str_of(x, "text")).collect::<Vec<_>>().join(" "), _ => String::new() };
                let tl = text.to_lowercase();
                if let Some(i) = tl.find(&ql) {
                    let start = text.char_indices().map(|(b, _)| b).filter(|b| *b <= i.saturating_sub(90)).last().unwrap_or(0);
                    phrase = text[start..].chars().take(220).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
                    when = str_of(&d, "timestamp").to_string();
                    break;
                }
            }
        }
        if phrase.is_empty() { continue; }
        let open_session = alive.get(&sid);
        results.push(json!({"sessione": sid, "titolo": open_session.map(|v| str_of(v, "nome").to_string()).filter(|x| !x.is_empty()).unwrap_or(if title.is_empty() { path_tail(&cwd) } else { title }),
            "frase": phrase, "t": when, "cwd": cwd, "viva": open_session.is_some(), "vista": open_session.map(|v| str_of(v, "vista")).unwrap_or("chiusa")}));
    }
    json!(results)
}

fn title_from(p: &Path) -> Option<String> {
    let f = fs::File::open(p).ok()?;
    let mut t = None;
    for r in BufReader::new(f).lines().map_while(Result::ok).take(400) {
        if r.contains("\"customTitle\"") || r.contains("\"aiTitle\"") {
            if let Ok(d) = serde_json::from_str::<Value>(&r) { t = Some(str_of(&d, "customTitle").to_string()).filter(|x| !x.is_empty()).or(Some(str_of(&d, "aiTitle").to_string())); }
        }
    }
    t
}

/// Closed conversation (not on the desk): read it from the transcript to show it and be able to resume it.
#[tauri::command(async)]
fn sessione_chiusa(id: String) -> Value {
    if !storage::is_valid_id(&id) { return Value::Null; }
    let v = json!({"sessione": id});
    let Some(p) = find_transcript(&v) else { return Value::Null };
    let mut d = detail_from(Some(p.clone()), false);
    let Ok(fh) = fs::File::open(&p) else { return Value::Null };
    let cwd = BufReader::new(fh).lines().map_while(Result::ok).take(200)
        .find_map(|r| r.find("\"cwd\":\"").map(|i| r[i + 7..].split('"').next().unwrap_or("").to_string())).unwrap_or_default();
    d["sessione"] = json!(id); d["nome"] = json!(title_from(&p).unwrap_or_else(|| path_tail(&cwd))); d["vista"] = json!("chiusa"); d["cwd"] = json!(cwd);
    // archived: name, project, date and settings from its state
    if let Some(a) = archived_sessions().into_iter().find(|x| str_of(x, "sessione") == id) {
        for k in ["nome", "vista", "progetto", "archiviata_il", "modello", "effort"] { d[k] = a[k].clone(); }
        if !str_of(&a, "cwd").is_empty() { d["cwd"] = a["cwd"].clone(); }
    }
    d
}

/// Resumes a closed conversation inside the app (claude --resume in its folder).
#[tauri::command(async)]
fn riprendi_chiusa(app: tauri::AppHandle, id: String) -> Value {
    if !storage::is_valid_id(&id) { return json!({"ok": false, "msg": "id non valido"}) }
    // the original folder (transcript, project), never a temporary one
    let cwd = hosted::resume_dir(&id);
    match hosted::start(&app, &cwd, "", Some(&id)) { Ok(_) => json!({"ok": true}), Err(e) => json!({"ok": false, "msg": e}) }
}

#[tauri::command(async)]
fn notifiche() -> Value {
    let l = load_json(&base_dir().join("notifiche.json")).unwrap_or(json!([]));
    let read_until = load_json(&base_dir().join("notifiche-lette.json")).map(|v| f64_of(&v, "t")).unwrap_or(0.0);
    json!({"voci": l, "letta_fino": read_until})
}
#[tauri::command(async)]
fn notifiche_lette() -> Value { write_state(&base_dir().join("notifiche-lette.json"), &json!({"t": unix_now()})); json!({"ok": true}) }

/// The user's requests understood by the small model (log ~/tavolo/richieste-ai.json).
#[tauri::command(async)]
fn richieste_ai() -> Value { requests::all_requests() }
/// Actions on a request: controllata, non_va, riapri, ricordaglielo, chiedi, chiudi.
#[tauri::command(async)]
fn richiesta_ai_azione(id: String, azione: String) -> Value { requests::action(&id, &azione) }
/// Re-evaluates all open requests with the current rules (in the background, two sessions at a time).
#[tauri::command(async)]
fn richieste_ai_rivaluta() -> Value { requests::reevaluate_all() }
/// New answers of a session marked as seen (the iPhone app when it opens the chat).
fn ai_requests_mark_read(sessione: String) -> Value { requests::mark_read(&sessione) }
/// Oks and things left open, from all sessions (Requests view).
#[tauri::command(async)]
fn ok_voci() -> Value { approvals::all_items() }
/// Ok, No, Rispondi (with text) and Togli buttons on an entry.
#[tauri::command(async)]
fn ok_azione(id: String, azione: String, testo: Option<String>) -> Value { approvals::action(&id, &azione, &testo.unwrap_or_default()) }
/// Buttons of the block at the end of the answer, for a line that is not in the ok log.
#[tauri::command(async)]
fn ok_manda(sessione: String, azione: String, titolo: String, testo: Option<String>) -> Value { approvals::send_answer(&sessione, &azione, &titolo, &testo.unwrap_or_default()) }

#[tauri::command(async)]
fn stato_mac() -> Value { session_care::mac_status(&list_sessions()) }
#[tauri::command(async)]
fn pulisci_mac(urgente: bool) -> Value { session_care::cleanup(urgente) }
#[tauri::command(async)]
fn esci_app(nome: String) -> Value { session_care::quit_app(&nome) }
// engine ~/tavolo/bin/mac: scan (normal 10-15 s, deep about 1 min), cleanup, memory, history
#[tauri::command(async)]
fn mac_scansione(profonda: bool) -> Value { session_care::mac_scan(profonda) }
#[tauri::command(async)]
fn mac_ultima() -> Value { session_care::mac_latest() }
#[tauri::command(async)]
fn mac_libera(id: String, conferma: bool) -> Value { let r = session_care::mac_free(&id, conferma); notify_changed(); r }
#[tauri::command(async)]
fn mac_libera_auto(obiettivo_gb: f64) -> Value { let r = session_care::mac_free_auto(obiettivo_gb); notify_changed(); r }
#[tauri::command(async)]
fn mac_ram() -> Value { session_care::mac_ram() }
#[tauri::command(async)]
fn mac_storico() -> Value { session_care::mac_history() }
#[tauri::command(async)]
fn mac_ripristina(n: String) -> Value { let r = session_care::mac_restore(&n); notify_changed(); r }
// space (14/11): entries, full scan, reversible actions (bin/spazio.py)
#[tauri::command(async)]
fn mac_spazio(args: Vec<String>) -> Value {
    let r = session_care::mac_space(&args);
    if args.first().map(|x| x != "voci" && x != "avanza").unwrap_or(false) { notify_changed(); }
    r
}
fn notify_changed() { if let Some(a) = APP.get() { let _ = a.emit("cambiato", ()); } }
#[tauri::command(async)]
fn nuova_locale(modello: String, testo: String) -> Value {
    if !license::is_valid() { return json!({"ok": false, "msg": "Juggl non ha un account attivo: entra con il tuo account per avviare le chat"}); }
    match APP.get() { Some(a) => local_models::new_chat(&modello, &testo, a), None => json!({"ok": false, "msg": "app non pronta"}) }
}
/// Stops the current turn: inside the app with Claude's command, in the terminal by pressing Esc as you would by hand.
#[tauri::command(async)]
fn interrompi(id: String) -> Value {
    if let Some(l) = id.strip_prefix("locale:") { local_models::stop(l); return json!({"ok": true}); }
    if hosted::is_hosted(&id) {
        return match hosted::interrupt(&id) { Ok(_) => json!({"ok": true}), Err(e) => json!({"ok": false, "msg": e}) };
    }
    let Some(v) = find_session(&id) else { return json!({"ok": false, "msg": "sessione non trovata"}) };
    let sf = str_of(&v, "surface");
    if sf.is_empty() { return json!({"ok": false, "msg": "sessione non raggiungibile"}) }
    let ok = cmux(&["send-key", "--surface", sf, "Escape"]).map(|o| o.status.success()).unwrap_or(false);
    json!({"ok": ok, "msg": if ok { "" } else { "il terminale non risponde" }})
}

/// A queued message goes out now, even if the session is working.
#[tauri::command(async)]
fn invia_ora_dalla_coda(id: String, indice: usize, mid: Option<String>) -> Value {
    if !storage::is_valid_id(&id) { return json!({"ok": false, "msg": "id non valido"}) }
    // by id (since 8/11): the index may have changed already if meanwhile the queue went out or got shorter
    let mid = mid.unwrap_or_default();
    let Some((indice, m)) = with_queue(&id, |c| {
        let i = if mid.is_empty() { Some(indice).filter(|&i| i < c.len()) } else { c.iter().position(|x| str_of(x, "id") == mid) };
        i.map(|i| (i, c.remove(i)))
    }) else { return json!({"ok": false, "msg": "messaggio non piu' in coda"}) };
    // already analyzed when it entered the queue
    requests::mark_handled_text(&id, str_of(&m, "testo"));
    let r = send_core(id.clone(), str_of(&m, "testo").to_string(), None, Some(true));
    if !r["ok"].as_bool().unwrap_or(false) {
        // not sent: it goes back to its place (unless someone put it back meanwhile)
        let k = queue_key(&m);
        with_queue(&id, |c| if !c.iter().any(|x| queue_key(x) == k) { c.insert(indice.min(c.len()), m); });
    } else if str_of(&r, "esito") == "in_coda" {
        // still not possible (e.g. waiting for a choice): it went back to the end of the queue, put it back where it was
        let new_mid = str_of(&r, "mid").to_string();
        with_queue(&id, |c| if let Some(p) = c.iter().position(|x| str_of(x, "id") == new_mid) { let x = c.remove(p); c.insert(indice.min(c.len()), x); });
    }
    r
}
pub(crate) fn juggl_id() -> String { fs::read_to_string(base_dir().join("juggl-id")).map(|x| x.trim().to_string()).unwrap_or_default() }

/// The general "Juggl" session (folder ~/tavolo/generale-juggl with its instructions): creates or resumes it.
fn start_juggl_chat(h: &tauri::AppHandle) {
    let cwd = base_dir().join("generale-juggl").to_string_lossy().to_string();
    let id = juggl_id();
    if !id.is_empty() && base_dir().join("stato").join(format!("{id}.json")).exists() {
        if !hosted::is_hosted(&id) { let _ = hosted::start(h, &cwd, "", Some(&id)); }
    } else if let Ok(new_id) = hosted::start(h, &cwd, "", None) {
        let _ = storage::write(&base_dir().join("juggl-id"), new_id.as_bytes());
    }
    let id = juggl_id();
    let p = base_dir().join("stato").join(format!("{id}.json"));
    if p.exists() { update_json(&p, |v| { v["nome_manuale"] = json!("Juggl"); v["fissata"] = json!(true); v["pilota"] = json!(false); true }); }
}

pub(crate) static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

// ---------- welcome (first launch, can be reopened from Settings) ----------

/// Opens (or brings to front) the welcome window. `step` = "" from the start, "giro" only the app tour.
fn create_welcome(app: &tauri::AppHandle, step: &str) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("benvenuto") { let _ = w.show(); let _ = w.set_focus(); return Ok(()); }
    let url = if step.is_empty() { "welcome.html".to_string() } else { format!("welcome.html#{step}") };
    // "Importa progetti" on its own (Settings, menu): taller window for the list
    let (title, height) = match step { "importa" => ("Importa progetti", 640.0), "giro" => ("Benvenuto in Juggl", 560.0), _ => ("Benvenuto in Juggl", 600.0) };
    tauri::WebviewWindowBuilder::new(app, "benvenuto", tauri::WebviewUrl::App(url.into()))
        .title(title).inner_size(760.0, height).resizable(false).maximizable(false).minimizable(false)
        .title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true).center().focused(true)
        .build().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn benvenuto_apri(app: tauri::AppHandle, passo: String) -> Value {
    match create_welcome(&app, &passo) { Ok(_) => json!({"ok": true}), Err(e) => json!({"ok": false, "msg": e}) }
}

/// End of the welcome (or tour skipped): marks it done, shows the main window, closes the welcome.
#[tauri::command(async)]
fn benvenuto_fine(app: tauri::AppHandle, giro: bool) -> Value {
    profile::mark_welcome(giro);
    if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.set_focus(); }
    let _ = app.emit_to("main", "cambiato", ());
    if let Some(w) = app.get_webview_window("benvenuto") { let _ = w.close(); }
    json!({"ok": true})
}

/// End of "Importa progetti" opened on its own: no marks on the welcome, shows the main window and closes.
#[tauri::command(async)]
fn importa_chiudi(app: tauri::AppHandle) -> Value {
    if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.set_focus(); }
    let _ = app.emit_to("main", "cambiato", ());
    if let Some(w) = app.get_webview_window("benvenuto") { let _ = w.close(); }
    json!({"ok": true})
}

/// A test notification: the first time macOS asks for permission.
#[tauri::command(async)]
fn prova_notifica(app: tauri::AppHandle) -> Value {
    use tauri_plugin_notification::NotificationExt;
    let r = app.notification().builder().title("Juggl").body("Così ti avviso quando una sessione ti aspetta.").show();
    json!({"ok": r.is_ok(), "msg": r.err().map(|e| e.to_string()).unwrap_or_default()})
}

/// Juggl helpers (0.1.70): settings (switches, extractor pause and cap) and usage of the last 24 hours.
#[tauri::command(async)]
fn aiutanti_stato() -> Value { json!({"impostazioni": helpers::settings(), "consumi": helpers::usage_24h()}) }

#[tauri::command(async)]
fn aiutanti_imposta(valori: Value) -> Value { helpers::set(&valori); aiutanti_stato() }

/// Idle compaction (16/11): Settings > Compaction (ui/compaction.js).
#[tauri::command(async)]
fn compattazione_stato() -> Value { compaction::status() }
#[tauri::command(async)]
fn compattazione_imposta(valori: Value) -> Value { compaction::set(&valori) }
/// Switch of a chat: true on, false off, null follows the general setting.
#[tauri::command(async)]
fn compattazione_chat(id: String, acceso: Option<bool>) -> Value { compaction::set_chat(&id, acceso) }
/// "Compatta ora" (chat menu and quota tips): `/compact` with Juggl's fixed instructions.
#[tauri::command(async)]
fn compattazione_ora(id: String) -> Value {
    if !storage::is_valid_id(&id) || id.starts_with("locale:") { return json!({"ok": false, "msg": "chat non valida"}); }
    let r = send_internal(id.clone(), format!("/compact {}", compaction::INSTRUCTIONS));
    compaction::record(json!({"sessione": id, "esito": "a_mano", "invio": str_of(&r, "esito")}));
    r
}

/// For the iPhone (and tests): new session inside the app, with optional model.
///
/// Anti-duplicates (0.1.68): `name` and `force` are optional. With a name, if a chat with the same name already exists in the
/// same folder (trailing numbers ignored) another one is NOT created: answer `{"ok": false, "esiste": "<id>", "msg": "esiste gia' <id>"}`.
/// With `force` the chat is created anyway. Without a name everything works as before. The name is always saved without trailing numbers.
fn new_without_app(folder: String, text: String, model: String, name: String, force: bool) -> Value {
    let Some(app) = APP.get() else { return json!({"ok": false, "msg": "app non pronta"}) };
    let mut cwd = session_care::expand_path(folder.trim());
    if cwd.is_empty() || !Path::new(&cwd).is_dir() { let d = base_dir().join("chat-veloci"); let _ = fs::create_dir_all(&d); cwd = d.to_string_lossy().to_string(); }
    let name = hook::name_without_number(name.trim());
    let name = if name.chars().count() < 3 { String::new() } else { name };
    if !name.is_empty() && !force {
        if let Some(id) = hook::similar_chat(&base_dir().join("stato"), &cwd, &name, "") {
            return json!({"ok": false, "esiste": id, "msg": format!("esiste gia' {id}")});
        }
    }
    // general setting "model for new chats" (key modello_nuove in impostazioni.json: fable|opus|sonnet|haiku, empty = as before):
    // applies only if the request does not already carry a model
    let model = if model.trim().is_empty() { session_care::settings().get("modello_nuove").and_then(|x| x.as_str()).filter(|m| matches!(*m, "fable" | "opus" | "sonnet" | "haiku")).unwrap_or("").to_string() } else { model };
    match hosted::start_with(app, &cwd, &text, None, Some(&model)) {
        Ok(id) => {
            if !name.is_empty() { let _ = rinomina(id.clone(), name); }
            json!({"ok": true, "id": id})
        }
        Err(e) => json!({"ok": false, "msg": e}),
    }
}

/// For the iPhone: resumes a hibernated session without going through the Mac UI.
fn resume_without_app(id: String) -> Value {
    match APP.get() { Some(a) => riprendi(a.clone(), id), None => json!({"ok": false, "msg": "app non pronta"}) }
}

#[tauri::command(async)]
fn riprendi(app: tauri::AppHandle, id: String) -> Value {
    if !storage::is_valid_id(&id) || id.starts_with("locale:") { return json!({"ok": false, "msg": "id non valido"}) }
    match resume_hosted(&app, &id) { Ok(_) => json!({"ok": true, "msg": ""}), Err(e) => json!({"ok": false, "msg": e}) }
}

// ---------- archive: sessions put aside, resumed whenever you want (nothing is lost) ----------

/// The archived sessions, most recent first: name, project, folder, date, last answer, settings.
pub(crate) fn archived_sessions() -> Vec<Value> {
    let names = load_json(&home().join(".claude/state/nomi-sessioni.json")).unwrap_or(json!({}));
    let pp = session_care::projects();
    let mut out = vec![];
    for e in fs::read_dir(base_dir().join("stato")).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        let Some(v) = load_json(&p) else { continue };
        if str_of(&v, "stato") != "archiviata" { continue; }
        let id = str_of(&v, "sessione");
        if !storage::is_valid_id(id) { continue; }
        // the name from when you archived it, unless it has a better one meanwhile
        let a = str_of(&v, "nome_archivio");
        let name = if !a.is_empty() && str_of(&v, "nome_manuale").is_empty() && str_of(&v, "titolo").is_empty() && names.get(id).is_none() { a.to_string() } else { name_of(&v, &names) };
        let prog = session_care::project_of(&v, &name, &pp);
        out.push(json!({"sessione": id, "nome": name, "progetto": prog, "cwd": str_of(&v, "cwd"), "vista": "archiviata",
            "archiviata_il": f64_of(&v, "archiviata_il"), "motivo_archivio": str_of(&v, "motivo_archivio"), "da": f64_of(&v, "da"),
            "risposta": str_of(&v, "risposta"), "chiesto": str_of(&v, "chiesto"), "modello": str_of(&v, "modello"), "effort": str_of(&v, "effort"),
            "ospite": str_of(&v, "ospite")}));
    }
    out.sort_by(|a, b| f64_of(b, "archiviata_il").partial_cmp(&f64_of(a, "archiviata_il")).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Archive: stops the process like Hibernate (hosted ones through the keeper), records date and name and removes the
/// session from the list. Transcript, name, logo, model, settings and requests stay where they are.
pub(crate) fn archive_core(id: &str, reason: &str) -> Value {
    if !storage::is_valid_id(id) || id.starts_with("locale:") { return json!({"ok": false, "msg": "id non valido"}) }
    if id == juggl_id() { return json!({"ok": false, "msg": "La chat Juggl resta sempre aperta"}) }   // rename (14/11)
    let p = base_dir().join("stato").join(format!("{id}.json"));
    let names = load_json(&home().join(".claude/state/nomi-sessioni.json")).unwrap_or(json!({}));
    let mut pid = 0;
    // first mark the state, then stop: whoever sees the process end (reader, hook) finds it already archived
    let ok = update_json(&p, |v| {
        if str_of(v, "stato") == "archiviata" { return false; }
        pid = v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
        let previous = str_of(v, "stato").to_string();
        // from hibernated keep the state from before hibernation: Resume puts it back as it was
        if previous != "ibernata" { v["stato_prima"] = json!(if previous == "lavora" || previous == "bloccata" { "aspetta".to_string() } else { previous.clone() }); }
        v["archiviata_da"] = json!(previous);
        v["nome_archivio"] = json!(name_of(v, &names));
        v["stato"] = json!("archiviata");
        v["archiviata_il"] = json!(unix_now());
        v["motivo_archivio"] = json!(reason);
        v["pilota_tra"] = json!(0);   // no pending autopilot answer
        true
    });
    if !ok { return json!({"ok": false, "msg": "sessione non trovata"}) }
    if hosted::is_hosted(id) { hosted::close(id, true); } else { session_care::terminate(pid); }
    if let Some(a) = APP.get() { let _ = a.emit("cambiato", ()); }
    json!({"ok": true, "msg": "archiviata"})
}

/// Removes from the archive without starting it: back to hibernated (in the list, Resume reopens it). True if it was archived.
fn unarchive_core(id: &str) -> bool {
    let p = base_dir().join("stato").join(format!("{id}.json"));
    let mut was_archived = false;
    update_json(&p, |v| {
        if str_of(v, "stato") != "archiviata" { return false; }
        was_archived = true;
        v["stato"] = json!("ibernata");
        // hibernated from now: the automatic archive does not take it back right away
        v["ibernata_il"] = json!(unix_now());
        v["motivo_ibernazione"] = json!("tolta dall'archivio");
        if let Some(o) = v.as_object_mut() { for k in ["archiviata_il", "motivo_archivio", "nome_archivio", "archiviata_da"] { o.remove(k); } }
        true
    });
    was_archived
}

#[tauri::command(async)]
fn archivia(id: String) -> Value { archive_core(&id, "a mano") }

#[tauri::command(async)]
fn togli_archivio(id: String) -> Value {
    if !storage::is_valid_id(&id) || id.starts_with("locale:") { return json!({"ok": false, "msg": "id non valido"}) }
    if !unarchive_core(&id) { return json!({"ok": false, "msg": "non e' nell'archivio"}) }
    if let Some(a) = APP.get() { let _ = a.emit("cambiato", ()); }
    json!({"ok": true, "msg": "tolta dall'archivio: e' tra le ibernate"})
}

/// Resumes an archived one: back in the list and restarted in the keeper with --resume, in its folder and with its model.
#[tauri::command(async)]
fn riprendi_archiviata(app: tauri::AppHandle, id: String) -> Value {
    if !storage::is_valid_id(&id) || id.starts_with("locale:") { return json!({"ok": false, "msg": "id non valido"}) }
    unarchive_core(&id);
    // if it does not restart it stays among the hibernated (resumable), never lost
    let r = match resume_hosted(&app, &id) { Ok(_) => json!({"ok": true, "msg": ""}), Err(e) => json!({"ok": false, "msg": e}) };
    let _ = app.emit("cambiato", ());
    r
}
fn resume_archived_without_app(id: String) -> Value {
    match APP.get() { Some(a) => riprendi_archiviata(a.clone(), id), None => json!({"ok": false, "msg": "app non pronta"}) }
}

/// Eco mode (eco.rs): on/off by hand, and permission to turn on by itself.
#[tauri::command(async)]
fn eco_imposta(attiva: bool, auto: Option<bool>) -> Value {
    let r = eco::set(attiva, auto, "a mano");
    if let Some(a) = APP.get() { let _ = a.emit("cambiato", ()); }
    r
}
#[tauri::command(async)]
fn imposta_pausa_auto(attiva: bool, soglia: f64) -> Value {
    // automatic pause of working chats (not pinned) above the weekly limit threshold: off by default (11/11)
    let s = if soglia.is_finite() { soglia.clamp(50.0, 100.0).round() } else { 95.0 };
    match storage::modify(&base_dir().join("impostazioni.json"), Some(json!({})), |v| { v["pausa_auto"] = json!({"attiva": attiva, "soglia": s}); true }) {
        Ok(_) => json!({"ok": true, "attiva": attiva, "soglia": s}),
        Err(e) => json!({"ok": false, "msg": e}),
    }
}
/// Automatic archive: days after which a hibernated chat archives itself (0 = off, the default).
#[tauri::command(async)]
fn imposta_archivio_auto(giorni: f64) -> Value {
    let g = if giorni.is_finite() { giorni.clamp(0.0, 365.0).round() } else { 0.0 };
    match storage::modify(&base_dir().join("impostazioni.json"), Some(json!({})), |v| { v["archivia_dopo_giorni"] = json!(g); true }) {
        Ok(_) => json!({"ok": true, "giorni": g}),
        Err(e) => json!({"ok": false, "msg": e}),
    }
}

/// From the keeper: archives chats hibernated for more than N days, if the setting is on. Never Juggl nor pinned ones.
fn archive_old(ss: &[Value]) -> usize {
    let days = f64_of(&session_care::settings(), "archivia_dopo_giorni");
    if days <= 0.0 { return 0; }
    let (juggl, now) = (juggl_id(), unix_now());
    let mut names = vec![];
    for v in ss {
        let id = str_of(v, "sessione");
        if str_of(v, "vista") != "ibernata" || id == juggl || id.starts_with("locale:") || v.get("fissata").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
        let since = if f64_of(v, "ibernata_il") > 0.0 { f64_of(v, "ibernata_il") } else { f64_of(v, "da") };
        if since <= 0.0 || now - since < days * 86400.0 { continue; }
        if archive_core(id, "automatica")["ok"].as_bool().unwrap_or(false) { names.push(str_of(v, "nome").to_string()); }
    }
    if !names.is_empty() {
        log_juggl(&format!("archivio automatico: {}", names.join(", ")));
        session_care::record("Juggl", &format!("Archiviate da sole (ibernate da più di {days:.0} giorni): {}. Le riprendi da Archiviate.", names.join(", ")), "", "sistema");
    }
    names.len()
}

/// For the iPhone: the ⌘K search with its filters and, for each result, name and state of the session.
/// `project`: id or name of a project (progetti.json) or a folder.
pub(crate) fn search_for_iphone(q: &str, project: &str, mine_only: bool, images: bool, days: f64, limit: u64) -> Value {
    let project = project.trim();
    let folder = if project.is_empty() { String::new() }
        else if project.starts_with('/') || project.starts_with("~/") { session_care::expand_path(project) }
        else { session_care::projects().into_iter().find(|p| str_of(p, "id") == project || str_of(p, "nome").eq_ignore_ascii_case(project)).map(|p| str_of(&p, "cartella").to_string()).unwrap_or_default() };
    if !project.is_empty() && folder.is_empty() { return json!({"risultati": [], "ms": 0, "errore": "progetto non trovato"}); }
    let filters = json!({"chi": if mine_only { "tu" } else { "" }, "img": images, "giorni": days, "cartella": folder, "limite": limit.clamp(1, 400)});
    let mut r = search_index::search(q, &filters);
    let juggl = juggl_id();
    let mut info: HashMap<String, (String, String)> = HashMap::new();
    for v in list_sessions().into_iter().chain(local_models::as_sessions()) {
        let id = str_of(&v, "sessione").to_string();
        let name = if id == juggl { "Juggl".to_string() } else { str_of(&v, "nome").to_string() };
        info.insert(id, (name, str_of(&v, "vista").to_string()));
    }
    for a in archived_sessions() { info.insert(str_of(&a, "sessione").to_string(), (str_of(&a, "nome").to_string(), "archiviata".into())); }
    if let Some(rr) = r.get_mut("risultati").and_then(|x| x.as_array_mut()) {
        for x in rr.iter_mut() {
            let id = str_of(x, "sessione").to_string();
            let (name, view) = match info.get(&id) {
                Some((n, v)) => (n.clone(), v.clone()),
                None => (if !str_of(x, "titolo").is_empty() { str_of(x, "titolo").to_string() } else { path_tail(str_of(x, "cwd")) },
                         view_not_open(&id)),
            };
            x["nome"] = json!(name); x["vista"] = json!(view);
        }
    }
    r
}

const RESTART_MSG: &str = "Juggl è stato riavviato mentre lavoravi: riprendi da dove eri.";

/// Resumes a session inside the app in the right folder and restores the state from before the pause.
/// If the turn had been interrupted (app closed or crashed, process died halfway) and the transcript does not have
/// the final answer, sends it an internal message (no new requests) to continue.
fn resume_hosted(app: &tauri::AppHandle, id: &str) -> Result<(), String> {
    // already hosted or being started by another thread (app start, queue, iPhone): no second message
    if hosted::is_hosted_or_starting(id) { return Ok(()); }
    static RESUMING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    {
        let mut r = RESUMING.lock().unwrap_or_else(|e| e.into_inner());
        if r.iter().any(|x| x == id) { return Ok(()); }
        r.push(id.to_string());
    }
    struct Guard<'a>(&'a str);
    impl Drop for Guard<'_> { fn drop(&mut self) { RESUMING.lock().unwrap_or_else(|e| e.into_inner()).retain(|x| x != self.0); } }
    let _guard = Guard(id);
    let p = base_dir().join("stato").join(format!("{id}.json"));
    let v = load_json(&p).unwrap_or(json!({}));
    // interrupted: marked by Juggl, or stopped by closing or by a crash (old versions did not mark it)
    let automatic = str_of(&v, "stato") == "ibernata" && matches!(str_of(&v, "motivo_ibernazione"), "app chiusa" | "interrotta");
    let interrupted = v.get("turno_interrotto").and_then(|x| x.as_bool()).unwrap_or(false) || automatic;
    let previous = if str_of(&v, "stato") == "ibernata" { str_of(&v, "stato_prima").to_string() } else { str_of(&v, "stato").to_string() };
    let cwd = hosted::resume_dir(id);
    hosted::start(app, &cwd, "", Some(id))?;
    let resume_turn = interrupted && !hosted::turn_finished(id);
    update_json(&p, |v2| {
        v2["stato"] = json!(if previous == "aspetta" || previous == "bloccata" || resume_turn { "aspetta" } else { "ferma" });
        v2["da"] = v.get("da").cloned().unwrap_or(json!(unix_now()));
        if let Some(o) = v2.as_object_mut() { o.remove("turno_interrotto"); o.remove("stato_prima"); }
        true
    });
    if resume_turn {
        log_juggl(&format!("{id}: turno interrotto dal riavvio, lo faccio continuare"));
        let _ = send_internal(id.to_string(), RESTART_MSG.to_string());
    }
    Ok(())
}

/// When Juggl opens: hosted sessions stopped by closing the app, or left halfway by a crash
/// (live state but dead process), restart by themselves with the whole conversation.
fn restore_hosted(h: &tauri::AppHandle) {
    if !hosted::main_instance() { return; }
    let mut to_resume: Vec<String> = vec![];
    for e in fs::read_dir(base_dir().join("stato")).into_iter().flatten().flatten() {
        let pe = e.path();
        if pe.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        let Some(v) = load_json(&pe) else { continue };
        let id = str_of(&v, "sessione").to_string();
        if str_of(&v, "ospite") != "tavolo" || !storage::is_valid_id(&id) || id.starts_with("locale:") || hosted::is_hosted(&id) { continue; }
        let state = str_of(&v, "stato");
        let closed_by_app = state == "ibernata" && str_of(&v, "motivo_ibernazione") == "app chiusa";
        // crash: Juggl did not manage to hibernate it in time. Only if recent (2 days), old ones stay hibernated
        let crash = !matches!(state, "ibernata" | "chiusa" | "archiviata" | "");
        if !closed_by_app && !crash { continue; }
        // process still alive from the previous launch: stop it before resuming (no two copies)
        hosted::stop_orphan(&id);
        if crash {
            let recent = unix_now() - f64_of(&v, "agg").max(f64_of(&v, "da")) < 2.0 * 86400.0;
            update_json(&pe, |x| {
                if state == "lavora" { x["turno_interrotto"] = json!(true); }
                x["stato_prima"] = json!(if state == "lavora" { "aspetta" } else { state });
                x["stato"] = json!("ibernata"); x["ibernata_il"] = json!(unix_now()); x["motivo_ibernazione"] = json!("interrotta");
                true
            });
            if !recent { continue; }
        }
        to_resume.push(id);
    }
    // on reopening the chats restart in a row, one at a time (like the restart queue): 26 chats in the same second
    // are 26 claude processes at once. Wait for the previous one to be back up (at most 60 s) and then another 8 s.
    // On a separate thread: the startup of the rest of Juggl does not wait for the row.
    if !to_resume.is_empty() {
        let h = h.clone();
        std::thread::spawn(move || {
            let n = to_resume.len();
            for (i, id) in to_resume.into_iter().enumerate() {
                if let Err(err) = resume_hosted(&h, &id) { log_juggl(&format!("{id}: non ripresa all'avvio: {err}")); continue; }
                if i + 1 < n { hosted::pause_after_resume(&id); }
            }
        });
    }
}
#[tauri::command(async)]
fn nuova(app: tauri::AppHandle, cartella: String, testo: String, modello: Option<String>, effort: Option<String>, motore: Option<String>) -> Value {   // engines (12/11): + engine
    // no project = quick chat: its own folder, so any files do not clutter the home
    let mut cwd = session_care::expand_path(cartella.trim());
    if cwd.is_empty() || !Path::new(&cwd).is_dir() {
        let d = base_dir().join("chat-veloci"); let _ = fs::create_dir_all(&d); cwd = d.to_string_lossy().to_string();
    }
    let id = new_uuid();
    // settings as a chat (16/11): with no model chosen, the one for new chats from Settings (Claude only)
    let modello = modello.filter(|m| !m.trim().is_empty()).or_else(|| if motore.as_deref() != Some("codex") { settings_chat::model_for_new() } else { None });
    let e = effort.unwrap_or_default();
    if !e.is_empty() { let p = base_dir().join("stato").join(format!("{id}.json")); write_state(&p, &json!({"sessione": id, "effort": e, "modello": modello.clone().unwrap_or_default()})); }
    // engines (12/11): engine chosen in the new chat window ("claude" if missing)
    let mt = motore.filter(|m| m == "codex");
    match hosted::start_engine(&app, &cwd, &testo, if e.is_empty() { None } else { Some(&id) }, modello.as_deref(), mt.as_deref()) { Ok(id) => json!({"ok": true, "id": id}), Err(e) => json!({"ok": false, "msg": e}) }
}
fn new_uuid() -> String {
    let o = crate::new_command("/usr/bin/uuidgen").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_lowercase()).unwrap_or_default();
    if storage::is_valid_id(&o) { o } else { hosted::uuid() }
}
#[tauri::command(async)]
fn copia_file(path: String) -> Value { session_care::copy_file(&path) }
#[tauri::command(async)]
fn copia_dati(dati: String) -> Value { session_care::copy_data(&dati) }
#[tauri::command(async)]
fn info_file(path: String) -> Value { session_care::file_info(&path) }

/// fast (14/11): the chat for the Mac app. Live edits arrive with only the summary (files, +/-): the diff
/// lines (522 KB in one chat) are requested with `modifiche_pezzi` when you open the card. The bridge (iPhone) uses sessione_dati.
#[tauri::command(async)]
fn sessione(id: String, dal: Option<String>, prima: Option<usize>) -> Value {
    let mut d = session_data(id, dal, prima);
    fast_open::slim_changes(&mut d);
    d
}

/// fast (14/11): diff lines of an answer ("modifiche" event with these da/a), from the same read of the chat
/// (cached: it is the one just sent). If it is not in the last messages, look for it reading from that time.
/// startup (17/11): the UI says after how long it got the data the first time (to measure Juggl's opening).
#[tauri::command(async)]
fn segna_avvio(ms: f64) { if ms.is_finite() && ms >= 0.0 { log_juggl(&format!("avvio: dati a schermo dopo {} ms", ms.round() as u64)); } }

#[tauri::command(async)]
fn modifiche_pezzi(sessione: String, da: String, a: String) -> Value {
    let Some(v) = find_session(&sessione) else { return json!({"ok": false, "msg": "sessione non trovata"}) };
    let find_edits = |d: &Value| d["eventi"].as_array().and_then(|ev| ev.iter().rev().find(|e| str_of(e, "tipo") == "modifiche" && str_of(e, "da") == da && str_of(e, "a") == a).cloned());
    let e = find_edits(&detail_view(&v, "", 6)).or_else(|| if da.is_empty() { None } else { find_edits(&detail_view(&v, &da, 6)) });
    match e { Some(e) => json!({"ok": true, "elenco": e["elenco"]}), None => json!({"ok": false, "msg": "modifiche non trovate nella chat"}) }
}

pub(crate) fn session_data(id: String, since: Option<String>, before: Option<usize>) -> Value {   // fast (14/11): + before
    if let Some(l) = id.strip_prefix("locale:") {
        let mut d = local_models::detail(l);
        if d.is_object() { d["richieste_ai"] = requests::of_session(&id); }
        return d;
    }
    // fast (14/11): timings of the parts; above 1 s a line in the log (only the start of the id) to find the real cause
    let t_in = std::time::Instant::now();
    let mut timings: Vec<(&str, u128)> = vec![];
    let mut mark = |name: &'static str, t: &mut std::time::Instant| { timings.push((name, t.elapsed().as_millis())); *t = std::time::Instant::now(); };
    let mut t = std::time::Instant::now();
    let Some(v) = find_session(&id) else { return Value::Null };
    mark("trova", &mut t);
    let mut o: Map<String, Value> = v.as_object().cloned().unwrap_or_default();
    let since = since.unwrap_or_default();
    // fast (14/11): capped and cached read (detail_view); requests and oks only for this window
    if let Value::Object(d) = detail_view(&v, &since, before.unwrap_or(6).clamp(1, 400)) { o.extend(d); }
    mark("messaggi", &mut t);
    o.insert("git".into(), git(str_of(&v, "cwd")));
    mark("git", &mut t);
    if turn_view(&v) == "lavora" && !str_of(&v, "surface").is_empty() { o.insert("schermo".into(), json!(clean_screen(str_of(&v, "surface")))); }
    // question open in the terminal (permission or choice): only if the session is not writing
    if !str_of(&v, "surface").is_empty() && (str_of(&v, "vista") == "bloccata" || !o.get("domanda").map(|x| x.is_null()).unwrap_or(true) || turn_view(&v) == "lavora") {
        let dt = terminal_question(str_of(&v, "surface"));
        if !dt.is_null() { o.insert("domanda_terminale".into(), dt); o.insert("vista".into(), json!("bloccata")); }
    }
    // permissions (12/11): permission request of a hosted chat, like a terminal question with three buttons
    if let Some(dt) = permissions::question(&v) { o.insert("domanda_terminale".into(), dt); o.insert("vista".into(), json!("bloccata")); }
    mark("terminale", &mut t);
    o.insert("coda".into(), json!(read_queue(&id)));
    // fast (14/11): only open requests or ones born from messages in the window (a long chat sent 452 of them, 519 KB)
    let t0 = o.get("eventi").and_then(|e| e.as_array()).and_then(|a| a.iter().find_map(|e| e["t"].as_str().filter(|t| !t.is_empty()))).map(requests::from_iso).unwrap_or(0.0);
    let window = if o.get("piu_vecchi").and_then(|x| x.as_bool()).unwrap_or(false) { t0 - 3600.0 } else { 0.0 };
    o.insert("richieste_ai".into(), requests::of_session_since(&id, window));
    o.insert("ok".into(), approvals::of_session(&id));
    mark("richieste", &mut t);
    o.insert("sotto".into(), json!(list_subagents(&v)));
    mark("aiutanti", &mut t);
    o.insert("ospitata".into(), json!(hosted::is_hosted(&id)));
    // ready session (Import projects): the prepared context, to show before the first message
    if v.get("pronta").and_then(|x| x.as_bool()).unwrap_or(false) { o.insert("contesto_testo".into(), json!(import::context_of(&v))); }
    summaries::enrich(&mut o);   // "adesso_chiaro" and errors told plainly (summaries.rs)
    let mut d = Value::Object(o);
    bridge::lighten_for_mac(&mut d);   // embedded images cached, size of every image: light reloads, no jumps
    mark("immagini", &mut t);
    let tot = t_in.elapsed().as_millis();
    if tot > 1000 {
        let parts: Vec<String> = timings.iter().filter(|x| x.1 > 0).map(|(n, ms)| format!("{n} {ms}")).collect();
        log_juggl(&format!("veloci: chat {} aperta in {tot} ms ({})", id.chars().take(8).collect::<String>(), parts.join(", ")));
    }
    d
}

fn cmux(args: &[&str]) -> Option<std::process::Output> { if !profile::is_enabled("cmux") { return None; } crate::new_command(CMUX).args(args).output().ok() }

#[tauri::command(async)]
fn apri(id: String) -> Value {
    let Some(v) = find_session(&id) else { return json!({"ok": false, "msg": "sessione non trovata"}) };
    let sf = str_of(&v, "surface");
    if sf.is_empty() { return json!({"ok": false, "msg": "sessione senza terminale"}) }
    let par = json!({"surface_id": sf, "workspace_id": str_of(&v, "workspace")}).to_string();
    let ok = cmux(&["rpc", "surface.focus", &par]).map(|o| o.status.success()).unwrap_or(false);
    let _ = crate::new_command("osascript").args(["-e", "tell application \"cmux\" to activate"]).output();
    json!({"ok": ok, "msg": ""})
}

// ---------- sending messages: right away if the session is free, otherwise queued (sent when it finishes) ----------

fn queue_file(id: &str) -> PathBuf {
    let clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).collect();
    base_dir().join("coda").join(format!("{clean}.json"))
}
fn read_queue(id: &str) -> Vec<Value> { load_json(&queue_file(id)).and_then(|v| v.as_array().cloned()).unwrap_or_default() }

/// Key of a queued message: its id (since 8/11) or, for old ones, time and text.
fn queue_key(m: &Value) -> String {
    if !str_of(m, "id").is_empty() { return str_of(m, "id").to_string(); }
    format!("{}|{}", f64_of(m, "t"), str_of(m, "testo"))
}

/// Changes a session's queue under lock (iPhone, external messages, autopilot and delivery write together).
/// Empty queue = file removed. Returns the function's value.
fn with_queue<R>(id: &str, fun: impl FnOnce(&mut Vec<Value>) -> R) -> R { with_queue_at(&queue_file(id), fun) }

fn with_queue_at<R>(p: &Path, fun: impl FnOnce(&mut Vec<Value>) -> R) -> R {
    let p = p.to_path_buf();
    let _ = fs::create_dir_all(p.parent().unwrap());
    let _l = storage::lock(&p).map_err(|e| log_juggl(&format!("coda {}: {e}", p.display()))).ok();
    let mut c = match storage::read_json(&p) {
        storage::ReadResult::Valida(v) => v.as_array().cloned().unwrap_or_default(),
        storage::ReadResult::Assente => vec![],
        // unreadable: do not rewrite it from scratch, set the broken file aside
        storage::ReadResult::Rotta => { let _ = fs::rename(&p, p.with_extension(format!("rotta.{}", unix_now() as u64))); vec![] }
    };
    let before = c.clone();
    let r = fun(&mut c);
    if c != before {
        if c.is_empty() { let _ = fs::remove_file(&p); } else { write_state(&p, &json!(c)); }
    }
    r
}

/// Appends a message to the end of the queue, with its own id and the reason it was not sent (2-3 words, in the bubble).
fn enqueue(id: &str, text: &str, reason: &str) -> String { enqueue_at(&queue_file(id), text, reason) }

fn enqueue_at(p: &Path, text: &str, reason: &str) -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mid = format!("m{}-{}", (unix_now() * 1000.0) as u64, N.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    with_queue_at(p, |c| c.push(json!({"id": mid, "testo": text, "t": unix_now(), "motivo": reason})));
    mid
}

/// "Queued" outcome with the short reason and the message id (the bubble stays the same, only the line below changes).
fn queued_outcome(id: &str, text: &str, reason: &str) -> Value {
    let mid = enqueue(id, text, reason);
    json!({"ok": true, "esito": "in_coda", "msg": reason, "motivo": reason, "mid": mid})
}

/// Marks the same reason on the whole session queue. true = something changed.
fn mark_reason(id: &str, reason: &str) -> bool {
    with_queue(id, |c| { let mut x = false; for m in c.iter_mut() { if str_of(m, "motivo") != reason { m["motivo"] = json!(reason); x = true; } } x })
}

/// App session not hosted right now: why the message is waiting.
fn host_reason(id: &str, v: &Value) -> &'static str {
    if str_of(v, "vista") == "ibernata" { return "sessione ibernata"; }
    if !hosted::is_hosted_or_starting(id) && keeper::socket(id).exists() { return "custode non collegato"; }
    "sessione in ripresa"
}

/// From the reply_now message to the short reason. None = not a reason to wait (real error).
fn terminal_reason(msg: &str) -> Option<&'static str> {
    if msg.contains("domanda aperta") { Some("aspetta una tua scelta") }
    else if msg.contains("scritto a mano") { Some("testo nel terminale") }
    else if msg.contains("lavorando") { Some("terminale non pronto") }
    else { None }
}


/// Message from the user (Mac app or iPhone): it is sent, and the small model derives requests from it (even if queued).
#[tauri::command(async)]
fn invia(id: String, testo: String, immagini: Option<Vec<String>>, subito: Option<bool>) -> Value {
    // commands (13/11): Juggl runs its own "/" commands, the ones that do not exist in -p say what to use,
    // unknown ones never reach the agent (instead of the switch account check of 12/11, which stays as a fallback)
    if let commands::Outcome::Gestito(r) = commands::before_send(&id, &testo) { return r; }
    if hosted::is_hosted(&id) && !id.starts_with("locale:") { if let Some(r) = claude_login::local_command(&testo) { return r; } }
    requests::mark_handled_text(&id, &testo);   // before sending it: the Sentinel must not read it again from the state
    let r = send_core(id.clone(), testo.clone(), immagini, subito);
    if r["ok"].as_bool().unwrap_or(false) { requests::on_send(&id, &testo, str_of(&r, "esito") == "in_coda"); }
    r
}

/// Message between chats (`juggl manda`), not written by the user: sent like the others but generates no requests (0.1.79).
pub(crate) fn send_between_chats(id: String, text: String, immediate: bool) -> Value {
    requests::mark_handled_text(&id, &text);
    send_core(id, text, None, Some(immediate))
}

/// Message written by Juggl or by the autopilot, not by the user: no new requests.
pub(crate) fn send_internal(id: String, text: String) -> Value {
    requests::mark_handled_text(&id, &text);
    send_core(id, text, None, Some(false))
}

/// Claude closed in the terminal tab: the message stays queued and the session restarts inside Juggl
/// with the whole conversation (then the queue goes out by itself).
fn dead_in_terminal(id: &str, text: Option<&str>) -> Value {
    let p = base_dir().join("stato").join(format!("{id}.json"));
    // the screen can deceive: it is dead only if Claude's process is gone.
    // Otherwise the message just stays queued (no copies of the conversation).
    let v = load_json(&p).unwrap_or(json!({}));
    let pid = v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
    // unknown pid: then cmux decides (if it still maps the tab, Claude is there)
    let alive = (pid > 0 && is_alive(pid) && hosted::is_claude(pid)) || (pid <= 0 && real_tabs().contains_key(id));
    let reason = if alive { "terminale non pronto" } else { "sessione in ripresa" };
    let mid = match text { Some(t) => enqueue(id, t, reason), None => { mark_reason(id, reason); String::new() } };
    if alive {
        return json!({"ok": true, "esito": "in_coda", "msg": reason, "motivo": reason, "mid": mid});
    }
    update_json(&p, |v| {
        v["stato"] = json!("ibernata"); v["ibernata_il"] = json!(unix_now()); v["motivo_ibernazione"] = json!("Claude si era chiuso nel terminale");
        if let Some(o) = v.as_object_mut() { o.remove("surface"); }
        true
    });
    // often Claude is not dead: after an update the session stays "in background" in the Claude Code daemon
    // and cannot be resumed until it is stopped (the conversation stays saved). Full id, never a prefix.
    let _ = crate::new_command(profile::claude_cmd()).no_console_window().args(["stop", id]).env("TAVOLO_INTERNO", "1").current_dir(home()).output();
    let _ = resume_without_app(id.to_string());
    json!({"ok": true, "esito": "in_coda", "msg": reason, "motivo": reason, "mid": mid})
}

/// Text with the "Allegato: /path" lines for images not already in the text.
fn with_attachments(text: &str, images: &[String]) -> String {
    let new_ones: Vec<&String> = images.iter().filter(|p| !p.is_empty() && !text.contains(&format!("Allegato: {p}"))).collect();
    if new_ones.is_empty() { return text.to_string(); }
    let lines = new_ones.iter().map(|p| format!("Allegato: {p}")).collect::<Vec<_>>().join("\n");
    if text.trim().is_empty() { lines } else { format!("{}\n\n{lines}", text.trim_end()) }
}

fn send_core(id: String, text: String, images: Option<Vec<String>>, immediate: Option<bool>) -> Value {
    let immediate = immediate.unwrap_or(false);
    let images = images.unwrap_or_default();
    if let Some(l) = id.strip_prefix("locale:") {
        return match APP.get() { Some(a) => local_models::send(l, &text, a), None => json!({"ok": false, "esito": "errore", "msg": "app non pronta"}) };
    }
    // sessions inside the app: straight to stdin, even mid-turn. Claude Code puts it in its queue and reads it
    // at the next step (absorbed_mid_turn), like when you type in the terminal while it works. "Subito" no longer stops the turn.
    if hosted::is_hosted(&id) {
        return match hosted::send(&id, &text, &images) { Ok(_) => json!({"ok": true, "esito": "inviato", "msg": ""}), Err(e) => json!({"ok": false, "esito": "errore", "msg": e}) };
    }
    // from here the message goes to the terminal or to Juggl's queue, which carry text only: images go as
    // "Allegato: /path" (before they were lost when the session was resuming or hibernated, 09/11)
    let text = with_attachments(&text, &images);
    let Some(v) = find_session(&id) else { return json!({"ok": false, "esito": "errore", "msg": "sessione non trovata"}) };
    // app session restarting, with the keeper detached, or hibernated: waits in the queue and goes out as soon as it is ready
    if str_of(&v, "ospite") == "tavolo" || str_of(&v, "vista") == "ibernata" {
        if str_of(&v, "vista") == "ibernata" {
            let r = queued_outcome(&id, &text, "sessione in ripresa");
            let _ = resume_without_app(id.clone());
            return r;
        }
        return queued_outcome(&id, &text, host_reason(&id, &v));
    }
    // Send now / ⌘↩: writes even while working and clears any text left in the terminal input box
    if immediate {
        let r = reply_now_with(&v, &text, true);
        if r["ok"].as_bool().unwrap_or(false) { return json!({"ok": true, "esito": "inviato", "msg": str_of(&r, "msg")}); }
        if r["morta"].as_bool().unwrap_or(false) { return dead_in_terminal(&id, Some(&text)); }
    }
    // permission or choice open: the text would end up in the wrong answer
    if turn_view(&v) == "bloccata" { return queued_outcome(&id, &text, "aspetta una tua scelta"); }
    // there is already something queued: this goes to the end and the queue tries to go out now, all together and in order
    if !read_queue(&id).is_empty() {
        let mid = enqueue(&id, &text, "");
        deliver_session(&id);
        return match read_queue(&id).into_iter().find(|m| str_of(m, "id") == mid) {
            None => json!({"ok": true, "esito": "inviato", "msg": ""}),
            Some(m) => json!({"ok": true, "esito": "in_coda", "msg": str_of(&m, "motivo"), "motivo": str_of(&m, "motivo"), "mid": mid}),
        };
    }
    // free or working: in the terminal it is written right away, as you would. While working Claude Code reads it
    // at the next step, within the current turn. Queue only if the input box cannot be used.
    let working = turn_view(&v) == "lavora";
    let r = reply_now(&v, &text);
    if r["ok"].as_bool().unwrap_or(false) { return json!({"ok": true, "esito": "inviato", "msg": if working { "letto durante il lavoro" } else { "" }}); }
    if r["morta"].as_bool().unwrap_or(false) { return dead_in_terminal(&id, Some(&text)); }
    let msg = str_of(&r, "msg").to_string();
    if let Some(m) = terminal_reason(&msg) { return queued_outcome(&id, &text, m); }
    json!({"ok": false, "esito": "errore", "msg": msg})
}

#[tauri::command(async)]
fn togli_dalla_coda(id: String, indice: usize, mid: Option<String>) -> Value {
    if !storage::is_valid_id(&id) { return json!({"ok": false, "msg": "id non valido"}) }
    let mid = mid.unwrap_or_default();
    with_queue(&id, |c| {
        let i = if mid.is_empty() { Some(indice).filter(|&i| i < c.len()) } else { c.iter().position(|x| str_of(x, "id") == mid) };
        if let Some(i) = i { c.remove(i); }
    });
    json!({"ok": true})
}

/// Delivers the queues: every 3 seconds and right away when a message arrives behind a queue.
fn deliver_queues() -> bool {
    let Ok(rd) = fs::read_dir(base_dir().join("coda")) else { return false };
    let mut changed = false;
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }   // locks, temp files, broken queues
        let id = p.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string();
        if !storage::is_valid_id(&id) { continue; }
        changed |= deliver_session(&id);
    }
    changed
}

/// Tries to deliver a session's queue now, even if it is working (it reads it mid-turn, like the terminal).
/// Waits only if sending it now is not possible or not safe, and writes it on the messages (reason). true = something changed.
fn deliver_session(id: &str) -> bool {
    if read_queue(id).is_empty() { return false; }
    let Some(v) = find_session(id) else { return false };
    let is_hosted = hosted::is_hosted(id);
    let wait_reason = if is_hosted { None }
        else if str_of(&v, "ospite") == "tavolo" || str_of(&v, "vista") == "ibernata" { Some(host_reason(id, &v)) }
        else if turn_view(&v) == "bloccata" { Some("aspetta una tua scelta") }
        else if !matches!(turn_view(&v), "aspetta" | "ferma" | "fatta" | "lavora") { Some("sessione non pronta") }
        else { None };
    if let Some(m) = wait_reason { return mark_reason(id, m); }
    let mut changed = false;
    {
        let id = id.to_string();
        let changed = &mut changed;
        // under the queue lock: whoever adds a message now waits for the delivery to end,
        // and whoever removes it by hand does not see it go out twice
        let dead = with_queue(&id, |c| {
            if c.is_empty() { return false; }
            // all queued messages go out together, as a single message
            let text = c.iter().map(|x| str_of(x, "testo").to_string()).collect::<Vec<_>>().join("\n\n");
            requests::mark_handled_text(&id, &text);   // already analyzed when they entered the queue
            let r = if is_hosted { match hosted::send(&id, &text, &[]) { Ok(_) => json!({"ok": true}), Err(e) => json!({"ok": false, "msg": e}) } } else { reply_now(&v, &text) };
            if r["morta"].as_bool().unwrap_or(false) { *changed = true; return true; }
            if r["ok"].as_bool().unwrap_or(false) { c.clear(); *changed = true; }
            else {
                // the short reason on all queued messages (e.g. text typed by hand in the terminal)
                let m = terminal_reason(str_of(&r, "msg")).unwrap_or("terminale non risponde");
                for x in c.iter_mut() { if str_of(x, "motivo") != m { x["motivo"] = json!(m); *changed = true; } }
            }
            false
        });
        if dead { let _ = dead_in_terminal(&id, None); }
    }
    changed
}

fn reply_now(v: &Value, text: &str) -> Value { reply_now_with(v, text, false) }

/// force = "Invia ora": if there is leftover text in the terminal input box, clears it (Ctrl+U) and says so.
fn reply_now_with(v: &Value, text: &str, force: bool) -> Value {
    let sf = str_of(&v, "surface").to_string();
    if sf.is_empty() { return json!({"ok": false, "msg": "sessione senza terminale"}) }
    let Some(o) = cmux(&["read-screen", "--surface", &sf, "--lines", "40"]) else { return json!({"ok": false, "msg": "terminale non leggibile"}) };
    let sc = String::from_utf8_lossy(&o.stdout).to_string();
    let lines: Vec<&str> = sc.trim_end().split('\n').collect();
    // question or permission open: only a real menu (❯ cursor on an option + instructions), not scattered words in the output
    if !terminal_question(&sf).is_null() { return json!({"ok": false, "msg": "nel terminale c'è una domanda aperta: rispondi dal riquadro della domanda"}) }
    // SECURITY: write only if Claude is alive in the tab. First look for Claude's input box (❯ between two horizontal
    // lines) in the last lines: if it is there, Claude is alive, whatever text is above it (even "claude --resume"
    // printed in a log: it was a false alarm that restarted a copy of the session).
    let n = lines.len();
    // the input box can sit above the background agents panel: look for it in the whole screen, from the bottom
    let box_k = (1..n.saturating_sub(1)).rev().find(|&k| lines[k].trim_start().starts_with('\u{276f}') && lines[k - 1].contains("────") && lines[k + 1].contains("────"));
    let Some(ic) = box_k else {
        // no input box: it is dead only if the last lines are a normal terminal or Claude's exit messages
        let last_lines: Vec<&str> = lines.iter().rev().filter(|l| !l.trim().is_empty()).take(4).copied().collect();
        let dead = last_lines.iter().any(|l| l.contains("worker crashed") || l.contains("Resume this session with"))
            || last_lines.first().map(|l| { let t = l.trim_end(); t.contains("Password:") || ((t.ends_with(" %") || t.ends_with(" $") || t.ends_with(" #")) && t.contains('@')) || (t.contains('@') && (t.contains(" % ") || t.contains(" $ "))) }).unwrap_or(false);
        if dead { return json!({"ok": false, "morta": true, "msg": "Claude non è più attivo in quella scheda del terminale"}) }
        return json!({"ok": false, "msg": "la sessione sta ancora lavorando"})
    };
    let input_box = lines[ic];
    let inner = input_box.trim_start_matches('\u{276f}').trim().to_string();
    let mut removed = String::new();
    if !inner.is_empty() && !PLACEHOLDERS.iter().any(|p| inner.starts_with(p)) {
        if !force { return json!({"ok": false, "msg": format!("nella casella del terminale c'è già del testo scritto a mano: \"{inner}\"")}) }
        for _ in 0..4 { cmux(&["send-key", "--surface", &sf, "ctrl+u"]); std::thread::sleep(Duration::from_millis(80)); }
        removed = inner;
    }
    // a trailing backslash in Claude's terminal means "newline": the message would not go out
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches(|c: char| c == '\\' || c.is_whitespace()).to_string();
    let t = t.as_str();
    // checked outcome: if cmux did not write, the message is not delivered and stays queued
    if !cmux(&["send", "--surface", &sf, t]).map(|o| o.status.success()).unwrap_or(false) { return json!({"ok": false, "msg": "il terminale non ha preso il testo"}) }
    std::thread::sleep(Duration::from_millis(300));
    if !cmux(&["send-key", "--surface", &sf, "Enter"]).map(|o| o.status.success()).unwrap_or(false) { return json!({"ok": false, "msg": "il terminale non ha preso l'Invio"}) }
    json!({"ok": true, "msg": if removed.is_empty() { "inviato".to_string() } else { format!("inviato (ho tolto dal terminale: \"{removed}\")") }})
}

#[tauri::command(async)]
fn apri_file(path: String, finder: bool) {
    #[cfg(target_os = "linux")]
    if Path::new(&path).exists() { let target = if finder { Path::new(&path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or(path.clone()) } else { path.clone() }; system_open(&target); return; }
    #[cfg(windows)]
    if Path::new(&path).exists() { if finder { sys::reveal_in_folder(&path); } else { sys::open(&path); } return; }
    if Path::new(&path).exists() {
        let mut c = crate::new_command("open");
        if finder { c.arg("-R"); }
        let _ = c.arg(path).spawn();
    }
}

/// Esc captured by the system while the viewer is open (and the app is in front): does not depend on the page.
#[tauri::command(async)]
fn esc_cattura(app: tauri::AppHandle, on: bool) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, Code};
    let sc = Shortcut::new(None, Code::Escape);
    let gs = app.global_shortcut();
    let in_front = app.get_webview_window("visore").and_then(|w| w.is_focused().ok()).unwrap_or(false);
    if on && in_front { if !gs.is_registered(sc) { let _ = gs.register(sc); } } else { let _ = gs.unregister(sc); }
}

#[tauri::command(async)]
fn copia_testo(testo: String) -> bool {
    use std::io::Write;
    #[cfg(windows)] { return sys::copy_text(&testo); }
    #[allow(unreachable_code)]
    let Ok(mut c) = crate::new_command("pbcopy").stdin(std::process::Stdio::piped()).spawn() else { return false };
    if let Some(mut i) = c.stdin.take() { let _ = i.write_all(testo.as_bytes()); }
    c.wait().map(|x| x.success()).unwrap_or(false)
}

#[tauri::command(async)]
fn salva_allegato(nome: String, dati: String) -> Value {
    use base64::Engine;
    let Some(b64) = dati.split(',').nth(1) else { return json!({"ok": false, "msg": "dati non validi"}) };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else { return json!({"ok": false, "msg": "dati non validi"}) };
    let dir = base_dir().join("allegati");
    let _ = fs::create_dir_all(&dir);
    let clean: String = nome.chars().map(|c| if c.is_alphanumeric() || ".-_".contains(c) { c } else { '-' }).collect();
    let p = dir.join(format!("{}-{}", unix_now() as u64, if clean.is_empty() { "allegato".into() } else { clean }));
    match fs::write(&p, bytes) { Ok(_) => json!({"ok": true, "path": p.to_string_lossy()}), Err(e) => json!({"ok": false, "msg": e.to_string()}) }
}

#[tauri::command(async)]
fn imposta_logo(id: String, dati: String) -> Value {
    use base64::Engine;
    let Some(v) = find_session(&id) else { return json!({"ok": false, "msg": "sessione non trovata"}) };
    let Some(b64) = dati.split(',').nth(1) else { return json!({"ok": false, "msg": "immagine non valida"}) };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else { return json!({"ok": false, "msg": "immagine non valida"}) };
    let pf = base_dir().join("progetti.json");
    let pid = v.get("progetto").and_then(|p| p.get("id")).and_then(|x| x.as_str()).map(|x| x.to_string()).unwrap_or_else(|| {
        str_of(&v, "nome").to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>().trim_matches('-').to_string()
    });
    let dir = base_dir().join("loghi");
    let _ = fs::create_dir_all(&dir);
    let logo_file = dir.join(format!("{pid}.png"));
    if fs::write(&logo_file, bytes).is_err() { return json!({"ok": false, "msg": "non salvato"}) }
    let r = storage::modify(&pf, Some(json!({"progetti": []})), |reg| {
        if !reg["progetti"].is_array() { reg["progetti"] = json!([]); }
        let list = reg["progetti"].as_array_mut().unwrap();
        if let Some(p) = list.iter_mut().find(|p| str_of(p, "id") == pid) { p["logo"] = json!(logo_file.to_string_lossy()); if let Some(o) = p.as_object_mut() { o.remove("logo_auto"); } }   // list (15/11): set by hand, no longer automatic
        else {
            let words: Vec<String> = str_of(&v, "nome").to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).map(|w| w.to_string()).collect();
            list.push(json!({"id": pid, "nome": str_of(&v, "nome"), "cartella": str_of(&v, "cwd"), "logo": logo_file.to_string_lossy(), "parole": words}));
        }
        true
    });
    match r { Ok(_) => json!({"ok": true}), Err(e) => json!({"ok": false, "msg": e}) }
}

/// Copies into ~/tavolo/allegati the files the system may delete (screenshots dragged from the thumbnail, temporary folders).
#[tauri::command(async)]
fn tieni_allegati(paths: Vec<String>) -> Vec<String> {
    let dir = base_dir().join("allegati");
    let _ = fs::create_dir_all(&dir);
    paths.into_iter().map(|p| {
        let is_temp = p.contains("/TemporaryItems/") || p.starts_with("/var/folders/") || p.starts_with("/private/var/folders/") || p.starts_with("/tmp/");
        if !is_temp { return p; }
        let name = Path::new(&p).file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or("allegato".into());
        let dest = dir.join(format!("{}-{}", unix_now() as u64, name.replace(' ', "-")));
        if fs::copy(&p, &dest).is_ok() { dest.to_string_lossy().to_string() } else { p }
    }).collect()
}

/// Last notification tied to a session (to take you there when you open the app after clicking it).
#[tauri::command(async)]
fn ultima_notifica() -> Value {
    let l = load_json(&base_dir().join("notifiche.json")).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    l.into_iter().rev().find(|x| !str_of(x, "sessione").is_empty() || str_of(x, "tipo") == "mac").unwrap_or(Value::Null)
}

static VIEWER: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

/// Separate viewer: full screen window on the largest monitor, without touching the Juggl window.
#[tauri::command(async)]
fn apri_visore(app: tauri::AppHandle, media: Value, indice: usize) -> Value {
    let data = json!({"media": media_with_source(media), "indice": indice});
    *VIEWER.lock().unwrap() = Some(data.clone());
    if let Some(w) = app.get_webview_window("visore") {
        let _ = w.emit("visore-dati", data); let _ = w.show(); let _ = w.set_focus();
        return json!({"ok": true});
    }
    match create_viewer(&app, true) { Ok(_) => json!({"ok": true}), Err(e) => json!({"ok": false, "msg": e}) }
}

/// Media for the viewer with their source: to those that come with the session add project, name and logo (read from its
/// state, without recomputing the session list); to those without a time, the file time (`mt`, seconds) to sort them.
/// The viewer filters by project ("Questo progetto") or mixes them with everyone's ("Tutti").
fn media_with_source(media: Value) -> Value {
    let Value::Array(mut l) = media else { return media };
    let pp = session_care::projects();
    let names = load_json(&home().join(".claude/state/nomi-sessioni.json")).unwrap_or(json!({}));
    let mut sources: HashMap<String, Option<(String, Value)>> = HashMap::new();
    for m in l.iter_mut() {
        let Some(o) = m.as_object_mut() else { continue };
        let sid = o.get("sessione").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if !sid.is_empty() && o.get("progetto").map(|x| x.is_null()).unwrap_or(true) {
            let source = sources.entry(sid.clone()).or_insert_with(|| {
                if !storage::is_valid_id(&sid) { return None; }
                let v = load_json(&base_dir().join("stato").join(format!("{sid}.json")))?;
                let name = name_of(&v, &names);
                let prog = session_care::project_of(&v, &name, &pp);
                Some((name, prog))
            });
            if let Some((name, prog)) = source.clone() {
                if o.get("nome").and_then(|x| x.as_str()).unwrap_or("").is_empty() { o.insert("nome".into(), json!(name)); }
                if o.get("logo").and_then(|x| x.as_str()).unwrap_or("").is_empty() { if let Some(lg) = prog["logo"].as_str() { o.insert("logo".into(), json!(lg)); } }
                o.insert("progetto".into(), prog);
            }
        }
        if o.get("t").and_then(|x| x.as_str()).unwrap_or("").is_empty() {
            let f = o.get("f").and_then(|x| x.as_str()).unwrap_or("");
            let mt = if f.is_empty() { None } else { fs::metadata(f).ok().and_then(|md| md.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()) };
            if let Some(d) = mt { o.insert("mt".into(), json!(d.as_secs())); }
        }
    }
    Value::Array(l)
}

/// For the viewer's "Tutti" selector: recent media of all sessions (the same as Juggl's strip),
/// in the viewer format, with the session and project of each.
#[tauri::command(async)]
fn visore_tutti() -> Value {
    let l = media_strip::recent_media();
    json!(l.as_array().into_iter().flatten().map(|m| json!({
        "f": str_of(m, "path"), "mini": m["mini"].as_str().unwrap_or(""), "sessione": str_of(m, "sessione"), "nome": str_of(m, "nome"),
        "logo": m["progetto"]["logo"].as_str().filter(|x| !x.is_empty()).or(m["logo"].as_str()).unwrap_or(""),
        "t": str_of(m, "t"), "progetto": m["progetto"].clone(),
    })).collect::<Vec<_>>())
}

/// The viewer is created once (at startup, hidden) and then only shown and hidden: instant opening.
pub(crate) fn create_viewer(app: &tauri::AppHandle, visible: bool) -> Result<(), String> {
    if app.get_webview_window("visore").is_some() { return Ok(()); }
    let monitor = app.available_monitors().ok().and_then(|ms| ms.into_iter().max_by_key(|m| (m.size().width as u64) * (m.size().height as u64)));
    let mut b = tauri::WebviewWindowBuilder::new(app, "visore", tauri::WebviewUrl::App("viewer.html".into()))
        .title("Anteprima").title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true).resizable(true).visible(visible);
    if let Some(m) = monitor {
        // large (80% of the largest monitor) and centered: closed with the red button, Cmd+W or Esc
        let sc = m.scale_factor();
        let (mw, mh) = (m.size().width as f64 / sc, m.size().height as f64 / sc);
        let (w, h) = (mw * 0.8, mh * 0.82);
        b = b.position(m.position().x as f64 / sc + (mw - w) / 2.0, m.position().y as f64 / sc + (mh - h) / 2.0).inner_size(w, h);
    } else { b = b.inner_size(1200.0, 800.0); }
    let w = b.build().map_err(|e| e.to_string())?;
    // the red button hides instead of destroying: the next opening is instant
    let w2 = w.clone();
    let app2 = app.clone();
    w.on_window_event(move |ev| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = ev {
            api.prevent_close();
            let _ = w2.emit("visore-nascosto", ());
            let a = app2.clone();
            std::thread::spawn(move || { close_viewer_from(a); });
        }
    });
    if visible { let _ = w.set_focus(); }
    Ok(())
}

/// Sends an image to the shared "mostra" viewer of all chats (optional tool); without it, Preview opens it.
#[tauri::command(async)]
fn apri_in_mostra(path: String) -> Value {
    let ok = match profile::tool("mostra") { Some(m) => crate::new_command(m).arg(&path).spawn().is_ok(), None => if cfg!(windows) { system_open(&path) } else { crate::new_command("/usr/bin/open").arg(&path).spawn().is_ok() } };
    json!({"ok": ok})
}

/// Closes the viewer from the Rust side (the page is not allowed to close windows) and frees Esc.
#[tauri::command(async)]
fn chiudi_visore(app: tauri::AppHandle) -> Value { close_viewer_from(app) }

pub(crate) fn close_viewer_from(app: tauri::AppHandle) -> Value {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, Code};
    let _ = app.global_shortcut().unregister(Shortcut::new(None, Code::Escape));
    // before hiding it stop the videos: the window stays alive (to open right away) and otherwise they would keep playing
    if let Some(w) = app.get_webview_window("visore") { let _ = w.emit("visore-nascosto", ()); std::thread::sleep(Duration::from_millis(60)); let _ = w.hide(); }
    if let Some(m) = app.get_webview_window("main") { let _ = m.set_focus(); }
    json!({"ok": true})
}

#[tauri::command]
fn visore_dati() -> Value { VIEWER.lock().unwrap().clone().unwrap_or(Value::Null) }

/// From the viewer: closes the preview, brings Juggl to front and takes it to that media's session
/// (scorri = up to the message with the photo, otherwise just to the session).
#[tauri::command(async)]
fn vai_al_media(app: tauri::AppHandle, sessione: String, path: String, t: String, scorri: Option<bool>) -> Value {
    close_viewer_from(app.clone());
    if let Some(m) = app.get_webview_window("main") { let _ = m.show(); let _ = m.unminimize(); let _ = m.set_focus(); }
    let _ = app.emit("vai-media", json!({"sessione": sessione, "path": path, "t": t, "scorri": scorri.unwrap_or(true)}));
    json!({"ok": true})
}

/// Document viewer: ready content (text, CSV rows, Word/RTF as HTML). Never secret files.
#[tauri::command(async)]
fn leggi_documento(path: String) -> Value { documents::read_document(&path, documents::MAC_LIMIT) }

/// All the text of a document ("Copia tutto"), PDF and Word too.
#[tauri::command(async)]
fn testo_documento(path: String) -> Value { documents::full_text(&path) }

/// Image preview (Quick Look) of Pages, Keynote, Numbers, Excel, PowerPoint.
#[tauri::command(async)]
fn anteprima_documento(path: String) -> Value {
    let p = session_care::expand_path(&path);
    if documents::is_secret(&p) { return json!({"ok": false}); }
    match documents::image_preview(&p) { Some(x) => json!({"ok": true, "path": x.to_string_lossy()}), None => json!({"ok": false}) }
}

/// Document cards for arbitrary paths (a path clicked in the chat): only those the viewer can show.
#[tauri::command(async)]
fn tipo_documento(path: String) -> String { let p = session_care::expand_path(&path); if documents::is_secret(&p) { String::new() } else { documents::doc_type(&p).to_string() } }

/// Outside the Mac there is no "overlay" title bar: the window keeps its normal bar.
#[cfg(not(target_os = "macos"))]
trait MacTitleBar: Sized { fn title_bar_style(self, _: tauri::TitleBarStyle) -> Self { self } fn hidden_title(self, _: bool) -> Self { self } }
#[cfg(not(target_os = "macos"))]
impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> MacTitleBar for tauri::WebviewWindowBuilder<'a, R, M> {}

/// Open with the default program: Windows with Explorer, the Mac with `open`.
fn system_open(target: &str) -> bool {
    #[cfg(target_os = "linux")] { return crate::new_command("xdg-open").arg(target).spawn().is_ok(); }
    #[cfg(windows)] { return sys::open(target); }
    #[allow(unreachable_code)]
    crate::new_command("/usr/bin/open").arg(target).spawn().is_ok()
}

#[tauri::command(async)]
fn esiste(path: String) -> bool { Path::new(&session_care::expand_path(&path)).exists() }

#[tauri::command(async)]
fn apri_link(url: String) {
    if url.starts_with("http://") || url.starts_with("https://") { if cfg!(windows) || cfg!(target_os = "linux") { system_open(&url); } else { let _ = crate::new_command("open").arg(url).spawn(); } }
}

/// Cleanup of states (only here, no longer in the Python hook): removes those closed for 7 days and terminal ones
/// whose process has been dead for 7 days. Never hibernated, archived, app-hosted, general Juggl or pinned ones.
/// Then leftover service files (temp files of dead writers, locks of files that no longer exist).
fn clean_up_states() {
    let dir = base_dir().join("stato");
    let now = unix_now();
    let juggl = juggl_id();
    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".json") else { continue };
        if !storage::is_valid_id(id) || id == juggl { continue; }
        let Some(v) = load_json(&p) else { continue };
        let old = now - f64_of(&v, "agg").max(f64_of(&v, "da")) > 7.0 * 86400.0;
        if !old || str_of(&v, "stato") == "ibernata" || str_of(&v, "stato") == "archiviata" || str_of(&v, "ospite") == "tavolo" || v.get("fissata").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
        let pid = v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
        if str_of(&v, "stato") == "chiusa" || !is_alive(pid) {
            // check again under lock: meanwhile the hook may have revived it
            let _l = storage::lock(&p);
            if load_json(&p).map(|x| f64_of(&x, "agg") == f64_of(&v, "agg")).unwrap_or(false) { let _ = fs::remove_file(&p); log_juggl(&format!("pulizia: tolto lo stato di {id}")); }
        }
    }
    for d in [dir, base_dir().join("coda")] { storage::clean_service_dir(&d, Duration::from_secs(3600)); }
}

fn main() {
    // Windows: no HOME, the user folder is USERPROFILE (before any thread: set_var is only safe here)
    #[cfg(windows)]
    if std::env::var_os("HOME").is_none() { if let Some(u) = std::env::var_os("USERPROFILE") { std::env::set_var("HOME", u); } }
    // `Juggl hook`: entry point of the Claude and Codex hooks (Windows; on the Mac it stays bin/tavolo hook)
    if std::env::args().nth(1).as_deref() == Some("hook") {
        let state_dir = std::env::var_os("TAVOLO_STATO_PROVA").map(PathBuf::from).unwrap_or_else(|| base_dir().join("stato"));
        hook::run(state_dir);
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("nomina") {
        let state_dir = std::env::var_os("TAVOLO_STATO_PROVA").map(PathBuf::from).unwrap_or_else(|| base_dir().join("stato"));
        if let Some(id) = std::env::args().nth(2) { hook::name_chat(state_dir, &id); }
        return;
    }
    // Webview permissions (17/11, from the preview engine report): wry without a handler GRANTS camera and microphone to any
    // page. Juggl's webviews only show its own UI, but the denial is global: camera, microphone, location and
    // notifications of pages. Dictation and voice use the native microphone (microphone.rs), not the page's.
    let builder = tauri::Builder::default().on_permission_request(|_, kind| webview_permission(&kind));
    // Windows and Linux: a single copy of Juggl. Two copies steal each other's keeper connections and the bridge port.
    #[cfg(not(target_os = "macos"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        if let Some(w) = app.get_webview_window("main") { let _ = w.unminimize(); let _ = w.show(); let _ = w.set_focus(); }
    }));
    builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(|app, _sc, ev| {
            // snap (16/11): the snap shortcut (and its Alt version) is not the viewer's Esc
            if snap::handle(app, _sc, ev.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed) { return; }
            // Esc with the viewer in front: close it from here, without going through the page (on another thread: no blocking)
            if ev.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed { let a = app.clone(); std::thread::spawn(move || { close_viewer_from(a); }); }
        }).build())
        .setup(|app| {
            // hook and assistant scripts: from the bundle to the Juggl folder (first launch and updates)
            profile::install_scripts(app.handle());
            snap::start(app.handle());   // snap (16/11): Mac only, cleanup of old snaps and global shortcut
            // account and license (license.rs): renewal at startup and every 6 hours. Without a valid license the main window
            // only shows "Entra in Juggl"; when it unlocks, whatever was left stopped at startup starts again
            let _ = license::ON_UNLOCK.set(|h| {
                if profile::needs_welcome() {
                    if let Some(w) = h.get_webview_window("main") { let _ = w.hide(); }
                    let _ = create_welcome(h, "");
                }
                run_guarded("ripristino", || restore_hosted(h));
                run_guarded("juggl", || start_juggl_chat(h));
                let _ = h.emit("cambiato", ());
            });
            license::start(app.handle().clone());
            support::start();   // help and reports (17/11): report states at startup and every 6 hours, only if open
            // first launch: welcome window instead of the main window (after signing in with the account)
            if profile::needs_welcome() && license::is_valid() {
                if let Some(w) = app.get_webview_window("main") { let _ = w.hide(); }
                let _ = create_welcome(app.handle(), "");
            }
            updates::start(app.handle().clone());
            // commands (13/11): list of Claude Code commands ready in advance for the folders of open chats
            commands::prepare(list_sessions_with_pid().iter().filter(|v| matches!(str_of(v, "vista"), "lavora" | "aspetta" | "bloccata" | "ferma")).map(|v| str_of(v, "cwd").to_string()).filter(|c| !c.is_empty()).collect());
            search_index::start();   // search index: built in the background the first time, then a round every 2 minutes
            // native Esc: on macOS the WebView sometimes does not receive the Esc key, the menu item does
            // Mac only: on Windows and Linux the app menu would become a "Juggl Modifica Vista Finestra" bar above
            // every window (the page already handles the shortcuts)
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
                let esc = MenuItemBuilder::with_id("esc", "Chiudi anteprima").accelerator("Escape").build(app)?;
                // navigate (16/11): history of opened chats (like Safari and Finder) and neighboring chats in the list order.
                // In the menu, so the keys arrive even while typing in the input box (⌘[ does not indent).
                let back = MenuItemBuilder::with_id("naviga_indietro", "Chat precedente (cronologia)").accelerator("CmdOrCtrl+BracketLeft").build(app)?;
                let forward = MenuItemBuilder::with_id("naviga_avanti", "Chat successiva (cronologia)").accelerator("CmdOrCtrl+BracketRight").build(app)?;
                let next_item = MenuItemBuilder::with_id("naviga_succ", "Chat successiva nell'elenco").accelerator("Ctrl+Tab").build(app)?;
                let prev_item = MenuItemBuilder::with_id("naviga_prec", "Chat precedente nell'elenco").accelerator("Ctrl+Shift+Tab").build(app)?;
                let view_menu = SubmenuBuilder::new(app, "Vista").item(&back).item(&forward).separator().item(&next_item).item(&prev_item).separator().item(&esc).build()?;
                // Cmd+Q closes only the app: sessions stay alive in their keepers. "Esci da tutto" stops them too.
                let quit_all = MenuItemBuilder::with_id("esci_tutto", "Esci da tutto e ferma le sessioni").build(app)?;
                let settings_item = MenuItemBuilder::with_id("impostazioni", "Impostazioni…").accelerator("CmdOrCtrl+,").build(app)?;
                let updates_item = MenuItemBuilder::with_id("aggiornamenti", "Cerca aggiornamenti…").build(app)?;
                let import_item = MenuItemBuilder::with_id("importa", "Importa progetti…").build(app)?;
                let app_menu = SubmenuBuilder::new(app, "Juggl").about(None).separator().item(&settings_item).item(&updates_item).item(&import_item).separator().hide().hide_others().separator().item(&quit_all).quit().build()?;
                let edit_menu = SubmenuBuilder::new(app, "Modifica").undo().redo().separator().cut().copy().paste().select_all().build()?;
                let window_menu = SubmenuBuilder::new(app, "Finestra").minimize().maximize().separator().close_window().build()?;
                let menu = MenuBuilder::new(app).items(&[&app_menu, &edit_menu, &view_menu, &window_menu]).build()?;
                app.set_menu(menu)?;
                app.on_menu_event(|app, ev| { if ev.id().as_ref() == "esci_tutto" { hosted::quit_all(app.clone()); return; }
                    if let Some(target) = ev.id().as_ref().strip_prefix("naviga_") { let _ = app.emit_to("main", "naviga", target.to_string()); return; }
                    if ev.id().as_ref() == "importa" { let _ = create_welcome(app, "importa"); return; }
                    if ev.id().as_ref() == "impostazioni" || ev.id().as_ref() == "aggiornamenti" {
                        if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.set_focus(); }
                        let _ = app.emit_to("main", ev.id().as_ref(), ()); return;
                    } if ev.id().as_ref() == "esc" {
                    let viewer_in_front = app.get_webview_window("visore").and_then(|w| w.is_focused().ok()).unwrap_or(false);
                    if viewer_in_front { let a = app.clone(); std::thread::spawn(move || { close_viewer_from(a); }); } else { let _ = app.emit("esc", ()); }
                } });
            }
            // viewer ready in advance (hidden): the first image opens with no wait
            { let a = app.handle().clone(); std::thread::spawn(move || { std::thread::sleep(Duration::from_secs(3)); let _ = create_viewer(&a, false); }); }
            // a state file changes (a session hook) -> the UI updates right away
            let h = app.handle().clone();
            let dir = base_dir().join("stato");
            let _ = fs::create_dir_all(&dir);
            // only session files (<id>.json), not temp files, locks, state files of other features or logs; and a
            // burst of writes (hook, autopilot, requests) becomes a single "cambiato" every 400 ms
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            std::thread::spawn(move || {
                while rx.recv().is_ok() {
                    std::thread::sleep(Duration::from_millis(400));
                    while rx.try_recv().is_ok() {}
                    let _ = h.emit("cambiato", ());
                }
            });
            let mut w = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
                let Ok(ev) = r else { return };
                let is_session_file = ev.paths.iter().any(|p| {
                    let n = p.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
                    n.strip_suffix(".json").map(storage::is_valid_id).unwrap_or(false)
                });
                if is_session_file { let _ = tx.send(()); }
            })?;
            w.watch(&dir, RecursiveMode::NonRecursive)?;
            app.manage(std::sync::Mutex::new(w));
            // chats on screen (active column and side-by-side columns): as soon as a transcript grows, the UI reloads
            // only that column (before it was every 3 s)
            {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    // per session: transcript, signature (size, date) and when it was last looked up
                    let mut watched: HashMap<String, (Option<PathBuf>, (u64, u128), f64)> = HashMap::new();
                    loop {
                        std::thread::sleep(Duration::from_millis(250));
                        let mut ids: Vec<String> = autopilot::WATCHED_ALL.lock().unwrap().clone();
                        let g = autopilot::WATCHED.lock().unwrap().clone();
                        if !g.is_empty() && !ids.contains(&g) { ids.push(g); }
                        ids.retain(|x| !x.is_empty() && !x.starts_with("locale:"));
                        watched.retain(|k, _| ids.contains(k));
                        for id in &ids {
                            let e = watched.entry(id.clone()).or_insert((None, (0, 0), 0.0));
                            // transcript not found yet (session just born): retry every 5 s
                            if e.0.is_none() && unix_now() - e.2 > 5.0 { e.0 = find_session(id).and_then(|v| find_transcript(&v)); e.2 = unix_now(); }
                            let Some(p) = &e.0 else { continue };
                            let Ok(m) = fs::metadata(p) else { continue };
                            let f = (m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis()).unwrap_or(0));
                            if e.1 != (0, 0) && f != e.1 { let _ = h.emit("transcript", id.clone()); }
                            e.1 = f;
                        }
                    }
                });
            }
            // bridge for the iPhone: same events as the Mac UI
            let _ = APP.set(app.handle().clone());
            // requests: on first launch derive them from the latest conversations
            requests::populate();
            // hosted sessions stopped by closing the app restart by themselves, with the whole conversation
            {
                let h = app.handle().clone();
                // keeper died unexpectedly (rare): the session restarts by itself, with the interrupted turn to continue
                let _ = hosted::RESUME.set(|h, id| { if let Err(e) = resume_hosted(h, id) { log_juggl(&format!("{id}: non ripresa dopo la caduta del custode: {e}")); } });
                std::thread::spawn(move || {
                    // first of all reconnect to the sessions still alive in the keepers (they must be neither stopped nor resumed)
                    run_guarded("permessi-file", || storage::close_data_dir(&base_dir()));
                    run_guarded("custode", || { hosted::reattach_all(&h); });
                    std::thread::sleep(Duration::from_secs(2));
                    run_guarded("ripristino", || restore_hosted(&h));
                    // Juggl, the general assistant: always on, pinned at the top
                    run_guarded("juggl", || start_juggl_chat(&h));
                    // new user: projects are derived from their folders (git repos, recent chats), only once
                    run_guarded("progetti-auto", import::derive_at_startup);
                    let _ = h.emit("cambiato", ());
                });
            }
            bridge::start();
            // list (15/11): automatic icon for projects without a logo (project file, then website), a round every 30 min
            logos::start();
            {
                use tauri::Listener;
                app.listen_any("cambiato", |_| bridge::broadcast("cambiato", "{}"));
                app.listen_any("parziale", |ev| bridge::broadcast("parziale", ev.payload()));
            }
            // Claude limits of the connected account, every 2 minutes (quota.rs); alerts at 50/80/95%
            std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(5)); loop {
                // "too many requests" from Anthropic: wait 10 minutes (the data stays good for 20 minutes)
                let mut wait_secs = 120;
                run_guarded("limiti", || match quota::refresh_from_server() { Ok(p) => quota::notify_thresholds(p), Err(e) => { if e.to_lowercase().contains("rate limit") { wait_secs = 600; } log_juggl(&format!("limiti: {e}")) } });
                // usage: chats that burn a lot, automatic pause (off by default), evening summary
                run_guarded("consumi", || {
                    let mut ss = list_sessions(); ss.extend(archived_sessions());
                    let (alerts, paused) = quota::check(ss, &session_care::settings());
                    for a in alerts {
                        if let Some(app) = APP.get() { let _ = app.emit("consumo-alto", a.clone()); }
                        bridge::broadcast("consumo-alto", &a.to_string());
                    }
                    for id in paused {
                        log_juggl(&format!("{id}: pausa automatica (limite settimanale)"));
                        let _ = hosted::interrupt(&id);
                        std::thread::sleep(Duration::from_millis(800));
                        session_care::hibernate(&id, "pausa automatica: limite settimanale");
                    }
                    // automatic Eco mode (only if allowed): turned on when the limit runs too fast (eco.rs)
                    let mut ss2 = list_sessions(); ss2.extend(archived_sessions());
                    if let Some(t) = eco::check_auto(&quota::analysis(ss2)) {
                        session_care::record(&t, "Chat non fissate su Sonnet, ragionamento medio, risposte brevi. La spegni dalla vista Consumi.", "", "quota");
                        session_care::notify(&t, "Chat non fissate su Sonnet, ragionamento medio, risposte brevi.");
                    }
                    if let Some(app) = APP.get() { let _ = app.emit("cambiato", ()); }
                });
                std::thread::sleep(Duration::from_secs(wait_secs));
            } });
            // Claude account switch: hosted sessions restart with the new one (src/claude_account.rs)
            {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    let mut g = claude_account::AccountGuard::default();
                    loop { run_guarded("account", || g.tick(&h)); std::thread::sleep(Duration::from_secs(15)); }
                });
            }
            // keeper: memory and hibernation every 30 seconds
            let h2 = app.handle().clone();
            std::thread::spawn(|| loop { run_guarded("impronta", session_care::refresh_footprint); std::thread::sleep(Duration::from_secs(20)); });
            // Claude weekly quota: one point per hour in the history, transcript spend ready in advance
            std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(20)); let mut round = 0u32; loop {
                run_guarded("quota", quota::record_history);
                if round % 3 == 0 { run_guarded("quota-spesa", || { let _ = consumi_analisi(); }); }
                round += 1; std::thread::sleep(Duration::from_secs(300));
            } });
            // alerts and autopilot
            let h4 = app.handle().clone();
            std::thread::spawn(move || {
                let mut sent = autopilot::Sentinel::new();
                // automatic coordinator (12/11): same list as the Sentinel, a coordinator round right after
                loop { let ss = list_sessions(); run_guarded("sentinella", || sent.tick(&h4, &ss)); run_guarded("regia", || coordinator::tick(&h4, &ss)); run_guarded("live", || live::tick(&ss)); run_guarded("modifiche", || live_edits::tick(&ss)); run_guarded("compattazione", || compaction::tick(&ss)); std::thread::sleep(Duration::from_secs(3)); }
            });
            let h3 = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(3));
                run_guarded("coda", || if deliver_queues() { let _ = h3.emit("cambiato", ()); });
            });
            std::thread::spawn(move || {
                let mut g = session_care::Watchdog::new();
                let mut last_cleanup = 0.0;
                loop {
                    std::thread::sleep(Duration::from_secs(30));
                    run_guarded("guardiano", || {
                        let ss = list_sessions_with_pid();
                        if let Some(ev) = g.tick(&ss) { let _ = h2.emit("allarme", ev); let _ = h2.emit("cambiato", ()); }
                        if archive_old(&ss) > 0 { let _ = h2.emit("cambiato", ()); }
                    });
                    if unix_now() - last_cleanup > 3600.0 { last_cleanup = unix_now(); run_guarded("pulizia", clean_up_states); }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![tavolo, snap::scatto_stato, snap::scatto_imposta, snap::scatto_permesso_chiedi, snap::scatto_ora, snap::scatto_apri_impostazioni, summaries::riassunto_azioni, media_recenti, ponte_info, ok_voci, ok_azione, ok_manda, richieste_ai, richiesta_ai_azione, richieste_ai_rivaluta, elimina_chat, non_serve, nota_spostato, regola, regola_locale, consumi, consumi_analisi, notifiche, notifiche_lette, cerca_ovunque, cerca_tutto, cerca_in_sessione, indice_stato, sessione_chiusa, riprendi_chiusa, archivia, riprendi_archiviata, togli_archivio, imposta_archivio_auto, stato_mac, pulisci_mac, esci_app, mac_scansione, mac_ultima, mac_libera, mac_libera_auto, mac_ram, mac_storico, mac_ripristina, modelli_locali, cancella_chat_locali, schermo_agente, imposta_pausa_auto, eco_imposta, nuova_locale, rispondi_scelta, pilota_annulla, pilota_imposta, pilota_ora, guardando, guardando_tutte, invia_ora_dalla_coda, sessione, modifiche_pezzi, segna_avvio, sottoagente, ferma_sottoagente, rinomina, fissa_sessione, apri, invia, togli_dalla_coda, interrompi, apri_link, apri_file, progetti, iberna, chiudi, riprendi, nuova, copia_file, copia_dati, info_file, copia_testo, esc_cattura, apri_visore, visore_dati, visore_tutti, vai_al_media, apri_in_mostra, chiudi_visore, tieni_allegati, ultima_notifica, esiste, leggi_documento, anteprima_documento, tipo_documento, testo_documento, salva_allegato, imposta_logo, profile::profilo, profile::profilo_imposta, profile::controlli, profile::hook_installa, profile::hook_togli, profile::installa_strumenti_apple, benvenuto_apri, benvenuto_fine, importa_chiudi, import::importa_cerca, import::importa_ricava, import::importa_aggiungi_radice, import::importa_crea, prova_notifica, updates::aggiornamento_cerca, updates::aggiornamento_stato, updates::aggiornamento_installa, license::licenza_stato, license::licenza_rinnova, license::account_codice, license::account_entra, license::account_esci, wa::wa_dati, wa::wa_sposta, wa::wa_livello, wa::wa_fatto, wa::wa_consegna, wa::wa_bozza, wa::wa_silenzia, wa::wa_apri, wa::wa_nuova_bozza, wa::wa_trascrivi, microphone::mic_avvia, microphone::mic_riprova, microphone::mic_ferma, microphone::mic_annulla, microphone::mic_stato, microphone::mic_scarica_modello, microphone::mic_impostazioni, microphone::mic_imposta_qualita, coordinator::regia_imposta, permissions::permessi_stato, permissions::permessi_imposta, permissions::permessi_eccezione, permissions::permessi_chat, permissions::permessi_rispondi, permissions::permessi_avviso_visto, permissions::permessi_pilota, permissions::permessi_pilota_annulla, open_as_chat::sottoagente_in_chat, engines::motori_stato, engines::motori_entra_codex, live_edits::modifiche_annulla, pc_local_engine::locale_stato, pc_local_engine::locale_ferma, fallback::continua_su_codex, claude_login::accesso_avvia, claude_login::accesso_codice, claude_login::accesso_annulla, claude_login::accesso_esci, claude_login::accesso_stato, claude_login::accesso_dettagli, commands::comandi_elenco, commands::comando_esegui, commands::modelli_claude, logos::progetto_colore, logos::progetto_logo_sito, logos::progetto_logo_iniziali, logos::loghi_online_imposta, logos::progetto_cerca_online, requests::richieste_pulizia_imposta, segna_letta, unisci_chat, separa_chat, mac_spazio, aiutanti_stato, aiutanti_imposta, consumi_dettaglio, compattazione_stato, compattazione_imposta, compattazione_chat, compattazione_ora, requests::richieste_pulizia_estesa_imposta, requests::richieste_annulla_pulizia, requests::richieste_pulizia_stato, requests::richieste_campione_pulizia, settings_chat::regolazioni_stato, settings_chat::regolazioni_registra, settings_chat::regolazioni_capisci, support::aiuto_storia, support::aiuto_chiedi, support::aiuto_anteprima, support::aiuto_invia, support::aiuto_stati, support::aiuto_svuota, support::aiuto_salva_immagine])
        .build(tauri::generate_context!())
        .expect("errore avviando Juggl")
        .run(|_, ev| { if let tauri::RunEvent::Exit = ev { hosted::shut_all(); } });
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    fn test_file(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("juggl-coda-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d.join("02ced450-9036-453a-897c-818b366c2ea0.json")
    }

    fn read_test_file(p: &Path) -> Vec<Value> { load_json(p).and_then(|v| v.as_array().cloned()).unwrap_or_default() }

    /// iPhone, external messages and autopilot enqueue while delivery empties the queue: no message lost, none sent twice.
    #[test]
    fn no_message_lost_during_delivery() {
        let p = test_file("parallelo");
        let writers: Vec<_> = (0..8).map(|k| {
            let p = p.clone();
            std::thread::spawn(move || for i in 0..40 { enqueue_at(&p, &format!("msg {k}-{i}"), ""); })
        }).collect();
        let pc = p.clone();
        let delivery = std::thread::spawn(move || {
            let mut delivered: Vec<String> = vec![];
            for _ in 0..200 {
                // like deliver_queues: reads, "sends" (a bit of waiting), removes the sent ones
                with_queue_at(&pc, |c| {
                    if c.is_empty() { return; }
                    std::thread::sleep(Duration::from_millis(2));
                    delivered.extend(c.iter().map(|m| str_of(m, "testo").to_string()));
                    c.clear();
                });
                std::thread::sleep(Duration::from_millis(1));
            }
            delivered
        });
        for s in writers { s.join().unwrap(); }
        let mut all = delivery.join().unwrap();
        all.extend(read_test_file(&p).iter().map(|m| str_of(m, "testo").to_string()));
        let n = all.len();
        all.sort(); all.dedup();
        assert_eq!(n, 320, "messaggi persi o doppi");
        assert_eq!(all.len(), 320, "messaggi mandati due volte");
        let _ = fs::remove_dir_all(p.parent().unwrap());
    }

    /// The message that arrives while delivery is writing to the terminal is not deleted (it was bug B8).
    #[test]
    fn message_arriving_during_send_stays() {
        let p = test_file("durante");
        enqueue_at(&p, "primo", "");
        let pc = p.clone();
        let delivery = std::thread::spawn(move || with_queue_at(&pc, |c| {
            let sent: Vec<String> = c.iter().map(|m| str_of(m, "testo").to_string()).collect();
            std::thread::sleep(Duration::from_millis(400));   // slow send (cmux, images)
            c.clear();
            sent
        }));
        std::thread::sleep(Duration::from_millis(100));
        enqueue_at(&p, "arrivato dopo", "");   // waits for the lock, then gets appended
        assert_eq!(delivery.join().unwrap(), vec!["primo".to_string()]);
        let remaining = read_test_file(&p);
        assert_eq!(remaining.len(), 1);
        assert_eq!(str_of(&remaining[0], "testo"), "arrivato dopo");
        assert!(!str_of(&remaining[0], "id").is_empty(), "ogni messaggio ha il suo id");
        let _ = fs::remove_dir_all(p.parent().unwrap());
    }

    /// Removing a message by hand keeps the others; empty queue = file removed.
    #[test]
    fn removing_keeps_the_others() {
        let p = test_file("togli");
        for t in ["a", "b", "c"] { enqueue_at(&p, t, ""); }
        with_queue_at(&p, |c| { c.remove(1); });
        assert_eq!(read_test_file(&p).iter().map(|m| str_of(m, "testo")).collect::<Vec<_>>(), vec!["a", "c"]);
        with_queue_at(&p, |c| c.clear());
        assert!(!p.exists());
        let _ = fs::remove_dir_all(p.parent().unwrap());
    }

    /// Old messages without id: the key is time + text.
    #[test]
    fn key_of_old_messages() {
        assert_eq!(queue_key(&json!({"testo": "x", "t": 5.0})), "5|x");
        assert_eq!(queue_key(&json!({"id": "m1", "testo": "x"})), "m1");
    }

    /// Mid-turn message: "delivered" already from the enqueue, a single bubble whether it is read in the turn or sent later.
    #[test]
    fn enqueue_counts_as_delivered() {
        let d = test_file("enqueue").parent().unwrap().to_path_buf();
        let p = d.join("t.jsonl");
        let r = |x: Value| x.to_string();
        let lines = [
            r(json!({"type": "user", "timestamp": "2026-11-08T10:00:00Z", "message": {"content": "primo"}})),
            r(json!({"type": "queue-operation", "operation": "enqueue", "timestamp": "2026-11-08T10:00:05Z", "content": "a meta turno"})),
            r(json!({"type": "queue-operation", "operation": "enqueue", "timestamp": "2026-11-08T10:00:06Z", "content": "dopo il turno"})),
            r(json!({"type": "queue-operation", "operation": "enqueue", "timestamp": "2026-11-08T10:00:07Z", "content": "ripreso"})),
            r(json!({"type": "queue-operation", "operation": "remove", "reason": "absorbed_mid_turn", "timestamp": "2026-11-08T10:01:30Z", "content": "a meta turno"})),
            r(json!({"type": "assistant", "timestamp": "2026-11-08T10:02:00Z", "message": {"content": [{"type": "text", "text": "fine turno"}]}})),
            r(json!({"type": "queue-operation", "operation": "dequeue", "timestamp": "2026-11-08T10:02:01Z"})),
            r(json!({"type": "user", "timestamp": "2026-11-08T10:02:01Z", "message": {"content": "dopo il turno"}})),
        ];
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        let yours = |v: &Value| v["eventi"].as_array().unwrap().iter().filter(|e| str_of(e, "tipo") == "tu").map(|e| str_of(e, "testo").to_string()).collect::<Vec<_>>();
        // before Claude reads them: already on screen as yours
        let mid_file = d.join("m.jsonl"); fs::write(&mid_file, lines[..4].join("\n") + "\n").unwrap();
        assert_eq!(yours(&detail_tail(Some(mid_file), false, 1_000_000, "")), ["primo", "a meta turno", "dopo il turno", "ripreso"]);
        // "ripreso" stays (no popAll); "dopo il turno" moves after the answer, with no duplicates
        assert_eq!(yours(&detail_tail(Some(p.clone()), false, 1_000_000, "")), ["primo", "a meta turno", "ripreso", "dopo il turno"]);
        let mut with_pop = lines.to_vec(); with_pop.insert(4, r(json!({"type": "queue-operation", "operation": "popAll", "timestamp": "2026-11-08T10:00:08Z"})));
        fs::write(&p, with_pop.join("\n") + "\n").unwrap();
        // taken back to edit them: they disappear; the "remove" after popAll finds nothing to move
        assert_eq!(yours(&detail_tail(Some(p), false, 1_000_000, "")), ["primo", "a meta turno", "dopo il turno"]);
        // sessions inside the app: enqueue without text = already reached Claude, waits for the next step
        let q = d.join("q.jsonl");
        let hosted_lines = [r(json!({"type": "queue-operation", "operation": "enqueue", "timestamp": "2026-11-08T10:00:05Z"})),
                   r(json!({"type": "queue-operation", "operation": "dequeue", "timestamp": "2026-11-08T10:00:05Z"})),
                   r(json!({"type": "queue-operation", "operation": "enqueue", "timestamp": "2026-11-08T10:00:09Z"}))];
        fs::write(&q, hosted_lines.join("\n") + "\n").unwrap();
        assert_eq!(detail_tail(Some(q.clone()), false, 1_000_000, "")["ricevuti_in_coda"], json!(1));
        let mut read_lines = hosted_lines.to_vec(); read_lines.push(r(json!({"type": "queue-operation", "operation": "remove", "reason": "absorbed_mid_turn", "timestamp": "2026-11-08T10:00:30Z", "content": "zebra"})));
        fs::write(&q, read_lines.join("\n") + "\n").unwrap();
        let v = detail_tail(Some(q), false, 1_000_000, "");
        assert_eq!(v["ricevuti_in_coda"], json!(0));
        assert_eq!(yours(&v), ["zebra"]);
    }
}

#[cfg(test)]
mod message_timings {
    /// fast (15/11): where the time of a big chat's messages goes. Read only.
    /// JUGGL_TRANSCRIPT=<file .jsonl> cargo test --release message_timings -- --ignored --nocapture
    #[test]
    #[ignore]
    fn timings() {
        let p = std::path::PathBuf::from(std::env::var("JUGGL_TRANSCRIPT").expect("JUGGL_TRANSCRIPT"));
        for round in 0..6 {
            let t = std::time::Instant::now();
            let (lines, _) = super::tail_file(&p, super::FIRST_READ);
            let t_read = t.elapsed();
            let n = lines.iter().filter(|r| serde_json::from_str::<serde_json::Value>(r).is_ok()).count();
            let t_json = t.elapsed();
            let since = std::env::var("JUGGL_DAL").unwrap_or_default();
            let before: usize = std::env::var("JUGGL_PRIMA").ok().and_then(|x| x.parse().ok()).unwrap_or(6);
            let d = if since.is_empty() { super::detail_since_n(Some(p.clone()), false, "", 6, super::FIRST_READ) } else { super::detail_since_n(Some(p.clone()), false, &since, before, 60_000_000) };
            println!("TEMPI giro {round}: lettura {:?}, json {:?} ({n} righe), dettaglio intero {:?}, {} eventi", t_read, t_json - t_read, t.elapsed() - t_json, d["eventi"].as_array().map(|a| a.len()).unwrap_or(0));
        }
    }
}

#[cfg(test)]
mod desk_timings {
    /// Measures the parts of dati_tavolo on the first round (as at startup) and on the second. Read only, on a copy of the data:
    /// JUGGL_BASE=<copy> cargo test --release desk_timings -- --ignored --nocapture
    #[test]
    #[ignore]
    fn timings() {
        for round in 0..2 {
            let mut t = std::time::Instant::now();
            let mut p = |n: &str, t: &mut std::time::Instant| { println!("TEMPI giro {round} {n}: {:?}", t.elapsed()); *t = std::time::Instant::now(); };
            let mut ss = super::list_sessions(); p("sessioni", &mut t);
            ss.extend(super::local_models::as_sessions()); p("locali", &mut t);
            let _ = super::requests::counts(); p("richieste conteggi", &mut t);
            super::in_progress::enrich(&mut ss); p("incorso arricchisci", &mut t);
            let _ = super::quota::valid_limits(); p("limiti", &mut t);
            let _ = super::eco::status(); p("eco", &mut t);
            let _ = super::approvals::n_ok(&ss); p("ok_n", &mut t);
            let _ = super::coordinator::for_list(&ss); p("regie", &mut t);
            let _ = super::archived_sessions(); p("archiviate", &mut t);
            let _ = super::user_at_mac(); p("al_mac", &mut t);
            let _ = super::session_care::mac_memory(); p("memoria_mac", &mut t);
            let _ = super::session_care::free_disk_gb(); p("disco", &mut t);
            let _ = super::profile::enabled_map(); p("profilo", &mut t);
            let _ = super::juggl_id(); p("juggl_id", &mut t);
            println!("TEMPI giro {round} sessioni {}", ss.len());
        }
    }
}

#[cfg(test)]
mod session_timings {
    /// Measures the parts of `sessione`: cargo test --release session_timings -- --ignored --nocapture (ID in JUGGL_ID)
    #[test]
    #[ignore]
    fn timings() {
        let id = std::env::var("JUGGL_ID").unwrap_or_default();
        let t = std::time::Instant::now();
        let v = super::find_session(&id).expect("sessione");
        println!("TEMPI trova {:?}", t.elapsed());
        let t = std::time::Instant::now(); let _ = super::session_detail(&v); println!("TEMPI dettaglio {:?}", t.elapsed());
        let t = std::time::Instant::now(); let _ = super::session_detail(&v); println!("TEMPI dettaglio2 {:?}", t.elapsed());
        let t = std::time::Instant::now(); let _ = super::git(super::str_of(&v, "cwd")); println!("TEMPI git {:?}", t.elapsed());
        let t = std::time::Instant::now(); let _ = super::requests::of_session(&id); println!("TEMPI richieste {:?}", t.elapsed());
        let t = std::time::Instant::now(); let _ = super::approvals::of_session(&id); println!("TEMPI ok {:?}", t.elapsed());
        let t = std::time::Instant::now(); let _ = super::list_subagents(&v); println!("TEMPI sotto {:?}", t.elapsed());
        let t = std::time::Instant::now(); let _ = super::read_queue(&id); println!("TEMPI coda {:?}", t.elapsed());
        let t = std::time::Instant::now(); let _ = super::sessione(id.clone(), None, None); println!("TEMPI tutto {:?}", t.elapsed());
        let t = std::time::Instant::now(); let _ = super::sessione(id.clone(), None, None); println!("TEMPI tutto2 {:?}", t.elapsed());
    }
}

#[cfg(test)]
mod queued_photo_tests {
    use serde_json::json;
    /// Real transcript (FOTO_TRANSCRIPT): photos sent while it was working show up as your galleries before the text.
    #[test]
    #[ignore]
    fn real_queued_photos() {
        let Ok(p) = std::env::var("FOTO_TRANSCRIPT") else { return };
        let d = super::detail_since(Some(std::path::PathBuf::from(p)), false, "2026-11-09T13:50:00");
        for (i, e) in d["eventi"].as_array().unwrap().iter().enumerate() {
            let kind = e["tipo"].as_str().unwrap_or("");
            if kind == "tu" || (kind == "media" && e["da"] == "tu") {
                println!("{i} {} {} {} {}", e["t"].as_str().unwrap_or(""), kind, e["dati"].as_array().map(|a| a.len()).unwrap_or(0),
                    e["testo"].as_str().unwrap_or("").chars().take(50).collect::<String>());
            }
        }
    }
    #[test]
    fn moves_before_message() {
        let mut v = vec![
            json!({"tipo": "tu", "testo": "rimane così", "t": "2026-11-09T19:54:53.743Z", "durante": true}),
            json!({"tipo": "media", "da": "tu", "dati": ["data:image/png;base64,AAAA"], "file": [], "t": "2026-11-09T19:54:36.488Z", "coda_k": "rimane così"}),
            json!({"tipo": "agente", "testo": "ok", "t": "2026-11-09T19:55:00Z"}),
            json!({"tipo": "media", "da": "tu", "dati": ["data:image/png;base64,BBBB"], "file": [], "t": "2026-11-09T19:56:00Z", "coda_k": "Guarda l'immagine allegata."}),
            json!({"tipo": "media", "da": "tu", "dati": ["data:image/png;base64,BBBB"], "file": [], "t": "2026-11-09T19:56:05Z"}),
            json!({"tipo": "tu", "testo": "Guarda l'immagine allegata.", "t": "2026-11-09T19:56:05Z"}),
        ];
        super::move_queued_photos(&mut v);
        let kinds: Vec<String> = v.iter().map(|e| format!("{}{}", e["tipo"].as_str().unwrap(), e["dati"].as_array().map(|a| a.len().to_string()).unwrap_or_default())).collect();
        assert_eq!(kinds, vec!["media1", "tu", "agente", "media1", "tu"]);
        assert!(v.iter().all(|e| e.get("coda_k").is_none()));
        assert_eq!(v[0]["t"], json!("2026-11-09T19:54:53.743Z"));
    }
    #[test]
    fn attachments_in_text() {
        assert_eq!(super::with_attachments("ciao", &["/a/b.jpg".into()]), "ciao\n\nAllegato: /a/b.jpg");
        assert_eq!(super::with_attachments("", &["/a/b.jpg".into()]), "Allegato: /a/b.jpg");
        assert_eq!(super::with_attachments("x\n\nAllegato: /a/b.jpg", &["/a/b.jpg".into()]), "x\n\nAllegato: /a/b.jpg");
    }
}

#[cfg(test)]
mod origin_name_tests {
    use serde_json::json;
    #[test]
    fn name_from_start_folder() {
        let v = json!({"sessione": "x", "cwd": "/a/CLIENTI/ROSSI/security-audit/2026-10-03", "cwd_origine": "/b/CLIENTI/ROSSI"});
        assert_eq!(super::name_of(&v, &json!({})), "ROSSI");
        let w = json!({"sessione": "x", "cwd": "/a/PROGETTO"});
        assert_eq!(super::name_of(&w, &json!({})), "PROGETTO");
    }
}

#[cfg(test)]
mod task_outcome_tests {
    use super::*;
    #[test]
    fn stopped_with_taskstop() {
        let d = std::env::temp_dir().join(format!("juggl-esiti-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        let t = d.join("s.jsonl");
        let n = |id: &str, st: &str, ts: &str| format!("{{\"type\":\"user\",\"timestamp\":\"{ts}\",\"message\":{{\"content\":\"<task-notification>\\n<task-id>{id}</task-id>\\n<output-file>/x</output-file>\\n<status>{st}</status>\\n<summary>Agent stopped</summary>\\n</task-notification>\"}}}}\n");
        fs::write(&t, n("a939f06e4d07e617e", "killed", "2026-11-10T00:52:00.000Z")).unwrap();
        let e = task_outcomes(&t);
        assert_eq!(e["a939f06e4d07e617e"].0, "killed");
        assert!(e["a939f06e4d07e617e"].1 > 1.79e9);
        // the file grows: only the new part is read, the last outcome wins
        use std::io::Write;
        let mut f = fs::OpenOptions::new().append(true).open(&t).unwrap();
        f.write_all(n("a939f06e4d07e617e", "completed", "2026-11-10T01:00:00.000Z").as_bytes()).unwrap();
        f.write_all(b"{\"type\":\"user\",\"riga a meta").unwrap();
        let e = task_outcomes(&t);
        assert_eq!(e["a939f06e4d07e617e"].0, "completed");
        let _ = fs::remove_dir_all(&d);
    }

    /// fast (14/11): big file after a restart: only the last 8 MB, in chunks, then the new part; truncated = from scratch.
    #[test]
    fn big_file_only_tail() {
        let d = std::env::temp_dir().join(format!("juggl-esiti-grande-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        let t = d.join("g.jsonl");
        let n = |id: &str, st: &str| format!("{{\"type\":\"user\",\"timestamp\":\"2026-11-10T00:52:00.000Z\",\"message\":{{\"content\":\"<task-notification><task-id>{id}</task-id><status>{st}</status></task-notification>\"}}}}\n");
        let filler = format!("{{\"type\":\"user\",\"x\":\"{}\"}}\n", "z".repeat(1000));
        let mut text = n("vecchio", "killed");
        while text.len() < 10 * 1024 * 1024 { text.push_str(&filler); }
        text.push_str(&"{\"type\":\"user\",\"lunga\":\"".to_string()); text.push_str(&"y".repeat(5 * 1024 * 1024)); text.push_str("\"}\n");   // line longer than a chunk
        text.push_str(&n("recente", "failed"));
        for _ in 0..100 { text.push_str(&filler); }
        fs::write(&t, &text).unwrap();
        let e = task_outcomes(&t);
        assert!(e.get("vecchio").is_none(), "fuori dagli ultimi 8 MB: non letto");
        assert_eq!(e["recente"].0, "failed");
        use std::io::Write;
        fs::OpenOptions::new().append(true).open(&t).unwrap().write_all(n("nuovo", "completed").as_bytes()).unwrap();
        let e = task_outcomes(&t);
        assert_eq!((e["nuovo"].0.as_str(), e["recente"].0.as_str()), ("completed", "failed"));
        fs::write(&t, n("dopo", "killed")).unwrap();   // truncated
        let e = task_outcomes(&t);
        assert_eq!(e["dopo"].0, "killed");
        assert!(e.get("recente").is_none());
        let _ = fs::remove_dir_all(&d);
    }

    /// On the real print-monitor session (10/11): the agent stopped with TaskStop shows as interrupted.
    #[test]
    #[ignore]
    fn real_print_monitor_session() {
        let v = json!({"sessione": "199ff923-2ef6-4ec7-ae0b-b6d2146f7367"});
        let sa = list_subagents(&v);
        let x = sa.iter().find(|x| str_of(x, "id") == "a939f06e4d07e617e").expect("agente");
        println!("{x}");
        assert_eq!(str_of(x, "stato"), "interrotto");
        assert_eq!(str_of(x, "esito"), "fermato");
    }
}

// rename (14/11): the data folder for existing users (~/tavolo) and for new users (~/.juggl)
#[cfg(test)]
mod data_dir_tests {
    use super::*;

    fn temp_home(name: &str) -> PathBuf {
        let h = std::env::temp_dir().join(format!("juggl-prova-dati-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&h);
        fs::create_dir_all(&h).unwrap();
        h
    }

    #[test]
    fn existing_user_stays_on_tavolo() {
        for marker in ["ponte.json", "impostazioni.json", "stato"] {
            let h = temp_home(&format!("prima-{}", marker.replace('.', "-")));
            let v = h.join("tavolo");
            fs::create_dir_all(&v).unwrap();
            if marker == "stato" { fs::create_dir_all(v.join("stato")).unwrap(); } else { fs::write(v.join(marker), "{}").unwrap(); }
            assert_eq!(choose_data_dir(&h), v, "{marker}");
            let _ = fs::remove_dir_all(&h);
        }
    }

    #[test]
    fn new_user_on_juggl() {
        let h = temp_home("nuovo");
        assert_eq!(choose_data_dir(&h), h.join(".juggl"));
        // any ~/tavolo (a project with that name), without the Juggl files: it is not ours
        fs::create_dir_all(h.join("tavolo/disegni")).unwrap();
        fs::write(h.join("tavolo/note.txt"), "misure").unwrap();
        assert_eq!(choose_data_dir(&h), h.join(".juggl"));
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn juggl_base_always_wins() {
        // JUGGL_BASE is the test folder: the choice does not even look at it
        if let Ok(b) = std::env::var("JUGGL_BASE") { if Path::new(&b).is_absolute() { assert_eq!(data_dir(), PathBuf::from(b)); return; } }
        let d = data_dir();
        assert!(d == home().join("tavolo") || d == home().join(".juggl"), "{}", d.display());
        assert!(data_dir_short().starts_with("~/"), "{}", data_dir_short());
    }
}
