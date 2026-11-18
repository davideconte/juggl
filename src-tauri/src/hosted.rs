// Sessions hosted by the app, without a terminal: Claude Code in stream-json mode (the same system as the SDK).
// The app writes the messages on the process stdin and reads everything in real time: text word by word, commands,
// end of turn. The conversation stays in Claude's transcript as always, so it can be resumed with --resume.
//
// Safety rules (Nov 8):
// - only one copy per conversation: "starting" set under a lock and a check of the processes already alive;
// - the reader of an old process does not touch the entry nor the state of the new process (the pid is compared);
// - each process leads its own group: closing also stops MCP servers and child commands;
// - each session's stdin has its own lock: a long send does not block the other sessions.
//
// Keeper (Nov 8): each session lives in a `juggl-custode` process detached from the app (src/keeper.rs). Closing or
// reinstalling Juggl no longer stops Claude nor the sub-agents: on reopening `reattach_all` reconnects and gets
// the missed events again (numbered: the last seen one is `custode_seq` in the state). If the keeper program is missing
// it falls back to the old mode (Claude as a direct child of the app, stopped on close).
#[cfg(windows)] use crate::sys as libc;
use crate::{storage, base_dir, keeper, session_care, home, load_json, unix_now, str_of, is_alive};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)] use std::os::unix::process::CommandExt;
#[cfg(windows)] use crate::sys::CommandExtJuggl;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

type Output = Arc<Mutex<Box<dyn Write + Send>>>;
/// figlio: only in the old mode. custode: pid of the keeper (0 = old mode). seq: last event read from the keeper.
/// account: Claude account the process started with (src/claude_account.rs). in_turno: Claude is replying (from the
/// events: true from the first event of the turn to the "result").
struct HostedSession { figlio: Option<Child>, stdin: Output, chiusa_da_me: bool, pid: u32, custode: u32, custode_ver: u64, seq: Arc<AtomicU64>, account: String, in_turno: Arc<AtomicBool> }

/// Where Claude's events come from: the child's pipe (old mode) or the keeper's socket.
enum EventSource { Pipe(std::process::ChildStdout), Custode(BufReader<crate::keeper::UnixStream>) }

/// Automatic resume (riprendi_ospitata in main.rs) when the keeper of a session dies unexpectedly.
pub static RESUME: OnceLock<fn(&AppHandle, &str)> = OnceLock::new();

/// The app is closing: readers that see the process end no longer write anything (the state was already
/// written by `shut_all`, "app chiusa", and on the next opening the session restarts).
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Lock that does not stay poisoned if a thread panics.
fn lock_or_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

/// sessions to restart right away (model or reasoning change), with the whole conversation
fn to_restart() -> &'static Mutex<HashSet<String>> { static M: OnceLock<Mutex<HashSet<String>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(Default::default())) }
/// sessions to restart for the account change with the message to resend (content of the user message)
fn to_resend() -> &'static Mutex<HashMap<String, Value>> { static M: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(Default::default())) }
/// sessions that are starting: more than a second passes between the check and the insertion in the table
fn starting() -> &'static Mutex<HashSet<String>> { static M: OnceLock<Mutex<HashSet<String>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(Default::default())) }
struct StartGuard(String);
impl Drop for StartGuard { fn drop(&mut self) { lock_or_recover(starting()).remove(&self.0); } }

fn table() -> MutexGuard<'static, HashMap<String, HostedSession>> {
    static T: OnceLock<Mutex<HashMap<String, HostedSession>>> = OnceLock::new();
    lock_or_recover(T.get_or_init(|| Mutex::new(HashMap::new())))
}

pub fn restart(id: &str) {
    let t = table();
    if let Some(o) = t.get(id) { lock_or_recover(to_restart()).insert(id.to_string()); signal_group(o.pid as i64, libc::SIGTERM); }
}

pub fn is_hosted(id: &str) -> bool { table().contains_key(id) }

/// Hosted or already starting (whoever resumes must do nothing: the one that started it takes care of it).
pub fn is_hosted_or_starting(id: &str) -> bool { table().contains_key(id) || lock_or_recover(starting()).contains(id) }

/// Signal to the process group (claude and all its children); if it does not lead a group, only to it.
pub fn signal_group(pid: i64, sig: i32) {
    if pid <= 1 { return; }
    unsafe {
        if libc::getpgid(pid as i32) == pid as i32 { libc::kill(-(pid as i32), sig); } else { libc::kill(pid as i32, sig); }
    }
}

/// SIGTERM to the group, then SIGKILL after 8 seconds if it is still alive.
pub fn terminate_group(pid: i64) {
    if pid <= 1 { return; }
    let is_leader = unsafe { libc::getpgid(pid as i32) } == pid as i32;
    signal_group(pid, libc::SIGTERM);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(8));
        unsafe {
            if is_leader { libc::kill(-(pid as i32), libc::SIGKILL); } else if libc::kill(pid as i32, 0) == 0 { libc::kill(pid as i32, libc::SIGKILL); }
        }
    });
}

/// PATH of the login shell: opened from the Finder the app has a minimal PATH and Claude would not find git, node, python...
pub(crate) fn path_login() -> String {   // engines (Nov 12): pub(crate) for engines.rs
    #[cfg(windows)] { return std::env::var("PATH").unwrap_or_default(); }
    #[cfg(target_os = "linux")] { return crate::profile::linux_shell_path(); }   // Ubuntu: with nvm, node and claude are there (Nov 12)
    #[allow(unreachable_code)]
    static P: OnceLock<String> = OnceLock::new();
    P.get_or_init(|| {
        crate::new_command("/bin/zsh").args(["-ilc", "printf %s \"$PATH\""]).output().ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|p| p.contains('/'))
            .unwrap_or_else(|| "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into())
    }).clone()
}

pub(crate) fn uuid() -> String {
    let mut b = [0u8; 16];
    #[cfg(windows)] crate::sys::random_bytes(&mut b);
    #[cfg(unix)]
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") { use std::io::Read; let _ = f.read_exact(&mut b); }
    b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn state_file(id: &str) -> std::path::PathBuf { base_dir().join("stato").join(format!("{id}.json")) }

/// Read, change, write the state under a lock. If missing it starts from {"sessione": id}; if present but unreadable
/// it does not rewrite it (before, nome_manuale, modello and transcript were lost).
fn update_state(id: &str, f: impl FnOnce(&mut Value)) {
    if !storage::is_valid_id(id) { return; }
    if let Err(e) = storage::modify(&state_file(id), Some(json!({"sessione": id})), |v| { f(v); true }) { crate::log_juggl(&format!("stato {id}: {e}")); }
}

/// Like `update_state`, but only if the state still belongs to this process: an old process that exits does not
/// overwrite the state of the new one.
fn update_if_mine(id: &str, pid: u32, f: impl FnOnce(&mut Value)) {
    let p = state_file(id);
    if !p.exists() { return; }
    let _ = storage::modify(&p, None, |v| {
        let owner = v.get("pid").and_then(|x| x.as_i64());
        if owner.is_some() && owner != Some(pid as i64) { return false; }
        f(v);
        true
    });
}

fn claude() -> String { crate::profile::claude_cmd().to_string_lossy().to_string() }

/// Is the process Claude Code (and not another program that took the same pid)?
pub fn is_claude(pid: i64) -> bool {
    if pid <= 0 { return false; }
    #[cfg(windows)] { return crate::sys::process_info(pid as u32).map(|(_, n, _)| n.to_lowercase().contains("claude")).unwrap_or(false); }
    #[allow(unreachable_code)]
    crate::new_command("ps").args(["-p", &pid.to_string(), "-o", "command="]).output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("claude")).unwrap_or(false)
}

/// engines (Nov 12): is the process the `codex app-server` of a Codex chat?
pub fn is_codex(pid: i64) -> bool {
    if pid <= 0 { return false; }
    #[cfg(windows)] { return crate::sys::process_info(pid as u32).map(|(_, n, _)| n.to_lowercase().contains("codex")).unwrap_or(false); }
    #[allow(unreachable_code)]
    crate::new_command("ps").args(["-p", &pid.to_string(), "-o", "command="]).output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("codex")).unwrap_or(false)
}

/// Live process (not hosted by me) already using this conversation: `claude --resume <id>` or
/// `claude --session-id <id>`, also launched from the terminal.
pub fn conversation_process(id: &str) -> Option<i64> {
    if id.len() < 8 { return None; }
    let own: Vec<i64> = table().values().flat_map(|o| [o.pid as i64, o.custode as i64]).filter(|p| *p > 0).collect();
    let o = crate::new_command("ps").args(["-axo", "pid=,command="]).output().ok()?;
    for r in String::from_utf8_lossy(&o.stdout).lines() {
        if !r.contains(id) || !r.contains("claude") { continue; }
        if !(r.contains("--resume") || r.contains("--session-id") || r.contains(" -r ")) { continue; }
        let Some(pid) = r.split_whitespace().next().and_then(|x| x.parse::<i64>().ok()) else { continue };
        if pid as u32 == std::process::id() || own.contains(&pid) { continue; }
        return Some(pid);
    }
    None
}

