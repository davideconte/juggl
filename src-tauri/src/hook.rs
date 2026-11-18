// Claude Code and Codex hook inside Juggl: `Juggl.exe hook` reads the event JSON from stdin and updates
// ~/tavolo/stato/<session>.json. Same logic as `bin/tavolo hook` (Python), which stays on the Mac: on Windows Python
// is not guaranteed, so the hook lives in the program.
// Rule: the hook must never fail or slow down, every error is swallowed.
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const EVENTS: [&str; 5] = ["UserPromptSubmit", "Stop", "Notification", "SessionStart", "SessionEnd"];
const SHELL: [&str; 9] = ["sh", "bash", "zsh", "dash", "env", "cmd", "powershell", "pwsh", "conhost"];

fn now_secs() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

fn shorten(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= n { s } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

fn read_tail(p: &Path, max_bytes: u64) -> Vec<u8> {
    let Ok(mut f) = std::fs::File::open(p) else { return vec![] };
    let n = f.seek(SeekFrom::End(0)).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(n.saturating_sub(max_bytes)));
    let mut v = vec![]; let _ = f.read_to_end(&mut v); v
}

/// Last assistant text from the transcript (only if the hook does not pass it already).
fn last_assistant_message(tr: &str) -> String {
    if tr.is_empty() { return String::new(); }
    let b = read_tail(Path::new(tr), 200_000);
    for r in String::from_utf8_lossy(&b).lines().rev() {
        let Ok(d) = serde_json::from_str::<Value>(r) else { continue };
        let m = &d["message"];
        if d["type"] != "assistant" || m["role"] != "assistant" { continue; }
        let t = match &m["content"] {
            Value::Array(a) => a.iter().filter(|x| x["type"] == "text").filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join(" "),
            Value::String(s) => s.clone(),
            _ => String::new(),
        };
        if !t.trim().is_empty() { return t.trim().to_string(); }
    }
    String::new()
}

/// Session title from the transcript (manual rename or AI title), the last one wins.
fn transcript_title(tr: &str) -> Option<String> {
    if tr.is_empty() { return None; }
    let p = Path::new(tr);
    let n = std::fs::metadata(p).map(|m| m.len()).ok()?;
    let mut chunks = vec![];
    { let mut f = std::fs::File::open(p).ok()?; let mut v = vec![0u8; n.min(60_000) as usize]; let _ = f.read_exact(&mut v); chunks.push(v); }
    if n > 60_000 { chunks.push(read_tail(p, (n - 60_000).min(300_000))); }
    let mut t = None;
    for chunk in chunks {
        let s = String::from_utf8_lossy(&chunk);
        for key in ["\"customTitle\":\"", "\"aiTitle\":\""] {
            let mut pos = 0;
            while let Some(i) = s[pos..].find(key) {
                let start = pos + i + key.len() - 1;   // on the opening quote
                let mut end = start + 1; let b = s.as_bytes(); let mut esc = false;
                while end < b.len() { if esc { esc = false } else if b[end] == b'\\' { esc = true } else if b[end] == b'"' { break } end += 1; }
                if end < b.len() { if let Ok(x) = serde_json::from_str::<String>(&s[start..=end]) { t = Some(x); } }
                pos = end.min(s.len());
            }
        }
    }
    t
}

/// (pid of claude/codex, event time): walks up as long as intermediate shells are found.
fn hook_caller(t0: f64) -> (u32, f64) {
    let mut p = std::process::id();
    let mut when = t0;
    let Some((pp, _, started)) = info(p) else { return (0, t0) };
    p = pp; if let Some(a) = started { when = a; }
    for _ in 0..5 {
        let Some((pp, name, started)) = info(p) else { break };
        let n = name.rsplit(['/', '\\']).next().unwrap_or("").trim_start_matches('-').trim_end_matches(".exe").to_lowercase();
        if !SHELL.contains(&n.as_str()) { break; }
        p = pp; if let Some(a) = started { when = a; }
    }
    (p, when)
}

