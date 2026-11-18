// Migration of sessions from the cmux terminal to Juggl's keeper (Nov 8, "move those on cmux there too").
//
// For each Claude session running in a cmux tab:
//  1. it must be idle: no turn in progress (Claude Code says "idle"), turn finished in the transcript, no
//     sub-agents at work, no open question or unsent text in the input box. Otherwise it is skipped, saying why;
//  2. it closes the tab's Claude (TERM, KILL after 15 s) and checks that it is dead and that no other process
//     holds the transcript open;
//  3. it reopens it in the keeper with --resume, in its folder, with the same model and reasoning;
//  4. it updates the state (no more tab) and closes the cmux tab;
//  5. it writes everything to ~/tavolo/migrazioni.jsonl.
// The "Juggl (sviluppo)" session never enters --tutte: it is migrated separately, with quando_ferma (it waits by itself
// for the end of its own turn, because it is the one launching the command).
#[cfg(windows)] use crate::sys as libc;
use crate::NoConsoleWindow;
use crate::{storage, base_dir, home, load_json, log_juggl, unix_now, hosted, str_of, is_alive};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The session that coordinates work on Juggl: never inside "--tutte".
pub const DEV_SESSION: &str = "c130039d-bacd-4a7f-a99b-e2c75a8e63f2";

fn state_file(id: &str) -> PathBuf { base_dir().join("stato").join(format!("{id}.json")) }

fn log_entry(entry: &Value) {
    use std::io::Write;
    let mut v = entry.clone();
    v["t"] = json!(unix_now());
    if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(base_dir().join("migrazioni.jsonl")) { let _ = writeln!(fh, "{v}"); }
    log_juggl(&format!("migra: {v}"));
}

/// State of the sessions according to Claude Code (`claude agents --json`): id -> (pid, "idle"|"busy"|...).
fn claude_agents() -> std::collections::HashMap<String, (i64, String)> {
    let mut m = std::collections::HashMap::new();
    let Ok(o) = crate::new_command(crate::profile::claude_cmd()).no_console_window().args(["agents", "--json"]).env("TAVOLO_INTERNO", "1").current_dir(home()).output() else { return m };
    let Ok(Value::Array(a)) = serde_json::from_slice::<Value>(&o.stdout) else { return m };
    for x in a {
        let id = str_of(&x, "sessionId").to_string();
        let st = [str_of(&x, "status"), str_of(&x, "state")].into_iter().find(|t| !t.is_empty()).unwrap_or("").to_string();
        m.insert(id, (x.get("pid").and_then(|p| p.as_i64()).unwrap_or(0), st));
    }
    m
}

/// Text typed in Claude's input box and not yet sent (we would lose it by closing).
fn text_in_box(sf: &str) -> Option<String> {
    let o = crate::cmux(&["read-screen", "--surface", sf, "--lines", "12"])?;
    let t = String::from_utf8_lossy(&o.stdout).to_string();
    let r = t.lines().rev().find(|r| r.trim_start_matches(['│', ' ']).starts_with('❯'))?;
    // the box lines (─, │) sometimes end up on the same row as the cursor: they are not text
    let inside: String = r.trim_start_matches(['│', ' ']).trim_start_matches('❯').chars().filter(|c| !matches!(c, '─' | '│' | '╭' | '╮' | '╰' | '╯')).collect::<String>().trim().to_string();
    // Claude's grey suggestion ("Try ...") is not the user's text
    if inside.is_empty() || inside.starts_with("Try \"") { None } else { Some(inside) }
}

/// Claude sessions still living in a cmux tab. With `anche` we also accept that terminal session which cmux has
/// not registered (no known tab: it is moved only by asking for it by id).
fn candidates(also: &str) -> Vec<Value> {
    crate::list_sessions_with_pid().into_iter().filter(|v| {
        let pid = v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
        str_of(v, "ospite") != "tavolo" && (!str_of(v, "surface").is_empty() || (!also.is_empty() && str_of(v, "sessione") == also))
            && str_of(v, "agente") != "codex" && pid > 0 && is_alive(pid)
            && storage::is_valid_id(str_of(v, "sessione")) && !str_of(v, "sessione").starts_with("locale:")
    }).collect()
}