/// Is the conversation already open somewhere else? (process with the id in its arguments, or Claude alive
/// in the cmux terminal with the pid written in the state)
fn open_elsewhere(id: &str) -> Option<String> {
    if let Some(p) = conversation_process(id) { return Some(format!("la conversazione e' gia' aperta in un altro processo (pid {p})")); }
    let st = load_json(&state_file(id)).unwrap_or(json!({}));
    let pid = st.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
    if str_of(&st, "ospite") != "tavolo" && pid > 0 && is_alive(pid) && is_claude(pid) && (!str_of(&st, "surface").is_empty() || crate::real_tabs().contains_key(id)) {
        return Some(format!("la conversazione e' aperta nel terminale (pid {pid}): chiudila li' prima di riprenderla qui"));
    }
    None
}

/// Process left alive from a previous Juggl run (close or crash): stop it, so it can be resumed.
pub fn stop_orphan(id: &str) {
    let Some(pid) = conversation_process(id) else { return };
    if !can_stop(id, pid) { return; }
    crate::log_juggl(&format!("{id}: fermo il processo rimasto dall'apertura precedente (pid {pid})"));
    signal_group(pid, libc::SIGTERM);
    let start = Instant::now();
    while is_alive(pid) && start.elapsed() < Duration::from_secs(10) { std::thread::sleep(Duration::from_millis(200)); }
    if is_alive(pid) { signal_group(pid, libc::SIGKILL); std::thread::sleep(Duration::from_millis(300)); }
}

/// A process is really orphaned only if nobody is managing it. It is not stopped if:
/// - the chat state has a keeper still alive (it is the one managing it: Claude is its child, not an orphan);
/// - the process has a terminal (tty): it is a chat opened by a person in Terminal or in cmux.
/// Better to leave a duplicate than to close a live chat (review Nov 14, point 26).
fn can_stop(id: &str, pid: i64) -> bool {
    let keeper_pid = load_json(&state_file(id)).and_then(|v| v.get("custode_pid").and_then(|x| x.as_i64())).unwrap_or(0);
    if keeper_pid > 0 && is_alive(keeper_pid) {
        crate::log_juggl(&format!("{id}: il processo {pid} non e' un orfano, il custode {keeper_pid} e' vivo: non lo fermo"));
        return false;
    }
    if has_terminal(pid) {
        crate::log_juggl(&format!("{id}: il processo {pid} ha un terminale (chat aperta da una persona): non lo fermo"));
        return false;
    }
    true
}

/// Does the process have a controlling terminal? (`ps` prints "??" when it has none). If it cannot be known, say yes:
/// when in doubt nothing is stopped.
#[cfg(unix)]
fn has_terminal(pid: i64) -> bool {
    match crate::new_command("ps").args(["-p", &pid.to_string(), "-o", "tty="]).output() {
        Ok(o) if o.status.success() => {
            let tty = String::from_utf8_lossy(&o.stdout).trim().to_string();
            !(tty.is_empty() || tty == "??" || tty == "?")
        }
        _ => true,
    }
}
#[cfg(not(unix))]
fn has_terminal(_pid: i64) -> bool { false }

/// Folders not good for resuming: temporary ones (scratchpad in /private/tmp, which vanish) or the root.
pub fn is_temp_dir(c: &str) -> bool {
    c.is_empty() || c == "/" || c.starts_with("/private/tmp") || c.starts_with("/tmp/") || c.starts_with("/var/folders")
        || c.starts_with("/private/var/folders") || c.contains("/scratchpad")
}
fn good_folder(c: &str) -> bool { !is_temp_dir(c) && Path::new(c).is_dir() }

/// Working folder written in the first lines of the transcript (where the conversation was born).
fn cwd_from_transcript(id: &str) -> String {
    let Some(p) = crate::find_transcript(&json!({"sessione": id})) else { return String::new() };
    let Ok(f) = std::fs::File::open(&p) else { return String::new() };
    for r in BufReader::new(f).lines().map_while(Result::ok).take(300) {
        if !r.contains("\"cwd\"") { continue; }
        if let Ok(d) = serde_json::from_str::<Value>(&r) { let c = str_of(&d, "cwd"); if !c.is_empty() { return c.to_string(); } }
    }
    String::new()
}

/// Name of Claude's project folder for a working folder (each non-alphanumeric character becomes "-":
/// "/" -> "-", "/home/x/.y" -> "-home-x--y").
pub fn claude_project(cwd: &str) -> String { cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect() }

/// All the working folders recorded in the transcript of the main conversation, in order, without repetitions.
fn all_cwds_from_transcript(id: &str) -> Vec<String> {
    let Some(p) = crate::find_transcript(&json!({"sessione": id})) else { return vec![] };
    let Ok(f) = std::fs::File::open(&p) else { return vec![] };
    let mut out: Vec<String> = vec![];
    for r in BufReader::new(f).lines().map_while(Result::ok) {
        let Some(i) = r.find("\"cwd\":\"") else { continue };
        if r.contains("\"isSidechain\":true") { continue; }
        let c = r[i + 7..].split('"').next().unwrap_or("").to_string();
        if !c.is_empty() && !out.contains(&c) { out.push(c); }
    }
    out
}

/// Where to resume a session: never in the temporary folder where the agent ended up with `cd`.
/// First choice (Nov 8): the folder matching the project folder where the transcript is (for example "/"
/// for sessions born in "/"): so the session stays in the same Claude project, with the same scratchpad and
/// the same automatic memory. Then, as before: origin folder saved by Juggl, origin folder in the
/// transcript, project folder, last folder of the state, the home.
pub fn resume_dir(id: &str) -> String {
    let st = load_json(&state_file(id)).unwrap_or(json!({}));
    let mut candidate = vec![str_of(&st, "cwd_origine").to_string(), cwd_from_transcript(id)];
    let name = [str_of(&st, "nome_manuale"), str_of(&st, "titolo")].into_iter().find(|x| !x.is_empty()).unwrap_or("").to_string();
    let pr = session_care::project_of(&st, &name, &session_care::projects());
    candidate.push(str_of(&pr, "cartella").to_string());
    candidate.push(str_of(&st, "cwd").to_string());
    if let Some(dir) = crate::find_transcript(&json!({"sessione": id})).and_then(|p| p.parent().and_then(|d| d.file_name()).map(|d| d.to_string_lossy().to_string())) {
        let mut all = candidate.clone();
        all.extend(all_cwds_from_transcript(id));
        all.push("/".into());
        if let Some(c) = all.into_iter().find(|c| !c.is_empty() && claude_project(c) == dir && Path::new(c).is_dir() && (c == "/" || !is_temp_dir(c))) { return c; }
    }
    candidate.into_iter().find(|c| good_folder(c)).unwrap_or_else(|| home().to_string_lossy().to_string())
}

/// Is the last turn in the transcript finished? (last entry of the main conversation = reply with end_turn)
pub fn turn_finished(id: &str) -> bool {
    let Some(p) = crate::find_transcript(&json!({"sessione": id})) else { return true };
    let (lines, _) = crate::tail_file(&p, 1_500_000);
    let mut finished = true;
    for r in lines {
        if !(r.contains("\"type\":\"user\"") || r.contains("\"type\":\"assistant\"")) { continue; }
        let Ok(d) = serde_json::from_str::<Value>(&r) else { continue };
        if d.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) || d.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
        match str_of(&d, "type") {
            // "[Request interrupted by user]": the user stopped it, there is nothing to continue
            "user" => finished = r.contains("[Request interrupted"),
            "assistant" => finished = d.get("message").map(|m| str_of(m, "stop_reason") == "end_turn").unwrap_or(false),
            _ => {}
        }
    }
    finished
}

fn message(text: &str) -> String { message_with(text, &[]) }

/// Message with real images (base64): the agent sees them as when you paste them into Claude.
fn message_with(text: &str, images: &[String]) -> String {
    use base64::Engine;
    let mut c = vec![];
    let mut extra = String::new();
    for p in images {
        let ext = p.rsplit('.').next().unwrap_or("").to_lowercase();
        let mt = match ext.as_str() { "jpg" | "jpeg" => "image/jpeg", "gif" => "image/gif", "webp" => "image/webp", "png" => "image/png", _ => "" };
        match std::fs::read(p) {
            Ok(b) if !mt.is_empty() && b.len() < 4_500_000 => c.push(json!({"type": "image", "source": {"type": "base64", "media_type": mt, "data": base64::engine::general_purpose::STANDARD.encode(b)}})),
            _ => extra.push_str(&format!("\nAllegato: {p}")),   // too large or unsupported format: pass the path instead
        }
    }
    c.push(json!({"type": "text", "text": format!("{text}{extra}")}));
    json!({"type": "user", "message": {"role": "user", "content": c}}).to_string()
}

