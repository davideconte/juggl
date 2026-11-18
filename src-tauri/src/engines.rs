// engines (Nov 12): Juggl with Codex (ChatGPT account) besides Claude Code. Gemini later.
//
// The Codex chat lives in the keeper like the Claude ones: the keeper launches `codex app-server` and translates the
// Codex protocol into Claude's stream-json lines (src/programs/codex_engine.rs), and writes the "mirror" of the
// conversation in ~/tavolo/motori/<id>.jsonl in Claude's transcript format. The chat state points to the
// mirror ("transcript"), so history, search, summaries and naming work with no extra code.
//
// Here: where Codex lives, permission levels -> approvalPolicy/sandbox, configuration for the keeper, login
// status and "Sign in with ChatGPT" (account/login/start: opens the browser, no terminal).
use crate::NoConsoleWindow;
use crate::{base_dir, str_of};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Chat engine: "codex" or "claude" (also for old chats without the field).
pub fn engine_of(st: &Value) -> &str { if str_of(st, "motore") == "codex" { "codex" } else { "claude" } }

pub fn mirror_path(id: &str) -> PathBuf { base_dir().join("motori").join(format!("{id}.jsonl")) }

/// The Codex program: the most recent between the one in PATH and the one the Codex app updates by itself in
/// ~/.codex/packages. The hosted 0.160 (Homebrew) could not run commands (timeout), 0.167.2 can.
pub fn codex_cmd() -> Option<PathBuf> {
    static C: Mutex<Option<(Instant, Option<PathBuf>)>> = Mutex::new(None);
    if let Some((t, p)) = C.lock().unwrap_or_else(|e| e.into_inner()).as_ref() { if t.elapsed() < Duration::from_secs(300) { return p.clone(); } }
    let mut candidates: Vec<PathBuf> = codex_in_path().into_iter().collect();
    let rel = crate::home().join(".codex/packages/app-server-daemon/releases");
    for d in std::fs::read_dir(&rel).into_iter().flatten().flatten() {
        let p = d.path().join("bin").join(format!("codex{}", std::env::consts::EXE_SUFFIX));
        if p.is_file() { candidates.push(p); }
    }
    let best = candidates.into_iter().map(|p| (version_of(&p), p)).filter(|(v, _)| !v.is_empty()).max_by(|a, b| a.0.cmp(&b.0)).map(|x| x.1);
    *C.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), best.clone()));
    best
}

/// Version as comparable numbers ("codex-cli 0.167.2" -> [0, 167, 2]); empty if the program does not answer.
fn version_of(p: &std::path::Path) -> Vec<u64> {
    let o = crate::new_command(p).no_console_window().arg("--version").env("TAVOLO_INTERNO", "1").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    o.split_whitespace().last().unwrap_or("").split('.').map(|x| x.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0)).collect()
}

fn codex_in_path() -> Option<PathBuf> {
    let path = crate::hosted::path_login();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut dirs: Vec<PathBuf> = path.split(sep).filter(|x| !x.is_empty()).map(PathBuf::from).collect();
    for d in ["/opt/homebrew/bin", "/usr/local/bin"] { dirs.push(d.into()); }
    dirs.push(crate::home().join(".local/bin"));
    for d in dirs {
        #[cfg(windows)]
        {
            let exe = d.join("codex.exe");
            if exe.is_file() { return Some(exe); }
            if d.join("codex.cmd").is_file() {
                let v = d.join("node_modules/@openai/codex/vendor/x86_64-pc-windows-msvc/codex/codex.exe");
                if v.is_file() { return Some(v); }
            }
        }
        #[cfg(not(windows))]
        { let p = d.join("codex"); if p.is_file() { return Some(p); } }
    }
    None
}

/// Juggl permission level (permissions.rs) -> Codex (approvalPolicy, sandbox).
/// "mai" (Never ask, chosen by the user): no prompts and no sandbox, like --dangerously-skip-permissions.
/// "modifiche": files in the folder are free, asks for the rest. "chiedi": asks even for "trusted" commands.
pub fn codex_level(level: &str) -> (&'static str, &'static str) {
    match level { "mai" => ("never", "danger-full-access"), "modifiche" => ("on-request", "workspace-write"), _ => ("untrusted", "workspace-write") }
}

