// Permissions of hosted chats (Nov 12). Before, every chat started with --dangerously-skip-permissions: Claude could do
// everything without asking. Now every chat starts with one of three levels (Settings > Permissions):
// - "chiedi"     (Always ask, default for new accounts): --permission-mode default
// - "modifiche"  (File changes yes, commands on request): --permission-mode acceptEdits
// - "mai"        (Never ask, as before): --permission-mode bypassPermissions, only after an explicit confirmation
// In all three cases there is --permission-prompt-tool stdio: when Claude wants a permission it sends on stdout a
// control_request "can_use_tool" and waits for the reply on stdin (like the Agent SDK does). Juggl puts the request in the
// chat state ("permessi_attesa"): the chat becomes "blocked", the question box appears with the buttons
// Allow / Always allow in this chat / Deny (the same as the terminal questions, also on the iPhone with
// /api/scelta), and the reply goes back to Claude as control_response.
//
// Tested with CLI 2.1.321 (Nov 12):
// - without a reply Claude stays still and the command does not run; "deny" with a message: Claude reads it and does not retry;
// - "Always allow in this chat" = the CLI suggestions with destination "session" (never settings.json): the next
//   time it does not ask. Juggl saves them in the state ("permessi_sempre") and passes them again with --allowedTools / --add-dir when
//   the chat restarts (account or model change), so they are not lost;
// - the deny rules of settings.json also apply in bypassPermissions. Chats that do not read the user's
//   settings (the coordinator, --setting-sources project,local) receive the user's rules with --disallowedTools/--allowedTools;
// - in acceptEdits Edit/Write and file commands in the folder (mkdir, touch) pass, the other commands ask.
//
// Level of a chat: chat exception > folder exception (the longest one containing it) > project
// exception > general level. First start without a general level: "mai" if the license says admin (administrator
// account, from the account server), otherwise "chiedi" with a one-time notice (license.rs).
//
// Autopilot: never approves a permission by itself, unless "The pilot can allow permissions" is on in Settings
// (off by default). When the chat's pilot is on, it proposes (advice in the box).
#[cfg(windows)] use crate::sys as libc;
use crate::{update_json, base_dir, session_care, home, load_json, log_juggl, unix_now, str_of};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::Emitter;

pub const LEVELS: [&str; 3] = ["chiedi", "modifiche", "mai"];
/// Tools the CLI only enables when someone answers the permissions (see `argomenti`).
pub const INTERACTIVE_TOOLS: &str = "AskUserQuestion,EnterPlanMode,ExitPlanMode,SendToProject,SendToProjectContext";

fn state_file(id: &str) -> PathBuf { base_dir().join("stato").join(format!("{id}.json")) }
fn settings_file() -> PathBuf { base_dir().join("impostazioni.json") }
fn valid_level(l: &str) -> bool { LEVELS.contains(&l) }
fn notify_app() { if let Some(a) = crate::APP.get() { let _ = a.emit("cambiato", ()); } }

/// Level name for people.
pub fn level_name(l: &str) -> &'static str {
    match l { "mai" => "Mai chiedere", "modifiche" => "Modifiche ai file sì, comandi su richiesta", _ => "Chiedi sempre" }
}

/// Claude Code permission mode for a level.
pub fn cli_mode(l: &str) -> &'static str {
    match l { "mai" => "bypassPermissions", "modifiche" => "acceptEdits", _ => "default" }
}

// ---------- general level and first start ----------

/// What to decide at the first start without a general level. admin: Some(true) administrator, Some(false) no,
/// None the license does not say it yet (signed before Nov 12 or missing). existing: Juggl was already in use.
/// Returns (level, notice to show, to save).
pub fn initial_decision(admin: Option<bool>, existing: bool) -> (&'static str, bool, bool) {
    match admin {
        Some(true) => ("mai", false, true),
        Some(false) => ("chiedi", existing, true),
        None => ("chiedi", false, false),   // for now it asks; the real decision is at the next license renewal
    }
}

/// Juggl was already in use before this version (there are chats in the state).
fn juggl_in_use() -> bool {
    std::fs::read_dir(base_dir().join("stato")).map(|rd| rd.flatten().any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))).unwrap_or(false)
}

/// General level. At the first start it decides it from the license and saves it (only once).
pub fn general_level() -> String {
    let settings = session_care::settings();
    let l = str_of(&settings, "permessi");
    if valid_level(l) { return l.to_string(); }
    let mut admin = crate::license::admin();
    // old license at startup: the renewal starts together with the chats. Wait a little (only once per launch)
    static WAITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if admin.is_none() && crate::license::is_valid() && !crate::license::is_skipped() && !WAITED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        for _ in 0..50 { std::thread::sleep(Duration::from_millis(500)); admin = crate::license::admin(); if admin.is_some() { break; } }
    }
    let (level, notice, save) = initial_decision(admin, juggl_in_use());
    if save {
        let _ = crate::storage::modify(&settings_file(), Some(json!({})), |v| {
            if valid_level(str_of(v, "permessi")) { return false; }
            v["permessi"] = json!(level);
            if notice { v["permessi_avviso"] = json!(true); }
            true
        });
        log_juggl(&format!("permessi: livello generale alla prima partenza = {level}{}", if notice { " (con avviso)" } else { "" }));
        // whoever decided first may have written another value: the one in the file wins
        let l2 = str_of(&session_care::settings(), "permessi").to_string();
        if valid_level(&l2) { return l2; }
    }
    level.to_string()
}

/// Level of a chat and where it comes from ("chat", "cartella", "progetto", "generale").
pub fn level_for(st: &Value, cwd: &str) -> (String, &'static str) {
    let c = str_of(st, "permessi_chat");
    if valid_level(c) { return (c.to_string(), "chat"); }
    let settings = session_care::settings();
    // folder: the longest one containing the chat folder
    let mut best: Option<(usize, String)> = None;
    if let Some(o) = settings.get("permessi_cartelle").and_then(|x| x.as_object()) {
        for (k, v) in o {
            let l = v.as_str().unwrap_or("");
            let k2 = session_care::expand_path(k);
            let k2 = k2.trim_end_matches('/');
            if !valid_level(l) || k2.is_empty() { continue; }
            let inside = [cwd, str_of(st, "cwd_origine")].iter().any(|c| !c.is_empty() && (*c == k2 || c.starts_with(&format!("{k2}/"))));
            if inside && best.as_ref().map(|m| k2.len() > m.0).unwrap_or(true) { best = Some((k2.len(), l.to_string())); }
        }
    }
    if let Some((_, l)) = best { return (l, "cartella"); }
    if let Some(o) = settings.get("permessi_progetti").and_then(|x| x.as_object()).filter(|o| !o.is_empty()) {
        let name = [str_of(st, "nome_manuale"), str_of(st, "titolo")].into_iter().find(|x| !x.is_empty()).unwrap_or("").to_string();
        let mut st2 = st.clone();
        if str_of(&st2, "cwd").is_empty() { st2["cwd"] = json!(cwd); }
        let pr = session_care::project_of(&st2, &name, &session_care::projects());
        if let Some(l) = o.get(str_of(&pr, "id")).and_then(|x| x.as_str()).filter(|l| valid_level(l)) { return (l.to_string(), "progetto"); }
    }
    (general_level(), "generale")
}

// ---------- startup arguments ----------

/// Rule for --allowedTools from a suggestion rule: "Bash(echo ciao *)" or "WebFetch".
fn rule_text(r: &Value) -> String {
    let t = str_of(r, "toolName");
    let c = str_of(r, "ruleContent");
    if c.is_empty() { t.to_string() } else { format!("{t}({c})") }
}

/// deny/allow/ask rules of the user's settings.json (for chats that do not read it by themselves).
fn user_rules() -> (Vec<String>, Vec<String>) {
    let v = load_json(&home().join(".claude/settings.json")).unwrap_or(json!({}));
    let list = |k: &str| -> Vec<String> { v["permissions"][k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|x| x.to_string())).collect()).unwrap_or_default() };
    (list("deny"), list("allow"))
}