/// Starts (or resumes) a session inside the app. Returns the session id.
pub fn start(app: &AppHandle, cwd: &str, text: &str, resume_id: Option<&str>) -> Result<String, String> { start_with(app, cwd, text, resume_id, None) }

pub fn start_with(app: &AppHandle, cwd: &str, text: &str, resume_id: Option<&str>, model: Option<&str>) -> Result<String, String> { start_engine(app, cwd, text, resume_id, model, None) }

/// engines (Nov 12): like `start_with`, with the engine of the new chat ("claude" | "codex"); None = the one in the state.
pub fn start_engine(app: &AppHandle, cwd: &str, text: &str, resume_id: Option<&str>, model: Option<&str>, new_engine: Option<&str>) -> Result<String, String> {
    if QUITTING.load(Ordering::SeqCst) { return Err("Juggl si sta chiudendo".into()); }
    // without a valid license nothing new starts (license.rs); chats already working stay as they are
    if !crate::license::is_valid() { return Err("Juggl non ha un account attivo: entra con il tuo account per avviare le chat".into()); }
    let id = resume_id.map(|x| x.to_string()).unwrap_or_else(uuid);
    if !storage::is_valid_id(&id) || id.starts_with("locale:") { return Err("id non valido".into()); }
    // only one copy: already hosted or already starting = nothing new
    {
        let t = table();
        let mut a = lock_or_recover(starting());
        if t.contains_key(&id) || a.contains(&id) { return Ok(id); }
        a.insert(id.clone());
    }
    let _guard = StartGuard(id.clone());
    if resume_id.is_some() { if let Some(m) = open_elsewhere(&id) { return Err(m); } }
    let mut a: Vec<String> = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
        "--include-partial-messages"].iter().map(|x| x.to_string()).collect();
    // permissions (Nov 12): no more fixed --dangerously-skip-permissions: the chat level adds it further down (src/permissions.rs)
    // resuming only works if Claude already saved the conversation (after the first message): otherwise it restarts new with the same id
    let saved = resume_id.is_some() && std::fs::read_dir(home().join(".claude/projects")).into_iter().flatten().flatten()
        .any(|d| d.path().join(format!("{id}.jsonl")).exists());
    if saved { a.extend(["--resume".into(), id.clone()]); } else { a.extend(["--session-id".into(), id.clone()]); }
    // model and reasoning: the ones chosen for the session (Settings), if not passed
    let st = load_json(&state_file(&id)).unwrap_or(json!({}));
    // engines (Nov 12): Codex chat (chosen at birth or written in the state)
    let codex = new_engine.map(|x| x == "codex").unwrap_or_else(|| crate::engines::engine_of(&st) == "codex");
    let mut m = model.map(|x| x.to_string()).filter(|m| !m.is_empty()).unwrap_or_else(|| str_of(&st, "modello").to_string());
    // new chat without a chosen model with Eco mode on: Sonnet (eco.rs)
    if m.is_empty() && resume_id.is_none() && !codex { if let Some(x) = crate::eco::model_for_new() { m = x.to_string(); } }
    let chosen_model = m.clone();   // engines (Nov 12)
    if !m.is_empty() { a.extend(["--model".into(), m]); }
    let e = str_of(&st, "effort").to_string();
    if !e.is_empty() { a.extend(["--effort".into(), e]); }
    // session born from "Importa progetti": the project context prepared at import goes as an added system
    // instruction. Claude Code records it at the first request and reuses it at each resume; passing it always
    // also puts it back after a compaction. Only files in ~/tavolo/contesti.
    let ctx = str_of(&st, "contesto").to_string();
    let mut extra_prompt = String::new();
    if !ctx.is_empty() && ctx.starts_with(&base_dir().join("contesti").to_string_lossy().to_string()) {
        if let Ok(t) = std::fs::read_to_string(&ctx) { extra_prompt = t.trim().to_string(); }
    }
    // Eco mode (eco.rs): its rules are added to the system instructions of non-pinned chats, without messages
    let pinned = st.get("fissata").and_then(|x| x.as_bool()).unwrap_or(false);
    if !pinned && !cwd.ends_with("generale-juggl") { if let Some(r) = crate::eco::rules() { if !extra_prompt.is_empty() { extra_prompt.push_str("\n\n"); } extra_prompt.push_str(r); } }
    if !extra_prompt.is_empty() { a.extend(["--append-system-prompt".into(), extra_prompt.clone()]); }   // engines (Nov 12): clone, Codex needs it too
    // Juggl (general assistant) only coordinates: no tools to edit files
    if cwd.ends_with("generale-juggl") {
        a.extend(["--disallowedTools".into(), "Edit,Write,NotebookEdit,MultiEdit".into()]);
    }
    // automatic coordinator (Nov 12): a group's coordinator starts with read-only tools and without global memory (src/coordinator.rs)
    a.extend(crate::coordinator::extra_args(&st));
    // permissions (Nov 12): chat level (Always ask / File changes / Never ask) and requests to Juggl via stdio
    let (perm_args, perm_level) = crate::permissions::cli_args(&st, cwd, &a);
    a.extend(perm_args);
    // engines (Nov 12): Codex in the keeper (`--motore codex <codex> <configuration>`), same permission level
    let program = if codex {
        let cx = crate::engines::codex_cmd().ok_or("Codex non e' installato su questo computer")?;
        a = vec!["codex".into(), cx.to_string_lossy().to_string(), crate::engines::codex_config(&id, cwd, &st, &chosen_model, &perm_level, &extra_prompt).to_string()];
        "--motore".to_string()
    } else { claude() };
    let log = std::fs::OpenOptions::new().create(true).append(true).open(base_dir().join("ospite.log")).map_err(|e| e.to_string())?;
    // the account it starts with: Claude reads the token now and never rereads it (src/claude_account.rs)
    let account = if codex { String::new() } else { crate::claude_account::current_for(&id) };   // engines (Nov 12): Codex does not follow the Claude accounts
    let write_start = |pid: u32, keeper_pid: u32| update_state(&id, |v| {
        v["sessione"] = json!(id); v["ospite"] = json!("tavolo"); v["pid"] = json!(pid as i64); v["cwd"] = json!(cwd);
        if good_folder(cwd) { v["cwd_origine"] = json!(cwd); }
        v["agente"] = json!("claude"); v["agg"] = json!(unix_now()); v["account"] = json!(account);
        // permissions (Nov 12): level it starts with; the requests of the previous process are no longer valid
        v["permessi_livello"] = json!(perm_level); if let Some(o) = v.as_object_mut() { o.remove("permessi_attesa"); }
        // model chosen at birth: stays in the state, so Resume (also from the archive) restarts with the same one
        if let Some(m) = model.filter(|m| !m.is_empty()) { v["modello"] = json!(m); }
        // new keeper = event numbers from scratch
        v["custode_pid"] = json!(keeper_pid as i64); v["custode_seq"] = json!(0);
        if let Some(o) = v.as_object_mut() { o.remove("surface"); o.remove("ripresa_il"); o.remove("motivo_ibernazione"); o.remove("ibernata_il"); for k in ["archiviata_il", "motivo_archivio", "nome_archivio", "archiviata_da", "pronta"] { o.remove(k); } if keeper_pid == 0 { o.remove("custode_pid"); o.remove("custode_seq"); } }
        v["stato"] = json!(if text.trim().is_empty() { "ferma" } else { "lavora" }); v["da"] = json!(unix_now());
        if !text.trim().is_empty() { v["chiesto"] = json!(text.chars().take(200).collect::<String>()); }
        // engines (Nov 12): the conversation is read from the mirror written by the keeper
        if codex { v["agente"] = json!("codex"); v["motore"] = json!("codex"); v["transcript"] = json!(crate::engines::mirror_path(&id).to_string_lossy()); }
    });
    let mut keeper_ver = 0u64;
    let (pid, keeper_pid, child, mut stdin, source): (u32, u32, Option<Child>, Box<dyn Write + Send>, EventSource) = match keeper::program() {
        Some(prog) => {
            let c = keeper::start(&prog, &id, cwd, &program, &a, &path_login(), log)?;
            keeper_ver = c.versione;
            crate::log_juggl(&format!("{id}: custode {} versione {} (app {})", c.pid_custode, c.versione, c.app));
            (c.pid, c.pid_custode, None, Box::new(c.scrittore), EventSource::Custode(c.lettore))
        }
        None if codex => return Err("le chat Codex hanno bisogno del custode di Juggl".into()),   // engines (Nov 12)
        None => {
            let mut child = crate::new_command(claude()).args(&a).current_dir(cwd).env("PATH", path_login())
                .env_remove("CMUX_SURFACE_ID").env_remove("CMUX_WORKSPACE_ID")
                .process_group(0)   // leader of its own group: closing also stops the children (MCP servers, builds)
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(log))
                .spawn().map_err(|e| format!("Claude non parte: {e}"))?;
            let stdin = child.stdin.take().ok_or("stdin non disponibile")?;
            let stdout = child.stdout.take().ok_or("stdout non disponibile")?;
            (child.id(), 0, Some(child), Box::new(stdin), EventSource::Pipe(stdout))
        }
    };
    write_start(pid, keeper_pid);
    if !text.trim().is_empty() {
        if let Err(e) = writeln!(stdin, "{}", message(text)).and_then(|_| stdin.flush()) {
            signal_group(pid as i64, libc::SIGTERM);
            if let Some(mut f) = child { let _ = f.wait(); }
            return Err(e.to_string());
        }
    }
    let seq = Arc::new(AtomicU64::new(0));
    let in_turn = Arc::new(AtomicBool::new(!text.trim().is_empty()));
    table().insert(id.clone(), HostedSession { figlio: child, stdin: Arc::new(Mutex::new(stdin)), chiusa_da_me: false, pid, custode: keeper_pid, custode_ver: keeper_ver, seq: seq.clone(), account, in_turno: in_turn.clone() });
    start_reader(app, &id, pid, keeper_pid, source, seq, in_turn);
    Ok(id)
}