/// Essential Juggl rules for Codex and Antigravity, as system instructions. The full rules also come from
/// ~/.codex/AGENTS.md and ~/.gemini/GEMINI.md (generated from memory), but the small model of ChatGPT's free plan
/// ignored them; as system instructions it follows them (tested Nov 12: it ends with "Restano:").
pub const JUGGL_RULES: &str = "Stai lavorando dentro Juggl, l'app con cui «NOME» segue tutte le sue chat. Regole fisse, in ogni risposta:\n- Scrivi in italiano semplice, frasi piane, niente trattini lunghi.\n- Se restano richieste di «NOME» non ancora fatte, chiudi la risposta con una riga che inizia esattamente con \"Restano:\" seguita dall'elenco breve (es. \"Restano: logo, landing\").\n- Se ti serve una decisione o un ok di «NOME», scrivila in fondo con la formula esatta \"Serve il tuo ok: ...\" (prima di \"Restano:\").\n- Le chat locali di Juggl e i compiti del modello locale del PC non vanno mai al cloud: non leggerli e non citarli.";

/// emergency (Nov 12): fallback chat when Claude has used up the week ("prudente" state). The user's rule.
#[cfg(not(feature = "personale"))]
pub const CAUTIOUS_RULES: &str = "Questa chat e' un ripiego: Claude ha finito la settimana e lavori tu al suo posto, in modalita' prudente. Liberi: leggere, cercare, scrivere codice e file in locale, preparare bozze. MAI senza un ok esplicito di «NOME» in questa chat: messaggi o email a clienti o ad altri, deploy o pubblicazioni, Meta Ads, pagamenti, cancellazioni, riavvii del PC o dei server. Per queste cose prepara tutto e chiedi con \"Serve il tuo ok: ...\".";

/// Configuration to pass to the keeper (`--motore codex <codex> <json>`).
pub fn codex_config(id: &str, cwd: &str, st: &Value, model: &str, level: &str, instructions: &str) -> Value {
    let (ap, sb) = codex_level(level);
    let mut c = json!({"id": id, "cwd": cwd, "specchio": mirror_path(id).to_string_lossy(), "approvalPolicy": ap, "sandbox": sb});
    let th = str_of(st, "codex_thread");
    if !th.is_empty() { c["thread"] = json!(th); } else if let Some(t) = thread_from_mirror(id) { c["thread"] = json!(t); }
    // Claude models do not apply to Codex: pass only names that are not Claude's
    if !model.is_empty() && !matches!(model, "opus" | "sonnet" | "haiku" | "fable") && !model.starts_with("claude") { c["model"] = json!(model); }
    let e = str_of(st, "effort");
    if matches!(e, "low" | "medium" | "high" | "xhigh") { c["effort"] = json!(e); }
    // user's name in place of «NOME» (the name is not in the program: pubblica.sh privacy check)
    let (rj, rp) = (crate::profile::with_name(JUGGL_RULES), crate::profile::with_name(CAUTIOUS_RULES));
    c["istruzioni"] = json!(if instructions.is_empty() { rj.clone() } else { format!("{rj}\n\n{instructions}") });
    if st.get("prudente").and_then(|x| x.as_bool()).unwrap_or(false) { c["istruzioni"] = json!(format!("{}\n\n{rp}", str_of(&c, "istruzioni"))); }   // emergency (Nov 12)
    c
}

/// Codex conversation id written in the first line of the mirror (if the state lost it).
pub fn thread_from_mirror(id: &str) -> Option<String> {
    let f = std::fs::File::open(mirror_path(id)).ok()?;
    for r in BufReader::new(f).lines().map_while(Result::ok).take(5) {
        if let Ok(v) = serde_json::from_str::<Value>(&r) { let t = str_of(&v, "codex_thread"); if !t.is_empty() { return Some(t.to_string()); } }
    }
    None
}

// ---------- sign-in with the ChatGPT account ----------

