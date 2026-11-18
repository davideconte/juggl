// Notices and autopilot.
// Notices: when a session turns to "waits for you" or "asks for a permission" a Mac notification with sound arrives,
// unless you are already looking at it.
// Autopilot: when a session stops with a confirmation question ("shall I proceed?"), a small model decides whether
// it can go on by itself following the user's rules (best practice, but never production, clients, money,
// deletions, irreversible things, matters of taste). Before replying it waits a minute: it can be cancelled.
#[cfg(windows)] use crate::sys as libc;
use crate::{update_json, base_dir, f64_of, session_care, load_json, unix_now, str_of};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};

/// Session the user is looking at right now in the app (to avoid useless notices).
pub static WATCHED: Mutex<String> = Mutex::new(String::new());
/// All sessions open in the side-by-side columns (active one included): you see them on screen, so no notices.
pub static WATCHED_ALL: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The session is on screen: active column or one of the side-by-side columns.
pub fn on_screen(id: &str) -> bool {
    if id.is_empty() { return false; }
    WATCHED.lock().map(|g| *g == id).unwrap_or(false) || WATCHED_ALL.lock().map(|g| g.iter().any(|x| x == id)).unwrap_or(false)
}

fn state_file(id: &str) -> std::path::PathBuf {
    let clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).collect();
    base_dir().join("stato").join(format!("{clean}.json"))
}

fn play_sound(name: &str) {
    let p = format!("/System/Library/Sounds/{name}.aiff");
    let _ = crate::new_command("afplay").args(["-v", "0.6", &p]).spawn();
}

/// One pass every 3 seconds: compares the sessions' state with the previous pass.
pub struct Sentinel { prima: HashMap<String, (String, i64)> }

impl Sentinel {
    pub fn new() -> Self { Sentinel { prima: HashMap::new() } }

    pub fn tick(&mut self, app: &AppHandle, sessions: &[Value]) {
        // user's requests: new messages and finished turns (model calls run in other threads)
        crate::requests::tick(app, sessions);
        // approvals and things that remain: just-finished turns and session files (model calls run in another thread)
        crate::approvals::tick(sessions);
        let first_pass = self.prima.is_empty();
        let settings = session_care::settings();
        let window_active = app.get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(false);
        for v in sessions {
            let id = str_of(v, "sessione").to_string();
            // state of the turn, not of the row: a session that "works" only for sub-agents has its turn finished,
            // and when the sub-agents finish it is not a new turn (no fake notices)
            let view = crate::turn_view(v).to_string();
            // a just-finished turn: state "aspetta"/"bloccata" with a new end time (even if it was very fast)
            let since = f64_of(v, "da") as i64;
            let previous = self.prima.insert(id.clone(), (view.clone(), since));
            if first_pass {
                let without_what = v.get("serve_te").and_then(|x| x.as_bool()).unwrap_or(false) && str_of(v, "serve_te_cosa").is_empty();
                if view == "aspetta" && (v.get("serve_te").is_none() || without_what) {
                    let (app2, v2) = (app.clone(), v.clone());
                    std::thread::spawn(move || evaluate_with(&app2, &v2, false));
                }
                continue;
            }
            if !matches!(view.as_str(), "aspetta" | "bloccata") { continue; }
            if let Some((prev_view, prev_since)) = &previous { if prev_view == &view && *prev_since == since { continue; } } else { continue; }
            // automatic coordinator (Nov 12): coordinator turn started by an automatic Juggl message: no notice,
            // only the "needs you" evaluation for the group's summary line (src/coordinator.rs)
            if crate::coordinator::without_notice(v) { if view == "aspetta" { let (app2, v2) = (app.clone(), v.clone()); std::thread::spawn(move || evaluate(&app2, &v2)); } continue; }
            // notice, unless you are looking at it
            let preview = if id.starts_with("locale:") { String::new() } else { str_of(v, if view == "bloccata" { "domanda" } else { "risposta" }).chars().take(200).collect::<String>() };
            // local PC chat: its name is usually the start of the first message, so in the notices (which stay
            // in the Notification Center and in notifiche.json) only "Chat locale" is written
            let notice_name = if id.starts_with("locale:") { "Chat locale".to_string() } else { str_of(v, "nome").to_string() };
            session_care::record(&format!("{} {}", notice_name, if view == "bloccata" { "ti chiede qualcosa" } else { "ha finito" }),
                &preview, &id, if view == "bloccata" { "domanda" } else { "fine" });
            if !(window_active && on_screen(&id)) && settings["avvisi"].as_bool().unwrap_or(true) {
                let name = notice_name.as_str();
                let local_chat = id.starts_with("locale:");   // local PC chat: the text does not leave the Mac (Nov 10)
                let body = if local_chat { "Risposta pronta".to_string() } else if view == "bloccata" { format!("Chiede un permesso: {}", str_of(v, "domanda")) } else { str_of(v, "risposta").chars().take(140).collect() };
                session_care::notify(&format!("{name} {}", if view == "bloccata" { "ha bisogno di te" } else { "ti aspetta" }), &body);
                // push to the iPhone (even with the app closed) if the user is not at the Mac and the app is not open
                if crate::push::is_needed() && !local_chat {
                    let project = v.get("progetto").map(|p| str_of(p, "id").to_string()).unwrap_or_default();
                    crate::push::send_in_background(format!("{name} {}", if view == "bloccata" { "ha bisogno di te" } else { "ti aspetta" }), body.clone(), id.to_string(), project);
                }
                play_sound(if view == "bloccata" { "Funk" } else { "Glass" });
                if let Some(w) = app.get_webview_window("main") { let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational)); }
            }
            // classification (needs you?) and autopilot
            if view == "aspetta" {
                let (app2, v2) = (app.clone(), v.clone());
                std::thread::spawn(move || evaluate(&app2, &v2));
            }
        }
        deadlines(app, sessions);
    }
}