/// On reopening Juggl: reconnects to all the sessions still alive in their keepers. Must be called before
/// `restore_hosted` (which would otherwise take them for orphan processes to stop). Returns how many.
/// Instance lock on the Mac too (review Nov 14, point 26): whoever does not get it is not the main instance and does not
/// restore nor reattach the chats (it would steal them from the live one). The lock (flock) drops by itself when the process
/// dies. On a soft restart the new copy starts while the old one is still exiting: wait up to 20 s before
/// concluding it is the second one. The result is computed only once. Windows/Linux have the "single instance" plugin.
pub fn main_instance() -> bool {
    static RESULT: OnceLock<bool> = OnceLock::new();
    static HELD: OnceLock<std::fs::File> = OnceLock::new();
    *RESULT.get_or_init(|| {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let _ = std::fs::create_dir_all(base_dir());
            let Ok(f) = std::fs::OpenOptions::new().create(true).read(true).write(true).open(base_dir().join("istanza.lock")) else { return true };
            for _ in 0..40 {
                if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    let _ = HELD.set(f);
                    return true;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            crate::log_juggl("un'altra istanza di Juggl e' viva: questa non ripristina ne' riattacca le chat");
            return false;
        }
        #[allow(unreachable_code)]
        true
    })
}

/// After resuming a chat on reopening: waits for its process to be in the table (at most 60 s) and then
/// another 8 s, before the next chat (point 5 of the review, same pace as the restart queue).
pub fn pause_after_resume(id: &str) {
    let start = Instant::now();
    while !is_hosted(id) && start.elapsed() < Duration::from_secs(60) { std::thread::sleep(Duration::from_millis(300)); }
    std::thread::sleep(Duration::from_secs(8));
}

pub fn reattach_all(app: &AppHandle) -> usize {
    if !main_instance() { return 0; }
    let mut n = 0;
    for id in keeper::alive() {
        match reattach(app, &id) {
            // archived while Juggl was closed but with Claude still alive: stop it, it stays archived
            Ok(true) if load_json(&state_file(&id)).map(|v| str_of(&v, "stato") == "archiviata").unwrap_or(false) => {
                crate::log_juggl(&format!("{id}: archiviata ma ancora viva nel custode, la fermo"));
                close(&id, true);
            }
            Ok(true) => n += 1,
            Ok(false) => {}
            Err(e) => crate::log_juggl(&format!("{id}: non mi ricollego al custode: {e}")),
        }
    }
    if n > 0 { crate::log_juggl(&format!("custode: ricollegate {n} sessioni rimaste al lavoro")); }
    n
}