/// Ready to move now? Ok, or the reason to wait.
/// forza_turno: the last turn in the transcript may have been left open (interrupted a while ago): if Claude Code says it is
/// "idle" and the box is empty, I move it anyway (Juggl's "lavora" state in that case is stale).
fn is_ready(v: &Value, agents: &std::collections::HashMap<String, (i64, String)>, force_turn: bool) -> Result<(), String> {
    let id = str_of(v, "sessione");
    let pid = v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
    if !hosted::is_claude(pid) { return Err(format!("il processo {pid} non e' Claude")); }
    let idle = match agents.get(id) {
        Some((_, st)) if st != "idle" => return Err(format!("Claude Code la dice \"{st}\": sta lavorando")),
        None if matches!(str_of(v, "vista"), "lavora") => return Err("sta lavorando".into()),
        Some(_) => true,
        None => false,
    };
    if force_turn && (!idle || str_of(v, "surface").is_empty()) { return Err("per forzare serve che Claude Code la dica idle e che la scheda cmux sia nota".into()); }
    if str_of(v, "vista") == "bloccata" { return Err("ha una domanda aperta".into()); }
    let sub = v.get("sotto_attivi").and_then(|x| x.as_u64()).unwrap_or(0);
    if sub > 0 { return Err(format!("{sub} sotto-agenti al lavoro")); }
    if !force_turn && !hosted::turn_finished(id) { return Err("il turno nel transcript non e' finito (se nel terminale e' ferma: --forza-turno)".into()); }
    let sf = str_of(v, "surface");
    if sf.is_empty() {
        // without a tab I cannot read the screen: only the state given by Claude Code decides
        if !agents.contains_key(id) { return Err("nessuna scheda cmux e Claude Code non ne conosce lo stato".into()); }
    } else if !crate::terminal_question(sf).is_null() { return Err("domanda aperta nel terminale".into()); }
    if sf.is_empty() {} else if let Some(t) = text_in_box(sf) { return Err(format!("nella casella c'e' testo non inviato: \"{}\"", t.chars().take(60).collect::<String>())); }
    if let Some(p) = crate::find_transcript(v) {
        let idle_secs = std::fs::metadata(&p).ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map(|d| d.as_secs()).unwrap_or(999);
        if idle_secs < 20 { return Err("ha scritto nel transcript pochi secondi fa".into()); }
    }
    Ok(())
}

/// Model of the last answer in the transcript (to reopen it with the same one).
fn model_from_transcript(v: &Value) -> String {
    let Some(p) = crate::find_transcript(v) else { return String::new() };
    let (lines, _) = crate::tail_file(&p, 600_000);
    for r in lines.iter().rev() {
        if !r.contains("\"type\":\"assistant\"") { continue; }
        let Ok(d) = serde_json::from_str::<Value>(r) else { continue };
        if d.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
        let m = d.get("message").map(|m| str_of(m, "model").to_string()).unwrap_or_default();
        if m.starts_with("claude-") { return m; }
    }
    String::new()
}

/// Working folder of a process (lsof).
fn process_cwd(pid: i64) -> String {
    let Ok(o) = crate::new_command("/usr/sbin/lsof").args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"]).output() else { return String::new() };
    String::from_utf8_lossy(&o.stdout).lines().find_map(|r| r.strip_prefix('n').map(|x| x.to_string())).unwrap_or_default()
}

/// Pids holding the transcript open (lsof), excluding Juggl.
fn who_holds_transcript(p: &std::path::Path) -> Vec<i64> {
    let Ok(o) = crate::new_command("/usr/sbin/lsof").arg("-t").arg(p).output() else { return vec![] };
    String::from_utf8_lossy(&o.stdout).lines().filter_map(|x| x.trim().parse::<i64>().ok()).filter(|p| *p as u32 != std::process::id()).collect()
}