fn last_messages(v: &Value) -> (String, Vec<String>) {
    let d = crate::session_detail_pub(v);
    let ev = d["eventi"].as_array().cloned().unwrap_or_default();
    let last = ev.iter().rev().find(|e| str_of(e, "tipo") == "agente").map(|e| str_of(e, "testo").to_string()).unwrap_or_default();
    let yours: Vec<String> = ev.iter().rev().filter(|e| str_of(e, "tipo") == "tu").take(4).map(|e| str_of(e, "testo").chars().take(400).collect()).collect();
    (last, yours)
}

fn login_path() -> String {
    #[cfg(windows)] { return std::env::var("PATH").unwrap_or_default(); }
    #[cfg(target_os = "linux")] { return crate::profile::linux_shell_path(); }
    #[allow(unreachable_code)]
    static P: OnceLock<String> = OnceLock::new();
    P.get_or_init(|| crate::new_command("/bin/zsh").args(["-ilc", "printf %s \"$PATH\""]).output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()).clone()
}

/// Asks a model (haiku by default, the small one) and reads the JSON reply. No tools, 3 minutes at most.
/// It must always be called from a separate thread, never from the main one.
pub(crate) fn ask_model(question: &str, model: Option<&str>) -> Option<Value> {
    ask_for("altro", question, model, Cache::No, true)
}

/// How to remember replies (cache by text hash, 6 hours, in memory only).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Cache {
    /// Every call is a call.
    No,
    /// The same text already analysed is not analysed again (returns `None`): for the requests extractor, whose
    /// result was already applied the first time.
    SoloUna,
    /// The same text gets the previous reply: for summaries.
    Risposta,
}

/// Helper calls go through here (0.1.70): switch per category, 24-hour cap (not for `urgente` calls,
/// for example a turn waiting for the user), cache by hash and a counter per category. Whoever gets `None` because of cap or cache
/// must not discard the work: the requests extractor resends it (see `requests::estrai`).
pub(crate) fn ask_for(cat: &str, question: &str, model: Option<&str>, cache: Cache, urgent: bool) -> Option<Value> {
    ask_for_in(cat, question, model, cache, urgent, None)
}