/// Reconnects to the keeper of a session: no restart, Claude continues where it was. Events that arrived while
/// Juggl was closed (also the end of the turn) go through the reader as if they arrived now.
pub fn reattach(app: &AppHandle, id: &str) -> Result<bool, String> {
    if !storage::is_valid_id(id) || id.starts_with("locale:") { return Err("id non valido".into()); }
    {
        let t = table();
        let mut a = lock_or_recover(starting());
        if t.contains_key(id) || a.contains(id) { return Ok(false); }
        a.insert(id.to_string());
    }
    let _guard = StartGuard(id.to_string());
    let st = load_json(&state_file(id)).unwrap_or(json!({}));
    let my_keeper = st.get("custode_pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let mut after_seq = st.get("custode_seq").and_then(|x| x.as_u64()).unwrap_or(0);
    let mut c = keeper::connect(id, after_seq)?;
    // the state talks about another keeper (or none): the numbers are not valid, reread the whole buffer
    if after_seq > 0 && c.pid_custode != my_keeper { drop(c); after_seq = 0; c = keeper::connect(id, 0)?; }
    let (pid, keeper_pid, c_ver, c_app) = (c.pid, c.pid_custode, c.versione, c.app.clone());
    // account of the process: written in the state when it started (same pid). Sessions started before this
    // version do not have it: take the current one (a possible "quota finished" restarts them anyway, rule 3)
    let account = Some(str_of(&st, "account").to_string()).filter(|a| !a.is_empty() && st.get("pid").and_then(|x| x.as_u64()) == Some(pid as u64))
        .unwrap_or_else(|| crate::claude_account::current_for(id));
    let in_turn = Arc::new(AtomicBool::new(str_of(&st, "stato") == "lavora"));
    update_state(id, |v| {
        v["sessione"] = json!(id); v["ospite"] = json!("tavolo"); v["pid"] = json!(pid as i64); v["account"] = json!(account);
        v["custode_pid"] = json!(keeper_pid as i64); v["agg"] = json!(unix_now());
        if let Some(o) = v.as_object_mut() { o.remove("surface"); o.remove("turno_interrotto"); }
        // hibernated or closed by someone while the process was alive: it is alive, set it back to stopped
        if matches!(str_of(v, "stato"), "ibernata" | "chiusa" | "") {
            let before = str_of(v, "stato_prima").to_string();
            v["stato"] = json!(if before == "aspetta" || before == "lavora" { before } else { "ferma".to_string() });
            if let Some(o) = v.as_object_mut() { o.remove("stato_prima"); o.remove("motivo_ibernazione"); o.remove("ibernata_il"); }
        }
    });
    let seq = Arc::new(AtomicU64::new(after_seq));
    let stdin: Box<dyn Write + Send> = Box::new(c.scrittore);
    table().insert(id.to_string(), HostedSession { figlio: None, stdin: Arc::new(Mutex::new(stdin)), chiusa_da_me: false, pid, custode: keeper_pid, custode_ver: c_ver, seq: seq.clone(), account, in_turno: in_turn.clone() });
    crate::log_juggl(&format!("{id}: ricollegata al custode {keeper_pid} versione {c_ver} (app {c_app}) (claude {pid}, eventi dopo {after_seq}{})", if c.vivo { "" } else { ", Claude gia' finito" }));
    start_reader(app, id, pid, keeper_pid, EventSource::Custode(c.lettore), seq, in_turn);
    Ok(true)
}

/// Reader: real-time text to the interface, end of turn, end of process.
fn start_reader(app: &AppHandle, id: &str, pid: u32, keeper_pid: u32, source: EventSource, seq: Arc<AtomicU64>, in_turn: Arc<AtomicBool>) {
    let (app2, id2) = (app.clone(), id.to_string());
    std::thread::spawn(move || {
        let mut partial = String::new();
        let mut last_sent = Instant::now();
        let (mut pending, mut ia) = (false, None::<IncomingTool>);
        let mut end_seen = false;   // the keeper said Claude is finished
        let mut account_error = false;   // a quota or login error arrived during the turn
        let mut source = Some(source);
        let mut attempts = 0;
        while let Some(sg) = source.take() {
            let lines: Box<dyn Iterator<Item = std::io::Result<String>>> = match sg {
                EventSource::Pipe(p) => Box::new(BufReader::new(p).lines()),
                EventSource::Custode(r) => Box::new(r.lines()),
            };
            for r in lines {
                let r = match r { Ok(r) => r, Err(e) => { crate::log_juggl(&format!("{id2}: lettura interrotta ({e})")); break } };
                let body: &str = if keeper_pid > 0 {
                    if let Some(m) = r.strip_prefix("# ") {
                        if serde_json::from_str::<Value>(m).ok().and_then(|v| v.get("fine").cloned()).is_some() { end_seen = true; }
                        continue;
                    }
                    let Some((n, j)) = r.split_once(' ') else { continue };
                    if let Ok(n) = n.parse::<u64>() { seq.store(n, Ordering::SeqCst); }
                    j
                } else { &r };
                let Ok(d) = serde_json::from_str::<Value>(body) else { continue };
                attempts = 0;
                match str_of(&d, "type") { "result" => in_turn.store(false, Ordering::SeqCst), "stream_event" | "assistant" | "user" => in_turn.store(true, Ordering::SeqCst), _ => {} }
                handle_event(&app2, &id2, pid, &d, &mut partial, &mut last_sent, seq.load(Ordering::SeqCst), keeper_pid > 0, &mut pending, &mut ia, &mut account_error);
            }
            if keeper_pid == 0 || end_seen || QUITTING.load(Ordering::SeqCst) { break; }
            // connection dropped but keeper alive: reconnect and resume from the last event read
            if !is_alive(keeper_pid as i64) || attempts >= 5 || table().get(&id2).map(|o| o.pid) != Some(pid) { break; }
            attempts += 1;
            std::thread::sleep(Duration::from_millis(400 * attempts));
            match keeper::connect(&id2, seq.load(Ordering::SeqCst)) {
                Ok(c) if c.pid_custode == keeper_pid => {
                    if let Some(o) = table().get_mut(&id2) { if o.pid == pid { o.stdin = Arc::new(Mutex::new(Box::new(c.scrittore))); } }
                    source = Some(EventSource::Custode(c.lettore));
                }
                _ => {}
            }
        }
        if QUITTING.load(Ordering::SeqCst) { return; }
        // keeper died unexpectedly: Claude was left without stdin. Stop it and the session restarts by itself.
        let lost = keeper_pid > 0 && !end_seen;
        if lost {
            crate::log_juggl(&format!("{id2}: il custode {keeper_pid} non risponde piu': fermo Claude ({pid}) e la riprendo"));
            if is_alive(pid as i64) && (is_claude(pid as i64) || is_codex(pid as i64)) {   // engines (Nov 12)
                signal_group(pid as i64, libc::SIGTERM);
                let start = Instant::now();
                while is_alive(pid as i64) && start.elapsed() < Duration::from_secs(10) { std::thread::sleep(Duration::from_millis(200)); }
                if is_alive(pid as i64) { signal_group(pid as i64, libc::SIGKILL); std::thread::sleep(Duration::from_millis(300)); }
            }
        }
        // process finished. Remove the entry only if it is still own (a new process with the same id stays tracked)
        let removed = { let mut t = table(); if t.get(&id2).map(|o| o.pid) == Some(pid) { t.remove(&id2) } else { None } };
        let by_me = removed.as_ref().map(|o| o.chiusa_da_me).unwrap_or(false);
        if let Some(o) = removed { if let Some(mut f) = o.figlio { let _ = f.wait(); } }   // no zombies, and the pid is really free
        // the keeper closes right after delivering the last lines: wait for it to exit, so whoever resumes
        // the session (model change, Resume) does not mistake it for another copy of the conversation
        if keeper_pid > 0 {
            let start = Instant::now();
            while is_alive(keeper_pid as i64) && start.elapsed() < Duration::from_secs(10) { std::thread::sleep(Duration::from_millis(100)); }
        }
        if lock_or_recover(to_restart()).remove(&id2) {
            let cwd = resume_dir(&id2);
            let before = load_json(&state_file(&id2)).map(|v| str_of(&v, "stato").to_string()).unwrap_or_default();
            // the old Claude must really be finished, otherwise the resume sees it as "open elsewhere"
            let start = Instant::now();
            while is_alive(pid as i64) && start.elapsed() < Duration::from_secs(10) { std::thread::sleep(Duration::from_millis(100)); }
            let resend_msg = lock_or_recover(to_resend()).remove(&id2);
            match start_with(&app2, &cwd, "", Some(&id2), None) {
                Ok(_) => {
                    update_state(&id2, |v| { if before == "aspetta" { v["stato"] = json!("aspetta"); } });
                    // account change after "quota finished": the message that got no reply restarts by itself
                    if let Some(c) = resend_msg { resend(&id2, &c); }
                }
                Err(e) => {
                    crate::log_juggl(&format!("{id2}: non riparte dopo il cambio di modello o di account: {e}"));
                    // stays resumable by hand, without pretending to be alive
                    update_if_mine(&id2, pid, |v| {
                        v["stato_prima"] = json!(before); v["stato"] = json!("ibernata"); v["ibernata_il"] = json!(unix_now()); v["motivo_ibernazione"] = json!("riavvio non riuscito");
                    });
                }
            }
            { crate::fast_open::forget_id(&id2); let _ = app2.emit("cambiato", ()); }
            return;
        }
        // chat deleted: the state no longer exists and must not be recreated
        if by_me && !state_file(&id2).exists() { { crate::fast_open::forget_id(&id2); let _ = app2.emit("cambiato", ()); } return; }
        // if the user did not close it, it stays resumable. If it was already hibernated (by hand, watchdog) the reason stays.
        // Archived: stays archived.
        let mut resume = false;
        update_if_mine(&id2, pid, |v| {
            if str_of(v, "stato") == "archiviata" {}
            else if by_me { v["stato"] = json!("chiusa"); }
            else if str_of(v, "stato") != "ibernata" && str_of(v, "stato") != "chiusa" {
                let before = str_of(v, "stato").to_string();
                if before == "lavora" { v["turno_interrotto"] = json!(true); }
                v["stato_prima"] = json!(if before == "lavora" { "aspetta".to_string() } else { before });
                v["stato"] = json!("ibernata"); v["ibernata_il"] = json!(unix_now());
                v["motivo_ibernazione"] = json!(if lost { "interrotta" } else { "processo terminato" });
                resume = lost;
            }
        });
        let _ = app2.emit("parziale", json!({"id": id2, "testo": ""}));
        { crate::fast_open::forget_id(&id2); let _ = app2.emit("cambiato", ()); }
        // cap on automatic resumes (Nov 12): if Claude keeps closing by itself (expired login, startup error)
        // the chat does not restart forever. On Windows each restart opened and closed windows (reported by a tester).
        if resume && !resume_allowed(&id2) {
            resume = false;
            crate::log_juggl(&format!("{id2}: Claude si e' chiuso da solo {} volte in 10 minuti: ripresa automatica fermata", MAX_RESUMES));
            update_state(&id2, |v| { v["motivo_ibernazione"] = json!("Claude si chiude da solo: controlla che Claude sia collegato al tuo account (apri \"claude\" in un terminale) e poi riprendi"); });
        }
        if resume { if let Some(f) = RESUME.get() { f(&app2, &id2); { crate::fast_open::forget_id(&id2); let _ = app2.emit("cambiato", ()); } } }
    });
}

/// Automatic resumes after a dropped keeper: at most MAX_RESUMES every 10 minutes per chat.
const MAX_RESUMES: usize = 3;
fn resume_allowed(id: &str) -> bool {
    static RESUMES: Mutex<Option<std::collections::HashMap<String, Vec<f64>>>> = Mutex::new(None);
    let mut g = RESUMES.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(Default::default);
    let now_t = unix_now();
    let l = m.entry(id.to_string()).or_default();
    l.retain(|t| now_t - *t < 600.0);
    if l.len() >= MAX_RESUMES { return false; }
    l.push(now_t);
    true
}

/// Tool Claude is writing (tool_use block arriving): name, real start, input JSON so far.
#[derive(Default)]
struct IncomingTool { nome: String, t0: f64, json: String, ultimo: Option<Instant> }

/// Path of the file from the still incomplete input JSON (only when the string is already closed).
fn file_from_partial_json(j: &str) -> String {
    for k in ["\"file_path\"", "\"notebook_path\""] {
        let Some(i) = j.find(k) else { continue };
        let rest = j[i + k.len()..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('"') else { continue };
        let mut esc = false;
        for (n, c) in rest.char_indices() {
            if esc { esc = false; } else if c == '\\' { esc = true; } else if c == '"' {
                return serde_json::from_str::<String>(&format!("\"{}\"", &rest[..n])).unwrap_or_default();
            }
        }
    }
    String::new()
}

/// One Claude event (one stream-json line).
fn handle_event(app2: &AppHandle, id2: &str, pid: u32, d: &Value, partial: &mut String, last_sent: &mut Instant, seq: u64, with_keeper: bool, pending: &mut bool, ia: &mut Option<IncomingTool>, account_error: &mut bool) {
    // text arrived within the 60 ms window and not yet sent: send it as soon as the block changes or ends,
    // otherwise the bubble stays stuck mid-word until the complete message arrives
    let flush_text = |partial: &String, pending: &mut bool, last_sent: &mut Instant| {
        if *pending { *pending = false; *last_sent = Instant::now(); let _ = app2.emit("parziale", json!({"id": id2, "testo": partial})); }
    };
    let send_tool = |x: &IncomingTool, gen: bool| {
        // live edits (Nov 12): "coda" = last lines being written (Edit/Write), for the Now panel and the iPhone
        let _ = app2.emit("parziale", json!({"id": id2, "strumento": x.nome, "t0": x.t0, "byte": x.json.len(), "file": file_from_partial_json(&x.json), "gen": gen, "coda": crate::live_edits::preview(&x.nome, &x.json)}));
    };
    match str_of(d, "type") {
        "stream_event" => {
            let ev = d.get("event").cloned().unwrap_or(Value::Null);
            match str_of(&ev, "type") {
                "message_start" => { partial.clear(); *pending = false; *ia = None; let _ = app2.emit("parziale", json!({"id": id2, "testo": ""})); }
                "content_block_delta" => {
                    let dl = ev.get("delta").cloned().unwrap_or(Value::Null);
                    match str_of(&dl, "type") {
                        "text_delta" => {
                            partial.push_str(str_of(&dl, "text"));
                            *pending = true;
                            if last_sent.elapsed().as_millis() > 60 { flush_text(partial, pending, last_sent); }
                        }
                        // tool input arriving (Write of a long file: even minutes): real progress, 4 times per second
                        "input_json_delta" => if let Some(x) = ia.as_mut() {
                            x.json.push_str(str_of(&dl, "partial_json"));
                            if x.ultimo.map(|u| u.elapsed().as_millis() > 250).unwrap_or(true) { x.ultimo = Some(Instant::now()); send_tool(x, true); }
                        },
                        _ => {}
                    }
                }
                "content_block_start" => {
                    flush_text(partial, pending, last_sent);
                    let cb = ev.get("content_block").cloned().unwrap_or(Value::Null);
                    if str_of(&cb, "type") == "tool_use" {
                        let x = IncomingTool { nome: str_of(&cb, "name").to_string(), t0: unix_now(), json: String::new(), ultimo: Some(Instant::now()) };
                        send_tool(&x, true);
                        *ia = Some(x);
                    }
                }
                "content_block_stop" => {
                    flush_text(partial, pending, last_sent);
                    if let Some(x) = ia.as_ref() { send_tool(x, false); }
                }
                "message_delta" | "message_stop" => flush_text(partial, pending, last_sent),
                _ => {}
            }
        }
        // engines (Nov 12): the Codex translator says which Codex conversation the chat corresponds to, and the name it gives it
        "system" if str_of(d, "motore") == "codex" => {
            let (th, name) = (str_of(d, "codex_thread").to_string(), str_of(d, "nome").to_string());
            update_if_mine(id2, pid, |v| {
                if !th.is_empty() { v["codex_thread"] = json!(th); }
                if !name.is_empty() && str_of(v, "titolo").is_empty() { v["titolo"] = json!(name); }
            });
            { crate::fast_open::forget_id(&id2); let _ = app2.emit("cambiato", ()); }
        }
        "assistant" | "user" => {
            // account switch (Nov 12): "/xyz isn't available in this environment" -> clear Juggl message (src/claude_login.rs)
            if let Some(mut c) = crate::claude_login::from_event(d) {
                c["id"] = json!(id2);
                let _ = app2.emit("comando-non-disponibile", c.clone());
                crate::bridge::broadcast("comando-non-disponibile", &c.to_string());
            }
            if crate::claude_account::is_account_error(d) { *account_error = true; }
            { crate::fast_open::forget_id(&id2); let _ = app2.emit("cambiato", ()); }
        }
        "result" => {
            let error = std::mem::take(account_error) || crate::claude_account::is_account_error(d) || crate::claude_account::fake_limit(id2);
            crate::fallback::after_turn(id2, error && crate::fallback::limit_reached(d));   // emergency (Nov 12): "Continua su Codex" button
            partial.clear();
            let _ = app2.emit("parziale", json!({"id": id2, "testo": ""}));
            let mut apply = false;
            update_if_mine(id2, pid, |v| {
                if str_of(v, "stato") == "lavora" { v["stato"] = json!("aspetta"); v["da"] = json!(unix_now()); }
                // compaction while stopped (Nov 16): the turn was a Juggl /compact, the chat goes back as it was (no "ti aspetta")
                crate::compaction::end_of_turn(v);
                if let Some(o) = v.as_object_mut() { o.remove("turno_interrotto"); }
                if v.get("regole_da_applicare").and_then(|x| x.as_bool()).unwrap_or(false) { v["regole_da_applicare"] = json!(false); apply = true; }
                // read up to here: on the next reopening it restarts after this event
                if with_keeper { v["custode_seq"] = json!(seq); }
            });
            // quota finished or invalid login with an account different from the current one: restart with the new one and
            // resend the last message (rule 3 of the account switch, src/claude_account.rs)
            if error && prepare_resend(id2, pid) {
                apply = true;
                crate::claude_account::notify(app2, "Account cambiato: chat riavviata, messaggio rimandato".into());
            }
            if apply { restart_queued(id2, pid); }
            // permissions (Nov 12): turn finished (also interrupted): the remaining requests are no longer valid
            crate::permissions::end_of_turn(id2);
            { crate::fast_open::forget_id(&id2); let _ = app2.emit("cambiato", ()); }
        }
        // permissions (Nov 12): Claude asks for a permission (can_use_tool) or withdraws it (src/permissions.rs)
        "control_request" => crate::permissions::request(id2, pid, d),
        "control_cancel_request" => crate::permissions::withdrawn(id2, d),
        _ => {}
    }
}

/// Writes a line on the session stdin. The global lock is only used to take the stdin: the write
/// (even MBs of images with a full pipe) only blocks this session.
fn write_line(id: &str, line: &str) -> Result<(), String> {
    let stdin = table().get(id).map(|o| o.stdin.clone()).ok_or("sessione non ospitata")?;
    let mut si = lock_or_recover(&stdin);
    writeln!(si, "{line}").map_err(|e| e.to_string())?;
    si.flush().map_err(|e| e.to_string())
}

pub fn send(id: &str, text: &str, images: &[String]) -> Result<(), String> {
    if !is_hosted(id) { return Err("sessione non ospitata".into()); }
    write_line(id, &message_with(text, images))?;
    // sent while working (ends up queued): the turn is the same, the start does not move (the timer does not restart)
    update_state(id, |v| { if str_of(v, "stato") != "lavora" { v["da"] = json!(unix_now()); } v["stato"] = json!("lavora"); v["chiesto"] = json!(text.chars().take(200).collect::<String>()); });
    Ok(())
}

// ---------- account switch (Nov 9, src/claude_account.rs) ----------

/// Text to send when the quota ran out in the middle of the work (after commands and replies): resending the initial
/// request would start over from scratch.
pub const MSG_ACCOUNT: &str = "Il turno si era fermato per la quota dell'account Claude di prima. Ora l'account e' cambiato: riprendi da dove eri.";

/// What to resend after the restart, from the last lines of the transcript: the content of the last user message
/// (text and images, as it was) if nothing real happened after it (only API errors); otherwise the
/// "riprendi da dove eri" message.
pub fn choose_resend(lines: &[String]) -> Value {
    let mut last: Option<Value> = None;
    let mut work_after = false;
    for r in lines {
        if !(r.contains("\"type\":\"user\"") || r.contains("\"type\":\"assistant\"")) { continue; }
        let Ok(d) = serde_json::from_str::<Value>(r) else { continue };
        if d.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) || d.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
        let c = &d["message"]["content"];
        match str_of(&d, "type") {
            "user" => {
                let is_result = c.as_array().map(|a| a.iter().any(|x| str_of(x, "type") == "tool_result")).unwrap_or(false);
                // system notices (finished sub-agents, reminders): they are not messages from the user
                let from_system = str_of(&d, "promptSource") == "system" || d.get("origin").map(|o| str_of(o, "kind") != "human").unwrap_or(false);
                if is_result { work_after = true; } else if !from_system && !c.is_null() { last = Some(c.clone()); work_after = false; }
            }
            "assistant" => {
                let error = d.get("isApiErrorMessage").and_then(|x| x.as_bool()).unwrap_or(false) || str_of(&d["message"], "model") == "<synthetic>";
                if !error { work_after = true; }
            }
            _ => {}
        }
    }
    match last { Some(c) if !work_after => c, _ => json!(MSG_ACCOUNT) }
}

/// Plain text of a content (string or blocks) for the log and the requests.
fn text_of(c: &Value) -> String {
    if let Some(t) = c.as_str() { return t.to_string(); }
    c.as_array().map(|a| a.iter().filter(|x| str_of(x, "type") == "text").map(|x| str_of(x, "text")).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// Rule 3: the process got "quota finished" (or invalid login) and started with an account different from the
/// current one. Prepares the message to resend and says yes to the restart. Once every 3 minutes per session: if the
/// new account is also out of quota there is no loop (and the new process has the right account, so it stops there).
fn prepare_resend(id: &str, pid: u32) -> bool {
    let Some(started_with) = table().get(id).filter(|o| o.pid == pid).map(|o| o.account.clone()) else { return false };
    let current_account = crate::claude_account::current_for(id);
    if current_account.is_empty() || crate::claude_account::same_account(&current_account, &started_with) { return false; }   // account switch (Nov 12)
    static RECENT: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
    {
        let mut u = lock_or_recover(&RECENT);
        u.retain(|(_, t)| t.elapsed() < Duration::from_secs(180));
        if u.iter().any(|(i, _)| i == id) { return false; }
        u.push((id.to_string(), Instant::now()));
    }
    let lines = crate::find_transcript(&json!({"sessione": id})).map(|p| crate::tail_file(&p, 3_000_000).0).unwrap_or_default();
    let c = choose_resend(&lines);
    crate::log_juggl(&format!("{id}: quota o login dell'account {} (adesso {}): riparto e rimando {}", crate::claude_account::short(&started_with), crate::claude_account::short(&current_account),
        if c.as_str() == Some(MSG_ACCOUNT) { "\"riprendi da dove eri\"".to_string() } else { format!("l'ultimo messaggio ({} caratteri)", text_of(&c).chars().count()) }));
    lock_or_recover(to_resend()).insert(id.to_string(), c);
    true
}

/// Sends the resent message to the just restarted session (like a user message, images included).
fn resend(id: &str, c: &Value) {
    let text = text_of(c);
    crate::requests::mark_handled_text(id, &text);   // not a new request
    let line = json!({"type": "user", "message": {"role": "user", "content": c}}).to_string();
    match write_line(id, &line) {
        Ok(()) => update_state(id, |v| { v["stato"] = json!("lavora"); v["da"] = json!(unix_now()); v["chiesto"] = json!(text.chars().take(200).collect::<String>()); }),
        Err(e) => crate::log_juggl(&format!("{id}: messaggio da rimandare non consegnato: {e}")),
    }
}

/// Does the Claude process still have background commands (children with the shell snapshot)? `None` where it cannot be known (Windows).
pub fn background_commands(pid: u32) -> Option<bool> {
    #[cfg(windows)] { let _ = pid; return None; }
    #[allow(unreachable_code)]
    {
        let o = crate::new_command("ps").args(["-axo", "ppid=,command="]).output().ok()?;
        Some(String::from_utf8_lossy(&o.stdout).lines().any(|r| {
            let r = r.trim_start();
            r.split_whitespace().next().and_then(|x| x.parse::<u32>().ok()) == Some(pid) && r.contains("shell-snapshots")
        }))
    }
}

/// Does the (stopped) session still have background work? Commands launched with run_in_background (children with the
/// Claude shell snapshot) or sub-agents writing their transcript in the last 2 minutes.
fn busy(id: &str, pid: u32) -> Option<&'static str> {
    if let Some(p) = crate::find_transcript(&json!({"sessione": id})) {
        let dir = p.with_extension("").join("subagents");
        let recent = std::fs::read_dir(&dir).into_iter().flatten().flatten().any(|e| {
            e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map(|d| d < Duration::from_secs(120)).unwrap_or(false)
        });
        if recent { return Some("sotto-agenti al lavoro"); }
    }
    // Windows: no ps; a bash/sh under Claude with the turn finished = background command (Claude uses Git Bash)
    #[cfg(windows)] { return if crate::sys::descendants(pid).iter().any(|n| matches!(n.to_ascii_lowercase().as_str(), "bash.exe" | "sh.exe")) { Some("comandi in background") } else { None }; }
    #[allow(unreachable_code)]
    let o = crate::new_command("ps").args(["-axo", "ppid=,command="]).output().ok()?;
    let commands = String::from_utf8_lossy(&o.stdout).lines().any(|r| {
        let r = r.trim_start();
        r.split_whitespace().next().and_then(|x| x.parse::<u32>().ok()) == Some(pid) && r.contains("shell-snapshots")
    });
    if commands { Some("comandi in background") } else { None }
}

/// What an account alignment round did.
#[derive(Default)]
pub struct Alignment { pub riavviate: Vec<String>, pub a_fine_turno: Vec<String>, pub custodi: Vec<String> }

/// Each hosted session started with an account different from the current one restarts with the same --resume:
/// - stopped: right away (same conversation, new process);
/// - working: `regole_da_applicare`, restarts as soon as the turn ends (like the model change);
/// - stopped but with commands or sub-agents in the background: waits for them to finish, at most 20 minutes.
/// Called at every round (15 s): nothing happens to those already aligned.
/// Queued restarts (0.1.51): 25 chats restarted in the same second = 25 claude processes together and a Mac without memory
/// (115 MB free when installing 0.1.50). One chat at a time: wait for the previous one to have restarted
/// (new process in the table, at most 60 s) and then another 8 s. Applies to account change, old keeper and
/// rules at the end of the turn. A request already queued for the same process is not repeated.
fn restart_queued(id: &str, pid: u32) {
    static QUEUE: OnceLock<Mutex<std::collections::VecDeque<(String, u32)>>> = OnceLock::new();
    static STARTED: std::sync::Once = std::sync::Once::new();
    let queue = QUEUE.get_or_init(|| Mutex::new(Default::default()));
    {
        let mut c = lock_or_recover(queue);
        if c.iter().any(|(i, p)| i == id && *p == pid) { return; }
        c.push_back((id.to_string(), pid));
        if c.len() > 1 { crate::log_juggl(&format!("{id}: riavvio in fila ({} prima)", c.len() - 1)); }
    }
    STARTED.call_once(|| { std::thread::spawn(move || loop {
        let next_item = lock_or_recover(queue).pop_front();
        let Some((id, pid)) = next_item else { std::thread::sleep(Duration::from_millis(500)); continue };
        // meanwhile it already restarted, closed or changed process: nothing to do
        if table().get(&id).map(|o| o.pid) != Some(pid) { continue; }
        restart(&id);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(500));
            if table().get(&id).map(|o| o.pid != pid).unwrap_or(false) { break; }
        }
        std::thread::sleep(Duration::from_secs(8));
    }); });
}