#[cfg(windows)]
fn info(pid: u32) -> Option<(u32, String, Option<f64>)> { crate::sys::process_info(pid) }

#[cfg(unix)]
fn info(pid: u32) -> Option<(u32, String, Option<f64>)> {
    let o = crate::new_command("ps").args(["-o", "ppid=,comm=", "-p", &pid.to_string()]).output().ok()?;
    let s = String::from_utf8_lossy(&o.stdout).to_string();
    let mut it = s.trim().splitn(2, char::is_whitespace);
    let pp = it.next()?.trim().parse().ok()?;
    Some((pp, it.next().unwrap_or("").trim().to_string(), None))
}

fn safe_file_name(sid: &str) -> Option<String> {
    let n: String = sid.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).collect();
    if n.trim_matches(|c| c == '.' || c == '_').is_empty() { None } else { Some(n) }
}

pub fn run(state_dir: PathBuf) {
    let t0 = now_secs();
    if std::env::var_os("TAVOLO_INTERNO").is_some() { return; }
    let mut text = String::new();
    if std::io::stdin().read_to_string(&mut text).is_err() { return; }
    let Ok(data) = serde_json::from_str::<Value>(if text.trim().is_empty() { "{}" } else { &text }) else { return };
    let s = |k: &str| data.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let sid = { let a = s("session_id"); if a.is_empty() { s("sessionId") } else { a } };
    let ev = { let a = s("hook_event_name"); if a.is_empty() { s("hookEventName") } else { a } };
    if sid.is_empty() || !EVENTS.contains(&ev.as_str()) { return; }
    if ev == "Notification" {
        let m = s("message");
        if !(m.to_lowercase().contains("permission") || s("notification_type") == "permission_prompt") { return; }
    }
    let Some(name) = safe_file_name(&sid) else { return };
    let _ = std::fs::create_dir_all(&state_dir);
    let p = state_dir.join(format!("{name}.json"));
    let tr = s("transcript_path");
    let transcript_exists = !tr.is_empty() && Path::new(&tr).exists();
    if !p.exists() {
        if ev == "SessionEnd" || ((ev == "Stop" || ev == "Notification") && !tr.is_empty() && !transcript_exists) { return; }
    } else if ev == "SessionEnd" && !transcript_exists { return; }
    let (pid, when) = hook_caller(t0);
    let agent = if tr.contains("codex") || std::env::var_os("CODEX_HOME").is_some() || data.get("turn_id").is_some() { "codex" } else { "claude" };
    let msg = if ev == "Stop" { let m = s("last_assistant_message"); if m.is_empty() { last_assistant_message(&tr) } else { m } } else { String::new() };
    let title = if ev == "Stop" || ev == "SessionStart" {
        let t = if tr.is_empty() { match crate::storage::read_json(&p) { crate::storage::ReadResult::Valida(v) => v["transcript"].as_str().unwrap_or("").to_string(), _ => String::new() } } else { tr.clone() };
        transcript_title(&t)
    } else { None };
    let cwd = s("cwd");
    let mut to_name: Option<String> = None;
    let _ = crate::storage::modify(&p, Some(json!({})), |v| {
        if !v.is_object() { *v = json!({}); }
        if v["ultimo_evento"].as_f64().unwrap_or(0.0) > when { return false; }   // arrives late
        let hosted = v["ospite"] == "tavolo" && pid > 0 && v["pid"].as_i64().unwrap_or(0) == pid as i64;
        if v["agente"].as_str().unwrap_or("").is_empty() { v["agente"] = json!(agent); }
        v["sessione"] = json!(sid);
        if !cwd.is_empty() { v["cwd"] = json!(cwd); }
        v["pid"] = json!(pid); v["agg"] = json!(now_secs()); v["ultimo_evento"] = json!(when);
        if !tr.is_empty() { v["transcript"] = json!(tr); }
        match ev.as_str() {
            "UserPromptSubmit" => {
                crate::worked::record_start(v, when);
                let pr = s("prompt");
                if !hosted { v["stato"] = json!("lavora"); v["da"] = json!(when); v["domanda"] = json!(""); }
                if !pr.trim_start().starts_with('<') {
                    v["chiesto"] = json!(shorten(&pr, 200));
                    // unnamed chat: at the first real message the small model gives it a short name (like bin/tavolo)
                    let is_empty_field = |k: &str| v[k].is_null() || v[k] == "" || v[k] == false;
                    if is_empty_field("nome_manuale") && is_empty_field("nome_chiesto") && is_empty_field("risposta") && pr.trim().chars().count() >= 8 && !pr.trim_start().starts_with('/') {
                        v["nome_chiesto"] = json!(now_secs()); to_name = Some(pr.clone());
                    }
                }
            }
            "Stop" => {
                crate::worked::record_end(v, when);
                if !hosted { v["stato"] = json!("aspetta"); v["da"] = json!(when); }
                v["risposta"] = json!(shorten(&msg, 300));
                if let Some(t) = &title { v["titolo"] = json!(t); }
            }
            "Notification" => {
                if hosted { return false; }
                v["stato"] = json!("bloccata"); v["da"] = json!(when); v["domanda"] = json!(shorten(&s("message"), 200));
            }
            "SessionStart" => {
                if !cwd.is_empty() && v["cwd_origine"].as_str().unwrap_or("").is_empty() && !cwd.contains("scratchpad") && cwd != "/" { v["cwd_origine"] = json!(cwd); }
                let st = v["stato"].as_str().unwrap_or("").to_string();
                if !hosted && ["", "ibernata", "chiusa", "archiviata"].contains(&st.as_str()) { v["stato"] = json!("ferma"); v["da"] = json!(when); }
                if let Some(t) = &title { v["titolo"] = json!(t); }
            }
            "SessionEnd" => {
                let st = v["stato"].as_str().unwrap_or("");
                if st == "ibernata" || st == "archiviata" || v["ospite"] == "tavolo" { return false; }
                v["stato"] = json!("chiusa"); v["da"] = json!(when);
            }
            _ => return false,
        }
        true
    });
    if let Some(pr) = to_name {
        let _ = std::fs::write(state_dir.join(format!(".nomina-{name}.txt")), pr.chars().take(1500).collect::<String>());
        if let Ok(exe) = std::env::current_exe() {
            let mut c = crate::new_command(exe);
            c.args(["nomina", &name]).env("TAVOLO_INTERNO", "1")
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            #[cfg(windows)] { use std::os::windows::process::CommandExt; c.creation_flags(crate::sys::CREATE_NO_WINDOW | 0x0000_0008); } // DETACHED_PROCESS
            let _ = c.spawn();
        }
    }
}