/// Claude Code arguments for the permissions of this chat. `already`: the ones already decided (coordinator: without the user's
/// settings). Also returns the level, to write in the state.
pub fn cli_args(st: &Value, cwd: &str, already: &[String]) -> (Vec<String>, String) {
    let (level, _) = level_for(st, cwd);
    let mut a: Vec<String> = vec!["--permission-mode".into(), cli_mode(&level).into(), "--permission-prompt-tool".into(), "stdio".into()];
    // with someone answering permissions the CLI also enables the tools that ask the person something (multiple-choice
    // questions, plan mode, sending to other projects). They were not there before: they stay off, so chats behave as
    // before (questions arrive as text). Tested with CLI 2.1.321: exactly these five.
    a.extend(["--disallowedTools".into(), INTERACTIVE_TOOLS.into()]);
    let mut allowed: Vec<String> = vec![];
    let mut dirs: Vec<String> = vec![];
    // "Always allow in this chat" given before a restart
    for u in st.get("permessi_sempre").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        match str_of(&u, "type") {
            "addRules" if str_of(&u, "behavior") == "allow" => for r in u["rules"].as_array().cloned().unwrap_or_default() { let t = rule_text(&r); if !t.is_empty() && !allowed.contains(&t) { allowed.push(t); } },
            "addDirectories" => for d in u["directories"].as_array().cloned().unwrap_or_default() { if let Some(d) = d.as_str() { if Path::new(d).is_dir() && !dirs.iter().any(|x| x == d) { dirs.push(d.to_string()); } } },
            _ => {}
        }
    }
    // the coordinator and the general assistant work with Juggl commands: those pass without asking
    if crate::coordinator::is_coordinator(st) || cwd.ends_with("generale-juggl") {
        let j = base_dir().join("bin/juggl").to_string_lossy().to_string();
        for r in [format!("Bash({j} *)"), format!("Bash({}/bin/juggl *)", crate::data_dir_short()), "Bash(juggl *)".to_string()]   /* rename (Nov 14): no longer a fixed ~/tavolo */ { if !allowed.contains(&r) { allowed.push(r); } }
    }
    // chats that do not read the user's settings: the user's rules apply anyway
    let without_user = already.windows(2).any(|w| w[0] == "--setting-sources" && !w[1].split(',').any(|x| x.trim() == "user"));
    if without_user {
        let (deny, allow) = user_rules();
        if !deny.is_empty() { a.push("--disallowedTools".into()); a.push(deny.join(",")); }
        for r in allow { if !allowed.contains(&r) { allowed.push(r); } }
    }
    // one rule per argument: rules with spaces inside ("Bash(git status *)") stay whole
    for r in allowed { a.push("--allowedTools".into()); a.push(r); }
    for d in dirs { a.push("--add-dir".into()); a.push(d); }
    (a, level)
}

// ---------- requests arriving from Claude ----------

fn truncate_chars(t: &str, n: usize) -> String { let c: String = t.chars().take(n).collect(); if t.chars().count() > n { format!("{c}…") } else { c } }

/// Request text for people: what it wants to do.
pub fn request_text(r: &Value) -> String {
    let tool = str_of(r, "tool_name");
    let inp = r.get("input").cloned().unwrap_or(json!({}));
    let file = [str_of(&inp, "file_path"), str_of(&inp, "notebook_path"), str_of(&inp, "path")].into_iter().find(|x| !x.is_empty()).unwrap_or("").to_string();
    let mut t = match tool {
        "Bash" => format!("Vuole eseguire un comando: {}", truncate_chars(str_of(&inp, "command"), 600)),
        "Edit" | "MultiEdit" => format!("Vuole modificare il file {file}"),
        "Write" => format!("Vuole scrivere il file {file}"),
        "NotebookEdit" => format!("Vuole modificare il notebook {file}"),
        "WebFetch" => format!("Vuole aprire {}", truncate_chars(str_of(&inp, "url"), 300)),
        "WebSearch" => format!("Vuole cercare sul web: {}", truncate_chars(str_of(&inp, "query"), 200)),
        x if x.starts_with("mcp__") => {
            let mut p = x.splitn(3, "__").skip(1);
            let (srv, name) = (p.next().unwrap_or(""), p.next().unwrap_or(""));
            format!("Vuole usare {name} di {srv}")
        }
        "" => "Chiede un permesso".to_string(),
        x => if file.is_empty() { format!("Vuole usare {x}") } else { format!("Vuole usare {x} su {file}") },
    };
    let d = str_of(r, "description");
    let d = if d.is_empty() { str_of(&inp, "description") } else { d };
    if !d.is_empty() && tool == "Bash" { t.push_str(&format!(" ({})", truncate_chars(d, 160))); }
    if !str_of(r, "agent_id").is_empty() { t.push_str(" · da un sotto-agente"); }
    t
}

/// Rules proposed by "Always allow in this chat": the CLI suggestions with destination "session". Without
/// suggestions: the exact command for Bash, the tool for the others.
pub fn always_rules(r: &Value) -> Vec<Value> {
    let mut v: Vec<Value> = r.get("permission_suggestions").and_then(|x| x.as_array()).cloned().unwrap_or_default().into_iter()
        .filter(|u| str_of(u, "type") == "addRules" && str_of(u, "behavior") == "allow" || str_of(u, "type") == "addDirectories")
        .map(|mut u| { u["destination"] = json!("session"); u }).collect();
    if v.is_empty() {
        let tool = str_of(r, "tool_name");
        let rule = if tool == "Bash" { json!({"toolName": "Bash", "ruleContent": str_of(&r["input"], "command")}) } else { json!({"toolName": tool}) };
        v.push(json!({"type": "addRules", "rules": [rule], "behavior": "allow", "destination": "session"}));
    }
    v
}

/// Short description of what "always" covers (for the button).
fn describe_always(r: &Value) -> String {
    let mut parts = vec![];
    for u in always_rules(r) {
        if str_of(&u, "type") == "addRules" {
            for x in u["rules"].as_array().cloned().unwrap_or_default() {
                let c = str_of(&x, "ruleContent");
                parts.push(if c.is_empty() { format!("tutto con {}", str_of(&x, "toolName")) } else if str_of(&x, "toolName") == "Bash" { format!("comandi «{}»", truncate_chars(c, 80)) } else { format!("{} «{}»", str_of(&x, "toolName"), truncate_chars(c, 80)) });
            }
        } else if str_of(&u, "type") == "addDirectories" {
            for d in u["directories"].as_array().cloned().unwrap_or_default() { parts.push(format!("file in {}", d.as_str().unwrap_or(""))); }
        }
    }
    if parts.is_empty() { String::new() } else { format!("Non chiede più per: {}", parts.join(", ")) }
}

/// Simple glob for Bash rules: "*" = any text; "prefix:*" (old form) = starts with.
fn glob_matches(pattern: &str, text: &str) -> bool {
    if let Some(p) = pattern.strip_suffix(":*") { return text == p || text.starts_with(&format!("{p} ")); }
    let pieces: Vec<&str> = pattern.split('*').collect();
    if pieces.len() == 1 { return pattern == text; }
    let mut rest = text;
    for (i, p) in pieces.iter().enumerate() {
        if i == 0 { if !rest.starts_with(p) { return false; } rest = &rest[p.len()..]; continue; }
        if i == pieces.len() - 1 { return rest.ends_with(p); }
        match rest.find(p) { Some(k) => rest = &rest[k + p.len()..], None => return false }
    }
    true
}

/// Does a request fall under an "always" rule already given in this chat? (if the CLI asks the same thing again)
pub fn is_covered(always: &[Value], r: &Value) -> bool {
    let tool = str_of(r, "tool_name");
    let cmd = str_of(&r["input"], "command");
    // compound commands (&&, ;, |): let the CLI decide, it splits them
    let compound = ["&&", "||", ";", "|", "`", "$(", "\n"].iter().any(|x| cmd.contains(x));
    // the CLI asks for a file (e.g. "echo ciao > /altrove/x"): passes only if the file is in a folder already
    // allowed "always" in this chat, otherwise the person decides
    let blocked = str_of(r, "blocked_path");
    if !blocked.is_empty() {
        let norm = |p: &str| p.strip_prefix("/private").filter(|x| x.starts_with("/tmp/") || x.starts_with("/var/")).unwrap_or(p).trim_end_matches('/').to_string();
        let b = norm(blocked);
        let inside = always.iter().filter(|u| str_of(u, "type") == "addDirectories").flat_map(|u| u["directories"].as_array().cloned().unwrap_or_default())
            .filter_map(|d| d.as_str().map(norm)).any(|d| !d.is_empty() && !b.contains("/../") && (b == d || b.starts_with(&format!("{d}/"))));
        if !inside { return false; }
    }
    always.iter().filter(|u| str_of(u, "type") == "addRules" && str_of(u, "behavior") == "allow").flat_map(|u| u["rules"].as_array().cloned().unwrap_or_default()).any(|x| {
        if str_of(&x, "toolName") != tool { return false; }
        let c = str_of(&x, "ruleContent");
        if c.is_empty() { return tool != "Bash"; }   // "all of Bash" is never granted from here
        tool == "Bash" && !compound && glob_matches(c, cmd)
    })
}