pub fn align_account(account_now: &dyn Fn(&str) -> String) -> Alignment {
    static REQUESTED: Mutex<Vec<(String, u32, Instant)>> = Mutex::new(Vec::new());   // restart already requested for this process
    static WAITING: Mutex<Vec<(String, u32, Instant)>> = Mutex::new(Vec::new());    // since when it is waiting for the background work
    let mut out = Alignment::default();
    let entries: Vec<(String, u32, String, bool, u64)> = table().iter().filter(|(_, o)| o.custode != 0 || !o.account.is_empty())
        .map(|(id, o)| (id.clone(), o.pid, o.account.clone(), o.in_turno.load(Ordering::SeqCst), if o.custode != 0 { o.custode_ver } else { keeper::VERSION })).collect();
    {
        let alive: Vec<u32> = entries.iter().map(|v| v.1).collect();
        lock_or_recover(&REQUESTED).retain(|(_, p, t)| alive.contains(p) && t.elapsed() < Duration::from_secs(600));
        lock_or_recover(&WAITING).retain(|(_, p, _)| alive.contains(p));
    }
    for (id, pid, started_with, in_turn, keeper_ver) in entries {
        let a = account_now(&id);
        let account_changed = !(a.is_empty() || started_with.is_empty() || crate::claude_account::same_account(&a, &started_with));   // account switch (Nov 12): credentials signature
        // old keeper (0.1.50): restarts with the new one, NEVER during a turn nor with sub-agents or commands in the
        // background: stopped = right away, working = at the end of the turn (regole_da_applicare), busy = waits without limit
        let old_keeper = keeper::needs_update(keeper_ver);
        if !account_changed && !old_keeper { continue; }
        let reason = if account_changed { format!("account cambiato ({} -> {})", crate::claude_account::short(&started_with), crate::claude_account::short(&a)) } else { format!("custode vecchio (versione {keeper_ver}, nuova {})", keeper::VERSION) };
        if lock_or_recover(&REQUESTED).iter().any(|(_, p, _)| *p == pid) { continue; }
        let st = load_json(&state_file(&id)).unwrap_or(json!({}));
        if matches!(str_of(&st, "stato"), "archiviata" | "chiusa") { continue; }
        if in_turn || str_of(&st, "stato") == "lavora" {
            let mut marked = false;
            update_if_mine(&id, pid, |v| { if !v.get("regole_da_applicare").and_then(|x| x.as_bool()).unwrap_or(false) { v["regole_da_applicare"] = json!(true); marked = true; } });
            if marked {
                crate::log_juggl(&format!("{id}: {reason}, sta lavorando: riparte a fine turno"));
                if account_changed { out.a_fine_turno.push(id); }
            }
            continue;
        }
        if let Some(cause) = busy(&id, pid) {
            let mut at = lock_or_recover(&WAITING);
            let since = match at.iter().find(|(i, p, _)| i == &id && *p == pid) { Some(x) => x.2, None => { at.push((id.clone(), pid, Instant::now())); crate::log_juggl(&format!("{id}: {reason}, aspetto ({cause})")); Instant::now() } };
            if !account_changed || since.elapsed() < Duration::from_secs(1200) { continue; }
            crate::log_juggl(&format!("{id}: aspettavo da 20 minuti ({cause}): riparto lo stesso"));
        }
        lock_or_recover(&REQUESTED).push((id.clone(), pid, Instant::now()));
        crate::log_juggl(&format!("{id}: {reason}: riparto con --resume"));
        restart_queued(&id, pid);
        if account_changed { out.riavviate.push(id); } else { out.custodi.push(id); }
    }
    out
}