/// Closes the session's cmux tab (the workspace is needed too: without it, cmux looks in its own). If it was the only
/// tab of the workspace, cmux does not close it: then I close the workspace. cmux sometimes does not answer for a few seconds.
fn close_tab(sf: &str, ws: &str) -> String {
    for attempt in 0..4 {
        if attempt > 0 { std::thread::sleep(Duration::from_secs(3)); }
        let mut a = vec!["close-surface", "--surface", sf];
        if !ws.is_empty() { a.extend(["--workspace", ws]); }
        let Some(o) = crate::cmux(&a) else { continue };
        let msg = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        if o.status.success() { return "scheda cmux chiusa".into(); }
        if msg.contains("last surface") && !ws.is_empty() {
            let ok = crate::cmux(&["close-workspace", "--workspace", ws]).map(|o| o.status.success()).unwrap_or(false);
            return if ok { "spazio cmux chiuso (era la sua unica scheda)".into() } else { format!("spazio cmux {ws} non chiuso (chiudilo a mano)") };
        }
        if msg.contains("not found") { return "scheda cmux gia' chiusa".into(); }
    }
    format!("scheda cmux {sf} non chiusa (cmux non risponde: chiudila a mano)")
}

/// Moves ONE session (already checked) from the terminal to the keeper.
fn migrate_one(app: &tauri::AppHandle, v: &Value) -> Value {
    let id = str_of(v, "sessione").to_string();
    let name = str_of(v, "nome").to_string();
    let pid = v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
    let sf = str_of(v, "surface").to_string();
    let p = state_file(&id);
    let before = load_json(&p).unwrap_or(json!({}));
    let prev_view = str_of(v, "vista").to_string();
    let model = { let m = str_of(&before, "modello").to_string(); if m.is_empty() { model_from_transcript(v) } else { m } };
    let mut steps: Vec<String> = vec![];
    // folder where the terminal's Claude was running (before closing it): if it is the transcript's project folder
    // I reopen it there, identical to a `claude --resume` given in the tab
    let proc_cwd = process_cwd(pid);
    let failed = |steps: &Vec<String>, msg: String| {
        // the terminal's Claude is already stopped: the session stays hibernated and resumes inside Juggl with Riprendi
        let _ = storage::modify(&p, None, |x| {
            x["stato"] = json!("ibernata"); x["ibernata_il"] = json!(unix_now()); x["motivo_ibernazione"] = json!("spostamento da cmux non riuscito");
            x["ospite"] = json!("tavolo");
            if let Some(o) = x.as_object_mut() { o.remove("surface"); }
            true
        });
        let r = json!({"id": id, "nome": name, "esito": "errore", "msg": msg, "passi": steps});
        log_entry(&r);
        r
    };
    // 1. from here on the terminal hook no longer touches the state (Juggl decides): on close it does not mark it "chiusa"
    let _ = storage::modify(&p, None, |x| { x["ospite"] = json!("tavolo"); x["migrazione"] = json!(unix_now()); true });
    // 2. close the tab's Claude
    hosted::signal_group(pid, libc::SIGTERM);
    let start = Instant::now();
    while is_alive(pid) && start.elapsed() < Duration::from_secs(15) { std::thread::sleep(Duration::from_millis(200)); }
    if is_alive(pid) {
        steps.push("non si chiudeva con TERM: KILL".into());
        hosted::signal_group(pid, libc::SIGKILL);
        std::thread::sleep(Duration::from_millis(800));
    }
    if is_alive(pid) { return failed(&steps, format!("il Claude del terminale (pid {pid}) non si chiude")); }
    steps.push(format!("chiuso il Claude del terminale (pid {pid}) in {:.1} s", start.elapsed().as_secs_f64()));
    // session left in Claude Code's daemon (in background): I stop it, the conversation stays saved
    if claude_agents().contains_key(&id) {
        let _ = crate::new_command(crate::profile::claude_cmd()).no_console_window().args(["stop", &id]).env("TAVOLO_INTERNO", "1").current_dir(home()).output();
        steps.push("fermata anche nel demone di Claude Code".into());
    }
    // nobody else must have the conversation open
    std::thread::sleep(Duration::from_millis(600));
    if let Some(other) = hosted::conversation_process(&id) { return failed(&steps, format!("la conversazione e' ancora aperta nel processo {other}")); }
    if let Some(t) = crate::find_transcript(v) {
        let start = Instant::now();
        let mut holders = who_holds_transcript(&t);
        while !holders.is_empty() && start.elapsed() < Duration::from_secs(5) { std::thread::sleep(Duration::from_millis(300)); holders = who_holds_transcript(&t); }
        if !holders.is_empty() { return failed(&steps, format!("il transcript e' ancora aperto da {holders:?}")); }
    }
    // 3. clean state and reopening in the keeper, same folder and same model (reasoning is in the state)
    let _ = storage::modify(&p, None, |x| {
        if let Some(o) = x.as_object_mut() { o.remove("surface"); o.remove("workspace"); o.remove("migrazione"); }
        if !model.is_empty() && str_of(x, "modello").is_empty() { x["modello"] = json!(model); }
        true
    });
    let transcript_dir = crate::find_transcript(v).and_then(|t| t.parent().and_then(|d| d.file_name()).map(|d| d.to_string_lossy().to_string())).unwrap_or_default();
    let cwd = if !proc_cwd.is_empty() && hosted::claude_project(&proc_cwd) == transcript_dir && std::path::Path::new(&proc_cwd).is_dir() { proc_cwd.clone() } else { hosted::resume_dir(&id) };
    if let Err(e) = hosted::start_with(app, &cwd, "", Some(&id), None) { return failed(&steps, format!("non riparte nel custode: {e}")); }
    steps.push(format!("riaperta nel custode in {cwd}{}", if model.is_empty() { String::new() } else { format!(" ({model})") }));
    // same place in the list as before: it was waiting for you = it still waits for you
    let _ = storage::modify(&p, None, |x| {
        x["stato"] = json!(if prev_view == "aspetta" || prev_view == "fatta" { "aspetta" } else { "ferma" });
        for k in ["da", "letta_da", "serve_te", "chiesto"] { if let Some(val) = before.get(k) { x[k] = val.clone(); } }
        true
    });
    // 4. the cmux tab is no longer needed
    steps.push(if sf.is_empty() { "nessuna scheda cmux da chiudere".into() } else { close_tab(&sf, str_of(&before, "workspace")) });
    let r = json!({"id": id, "nome": name, "esito": "spostata", "passi": steps});
    log_entry(&r);
    if let Some(a) = crate::APP.get() { use tauri::Emitter; let _ = a.emit("cambiato", ()); }
    r
}