/// Reply to send to Claude.
pub fn response(request_id: &str, req: &Value, choice: &str, message: &str) -> Value {
    let r = match choice {
        "consenti" => json!({"behavior": "allow", "updatedInput": req.get("input").cloned().unwrap_or(json!({}))}),
        "sempre" => json!({"behavior": "allow", "updatedInput": req.get("input").cloned().unwrap_or(json!({})), "updatedPermissions": always_rules(req)}),
        _ => json!({"behavior": "deny", "message": if message.trim().is_empty() { "L'utente ha negato il permesso in Juggl. Non riprovare la stessa azione e non cercare altre strade per farla: chiedi all'utente come procedere.".to_string() } else { format!("L'utente ha negato il permesso in Juggl e ha scritto: {}", message.trim()) }}),
    };
    json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": r}})
}

/// Reply on the stdin of the hosted session (in tests: on the stdin of the test Claude).
fn write_control(id: &str, r: &Value) -> Result<(), String> {
    #[cfg(test)]
    {
        use std::io::Write;
        if let Some(w) = live_tests::stdin().lock().unwrap().get_mut(id) { return writeln!(w, "{r}").and_then(|_| w.flush()).map_err(|e| e.to_string()); }
    }
    crate::hosted::reply_control(id, r)
}

/// "$(mktemp -d)" and "$(mktemp -d /tmp/xxx)" (simple path under /tmp) -> "/tmp/mktemp"; the rest of the command does not change.
fn replace_mktemp(c: &str) -> String {
    let mut out = String::new(); let mut rest = c;
    while let Some(i) = rest.find("$(mktemp -d") {
        let after = &rest[i + "$(mktemp -d".len()..];
        let Some(end) = after.find(')') else { break };
        let arg = after[..end].trim();
        let ok = arg.is_empty() || (arg.starts_with("/tmp/") && !arg.contains("..") && !arg.contains(char::is_whitespace) && arg.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | 'X')));
        if !ok { break; }
        out.push_str(&rest[..i]); out.push_str("/tmp/mktemp");
        rest = &after[end + 1..];
    }
    out.push_str(rest); out
}

/// Is the command or file of the request within the safe boundaries? (automatic consent in the "Mai chiedere" level)
pub fn safe_consent(req: &Value, cwd: &str, origin_cwd: &str) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let roots: Vec<String> = [cwd, origin_cwd].iter().filter(|c| !c.is_empty() && **c != "/" && **c != home.as_str()).map(|c| c.trim_end_matches('/').to_string()).collect();
    if roots.is_empty() { return false; }   // chat in the home or in "/": never by itself
    let inside = |p: &str| -> bool {
        let p = if let Some(r) = p.strip_prefix("~/") { format!("{home}/{r}") } else if p == "~" { home.clone() } else { p.to_string() };
        if !p.starts_with('/') || p.contains("/../") || p.ends_with("/..") { return false; }
        const SENSITIVE: [&str; 9] = ["/.ssh", "/.claude/settings", "/.config", "/.aws", "/.gnupg", "/Library/Keychains", "/.zshrc", "/.bash_profile", "/.git/config"];
        if SENSITIVE.iter().any(|x| p.contains(x)) { return false; }
        p.starts_with("/tmp/") || p == "/tmp" || p.starts_with("/private/tmp/") || roots.iter().any(|r| p == *r || p.starts_with(&format!("{r}/")))
    };
    let input = req.get("input").cloned().unwrap_or(json!({}));
    match str_of(req, "tool_name") {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => { let p = [str_of(&input, "file_path"), str_of(&input, "notebook_path")].into_iter().find(|x| !x.is_empty()).unwrap_or(""); !p.is_empty() && inside(p) }
        "Bash" => {
            // safe substitution (Nov 14): only "$(mktemp -d)" and "$(mktemp -d /tmp/...)": becomes a path in /tmp; any other "$( )" stays forbidden
            let c = &replace_mktemp(str_of(&input, "command"));
            let l = c.to_lowercase();
            for v in ["sudo", "git push", "--force", "--no-verify", "chmod -r", "chown", "mkfs", "diskutil", "dd if=", "curl ", "wget ", "| sh", "| bash", "launchctl", "kill ", "pkill", "killall", "shutdown", "reboot", "ssh ", "scp ", "rsync", "git reset --hard", "git clean", "git checkout --", "$(", "`", "eval "] { if l.contains(v) { return false; } }
            // every word that looks like a path must be inside; relative paths count from the chat folder (no "..")
            for w in c.split(|ch: char| ch.is_whitespace() || matches!(ch, ';' | '|' | '&' | '>' | '<' | '(' | ')' | '"' | '\'')) {
                let w = w.trim_matches(|ch| ch == ',' || ch == ':');
                if w.is_empty() || w.starts_with('-') { continue; }
                if w.starts_with('/') || w.starts_with("~") || w.starts_with("$HOME") || w.starts_with("${") { if !inside(w) { return false; } }
                else if w.contains("..") && w.contains('/') { return false; }
                else if w == ".." { return false; }
            }
            !c.trim().is_empty()
        }
        _ => false,
    }
}

/// A Claude control_request (from the session reader, hosted.rs). `pid`: process that sent it.
pub fn request(id: &str, pid: u32, d: &Value) {
    let rid = str_of(d, "request_id").to_string();
    let req = d.get("request").cloned().unwrap_or(json!({}));
    if rid.is_empty() { return; }
    if str_of(&req, "subtype") != "can_use_tool" {
        // no other request is expected: reply with an error, so Claude does not keep waiting
        let r = json!({"type": "control_response", "response": {"subtype": "error", "request_id": rid, "error": "richiesta non gestita da Juggl"}});
        let _ = write_control(id, &r);
        return;
    }
    let p = state_file(id);
    let st = load_json(&p).unwrap_or(json!({}));
    // already answered (events replayed by the keeper after a reopening) or already pending: nothing
    if st["permessi_risposti"].as_array().map(|a| a.iter().any(|x| x.as_str() == Some(&rid))).unwrap_or(false) { return; }
    if st["permessi_attesa"].as_array().map(|a| a.iter().any(|x| str_of(x, "id") == rid)).unwrap_or(false) { return; }
    // "Mai chiedere" (Nov 13): the CLI still asks for some commands (rm -f, simctl...) even in bypassPermissions.
    // If the chat is on "mai" (explicit choice of the user) Juggl allows by itself: the CLI deny rules have already been
    // applied before the request, so only what would be allowed arrives here.
    // Only within the safe boundaries: command or file in the chat folder or in /tmp, never the CLI's internal protections
    // (rm -r outside the folder, home, git push --force, ~/.ssh, ~/.claude/settings.json, ~/.config...): those stay with the CLI.
    if level_for(&st, str_of(&st, "cwd")).0 == "mai" && safe_consent(&req, str_of(&st, "cwd"), str_of(&st, "cwd_origine")) {
        log_juggl(&format!("{id}: permesso {} consentito da solo (Mai chiedere, dentro la cartella della chat): {}", str_of(&req, "tool_name"), request_text(&req)));
        let _ = send_reply(id, &rid, &req, "consenti", "");
        return;
    }
    let always = st["permessi_sempre"].as_array().cloned().unwrap_or_default();
    if is_covered(&always, &req) {
        log_juggl(&format!("{id}: permesso {} gia' consentito sempre in questa chat", str_of(&req, "tool_name")));
        let _ = send_reply(id, &rid, &req, "consenti", "");
        return;
    }
    let entry = json!({"id": rid, "pid": pid, "t": unix_now(), "strumento": str_of(&req, "tool_name"), "testo": request_text(&req), "sempre": describe_always(&req), "richiesta": req});
    update_json(&p, |v| {
        let mut a = v["permessi_attesa"].as_array().cloned().unwrap_or_default();
        a.push(entry.clone());
        v["permessi_attesa"] = json!(a);
        true
    });
    log_juggl(&format!("{id}: chiede il permesso per {}", str_of(&req, "tool_name")));
    notify_app();
    let (id2, rid2) = (id.to_string(), rid.clone());
    std::thread::spawn(move || pilot_review(&id2, &rid2));
}

/// Claude withdrew the request (turn stopped).
pub fn withdrawn(id: &str, d: &Value) {
    let rid = str_of(d, "request_id").to_string();
    remove_pending(id, |x| str_of(x, "id") == rid);
}

/// End of the turn: the remaining requests are no longer valid (turn interrupted). New process: same.
pub fn end_of_turn(id: &str) { remove_pending(id, |_| true); }

fn remove_pending(id: &str, which: impl Fn(&Value) -> bool) {
    let p = state_file(id);
    if !p.exists() { return; }
    let mut changed = false;
    update_json(&p, |v| {
        let Some(a) = v["permessi_attesa"].as_array().cloned() else { return false };
        let rest: Vec<Value> = a.iter().filter(|x| !which(x)).cloned().collect();
        if rest.len() == a.len() { return false; }
        changed = true;
        if rest.is_empty() { if let Some(o) = v.as_object_mut() { o.remove("permessi_attesa"); } } else { v["permessi_attesa"] = json!(rest); }
        true
    });
    if changed { notify_app(); }
}