/// Stops a sub-agent (or a background command) of the session, like Claude's TaskStop: stop_task control command
/// on stdin. Tested on Nov 10 with Claude Code 2.1.321: reply "success", the agent ends "killed" and
/// the session receives the usual task-notification.
pub fn stop_task(id: &str, task: &str) -> Result<(), String> {
    let r = json!({"type": "control_request", "request_id": uuid(), "request": {"subtype": "stop_task", "task_id": task}});
    write_line(id, &r.to_string())
}

/// permissions (Nov 12): reply to a Claude control_request (permission granted or denied), on the session stdin.
pub fn reply_control(id: &str, response: &Value) -> Result<(), String> { write_line(id, &response.to_string()) }

/// Stops the current turn (like Esc in the terminal), the session stays open.
pub fn interrupt(id: &str) -> Result<(), String> {
    let r = json!({"type": "control_request", "request_id": uuid(), "request": {"subtype": "interrupt"}});
    write_line(id, &r.to_string())
}

pub fn close(id: &str, hibernate: bool) {
    let mut t = table();
    if let Some(o) = t.get_mut(id) {
        o.chiusa_da_me = !hibernate;
        terminate_group(o.pid as i64);
    }
}

/// On app exit (Cmd+Q, reinstall). Sessions in the keeper stay alive and keep working: only the last event read
/// is saved, to resume from there. Those in the old mode become hibernated (resumable at the
/// next opening; if they were working they receive "riprendi da dove eri").
pub fn shut_all() {
    QUITTING.store(true, Ordering::SeqCst);
    let entries: Vec<(String, u32, u32, u64)> = table().iter().map(|(id, o)| (id.clone(), o.pid, o.custode, o.seq.load(Ordering::SeqCst))).collect();
    for (id, pid, keeper_pid, seq) in entries {
        if keeper_pid > 0 {
            update_if_mine(&id, pid, |v| { v["custode_seq"] = json!(seq); v["agg"] = json!(unix_now()); });
            continue;
        }
        hibernate_on_quit(&id, pid, "app chiusa");
        signal_group(pid as i64, libc::SIGTERM);
    }
}