/// Like `ask_for`, with its own working folder (help and reports, Nov 17: empty folder and no saved session,
/// so the call does not enter the index, the Requests nor the per-project usage).
pub(crate) fn ask_for_in(cat: &str, question: &str, model: Option<&str>, cache: Cache, urgent: bool, folder: Option<&std::path::Path>) -> Option<Value> {
    use crate::helpers as ai;
    if !ai::is_enabled(cat) { ai::record(cat, "spenta"); return None; }
    if !urgent && ai::over_cap(cat) { ai::record(cat, "rimandata"); return None; }
    let h = ai::hash(cat, question);
    if cache != Cache::No {
        if let Some(remembered) = ai::seen_recently(h) {
            ai::record(cat, "cache");
            return if cache == Cache::Risposta { remembered } else { None };
        }
    }
    let r = ask_uncounted(question, model, folder);
    ai::record(cat, if r.is_some() { "ok" } else { "errore" });
    if cache != Cache::No && r.is_some() { ai::remember(h, if cache == Cache::Risposta { r.clone() } else { None }); }
    r
}

fn ask_uncounted(question: &str, model: Option<&str>, folder: Option<&std::path::Path>) -> Option<Value> {
    let claude = crate::profile::claude_cmd();
    #[cfg(unix)] use std::os::unix::process::CommandExt;
    #[cfg(windows)] use crate::sys::CommandExtJuggl;
    let question = crate::profile::with_name(question);   // the prompts say «NOME»: the name of the person using Juggl
    // --setting-sources project,local: no user memory, global CLAUDE.md, skills and MCP services, which are not needed here
    // (the question already contains everything). Measured on Nov 11: from 127,124 to about 6,000 input tokens per call (with --strict-mcp-config), same reply.
    let mut c = crate::new_command(claude);
    c.args(["-p", "--model", model.unwrap_or("haiku"), "--setting-sources", "project,local", "--strict-mcp-config", "--tools", "", "--output-format", "text"]);
    if folder.is_some() { c.arg("--no-session-persistence"); }
    let mut child = c.arg(&question)
        .env("PATH", login_path()).env("TAVOLO_INTERNO", "1").current_dir(folder.map(|p| p.to_path_buf()).unwrap_or_else(base_dir))
        .process_group(0)   // group leader: at the end the grandchildren holding stdout open are stopped too
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let group_id = child.id() as i32;
    let mut output = child.stdout.take()?;
    // the reader sends the text over a channel: if a grandchild keeps stdout open, I stop waiting for it
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let mut t = String::new(); let _ = std::io::Read::read_to_string(&mut output, &mut t); let _ = tx.send(t); });
    let start_time = unix_now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if unix_now() - start_time < 180.0 => std::thread::sleep(std::time::Duration::from_millis(200)),
            _ => { unsafe { libc::kill(-group_id, libc::SIGKILL); } let _ = child.kill(); let _ = child.wait(); break; }
        }
    }
    let t = match rx.recv_timeout(std::time::Duration::from_secs(3)) {
        Ok(t) => t,
        Err(_) => { unsafe { libc::kill(-group_id, libc::SIGKILL); } rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap_or_default() }
    };
    unsafe { libc::kill(-group_id, libc::SIGTERM); }   // no child is left around
    let a = t.find('{')?; let b = t.rfind('}')?;
    serde_json::from_str(&t[a..=b]).ok()
}