/// Sends the reply to Claude and updates the state (answers given, "always" rules).
fn send_reply(id: &str, rid: &str, req: &Value, choice: &str, message: &str) -> Result<(), String> {
    write_control(id, &response(rid, req, choice, message))?;
    let new_rules = if choice == "sempre" { always_rules(req) } else { vec![] };
    update_json(&state_file(id), |v| {
        let mut a = v["permessi_attesa"].as_array().cloned().unwrap_or_default();
        a.retain(|x| str_of(x, "id") != rid);
        if a.is_empty() { if let Some(o) = v.as_object_mut() { o.remove("permessi_attesa"); } } else { v["permessi_attesa"] = json!(a); }
        let mut r: Vec<Value> = v["permessi_risposti"].as_array().cloned().unwrap_or_default();
        r.push(json!(rid));
        let n = r.len();
        v["permessi_risposti"] = json!(r.into_iter().skip(n.saturating_sub(40)).collect::<Vec<_>>());
        if !new_rules.is_empty() {
            let mut sm = v["permessi_sempre"].as_array().cloned().unwrap_or_default();
            for u in &new_rules { if !sm.contains(u) { sm.push(u.clone()); } }
            v["permessi_sempre"] = json!(sm);
        }
        true
    });
    notify_app();
    Ok(())
}

/// First pending request of the chat (if the process that asked is still the chat's process).
fn first_pending(v: &Value) -> Option<Value> {
    let a = v.get("permessi_attesa")?.as_array()?;
    let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0);
    // process finished (chat hibernated or closed): the request is no longer waiting for anyone
    if pid > 0 && unsafe { libc::kill(pid as i32, 0) } != 0 { return None; }
    a.iter().find(|x| x.get("pid").and_then(|p| p.as_u64()).map(|p| p == pid || pid == 0).unwrap_or(true)).cloned()
}

/// Is the chat waiting for a permission? (for "blocked" in the list)
pub fn is_waiting(v: &Value) -> bool { first_pending(v).is_some() }

/// Question box (same shape as the terminal question: Mac and iPhone already show it).
pub fn question(v: &Value) -> Option<Value> {
    let p = first_pending(v)?;
    let n = v["permessi_attesa"].as_array().map(|a| a.len()).unwrap_or(1);
    let mut text = str_of(&p, "testo").to_string();
    if n > 1 { text.push_str(&format!(" (poi altre {})", n - 1)); }
    if let Some(c) = p.get("pilota").filter(|c| c.is_object()) {
        let az = match str_of(c, "azione") { "consenti" => "consentire", "nega" => "negare", _ => "chiedere a te" };
        text.push_str(&format!("\nPilota: consiglia di {az}{}", if str_of(c, "motivo").is_empty() { String::new() } else { format!(" ({})", str_of(c, "motivo")) }));
        if c["tra"].as_f64().unwrap_or(0.0) > unix_now() { text.push_str(" · consente da solo fra poco"); }
    }
    Some(json!({
        "testo": text,
        "opzioni": [
            {"n": "1", "testo": "Consenti", "scelta": true},
            {"n": "2", "testo": "Consenti sempre in questa chat", "desc": str_of(&p, "sempre")},
            {"n": "3", "testo": "Nega"},
        ],
        "permesso": {"richiesta": str_of(&p, "id"), "strumento": str_of(&p, "strumento"), "in_coda": n, "t": p["t"].clone()},
    }))
}

/// Answers the first pending request. choice: "consenti" | "sempre" | "nega".
pub fn answer(id: &str, request: Option<&str>, choice: &str, message: &str) -> Value {
    if !crate::storage::is_valid_id(id) { return json!({"ok": false, "msg": "id non valido"}); }
    let Some(v) = load_json(&state_file(id)) else { return json!({"ok": false, "msg": "chat non trovata"}) };
    let entry = match request.filter(|r| !r.is_empty()) {
        Some(r) => v["permessi_attesa"].as_array().and_then(|a| a.iter().find(|x| str_of(x, "id") == r).cloned()),
        None => first_pending(&v),
    };
    let Some(entry) = entry else { return json!({"ok": false, "msg": "nessun permesso in attesa"}) };
    let choice = if matches!(choice, "consenti" | "sempre") { choice } else { "nega" };
    match send_reply(id, str_of(&entry, "id"), &entry["richiesta"], choice, message) {
        Ok(()) => {
            log_juggl(&format!("{id}: permesso {} -> {choice}", str_of(&entry, "strumento")));
            json!({"ok": true, "msg": match choice { "consenti" => "Consentito", "sempre" => "Consentito sempre in questa chat", _ => "Negato" }})
        }
        Err(e) => json!({"ok": false, "msg": format!("Risposta non consegnata: {e}")}),
    }
}

/// Box buttons (rispondi_scelta in main.rs, /api/scelta from the iPhone): None if the chat is not waiting for a permission.
pub fn answer_choice(id: &str, n: &str, text: Option<&str>) -> Option<Value> {
    let v = load_json(&state_file(id))?;
    if !is_waiting(&v) { return None; }
    let choice = match n { "1" => "consenti", "2" => "sempre", _ => "nega" };
    Some(answer(id, None, choice, if choice == "nega" { text.unwrap_or("") } else { "" }))
}

/// All pending requests (for the bridge: the iPhone shows them and answers with /api/permesso or /api/scelta).
pub fn list() -> Value {
    let mut out = vec![];
    for e in std::fs::read_dir(base_dir().join("stato")).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        let Some(v) = load_json(&p) else { continue };
        if v.get("permessi_attesa").is_none() || str_of(&v, "stato") == "archiviata" { continue; }
        let pid = v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
        if pid <= 0 || unsafe { libc::kill(pid as i32, 0) } != 0 { continue; }
        let Some(d) = question(&v) else { continue };
        let name = [str_of(&v, "nome_manuale"), str_of(&v, "titolo")].into_iter().find(|x| !x.is_empty()).unwrap_or("Chat").to_string();
        out.push(json!({"id": str_of(&v, "sessione"), "nome": name, "testo": d["testo"], "opzioni": d["opzioni"], "permesso": d["permesso"]}));
    }
    json!({"ok": true, "permessi": out})
}

// ---------- autopilot ----------

/// Can the pilot allow permissions by itself? Only with the dedicated switch (off by default) and the pilot on.
pub fn pilot_can_allow(settings: &Value, st: &Value) -> bool {
    settings["pilota_permessi"].as_bool().unwrap_or(false) && settings["pilota"].as_bool().unwrap_or(true) && st.get("pilota").and_then(|x| x.as_bool()).unwrap_or(true)
}

const PILOT_RULES: &str = "Sei il pilota automatico di «NOME». Un agente AI di programmazione chiede il permesso di usare uno strumento. \
Valuta se e' un'azione sicura e normale per il lavoro in corso. Consiglia \"nega\" per: cancellazioni di file o dati, comandi distruttivi o irreversibili, \
deploy o pubblicazioni in produzione, invio di messaggi o email, spese, modifiche fuori dalla cartella del progetto, segreti e password, comandi da internet eseguiti al volo. \
Consiglia \"consenti\" solo per azioni locali, normali e reversibili. Nel dubbio \"chiedi\". \
Rispondi SOLO con un oggetto JSON: {\"azione\": \"consenti\" oppure \"nega\" oppure \"chiedi\", \"motivo\": \"una frase breve in italiano\", \"sicurezza\": numero tra 0 e 1}.";