fn hibernate_on_quit(id: &str, pid: u32, reason: &str) {
    update_if_mine(id, pid, |v| {
        let before = str_of(v, "stato").to_string();
        if before == "ibernata" || before == "chiusa" { return; }
        if before == "lavora" { v["turno_interrotto"] = json!(true); }
        v["stato_prima"] = json!(if before == "lavora" { "aspetta".to_string() } else { before });
        v["stato"] = json!("ibernata"); v["ibernata_il"] = json!(unix_now()); v["motivo_ibernazione"] = json!(reason);
    });
}

/// Quit everything: really stops every hosted session (also those in keepers not connected to this Juggl).
/// They become hibernated "esci da tutto": they do not restart by themselves, Resume makes them continue from the interrupted turn.
pub fn stop_all() {
    QUITTING.store(true, Ordering::SeqCst);
    let entries: Vec<(String, u32)> = table().iter().map(|(id, o)| (id.clone(), o.pid)).collect();
    for (id, pid) in &entries { hibernate_on_quit(id, *pid, "esci da tutto"); }
    // also the keepers without Juggl (sessions never reconnected): the state is marked from their file
    for id in keeper::alive() {
        if entries.iter().any(|(i, _)| *i == id) { continue; }
        let pid = load_json(&state_file(&id)).and_then(|v| v.get("pid").and_then(|x| x.as_u64())).unwrap_or(0) as u32;
        hibernate_on_quit(&id, pid, "esci da tutto");
    }
    keeper::stop_all();
    for (_, pid) in entries { terminate_group(pid as i64); }
}

/// "Esci da tutto" menu item: stops all sessions and closes the app (on another thread: the menu does not block).
pub fn quit_all(app: AppHandle) {
    std::thread::spawn(move || { stop_all(); app.exit(0); });
}

#[cfg(test)]
mod account_tests {
    use super::*;
    fn r(v: Value) -> String { v.to_string() }

    #[test]
    fn resends_last_unanswered_message() {
        let lines = vec![
            r(json!({"type": "user", "promptSource": "sdk", "message": {"role": "user", "content": "prima"}})),
            r(json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "content": [{"type": "text", "text": "ok"}]}})),
            r(json!({"type": "user", "promptSource": "sdk", "message": {"role": "user", "content": [{"type": "text", "text": "seconda"}, {"type": "image", "source": {}}]}})),
            r(json!({"type": "assistant", "isApiErrorMessage": true, "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "You've hit your weekly limit"}]}})),
        ];
        let c = choose_resend(&lines);
        assert_eq!(c[0]["text"], "seconda");
        assert_eq!(c[1]["type"], "image");   // images included
    }

    #[test]
    fn mid_work_resumes() {
        let lines = vec![
            r(json!({"type": "user", "promptSource": "sdk", "message": {"role": "user", "content": "fai il deploy"}})),
            r(json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "content": [{"type": "tool_use", "name": "Bash"}]}})),
            r(json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "content": "ok"}]}})),
            r(json!({"type": "assistant", "isApiErrorMessage": true, "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "You've hit your weekly limit"}]}})),
        ];
        assert_eq!(choose_resend(&lines), json!(MSG_ACCOUNT));
    }

    #[test]
    fn system_notices_and_sidechain_do_not_count() {
        let lines = vec![
            r(json!({"type": "user", "promptSource": "sdk", "message": {"role": "user", "content": "vera"}})),
            r(json!({"type": "user", "isSidechain": true, "message": {"role": "user", "content": "del sotto-agente"}})),
            r(json!({"type": "user", "promptSource": "system", "origin": {"kind": "task-notification"}, "message": {"role": "user", "content": "<task-notification>"}})),
            r(json!({"type": "assistant", "isApiErrorMessage": true, "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "You've hit your weekly limit"}]}})),
        ];
        assert_eq!(choose_resend(&lines), json!("vera"));
    }
    #[cfg(unix)]
    #[test]
    fn orphan_not_stopped_if_managed_or_with_terminal_and_single_instance() {
        use std::os::unix::io::AsRawFd;
        use std::os::unix::process::CommandExt;
        let dir = std::env::temp_dir().join(format!("juggl-prova-orfano-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("stato")).unwrap();
        let _base_guard = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (process-wide variable)
        std::env::set_var("JUGGL_BASE", &dir);
        let id = "11111111-2222-3333-4444-555555555555";
        // fake process without a terminal (setsid): "claude --resume <id>"
        let mut child = unsafe {
            crate::new_command("sleep").arg("30").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
                .pre_exec(|| { libc::setsid(); Ok(()) }).spawn().unwrap()
        };
        let pid = child.id() as i64;
        std::thread::sleep(Duration::from_millis(300));
        assert!(!has_terminal(pid), "un processo con setsid non ha terminale");
        // 1) no keeper in the state: it is an orphan, it can be stopped
        std::fs::write(dir.join("stato").join(format!("{id}.json")), json!({"sessione": id}).to_string()).unwrap();
        assert!(can_stop(id, pid));
        // 2) keeper alive (myself): NOT stopped
        std::fs::write(dir.join("stato").join(format!("{id}.json")), json!({"sessione": id, "custode_pid": std::process::id()}).to_string()).unwrap();
        assert!(!can_stop(id, pid));
        // 3) keeper dead: stopped again
        std::fs::write(dir.join("stato").join(format!("{id}.json")), json!({"sessione": id, "custode_pid": 999_999_999i64}).to_string()).unwrap();
        assert!(can_stop(id, pid));
        // 4) process with a terminal (pid 1 is launchd/init without tty, use my test runner if it has a tty): if it has one, not stopped
        if has_terminal(std::process::id() as i64) { assert!(!can_stop(id, std::process::id() as i64)); }
        let _ = child.kill(); let _ = child.wait();
        // single instance: the first copy takes the lock, a second one does not get it
        assert!(main_instance());
        let f2 = std::fs::OpenOptions::new().read(true).write(true).open(dir.join("istanza.lock")).unwrap();
        assert_ne!(unsafe { libc::flock(f2.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0, "il blocco di istanza deve essere tenuto");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