const GENERIC_DIRS: [&str; 5] = ["", "desktop", "documents", "downloads", "scrivania"];

/// Chat project for the name: the starting folder, empty for home, Desktop, temporary folders.
fn folder_name(v: &Value) -> String {
    let c = v["cwd_origine"].as_str().filter(|x| !x.is_empty()).or(v["cwd"].as_str()).unwrap_or("").replace('\\', "/");
    let c = c.trim_end_matches('/');
    let home = std::env::var("HOME").unwrap_or_default().replace('\\', "/");
    if c.is_empty() || c.eq_ignore_ascii_case(home.trim_end_matches('/')) || c.contains("/scratchpad") || c.to_lowercase().contains("/temp") || c.starts_with("/tmp") { return String::new(); }
    let last = c.rsplit('/').next().unwrap_or("");
    if GENERIC_DIRS.contains(&last.to_lowercase().as_str()) || last.ends_with(':') { String::new() } else { last.to_string() }
}

fn normalized(x: &str) -> String { x.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect() }

/// The name without trailing sequence numbers: "Logo cliente 2", "Logo cliente (3)", "Logo cliente - 4" -> "Logo cliente".
/// If it would end up empty (name made only of digits) it is left as is.
pub fn name_without_number(n: &str) -> String {
    let s = n.trim().trim_end_matches(|c: char| c.is_ascii_digit() || " -_#.:()\u{b7}".contains(c)).trim();
    if s.is_empty() { n.trim().to_string() } else { s.to_string() }
}