const RULES: &str = "Sei il pilota automatico di «NOME», che lavora con molte sessioni di agenti AI di programmazione insieme. \
Una sessione si e' fermata e aspetta una risposta. Decidi se puoi rispondere tu per farla andare avanti, invece di lasciarla ferma. \
Le regole di «NOME»: vuole che gli agenti decidano da soli seguendo la prassi del settore (best practice) e procedano, senza chiedere conferme su scelte tecniche o piccole. \
Va chiesto a lui SOLO per: deploy o pubblicazione in produzione, messaggi o email a clienti o terze persone, spese o pagamenti, cancellare dati o file, azioni irreversibili, \
scelte di gusto o di design dove conta la sua opinione (es. quale variante preferisci, colori, nomi, testi pubblici), informazioni che solo lui sa (password, preferenze personali, fatti che l'agente non puo' verificare). \
Attenzione, «NOME» si lamenta di sessioni segnate come \"ti aspetta\" quando non deve fare niente: usa \"serve_te\" SOLO se senza di lui il lavoro resta davvero fermo. \
Se l'agente ha finito e propone solo extra facoltativi (\"se vuoi posso...\", \"vuoi che...?\", \"fammi sapere se...\"), o chiede una conferma su cosa gia' deciso da «NOME» nei suoi messaggi, l'esito e' \"niente\" o \"procedi\", non \"serve_te\". \
Tre esiti possibili: \"procedi\" (chiede una conferma che puoi dare tu secondo le regole), \"serve_te\" (serve davvero «NOME»: una decisione, un'informazione, un permesso, qualcosa da guardare o approvare), \"niente\" (ha solo riferito un lavoro finito, non chiede niente e non c'e' niente che «NOME» debba fare). \
Rispondi SOLO con un oggetto JSON, senza altro testo: {\"azione\": \"procedi\" oppure \"serve_te\" oppure \"niente\", \"risposta\": \"testo breve da mandare all'agente, in italiano, es. 'Procedi pure con la soluzione migliore secondo la prassi.'\", \"motivo\": \"una frase in italiano\", \"cosa\": \"solo se serve_te: cosa deve fare «NOME», massimo 8 parole, all'imperativo, concreto (es. 'Scegli la variante del logo', 'Approva il messaggio per il cliente')\", \"sicurezza\": numero tra 0 e 1}.";

fn evaluate(app: &AppHandle, v: &Value) { evaluate_with(app, v, true) }

/// Classifies the just-finished turn (does the user need to act or not) and, if allowed, prepares the autopilot's reply.
pub fn evaluate_with(app: &AppHandle, v: &Value, autopilot_ok: bool) {
    let id = str_of(v, "sessione").to_string();
    let p = state_file(&id);
    let Some(st) = load_json(&p) else { return };
    // brakes: no double replies to the same message, at most 3 per hour per session
    let since = f64_of(&st, "da");
    let without_what = st.get("serve_te").and_then(|x| x.as_bool()).unwrap_or(false) && str_of(&st, "serve_te_cosa").is_empty();
    if f64_of(&st, "pilota_su_da") == since && st.get("serve_te").is_some() && !without_what { return; }
    let history: Vec<f64> = st.get("pilota_storia").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
    let autopilot_ok = autopilot_ok && history.iter().filter(|t| unix_now() - **t < 3600.0).count() < 3
        && st.get("pilota").and_then(|x| x.as_bool()).unwrap_or(true) && session_care::settings()["pilota"].as_bool().unwrap_or(true);
    let (last, yours) = last_messages(v);
    if last.trim().is_empty() { return; }
    let question = format!("{RULES}\n\nUltimi messaggi di «NOME» in questa sessione (dal piu' recente):\n{}\n\nUltimo messaggio dell'agente:\n{}",
        yours.iter().map(|x| format!("- {x}")).collect::<Vec<_>>().join("\n"), last.chars().take(3500).collect::<String>());
    let Some(r) = ask_for("pilota", &question, None, Cache::No, true) else { return };
    let action = str_of(&r, "azione").to_string();
    let replies = autopilot_ok && action == "procedi" && f64_of(&r, "sicurezza") >= 0.7 && !str_of(&r, "risposta").trim().is_empty();
    let wait_s = session_care::settings()["pilota_attesa_s"].as_f64().unwrap_or(60.0);
    // under lock, on the current state (the hook may have changed it while the model was thinking)
    update_json(&p, |st| {
        st["pilota_su_da"] = json!(since);
        // does the user really need to act? (a confirmation the autopilot cannot give counts as "needs you")
        st["serve_te"] = json!(action == "serve_te" || action == "aspetta" || (action == "procedi" && !autopilot_ok));
        st["serve_te_motivo"] = json!(str_of(&r, "motivo"));
        st["serve_te_cosa"] = json!(if action == "serve_te" || action == "aspetta" || (action == "procedi" && !autopilot_ok) { str_of(&r, "cosa").to_string() } else { String::new() });
        if replies {
            st["pilota_tra"] = json!(unix_now() + wait_s);
            st["pilota_risposta"] = json!(str_of(&r, "risposta"));
            st["pilota_motivo"] = json!(str_of(&r, "motivo"));
        } else { st["pilota_ultimo_motivo"] = json!(str_of(&r, "motivo")); }
        true
    });
    if replies {
        session_care::record(&format!("Pilota: rispondo a {}", str_of(v, "nome")), &str_of(&r, "risposta"), &id, "pilota");
        session_care::notify(&format!("Pilota automatico: {}", str_of(v, "nome")), &format!("Tra {} s rispondo \"{}\". Apri l'app per annullare.", wait_s as i64, str_of(&r, "risposta")));
    }
    let _ = app.emit("cambiato", ());
}