/// Pilot advice on a request; if allowed (dedicated switch) it allows it after the pilot's wait.
fn pilot_review(id: &str, rid: &str) {
    let settings = session_care::settings();
    let Some(st) = load_json(&state_file(id)) else { return };
    // pilot off (in general or in the chat): no advice
    if !settings["pilota"].as_bool().unwrap_or(true) || st.get("pilota").and_then(|x| x.as_bool()) == Some(false) { return; }
    let Some(entry) = st["permessi_attesa"].as_array().and_then(|a| a.iter().find(|x| str_of(x, "id") == rid).cloned()) else { return };
    let prompt = format!("{PILOT_RULES}\n\nCartella: {}\nRichiesta: {}\nDati dello strumento: {}", str_of(&st, "cwd"), str_of(&entry, "testo"),
        truncate_chars(&entry["richiesta"]["input"].to_string(), 2000));
    let Some(r) = crate::autopilot::ask_for("permessi", &prompt, None, crate::autopilot::Cache::No, true) else { return };
    let may = pilot_can_allow(&settings, &st);
    let allows = may && str_of(&r, "azione") == "consenti" && r["sicurezza"].as_f64().unwrap_or(0.0) >= 0.8;
    let wait_s = settings["pilota_attesa_s"].as_f64().unwrap_or(60.0).max(5.0);
    let due = if allows { unix_now() + wait_s } else { 0.0 };
    update_json(&state_file(id), |v| {
        let Some(a) = v["permessi_attesa"].as_array_mut() else { return false };
        let Some(x) = a.iter_mut().find(|x| str_of(x, "id") == rid) else { return false };
        x["pilota"] = json!({"azione": str_of(&r, "azione"), "motivo": str_of(&r, "motivo"), "sicurezza": r["sicurezza"].clone(), "tra": due});
        true
    });
    notify_app();
    if !allows { return; }
    session_care::record(&format!("Pilota: consento un permesso a {}", str_of(&st, "titolo")), str_of(&entry, "testo"), id, "pilota");
    std::thread::sleep(Duration::from_secs_f64(wait_s));
    // still the same request, the pilot was not turned off meanwhile, nobody cancelled
    let Some(st2) = load_json(&state_file(id)) else { return };
    let still = st2["permessi_attesa"].as_array().and_then(|a| a.iter().find(|x| str_of(x, "id") == rid).cloned());
    let Some(x) = still else { return };
    if x["pilota"]["tra"].as_f64().unwrap_or(0.0) <= 0.0 || !pilot_can_allow(&session_care::settings(), &st2) { return; }
    let _ = answer(id, Some(rid), "consenti", "");
}

/// Cancels the pilot's automatic consent on the pending request (left for you to decide).
pub fn pilot_cancel(id: &str) {
    update_json(&state_file(id), |v| {
        let Some(a) = v["permessi_attesa"].as_array_mut() else { return false };
        for x in a.iter_mut() { if x["pilota"].is_object() { x["pilota"]["tra"] = json!(0); } }
        true
    });
}

// ---------- level changes ----------

/// Hosted chats whose level changed restart (with the whole conversation): right away if stopped, at the end of the turn
/// if working (like the model change). Chats started before this version count as "mai".
pub fn apply(only: Option<&str>) -> usize {
    let mut n = 0;
    for e in std::fs::read_dir(base_dir().join("stato")).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        let Some(v) = load_json(&p) else { continue };
        let id = str_of(&v, "sessione").to_string();
        if id.is_empty() || only.map(|x| x != id).unwrap_or(false) || !crate::hosted::is_hosted(&id) { continue; }
        let before = if str_of(&v, "permessi_livello").is_empty() { "mai" } else { str_of(&v, "permessi_livello") };
        let (after, _) = level_for(&v, str_of(&v, "cwd"));
        if before == after { continue; }
        // working or stopped on a permission: restarts at the end of the turn (the current turn is not lost)
        let working = str_of(&v, "stato") == "lavora";
        if working { update_json(&p, |x| { x["regole_da_applicare"] = json!(true); true }); } else { crate::hosted::restart(&id); }
        log_juggl(&format!("{id}: permessi {before} -> {after}, {}", if working { "riparte a fine turno" } else { "riparte adesso" }));
        n += 1;
    }
    n
}

// ---------- commands for the interface ----------

#[tauri::command(async)]
pub fn permessi_stato() -> Value {
    let level = general_level();
    let settings = session_care::settings();
    let projects: Vec<Value> = session_care::projects().into_iter().map(|p| json!({"id": str_of(&p, "id"), "nome": str_of(&p, "nome"), "cartella": str_of(&p, "cartella")})).filter(|p| !str_of(p, "id").is_empty()).collect();
    json!({"ok": true, "livello": level, "nome": level_name(&level), "progetti": settings.get("permessi_progetti").cloned().unwrap_or(json!({})),
        "cartelle": settings.get("permessi_cartelle").cloned().unwrap_or(json!({})), "avviso": settings["permessi_avviso"].as_bool().unwrap_or(false),
        "pilota_permessi": settings["pilota_permessi"].as_bool().unwrap_or(false), "elenco_progetti": projects})
}

/// General level. "mai" only with the explicit confirmation (given by the interface after the risk notice).
#[tauri::command(async)]
pub fn permessi_imposta(livello: String, confermato: Option<bool>) -> Value {
    if !valid_level(&livello) { return json!({"ok": false, "msg": "livello non valido"}); }
    if livello == "mai" && confermato != Some(true) { return json!({"ok": false, "msg": "serve la conferma"}); }
    let ok = crate::storage::modify(&settings_file(), Some(json!({})), |v| { v["permessi"] = json!(livello); true }).is_ok();
    let n = apply(None);
    log_juggl(&format!("permessi: livello generale = {livello} ({n} chat ripartono)"));
    notify_app();
    json!({"ok": ok, "riparte": n})
}

/// Exception for a project (tipo "progetto", chiave = id) or a folder (tipo "cartella", chiave = path).
/// empty level = remove the exception.
#[tauri::command(async)]
pub fn permessi_eccezione(tipo: String, chiave: String, livello: String, confermato: Option<bool>) -> Value {
    let field = match tipo.as_str() { "progetto" => "permessi_progetti", "cartella" => "permessi_cartelle", _ => return json!({"ok": false, "msg": "tipo non valido"}) };
    let key = chiave.trim().to_string();
    if key.is_empty() || key.len() > 500 { return json!({"ok": false, "msg": "manca la cartella o il progetto"}); }
    if tipo == "cartella" && !Path::new(&session_care::expand_path(&key)).is_dir() { return json!({"ok": false, "msg": "cartella non trovata"}); }
    if !livello.is_empty() && !valid_level(&livello) { return json!({"ok": false, "msg": "livello non valido"}); }
    if livello == "mai" && confermato != Some(true) { return json!({"ok": false, "msg": "serve la conferma"}); }
    let ok = crate::storage::modify(&settings_file(), Some(json!({})), |v| {
        if !v[field].is_object() { v[field] = json!({}); }
        if livello.is_empty() { v[field].as_object_mut().map(|o| o.remove(&key)); } else { v[field][&key] = json!(livello); }
        true
    }).is_ok();
    let n = apply(None);
    notify_app();
    json!({"ok": ok, "riparte": n})
}

/// Exception for a chat (empty level = as in the settings). Restarts right away if stopped, at the end of the turn if working.
#[tauri::command(async)]
pub fn permessi_chat(id: String, livello: String, confermato: Option<bool>) -> Value {
    if !crate::storage::is_valid_id(&id) || id.starts_with("locale:") { return json!({"ok": false, "msg": "id non valido"}); }
    if !livello.is_empty() && !valid_level(&livello) { return json!({"ok": false, "msg": "livello non valido"}); }
    if livello == "mai" && confermato != Some(true) { return json!({"ok": false, "msg": "serve la conferma"}); }
    if !update_json(&state_file(&id), |v| { if livello.is_empty() { v.as_object_mut().map(|o| o.remove("permessi_chat")); } else { v["permessi_chat"] = json!(livello); } true }) {
        return json!({"ok": false, "msg": "chat non trovata"});
    }
    let n = apply(Some(&id));
    notify_app();
    json!({"ok": true, "msg": if n > 0 { "Applicato: la chat riparte con i permessi nuovi" } else if crate::hosted::is_hosted(&id) { "Applicato" } else { "Salvato: vale quando la riprendi" }})
}

#[tauri::command(async)]
pub fn permessi_rispondi(id: String, scelta: String, testo: Option<String>, richiesta: Option<String>) -> Value {
    answer(&id, richiesta.as_deref(), &scelta, testo.as_deref().unwrap_or(""))
}

#[tauri::command(async)]
pub fn permessi_avviso_visto() -> Value {
    let ok = crate::storage::modify(&settings_file(), Some(json!({})), |v| { v["permessi_avviso"] = json!(false); true }).is_ok();
    notify_app();
    json!({"ok": ok})
}

/// Switch "The pilot can allow permissions" (off by default).
#[tauri::command(async)]
pub fn permessi_pilota(acceso: bool) -> Value {
    let ok = crate::storage::modify(&settings_file(), Some(json!({})), |v| { v["pilota_permessi"] = json!(acceso); true }).is_ok();
    notify_app();
    json!({"ok": ok})
}