fn name_key(n: &str) -> String { normalized(&name_without_number(n)).split_whitespace().collect::<Vec<_>>().join(" ") }

/// Is there already a (non-archived) chat in the same folder with the same name, ignoring trailing numbers?
/// `state_dir` is the states folder; `exclude` is the id of the asking chat (empty if it is new).
pub fn similar_chat(state_dir: &std::path::Path, cwd: &str, name: &str, exclude: &str) -> Option<String> {
    let folder_of = |v: &Value| v["cwd_origine"].as_str().filter(|x| !x.is_empty()).or(v["cwd"].as_str()).unwrap_or("").trim_end_matches('/').to_string();
    let (cwd, key) = (cwd.trim_end_matches('/').to_string(), name_key(name));
    if cwd.is_empty() || key.is_empty() { return None; }
    for e in std::fs::read_dir(state_dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        let Ok(v) = serde_json::from_str::<Value>(&std::fs::read_to_string(&p).unwrap_or_default()) else { continue };
        let id = v["sessione"].as_str().unwrap_or("");
        if id.is_empty() || id == exclude || matches!(v["stato"].as_str().unwrap_or(""), "archiviata" | "chiusa") { continue; }
        let other = v["nome_manuale"].as_str().unwrap_or("");
        if !other.is_empty() && folder_of(&v) == cwd && name_key(other) == key { return Some(id.to_string()); }
    }
    None
}