/// Single command (bridge /api/migra, `juggl migra`):
///   {"prova": true}                       list only: who can be moved now and who must be waited for
///   {"tutte": true}                       moves all the ready ones (never Juggl sviluppo), skips the others with the reason
///   {"id": "..."}                         moves this one, if it is ready
///   {"id": "...", "quando_ferma": true}   waits in background until it is idle (up to 3 hours), then moves it
pub fn command(b: &Value) -> Value {
    let Some(app) = crate::APP.get().cloned() else { return json!({"ok": false, "msg": "app non pronta"}) };
    let id = str_of(b, "id").to_string();
    let dry_run = b.get("prova").and_then(|x| x.as_bool()).unwrap_or(false);
    let all = b.get("tutte").and_then(|x| x.as_bool()).unwrap_or(false);
    let when_idle = b.get("quando_ferma").and_then(|x| x.as_bool()).unwrap_or(false);
    let force = b.get("forza_turno").and_then(|x| x.as_bool()).unwrap_or(false);
    if !id.is_empty() && !storage::is_valid_id(&id) { return json!({"ok": false, "msg": "id non valido"}) }
    if crate::keeper::program().is_none() { return json!({"ok": false, "msg": "il custode non c'e': senza, spostarle dentro Juggl le farebbe fermare alla prossima chiusura"}) }
    let agents = claude_agents();
    let all_candidates = candidates(&id);
    let selected: Vec<Value> = all_candidates.iter().filter(|v| if id.is_empty() { str_of(v, "sessione") != DEV_SESSION } else { str_of(v, "sessione") == id }).cloned().collect();
    if !id.is_empty() && selected.is_empty() { return json!({"ok": false, "msg": "sessione non trovata tra quelle aperte in cmux"}) }
    if dry_run || (id.is_empty() && !all) {
        let mut listing: Vec<Value> = all_candidates.iter().map(|v| {
            let p = is_ready(v, &agents, force);
            json!({"id": str_of(v, "sessione"), "nome": str_of(v, "nome"), "vista": str_of(v, "vista"), "pronta": p.is_ok(), "motivo": p.err().unwrap_or_default(),
                "sviluppo": str_of(v, "sessione") == DEV_SESSION})
        }).collect();
        listing.sort_by_key(|x| (!x["pronta"].as_bool().unwrap_or(false), x["sviluppo"].as_bool().unwrap_or(false)));
        return json!({"ok": true, "elenco": listing});
    }
    if when_idle {
        let Some(v) = selected.into_iter().next() else { return json!({"ok": false, "msg": "con quando_ferma serve l'id"}) };
        let name = str_of(&v, "nome").to_string();
        std::thread::spawn(move || {
            let start = Instant::now();
            let mut ready_streak = 0;
            while start.elapsed() < Duration::from_secs(3 * 3600) {
                std::thread::sleep(Duration::from_secs(5));
                let agents = claude_agents();
                let Some(v) = candidates(&id).into_iter().find(|x| str_of(x, "sessione") == id) else { log_entry(&json!({"id": id, "esito": "saltata", "msg": "non e' piu' in cmux"})); return };
                // idle for two checks in a row (10 s): I do not take it between one turn and the next message
                if is_ready(&v, &agents, force).is_ok() { ready_streak += 1; } else { ready_streak = 0; }
                if ready_streak >= 2 { migrate_one(&app, &v); return; }
            }
            log_entry(&json!({"id": id, "esito": "saltata", "msg": "non si e' fermata entro 3 ore"}));
        });
        return json!({"ok": true, "msg": format!("{name}: la sposto appena e' ferma (controllo ogni 5 s, per 3 ore al massimo). Registro in {}/migrazioni.jsonl", crate::data_dir_short())});   // rename
    }
    let mut outcomes = vec![];
    if all && id.is_empty() && all_candidates.iter().any(|v| str_of(v, "sessione") == DEV_SESSION) {
        outcomes.push(json!({"id": DEV_SESSION, "nome": "Juggl (sviluppo)", "esito": "saltata", "msg": "la sposti a parte: juggl migra c130039d --quando-ferma"}));
    }
    for v in selected {
        // recheck right before touching it: it may have restarted in the meantime
        let agents = claude_agents();
        let v = candidates(&id).into_iter().find(|x| str_of(x, "sessione") == str_of(&v, "sessione")).unwrap_or(v);
        match is_ready(&v, &agents, force) {
            Ok(()) => outcomes.push(migrate_one(&app, &v)),
            Err(m) => { let r = json!({"id": str_of(&v, "sessione"), "nome": str_of(&v, "nome"), "esito": "saltata", "msg": m}); log_entry(&r); outcomes.push(r); }
        }
    }
    json!({"ok": true, "esiti": outcomes})
}