/// Autopilot replies that have expired: they go out if the session is still stopped on the same message.
fn deadlines(app: &AppHandle, sessions: &[Value]) {
    for v in sessions {
        let t = f64_of(v, "pilota_tra");
        if t <= 0.0 || unix_now() < t { continue; }
        let id = str_of(v, "sessione").to_string();
        let p = state_file(&id);
        let (mut reply, mut still) = (String::new(), false);
        if !update_json(&p, |st| {
            reply = str_of(st, "pilota_risposta").to_string();
            still = str_of(v, "vista") == "aspetta" && f64_of(st, "pilota_su_da") == f64_of(st, "da");
            st["pilota_tra"] = json!(0);
            if still && !reply.is_empty() {
                let mut history: Vec<Value> = st.get("pilota_storia").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                history.push(json!(unix_now()));
                st["pilota_storia"] = json!(history);
                let mut sent: Vec<Value> = st.get("pilota_inviati").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                sent.push(json!(reply));
                st["pilota_inviati"] = json!(sent.into_iter().rev().take(20).collect::<Vec<_>>());
            }
            true
        }) { continue; }
        if still && !reply.is_empty() { let _ = crate::send_internal(id, reply); }
        let _ = app.emit("cambiato", ());
    }
}

/// "Not needed": the session leaves Waiting for you and goes to To read.
pub fn not_needed(id: &str) -> Value {
    let ok = update_json(&state_file(id), |st| { st["serve_te"] = json!(false); st["serve_te_cosa"] = json!(""); st["serve_te_tolto"] = json!(true); true });
    json!({"ok": ok})
}

/// Cancels the pending reply; "spegni" turns the autopilot off for this session.
pub fn cancel(id: &str, turn_off: bool) -> Value {
    let ok = update_json(&state_file(id), |st| { st["pilota_tra"] = json!(0); if turn_off { st["pilota"] = json!(false); } true });
    json!({"ok": ok})
}

pub fn set(id: &str, on: bool) -> Value {
    let ok = update_json(&state_file(id), |st| { st["pilota"] = json!(on); if !on { st["pilota_tra"] = json!(0); } true });
    json!({"ok": ok})
}

/// Sends the autopilot's reply right away (without waiting the minute).
pub fn send_now(id: &str) -> Value {
    let mut r = String::new();
    if !update_json(&state_file(id), |st| { r = str_of(st, "pilota_risposta").to_string(); st["pilota_tra"] = json!(0); true }) { return json!({"ok": false}) }
    if r.is_empty() { return json!({"ok": false}) }
    crate::send_internal(id.to_string(), r)
}