/// `Juggl nomina <id>`: short name for an unnamed chat, from the first message (haiku, no tools).
pub fn name_chat(state_dir: PathBuf, name: &str) {
    if safe_file_name(name).as_deref() != Some(name) { return; }
    let p = state_dir.join(format!("{name}.json"));
    let ft = state_dir.join(format!(".nomina-{name}.txt"));
    let text = std::fs::read_to_string(&ft).unwrap_or_default();
    let _ = std::fs::remove_file(&ft);
    let text = { let t = text.trim_start(); if t.starts_with("[Da ") { t.splitn(2, ']').nth(1).unwrap_or("").trim().to_string() } else { t.trim().to_string() } };
    if text.chars().count() < 8 { return; }
    let st = match crate::storage::read_json(&p) { crate::storage::ReadResult::Valida(v) => v, _ => json!({}) };
    let folder = folder_name(&st);
    if !crate::helpers::is_enabled("nomi") { crate::helpers::record("nomi", "spenta"); return; }
    let prompt = format!("Dai un nome breve a questa chat di lavoro: italiano, da 2 a 4 parole, concreto (l'oggetto del lavoro e il cliente o progetto se c'e'), maiuscola solo all'inizio e nei nomi propri, niente virgolette, niente punto, niente emoji. Esempi: Logo nuovo, Backup server su R2, Spazio disco Mac, Preventivo sito web. {}Rispondi solo con il nome.\n\nPrimo messaggio della chat:\n{}",
        if folder.is_empty() { String::new() } else { format!("La chat e' nella cartella del progetto o cliente \"{folder}\": il nome deve contenerlo. ") },
        text.chars().take(1500).collect::<String>());
    let mut c = crate::new_command(crate::profile::claude_cmd());
    c.args(["-p", "--model", "haiku", "--setting-sources", "project,local", "--strict-mcp-config", "--tools", "", "--output-format", "text", &prompt])
        .env("TAVOLO_INTERNO", "1").stdin(std::process::Stdio::null()).current_dir(&state_dir);
    #[cfg(windows)] { use std::os::windows::process::CommandExt; c.creation_flags(crate::sys::CREATE_NO_WINDOW); }
    let Ok(o) = c.output() else { crate::helpers::record("nomi", "errore"); return };
    crate::helpers::record("nomi", "ok");
    let r = String::from_utf8_lossy(&o.stdout).to_string();
    let mut n = r.trim().lines().next().unwrap_or("").trim_matches(|c: char| c.is_whitespace() || "\"'`*#-.".contains(c)).chars().take(42).collect::<String>().trim().to_string();
    let words = n.split_whitespace().count();
    let bad = ["error", "limit", "login", "api"].iter().any(|w| normalized(&n).split_whitespace().any(|x| x == *w));
    if n.chars().count() < 3 || words > 7 || bad { return; }
    if !folder.is_empty() {
        let pc = normalized(&folder); let pn = normalized(&n);
        let folder_words: Vec<&str> = pc.split_whitespace().filter(|w| w.len() >= 3).collect();
        if !folder_words.is_empty() && !folder_words.iter().any(|w| pn.contains(w)) { n = folder.clone(); }
    }
    // no sequence numbers; if a chat with the same name already exists in the same folder the name is not applied
    // (two chats with the same name are a duplicate: better no name)
    let n = name_without_number(&n);
    if n.chars().count() < 3 { return; }
    let cwd = st["cwd_origine"].as_str().filter(|x| !x.is_empty()).or(st["cwd"].as_str()).unwrap_or("").to_string();
    if let Some(other) = similar_chat(&state_dir, &cwd, &n, name) { crate::log_juggl(&format!("{name}: nome \"{n}\" non applicato, esiste gia' la chat {other}")); return; }
    let _ = crate::storage::modify(&p, Some(json!({})), |v| {
        if !v.is_object() || v["nome_manuale"].as_str().map(|x| !x.is_empty()).unwrap_or(false) { return false; }
        v["nome_manuale"] = json!(n); v["nome_auto"] = json!(true);
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn title_and_message() {
        let d = std::env::temp_dir().join(format!("juggl-hook-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let tr = d.join("t.jsonl");
        std::fs::write(&tr, "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ciao \\\"tu\\\"\"}]}}\n{\"type\":\"custom-title\",\"customTitle\":\"Prova \\\"uno\\\"\"}\n").unwrap();
        assert_eq!(last_assistant_message(tr.to_str().unwrap()), "ciao \"tu\"");
        assert_eq!(transcript_title(tr.to_str().unwrap()).as_deref(), Some("Prova \"uno\""));
        assert_eq!(shorten("a  b\nc", 10), "a b c");
        assert!(safe_file_name("../..").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn names_without_number_and_similar_chats() {
        assert_eq!(name_without_number("Logo cliente 2"), "Logo cliente");
        assert_eq!(name_without_number("Logo cliente (3)"), "Logo cliente");
        assert_eq!(name_without_number("Logo cliente - 4"), "Logo cliente");
        assert_eq!(name_without_number("Sito v2 cielo #12"), "Sito v2 cielo");
        assert_eq!(name_without_number("2026"), "2026", "un nome di sole cifre resta");
        let d = std::env::temp_dir().join(format!("juggl-simili-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap();
        let write_state = |id: &str, name: &str, cwd: &str, state_dir: &str| std::fs::write(d.join(format!("{id}.json")), json!({"sessione": id, "nome_manuale": name, "cwd": cwd, "stato": state_dir}).to_string()).unwrap();
        write_state("a1", "Cielo video", "/lavori/cartella", "aspetta");
        write_state("a2", "Cielo video", "/lavori/altro", "aspetta");
        write_state("a3", "Cielo video", "/lavori/cartella", "archiviata");
        assert_eq!(similar_chat(&d, "/lavori/cartella", "Cielo video 2", "").as_deref(), Some("a1"), "stesso nome senza numero e stessa cartella");
        assert_eq!(similar_chat(&d, "/lavori/cartella/", "cielo  VIDEO", "").as_deref(), Some("a1"), "maiuscole, spazi e barra finale non contano");
        assert_eq!(similar_chat(&d, "/lavori/cartella", "Cielo video", "a1"), None, "non conta se stessa");
        assert_eq!(similar_chat(&d, "/lavori/nuova", "Cielo video", ""), None, "un'altra cartella e' un'altra chat");
        assert_eq!(similar_chat(&d, "/lavori/cartella", "Altro tema", ""), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