#[tauri::command(async)]
pub fn permessi_pilota_annulla(id: String) -> Value {
    if !crate::storage::is_valid_id(&id) { return json!({"ok": false}); }
    pilot_cancel(&id);
    notify_app();
    json!({"ok": true})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_start() {
        // license with admin: "mai", without notice, saved
        assert_eq!(initial_decision(Some(true), true), ("mai", false, true));
        assert_eq!(initial_decision(Some(true), false), ("mai", false, true));
        // without admin: "chiedi"; notice only once to those already using Juggl
        assert_eq!(initial_decision(Some(false), true), ("chiedi", true, true));
        assert_eq!(initial_decision(Some(false), false), ("chiedi", false, true));
        // license that does not say it yet: it asks, but does not save (decided at renewal)
        assert_eq!(initial_decision(None, true), ("chiedi", false, false));
    }

    #[test]
    fn cli_modes() {
        assert_eq!(cli_mode("chiedi"), "default");
        assert_eq!(cli_mode("modifiche"), "acceptEdits");
        assert_eq!(cli_mode("mai"), "bypassPermissions");
        assert_eq!(cli_mode("strano"), "default");
    }

    fn req_bash(cmd: &str) -> Value {
        json!({"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": cmd, "description": "prova"},
            "permission_suggestions": [{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "echo ciao *"}], "behavior": "allow", "destination": "localSettings"},
                {"type": "addDirectories", "directories": ["/tmp/x"], "destination": "session"}]})
    }

    #[test]
    fn always_stays_in_chat() {
        let r = always_rules(&req_bash("echo ciao > a.txt"));
        assert!(r.iter().all(|u| str_of(u, "destination") == "session"), "mai nel settings.json");
        let x = response("r1", &req_bash("echo ciao > a.txt"), "sempre", "");
        assert_eq!(x["response"]["request_id"], "r1");
        assert_eq!(x["response"]["response"]["behavior"], "allow");
        assert_eq!(x["response"]["response"]["updatedPermissions"][0]["destination"], "session");
        let n = response("r2", &req_bash("rm -rf x"), "nega", "");
        assert_eq!(n["response"]["response"]["behavior"], "deny");
        assert!(str_of(&n["response"]["response"], "message").contains("negato"));
        // without suggestions: the exact command, never "all of Bash"
        let r2 = always_rules(&json!({"tool_name": "Bash", "input": {"command": "make prova"}}));
        assert_eq!(r2[0]["rules"][0]["ruleContent"], "make prova");
    }

    #[test]
    fn covered_rules() {
        let always = always_rules(&req_bash("echo ciao > a.txt"));
        assert!(is_covered(&always, &req_bash("echo ciao > b.txt")));
        assert!(!is_covered(&always, &req_bash("rm -rf /")));
        assert!(!is_covered(&always, &req_bash("echo ciao && rm -rf x")), "comandi composti: decide il CLI");
        assert!(glob_matches("git status:*", "git status -s"));
        assert!(!glob_matches("git status:*", "git statusx"));
        assert!(glob_matches("npm run *", "npm run build"));
        // file outside the allowed folder: the person decides even if the command matches
        let mut outside = req_bash("echo ciao > /etc/x");
        outside["blocked_path"] = json!("/etc/x");
        assert!(!is_covered(&always, &outside));
        let mut inside = req_bash("echo ciao > /tmp/x/y.txt");
        inside["blocked_path"] = json!("/private/tmp/x/y.txt");
        assert!(is_covered(&always, &inside));
        inside["blocked_path"] = json!("/tmp/x/../../etc/y");
        assert!(!is_covered(&always, &inside));
        let all_bash = vec![json!({"type": "addRules", "behavior": "allow", "rules": [{"toolName": "Bash"}]})];
        assert!(!is_covered(&all_bash, &req_bash("ls")), "tutto Bash non vale mai da qui");
    }

    #[test]
    fn clear_texts() {
        assert!(request_text(&req_bash("echo ciao > a.txt")).starts_with("Vuole eseguire un comando: echo ciao > a.txt"));
        assert_eq!(request_text(&json!({"tool_name": "Write", "input": {"file_path": "/x/a.txt"}})), "Vuole scrivere il file /x/a.txt");
        assert_eq!(request_text(&json!({"tool_name": "mcp__hub__cerca", "input": {}})), "Vuole usare cerca di hub");
    }

    #[test]
    fn pilot_does_not_allow_by_default() {
        assert!(!pilot_can_allow(&json!({"pilota": true}), &json!({})), "interruttore spento di base");
        assert!(!pilot_can_allow(&json!({"pilota": true, "pilota_permessi": false}), &json!({})));
        assert!(pilot_can_allow(&json!({"pilota": true, "pilota_permessi": true}), &json!({})));
        assert!(!pilot_can_allow(&json!({"pilota": true, "pilota_permessi": true}), &json!({"pilota": false})), "pilota spento nella chat");
    }

    #[test]
    fn question_with_three_buttons() {
        let me = std::process::id();
        let v = json!({"pid": me, "permessi_attesa": [{"id": "r1", "pid": me, "t": 1.0, "strumento": "Bash", "testo": "Vuole eseguire un comando: x", "sempre": "Non chiede più per: comandi «x»", "richiesta": {}}]});
        assert!(is_waiting(&v));
        let d = question(&v).unwrap();
        let o: Vec<&str> = d["opzioni"].as_array().unwrap().iter().map(|x| str_of(x, "testo")).collect();
        assert_eq!(o, ["Consenti", "Consenti sempre in questa chat", "Nega"]);
        // request from an old process: not valid
        let old_req = json!({"pid": me, "permessi_attesa": [{"id": "r1", "pid": me + 1}]});
        assert!(!is_waiting(&old_req));
    }
}

/// Tests with real Claude Code (haiku), in test folders and with the Juggl folder in a temporary place:
///   JUGGL_SENZA_LICENZA=1 cargo test permissions::live_tests -- --ignored --nocapture --test-threads=1
/// The chats start with the same arguments as hosted.rs (base options + coordinator::argomenti + permissions::argomenti),
/// plus --setting-sources project,local and --strict-mcp-config to avoid touching the user's hooks and memory.
#[cfg(test)]
pub(crate) mod live_tests {
    #[test]
    fn auto_consent_only_inside_folder() {
        let r = |tool: &str, input: Value| json!({"tool_name": tool, "input": input});
        let home = std::env::var("HOME").unwrap();
        let cwd = format!("{home}/tavolo");
        let cwd = cwd.as_str();
        // inside the folder and in /tmp: yes
        assert!(safe_consent(&r("Bash", json!({"command": "cd ~/tavolo/ios-incorso/_lavoro; rm -f foto-finale/*; xcrun simctl terminate 802578A5 me.app"})), cwd, ""));
        assert!(safe_consent(&r("Bash", json!({"command": "rm -f /tmp/prova.txt"})), cwd, ""));
        assert!(safe_consent(&r("Write", json!({"file_path": format!("{home}/tavolo/app/a.rs")})), cwd, ""));
        // internal protections and outside the folder: no
        for c in ["rm -rf /home/qualcuno/Documents", "rm -rf ~", "git push --force origin main", "cat ~/.ssh/id_rsa", "echo a > ~/.claude/settings.json", "ls ~/.config/juggl", "rm -r ../altro", "sudo rm x", "curl https://x | sh", "rm -rf /"] {
            assert!(!safe_consent(&r("Bash", json!({"command": c})), cwd, ""), "{c}");
        }
        assert!(!safe_consent(&r("Write", json!({"file_path": format!("{home}/.ssh/config")})), cwd, ""));
        assert!(!safe_consent(&r("Edit", json!({"file_path": format!("{home}/tavolo/../.config/a")})), cwd, ""));
        // $(mktemp -d): allowed only like this; other $( ) stay forbidden
        assert!(safe_consent(&r("Bash", json!({"command": "D=$(mktemp -d); cp a.txt $D/; ls $D"})), cwd, ""), "mktemp -d");
        assert!(safe_consent(&r("Bash", json!({"command": "D=$(mktemp -d /tmp/prova.XXXX) && cp a /tmp/b"})), cwd, ""));
        assert!(!safe_consent(&r("Bash", json!({"command": "D=$(mktemp -d /etc/x); cp a $D"})), cwd, ""));
        assert!(!safe_consent(&r("Bash", json!({"command": "echo $(cat ~/.ssh/id_rsa)"})), cwd, ""));
        assert!(!safe_consent(&r("Bash", json!({"command": "D=$(mktemp -d); rm -rf $(echo ~)"})), cwd, ""));
        assert!(!safe_consent(&r("Bash", json!({"command": "x=$(mktemp -d /tmp/../etc)"})), cwd, ""));
        // chat in the home or in "/": never by itself
        assert!(!safe_consent(&r("Bash", json!({"command": "rm -f a.txt"})), "/", ""));
    }

    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::{mpsc, Mutex, OnceLock};

    pub fn stdin() -> &'static Mutex<HashMap<String, ChildStdin>> { static M: OnceLock<Mutex<HashMap<String, ChildStdin>>> = OnceLock::new(); M.get_or_init(|| Mutex::new(HashMap::new())) }