/// A short-lived `codex app-server` for account questions. Returns the answer to the `method` request.
fn short_request(method: &str, params: Value, timeout: Duration) -> Result<(Value, std::process::Child, BufReader<std::process::ChildStdout>), String> {
    let codex = codex_cmd().ok_or("Codex non e' installato")?;
    let mut c = crate::new_command(codex).no_console_window().arg("app-server").env("PATH", crate::hosted::path_login()).env("TAVOLO_INTERNO", "1")
        .current_dir(crate::home()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().map_err(|e| format!("Codex non parte: {e}"))?;
    let mut w = c.stdin.take().ok_or("stdin")?;
    let mut r = BufReader::new(c.stdout.take().ok_or("stdout")?);
    let _ = writeln!(w, "{}", json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "juggl", "version": "1"}}}));
    let _ = writeln!(w, "{}", json!({"method": "initialized"}));
    let _ = writeln!(w, "{}", json!({"id": 2, "method": method, "params": params}));
    let _ = w.flush();
    std::mem::forget(w);   // stdin open as long as the process is needed (login: waits for the browser)
    let start = Instant::now();
    let mut line = String::new();
    while start.elapsed() < timeout {
        line.clear();
        if r.read_line(&mut line).unwrap_or(0) == 0 { break; }
        let Ok(d) = serde_json::from_str::<Value>(line.trim()) else { continue };
        if d.get("id").and_then(|x| x.as_u64()) == Some(2) {
            if let Some(e) = d.get("error") { let _ = c.kill(); return Err(str_of(e, "message").to_string()); }
            return Ok((d["result"].clone(), c, r));
        }
    }
    let _ = c.kill();
    Err("Codex non risponde".into())
}

/// Codex status for Settings: installed, version, login (ChatGPT plan). Cached for 60 seconds.
#[tauri::command(async)]
pub fn motori_stato() -> Value {
    static CACHE: Mutex<Option<(Instant, Value)>> = Mutex::new(None);
    if let Some((t, v)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() { if t.elapsed() < Duration::from_secs(60) { return v.clone(); } }
    let codex = match codex_cmd() {
        None => json!({"installato": false}),
        Some(p) => {
            let version = crate::new_command(&p).no_console_window().arg("--version").env("TAVOLO_INTERNO", "1").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
            match short_request("account/read", json!({"refreshToken": true}), Duration::from_secs(15)) {
                Ok((r, mut c, _)) => {
                    let _ = c.kill(); let _ = c.wait();
                    let a = &r["account"];
                    json!({"installato": true, "versione": version, "accesso": !a.is_null(), "tipo": str_of(a, "type"), "email": str_of(a, "email"), "piano": str_of(a, "planType")})
                }
                Err(e) => json!({"installato": true, "versione": version, "accesso": false, "errore": e}),
            }
        }
    };
    let v = json!({"codex": codex});
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), v.clone()));
    v
}

/// "Sign in with ChatGPT": Codex prepares the link, Juggl opens it in the browser; the process stays alive (at most 10
/// minutes) to receive the browser's answer and save the login in ~/.codex.
#[tauri::command(async)]
pub fn motori_entra_codex() -> Value {
    match short_request("account/login/start", json!({"type": "chatgpt"}), Duration::from_secs(20)) {
        Ok((r, mut c, mut reader)) => {
            let url = str_of(&r, "authUrl").to_string();
            if url.is_empty() { let _ = c.kill(); return json!({"ok": false, "msg": "Codex non ha dato il link per entrare"}); }
            crate::apri_link(url.clone());
            std::thread::spawn(move || {
                let start = Instant::now();
                let mut line = String::new();
                while start.elapsed() < Duration::from_secs(600) {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 { break; }
                    if line.contains("\"account/login/completed\"") {
                        crate::log_juggl(&format!("codex: accesso {}", if line.contains("\"success\":true") { "riuscito" } else { "non riuscito" }));
                        break;
                    }
                }
                let _ = c.kill(); let _ = c.wait();
            });
            json!({"ok": true, "url": url})
        }
        Err(e) => json!({"ok": false, "msg": e}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_levels() {
        assert_eq!(codex_level("mai"), ("never", "danger-full-access"));
        assert_eq!(codex_level("modifiche"), ("on-request", "workspace-write"));
        assert_eq!(codex_level("chiedi"), ("untrusted", "workspace-write"));
        assert_eq!(codex_level(""), ("untrusted", "workspace-write"));
    }
    #[test]
    fn claude_models_not_passed() {
        let c = codex_config("x", "/tmp", &json!({"effort": "max"}), "opus", "chiedi", "");
        assert!(c.get("model").is_none() && c.get("effort").is_none());
        let c = codex_config("x", "/tmp", &json!({"effort": "high", "codex_thread": "t1"}), "gpt-6.1-sol", "mai", "");
        assert_eq!(c["model"], "gpt-6.1-sol"); assert_eq!(c["effort"], "high"); assert_eq!(c["thread"], "t1");
    }
}