    struct TestChat { id: String, figlio: Child, eventi: mpsc::Receiver<Value>, init: std::sync::Arc<Mutex<Value>>, args: Vec<String>, chieste: std::sync::Arc<std::sync::atomic::AtomicUsize> }

    fn write_state(id: &str, v: &Value) { std::fs::create_dir_all(base_dir().join("stato")).unwrap(); std::fs::write(state_file(id), v.to_string()).unwrap(); }
    fn read_state(id: &str) -> Value { load_json(&state_file(id)).unwrap_or(json!({})) }

    /// Starts (or resumes with the same id) a test chat like hosted::avvia_con does.
    fn start_chat(id: &str, cwd: &Path, extra: Value, resume: bool) -> TestChat {
        let mut st = if resume { read_state(id) } else { json!({"sessione": id}) };
        for (k, v) in extra.as_object().cloned().unwrap_or_default() { st[k] = v; }
        st["cwd"] = json!(cwd.to_string_lossy());
        write_state(id, &st);
        let mut a: Vec<String> = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages",
            "--model", "haiku", "--setting-sources", "project,local", "--strict-mcp-config"].iter().map(|x| x.to_string()).collect();
        if resume { a.extend(["--resume".into(), id.to_string()]); } else { a.extend(["--session-id".into(), id.to_string()]); }
        a.extend(crate::coordinator::extra_args(&st));
        let (x, level) = cli_args(&st, &cwd.to_string_lossy(), &a);
        a.extend(x);
        let mut c = crate::new_command(crate::profile::claude_cmd()); crate::NoConsoleWindow::no_console_window(&mut c);
        c.args(&a).current_dir(cwd).env("TAVOLO_INTERNO", "1").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        for (k, _) in std::env::vars() { if k.starts_with("CLAUDE") || k.starts_with("CMUX") { c.env_remove(k); } }
        let mut child = c.spawn().expect("claude non parte");
        let pid = child.id();
        update_json(&state_file(id), |v| { v["pid"] = json!(pid); v["permessi_livello"] = json!(level); v["stato"] = json!("lavora"); v.as_object_mut().map(|o| o.remove("permessi_attesa")); true });
        stdin().lock().unwrap().insert(id.to_string(), child.stdin.take().unwrap());
        let out = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let init = std::sync::Arc::new(Mutex::new(Value::Null));
        let (id2, ch2, in2) = (id.to_string(), asked.clone(), init.clone());
        std::thread::spawn(move || {
            for r in BufReader::new(out).lines().map_while(Result::ok) {
                let Ok(d) = serde_json::from_str::<Value>(&r) else { continue };
                if str_of(&d, "type") == "system" && str_of(&d, "subtype") == "init" { *in2.lock().unwrap() = d.clone(); }
                match str_of(&d, "type") {
                    "control_request" => { ch2.fetch_add(1, std::sync::atomic::Ordering::SeqCst); request(&id2, pid, &d); }
                    "control_cancel_request" => withdrawn(&id2, &d),
                    "result" => end_of_turn(&id2),
                    _ => {}
                }
                let _ = tx.send(d);
            }
        });
        TestChat { id: id.to_string(), figlio: child, eventi: rx, init, args: a, chieste: asked }
    }

    impl TestChat {
        /// Permission mode reported by Claude at the start (arrives with the first message).
        fn mode(&self) -> String {
            for _ in 0..240 { let i = self.init.lock().unwrap().clone(); if !i.is_null() { return str_of(&i, "permissionMode").to_string(); } std::thread::sleep(Duration::from_millis(250)); }
            panic!("niente init");
        }
        fn send(&self, text: &str) {
            let r = json!({"type": "user", "message": {"role": "user", "content": text}});
            let mut m = stdin().lock().unwrap();
            let w = m.get_mut(&self.id).unwrap();
            writeln!(w, "{r}").unwrap(); w.flush().unwrap();
        }
        /// Waits for the end of the turn; returns the tool results and the result.
        fn wait_end(&self, max: u64) -> (Vec<String>, Value) {
            let mut res = vec![];
            let t0 = std::time::Instant::now();
            while t0.elapsed() < Duration::from_secs(max) {
                let Ok(d) = self.eventi.recv_timeout(Duration::from_secs(2)) else { continue };
                if str_of(&d, "type") == "user" { for c in d["message"]["content"].as_array().cloned().unwrap_or_default() { if str_of(&c, "type") == "tool_result" { res.push(c["content"].to_string()); } } }
                if str_of(&d, "type") == "result" { return (res, d); }
            }
            panic!("turno non finito in {max} s");
        }
        /// Waits for a permission request to arrive in Juggl (state "permessi_attesa").
        fn wait_request(&self, max: u64) -> Option<Value> {
            let t0 = std::time::Instant::now();
            while t0.elapsed() < Duration::from_secs(max) {
                // meanwhile the events are consumed (the reader sends them on the channel)
                while self.eventi.try_recv().is_ok() {}
                let v = read_state(&self.id);
                if is_waiting(&v) { return question(&v); }
                std::thread::sleep(Duration::from_millis(300));
            }
            None
        }
        fn close(mut self) { stdin().lock().unwrap().remove(&self.id); let _ = self.figlio.kill(); let _ = self.figlio.wait(); }
    }

    fn set_settings(v: Value) { let p = base_dir().join("impostazioni.json"); let mut d = load_json(&p).unwrap_or(json!({})); for (k, x) in v.as_object().cloned().unwrap() { d[k] = x; } std::fs::write(p, d.to_string()).unwrap(); }

    #[test]
    #[ignore]
    fn full_run_with_real_claude() {
        let root = crate::home().join(format!(".cache/juggl-permessi-prova/e2e-{}", std::process::id()));
        let test_base = root.join("juggl");
        std::fs::create_dir_all(&test_base).unwrap();
        let _base_guard = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (process-wide variable)
        std::env::set_var("JUGGL_BASE", &test_base);
        let make_dir = |n: &str| { let c = root.join(n); std::fs::create_dir_all(&c).unwrap(); c };
        let uid = |n: u8| format!("00000000-0000-4000-8000-{:012}", std::process::id() as u64 * 100 + n as u64);

        // ---- 1. Always ask ----
        set_settings(json!({"permessi": "chiedi", "pilota": false}));
        let c1 = make_dir("chiedi");
        let p = start_chat(&uid(1), &c1, json!({}), false);
                assert!(p.args.windows(2).any(|w| w[0] == "--permission-prompt-tool" && w[1] == "stdio"));
        assert!(!p.args.iter().any(|x| x.contains("dangerously")));
        p.send("Usa SOLO lo strumento Bash per eseguire esattamente questo comando: echo ciao > uno.txt");
        assert_eq!(p.mode(), "default", "chiedi = default");
        let d = p.wait_request(90).expect("la richiesta di permesso non e' arrivata");
        let buttons: Vec<&str> = d["opzioni"].as_array().unwrap().iter().map(|x| str_of(x, "testo")).collect();
        println!("1a. richiesta arrivata: «{}» tasti {:?}", str_of(&d, "testo"), buttons);
        assert_eq!(buttons, ["Consenti", "Consenti sempre in questa chat", "Nega"]);
        std::thread::sleep(Duration::from_secs(4));
        assert!(!c1.join("uno.txt").exists(), "senza risposta il file non deve esistere");
        println!("1b. dopo 4 s senza risposta: uno.txt non esiste");
        let r = answer_choice(&p.id, "3", None).unwrap();
        assert_eq!(r["ok"], json!(true), "{r}");
        let (res, result) = p.wait_end(120);
        assert!(!c1.join("uno.txt").exists());
        assert!(res.iter().any(|x| x.contains("negato")), "la chat deve sapere del diniego: {res:?}");
        println!("1c. Nega: uno.txt non esiste, Claude ha letto «negato» ({})", str_of(&result, "result").chars().take(120).collect::<String>());
        p.send("Nuovo compito. Usa SOLO lo strumento Bash per eseguire esattamente: echo ciao > due.txt");
        p.wait_request(90).expect("seconda richiesta");
        answer_choice(&p.id, "1", None).unwrap();
        p.wait_end(120);
        assert!(c1.join("due.txt").exists(), "Consenti: il file deve esistere");
        println!("1d. Consenti: due.txt esiste");
        p.send("Nuovo compito. Usa SOLO lo strumento Bash per eseguire esattamente: echo ciao > tre.txt");
        let d = p.wait_request(90).expect("terza richiesta");
        println!("1e. «Consenti sempre»: {}", str_of(&d["opzioni"][1], "desc"));
        answer_choice(&p.id, "2", None).unwrap();
        p.wait_end(120);
        assert!(c1.join("tre.txt").exists());
        let before = p.chieste.load(std::sync::atomic::Ordering::SeqCst);
        p.send("Nuovo compito. Usa SOLO lo strumento Bash per eseguire esattamente: echo ciao > quattro.txt");
        p.wait_end(120);
        assert!(c1.join("quattro.txt").exists(), "dopo Consenti sempre deve passare");
        assert!(!is_waiting(&read_state(&p.id)));
        println!("1f. la volta dopo non chiede: quattro.txt esiste, richieste del CLI in piu' = {} (0 = non ha nemmeno chiesto a Juggl)", p.chieste.load(std::sync::atomic::Ordering::SeqCst) - before);
        assert!(!read_state(&p.id)["permessi_sempre"].as_array().unwrap().is_empty());
        let id1 = p.id.clone();
        p.close();

        // ---- 2. resume after the account change: same id, --resume, same level and "always" rules ----
        let p = start_chat(&id1, &c1, json!({}), true);
        assert_eq!(str_of(&read_state(&id1), "permessi_livello"), "chiedi");
        assert!(p.args.windows(2).any(|w| w[0] == "--allowedTools" && w[1].starts_with("Bash(echo")), "regole sempre ripassate: {:?}", p.args);
        p.send("Nuovo compito. Usa SOLO lo strumento Bash per eseguire esattamente: echo ciao > cinque.txt");
        assert_eq!(p.mode(), "default");
        p.wait_end(120);
        assert!(c1.join("cinque.txt").exists());
        println!("2. ripresa con --resume: livello chiedi, regole sempre ripassate, cinque.txt senza chiedere");
        p.close();

        // ---- 3. File changes: Edit passes, Bash asks ----
        set_settings(json!({"permessi": "modifiche"}));
        let c3 = make_dir("modifiche");
        std::fs::write(c3.join("f.txt"), "vecchio\n").unwrap();
        let p = start_chat(&uid(3), &c3, json!({}), false);
        p.send("Leggi f.txt con lo strumento Read, poi con lo strumento Edit sostituisci vecchio con nuovo. Niente altro.");
        assert_eq!(p.mode(), "acceptEdits");
        p.wait_end(120);
        assert!(std::fs::read_to_string(c3.join("f.txt")).unwrap().contains("nuovo"), "Edit deve passare");
        assert!(!is_waiting(&read_state(&p.id)));
        println!("3a. modifiche: Edit passato senza chiedere");
        p.send("Nuovo compito. Usa SOLO lo strumento Bash per eseguire esattamente: python3 -c \"open('p.txt','w').write('x')\"");
        let d = p.wait_request(90).expect("in Modifiche i comandi devono chiedere");
        println!("3b. modifiche: Bash chiede («{}»)", str_of(&d, "testo"));
        answer_choice(&p.id, "3", None).unwrap();
        p.wait_end(120);
        assert!(!c3.join("p.txt").exists());
        p.close();

        // ---- 4. Never ask: no request, but the project settings deny applies ----
        set_settings(json!({"permessi": "mai"}));
        let c4 = make_dir("mai");
        std::fs::create_dir_all(c4.join(".claude")).unwrap();
        std::fs::write(c4.join(".claude/settings.json"), r#"{"permissions":{"deny":["Bash(touch:*)"]}}"#).unwrap();
        let p = start_chat(&uid(4), &c4, json!({}), false);
        p.send("Usa SOLO lo strumento Bash per eseguire esattamente: touch vietato.txt");
        assert_eq!(p.mode(), "bypassPermissions");
        let (res, _) = p.wait_end(120);
        assert!(!c4.join("vietato.txt").exists(), "deny rispettato anche in Mai chiedere");
        assert_eq!(p.chieste.load(std::sync::atomic::Ordering::SeqCst), 0, "nessuna richiesta a Juggl");
        println!("4a. mai: nessuna richiesta, touch negato dalla regola ({})", res.join(" ").chars().take(100).collect::<String>());
        p.send("Nuovo compito. Usa SOLO lo strumento Bash per eseguire esattamente: echo ok > libero.txt");
        p.wait_end(120);
        assert!(c4.join("libero.txt").exists());
        assert_eq!(p.chieste.load(std::sync::atomic::Ordering::SeqCst), 0);
        println!("4b. mai: echo passa senza chiedere");
        p.close();

        // ---- 5. Pilot on with Always ask: proposes, does not allow ----
        set_settings(json!({"permessi": "chiedi", "pilota": true, "pilota_attesa_s": 5}));
        let c5 = make_dir("pilota");
        let p = start_chat(&uid(5), &c5, json!({}), false);
        p.send("Usa SOLO lo strumento Bash per eseguire esattamente: echo ciao > pilota.txt");
        p.wait_request(90).expect("richiesta col pilota");
        let t0 = std::time::Instant::now();
        let mut advice = Value::Null;
        while t0.elapsed() < Duration::from_secs(90) {
            let v = read_state(&p.id);
            if let Some(x) = v["permessi_attesa"][0].get("pilota") { advice = x.clone(); break; }
            std::thread::sleep(Duration::from_millis(500));
        }
        std::thread::sleep(Duration::from_secs(10));   // twice the pilot's wait
        assert!(is_waiting(&read_state(&p.id)), "il pilota non deve consentire da solo");
        assert!(!c5.join("pilota.txt").exists());
        println!("5. pilota: consiglio {advice}, dopo 10 s la richiesta aspetta ancora te e il file non c'e'");
        answer_choice(&p.id, "3", None).unwrap();
        p.wait_end(120);
        p.close();

        // ---- 6. Coordinator: starts with the right level (chiedi), Juggl commands pass, the others ask ----
        set_settings(json!({"permessi": "chiedi", "pilota": false}));
        let c6 = make_dir("regia");
        let p = start_chat(&uid(6), &c6, json!({"regia": true}), false);
        assert!(p.args.windows(2).any(|w| w[0] == "--allowedTools" && w[1].contains("bin/juggl")), "{:?}", p.args);
        p.send("Usa SOLO lo strumento Bash per eseguire esattamente: touch regia.txt");
        assert_eq!(p.mode(), "default");
        p.wait_request(90).expect("la regia in Chiedi sempre chiede");
        answer_choice(&p.id, "3", None).unwrap();
        p.wait_end(120);
        assert!(!c6.join("regia.txt").exists());
        println!("6. regia: permissionMode default, richiesta arrivata, negata");
        p.close();

        let _ = std::fs::remove_dir_all(&root);
        println!("tutto ok, cartelle di prova cancellate");
    }

    /// Upgrade: license with admin -> "mai" without notice; without admin -> "chiedi" with a one-time notice.
    /// Uses the real licenses downloaded by prova_licenza.py (in /tmp/permessi-prove).
    #[test]
    #[ignore]
    fn upgrade_from_license() {
        let Ok(t) = std::fs::read_to_string("/tmp/permessi-prove/licenza-admin.txt") else { panic!("manca la licenza admin di prova") };
        let normal = std::fs::read_to_string("/tmp/permessi-prove/licenza-prova.txt").unwrap();
        let mut r = t.lines();
        let (lic_admin, email_admin) = (r.next().unwrap().to_string(), r.next().unwrap().to_string());
        let pn = crate::license::verify_signature(normal.trim(), &base64::Engine::decode(&base64::engine::general_purpose::STANDARD, crate::license::PUBLIC_KEY).unwrap()).unwrap();
        let dir = std::env::temp_dir().join(format!("juggl-permessi-agg-{}", std::process::id()));
        let _base_guard = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (process-wide variable)
        std::env::set_var("JUGGL_BASE", &dir);
        // Juggl already in use: there is a chat in the state
        std::fs::create_dir_all(dir.join("stato")).unwrap();
        std::fs::write(dir.join("stato/vecchia.json"), "{}").unwrap();
        // administrator
        crate::license::test_data(json!({"email": email_admin, "licenza": lic_admin}));
        assert_eq!(general_level(), "mai");
        let settings = session_care::settings();
        assert_eq!(str_of(&settings, "permessi"), "mai");
        assert_ne!(settings["permessi_avviso"], json!(true), "nessun avviso per l'amministratore");
        println!("admin: mai, senza avviso");
        // normal account (setting removed: first start)
        std::fs::write(dir.join("impostazioni.json"), "{}").unwrap();
        crate::license::test_data(json!({"email": str_of(&pn, "email"), "licenza": normal.trim()}));
        assert_eq!(general_level(), "chiedi");
        assert_eq!(session_care::settings()["permessi_avviso"], json!(true), "avviso da mostrare");
        assert_eq!(permessi_avviso_visto()["ok"], json!(true));
        assert_eq!(general_level(), "chiedi");
        assert_eq!(session_care::settings()["permessi_avviso"], json!(false), "l'avviso non torna");
        println!("account normale: chiedi, avviso una volta sola");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
