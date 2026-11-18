// Change Claude account (Nov 12). The user cannot do /login inside Juggl chats (chats are `claude -p`, without a
// terminal: "/login isn't available in this environment"). Here login is done from Juggl, also from the iPhone:
//
//   1. `avvia` launches `claude auth login`, reads the sign-in link from its output and returns it;
//   2. `codice` writes to the login's stdin the code the page shows (on Mac/Linux stdin is a FIFO, `mkfifo`:
//      without a terminal it is the technique that works; on Windows a normal pipe) (if the page returns to Juggl by itself, not needed);
//   3. when login ends (the process exits) the new account is read and `hosted::align_account` is called, the same
//      mechanism as the account change in claude_account.rs: idle chats restart at once, those at work at end of turn,
//      one at a time. Nothing is restarted by hand here.
//
// Rules: one login at a time, expires after 10 minutes, can be cancelled. The code and the tokens are NEVER written
// to logs or disk: the code goes only to the CLI's stdin. The link (public, no secrets) is the only thing that comes out.
//
// Tests without touching the real account: in debug builds and tests `JUGGL_CLAUDE_FINTO=/path/script` launches
// that script instead of `claude auth login` (see the tests at the bottom and ~/tavolo/app/NOTE-MERGE-claude-login.md).
//
// "/..." commands in hosted chats: `local_command` recognizes those that do not exist in -p and says what to do;
// `from_event` recognizes the CLI's "isn't available in this environment" answer for those we did not know.
use crate::{base_dir, home, log_juggl, str_of};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const EXPIRY: Duration = Duration::from_secs(600);
const LINK_WAIT: Duration = Duration::from_secs(30);
const CODE_WAIT: Duration = Duration::from_secs(45);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase { Avvio, Link, Finito, Errore, Annullato, Scaduto }
impl Phase {
    fn label(self) -> &'static str {
        match self { Phase::Avvio => "avvio", Phase::Link => "link", Phase::Finito => "finito", Phase::Errore => "errore", Phase::Annullato => "annullato", Phase::Scaduto => "scaduto" }
    }
    fn in_progress(self) -> bool { matches!(self, Phase::Avvio | Phase::Link) }
}

struct Login {
    numero: u64,                 // tells one login from the next (old threads do not touch the new one)
    fase: Phase,
    link: String,                // first sign-in link found
    manuale: String,             // link "with code to paste", if the CLI prints a different one
    avviato: Instant,
    prima: String,               // account connected before the login
    stdin: Option<ChildStdin>,   // without FIFO (Windows)
    fifo: Option<std::path::PathBuf>,
    figlio: Arc<Mutex<Child>>,
    esito: Value,
}

static LOGIN: Mutex<Option<Login>> = Mutex::new(None);
static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn lock_login() -> MutexGuard<'static, Option<Login>> { LOGIN.lock().unwrap_or_else(|e| e.into_inner()) }

// ---------- reading the link from the output ----------

/// Removes the color codes and terminal links (ESC [ ... letter, ESC ] ... BEL or ESC \).
fn strip_escapes(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut it = t.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' { out.push(c); continue; }
        match it.peek() {
            Some('[') => { it.next(); while let Some(x) = it.next() { if x.is_ascii_alphabetic() || x == '~' { break; } } }
            Some(']') => { it.next(); while let Some(x) = it.next() { if x == '\u{7}' { break; } if x == '\u{1b}' { it.next(); break; } } }
            _ => { it.next(); }
        }
    }
    out
}

/// Sign-in link in an output line: the first https:// that looks like an authorization page.
pub fn find_link(line: &str) -> Option<String> {
    let cleaned = strip_escapes(line);
    let mut rest = cleaned.as_str();
    while let Some(i) = rest.find("https://") {
        let slice = &rest[i..];
        let end = slice.find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == '>').unwrap_or(slice.len());
        let l = slice[..end].trim_end_matches(|c: char| ".,;:)]}".contains(c));
        if l.contains("client_id=") || l.contains("/oauth/authorize") || l.contains("/authorize?") { return Some(l.to_string()); }
        rest = &slice[end..];
    }
    None
}

/// The "manual" link is the one leading to a page that SHOWS the code (redirect to the site, not to localhost).
fn is_manual(l: &str) -> bool { l.contains("code=true") || (l.contains("redirect_uri=") && !l.contains("localhost") && !l.contains("127.0.0.1")) }

// ---------- login process ----------

fn program() -> (std::path::PathBuf, Vec<String>) {
    #[cfg(any(test, debug_assertions))]
    if let Ok(p) = std::env::var("JUGGL_CLAUDE_FINTO") {
        if std::path::Path::new(&p).is_absolute() && std::path::Path::new(&p).is_file() { return (p.into(), vec!["auth".into(), "login".into()]); }
    }
    (crate::profile::claude_cmd(), vec!["auth".into(), "login".into(), "--claudeai".into()])
}

/// Do-nothing little program, given as BROWSER: so the CLI does not open the page by itself (Juggl opens it,
/// or the link goes to the iPhone). If the CLI ignores the variable and opens the browser, it is just one more tab.
#[cfg(unix)]
fn fake_browser() -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let p = base_dir().join("accesso-browser.sh");
    if !p.is_file() {
        std::fs::write(&p, "#!/bin/sh\nexit 0\n").ok()?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).ok()?;
    }
    Some(p)
}

/// FIFO for the login's stdin (Mac and Linux only). Name with the pid: one FIFO per app, redone at every login.
fn make_fifo() -> Option<std::path::PathBuf> {
    #[cfg(unix)]
    {
        let p = base_dir().join(format!("accesso-{}.fifo", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).ok()?;
        if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0 { return Some(p); }
    }
    None
}
fn remove_fifo(f: &Option<std::path::PathBuf>) { if let Some(p) = f { let _ = std::fs::remove_file(p); } }

fn update_login(number: u64, f: impl FnOnce(&mut Login)) {
    let mut g = lock_login();
    if let Some(l) = g.as_mut() { if l.numero == number { f(l); } }
}

fn read_output(number: u64, stream: impl std::io::Read + Send + 'static) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            // the output is NOT logged: it may contain more than the link
            if let Some(l) = find_link(&line) {
                update_login(number, |x| {
                    if x.link.is_empty() { x.link = l.clone(); }
                    if is_manual(&l) && x.manuale.is_empty() { x.manuale = l; }
                    if x.fase == Phase::Avvio { x.fase = Phase::Link; }
                });
            }
        }
    });
}

/// Starts the login and returns the link. If one is already in progress it returns that one (one login at a time).
pub fn start() -> Value {
    let (numero, figlio) = {
        let mut g = lock_login();
        if let Some(l) = g.as_ref() {
            if l.fase.in_progress() && l.avviato.elapsed() < EXPIRY {
                return if l.link.is_empty() { json!({"ok": false, "msg": "Il login e' gia' in partenza, aspetta qualche secondo"}) }
                else { json!({"ok": true, "gia": true, "link": l.link, "manuale": l.manuale}) };
            }
        }
        let (prog, args) = program();
        let mut c = crate::new_command(&prog);
        c.args(&args).env("TAVOLO_INTERNO", "1").env("PATH", crate::hosted::path_login())
            .current_dir(home()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let fifo = make_fifo();
        match fifo.as_ref().and_then(|p| std::fs::OpenOptions::new().read(true).write(true).open(p).ok()) {
            Some(f) => { c.stdin(Stdio::from(f)); }
            None => { c.stdin(Stdio::piped()); }
        }
        #[cfg(unix)] if let Some(b) = fake_browser() { c.env("BROWSER", b); }
        let mut figlio = match c.spawn() { Ok(f) => f, Err(e) => { remove_fifo(&fifo); return json!({"ok": false, "msg": format!("Non riesco ad avviare Claude: {e}")}) } };
        let stdin = figlio.stdin.take();
        let (so, se) = (figlio.stdout.take(), figlio.stderr.take());
        let numero = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let figlio = Arc::new(Mutex::new(figlio));
        *g = Some(Login { numero, fase: Phase::Avvio, link: String::new(), manuale: String::new(), avviato: Instant::now(),
            prima: crate::claude_account::current(), stdin, fifo, figlio: figlio.clone(), esito: Value::Null });
        if let Some(o) = so { read_output(numero, o); }
        if let Some(e) = se { read_output(numero, e); }
        (numero, figlio)
    };
    log_juggl("accesso: login di Claude avviato");
    watch(numero, figlio);
    // wait for the link (usually less than a second)
    let start = Instant::now();
    loop {
        {
            let g = lock_login();
            let Some(l) = g.as_ref().filter(|l| l.numero == numero) else { return json!({"ok": false, "msg": "Login annullato"}) };
            if !l.link.is_empty() { return json!({"ok": true, "link": l.link, "manuale": l.manuale}); }
            if !l.fase.in_progress() { return json!({"ok": false, "msg": closed_msg(l)}); }
        }
        if start.elapsed() > LINK_WAIT {
            close_login(numero, Phase::Errore, json!({"ok": false, "msg": "Claude non ha dato il link di accesso"}));
            return json!({"ok": false, "msg": "Claude non ha dato il link di accesso. Riprova, o apri il terminale e scrivi: claude auth login"});
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn closed_msg(l: &Login) -> String {
    let m = str_of(&l.esito, "msg");
    if !m.is_empty() { m.to_string() } else { "Il login si e' chiuso senza dare il link. Riprova.".into() }
}

/// Marks the end of the login (if it is still that one) and stops the process.
fn close_login(number: u64, phase: Phase, outcome: Value) {
    let child = {
        let mut g = lock_login();
        let Some(l) = g.as_mut().filter(|l| l.numero == number) else { return };
        if !l.fase.in_progress() { return; }
        l.fase = phase; l.esito = outcome; l.stdin = None; remove_fifo(&l.fifo);
        l.figlio.clone()
    };
    let mut f = child.lock().unwrap_or_else(|e| e.into_inner());
    let _ = f.kill();
    let _ = f.wait();
}

/// Waits for the login to finish by itself, or expires after 10 minutes.
fn watch(number: u64, child: Arc<Mutex<Child>>) {
    std::thread::spawn(move || {
        let start = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let stato = child.lock().unwrap_or_else(|e| e.into_inner()).try_wait();
            match stato {
                Ok(Some(st)) => { finish(number, st.success()); return; }
                Ok(None) => {}
                Err(_) => { finish(number, false); return; }
            }
            let still = lock_login().as_ref().map(|l| l.numero == number && l.fase.in_progress()).unwrap_or(false);
            if !still { return; }
            if start.elapsed() > EXPIRY {
                close_login(number, Phase::Scaduto, json!({"ok": false, "msg": "Login scaduto dopo 10 minuti. Riprova."}));
                log_juggl("accesso: login scaduto");
                return;
            }
        }
    });
}

/// The login process exited: if it went well the new account is read and the chats align.
fn finish(number: u64, succeeded: bool) {
    let (before, still) = {
        let g = lock_login();
        match g.as_ref().filter(|l| l.numero == number) { Some(l) => (l.prima.clone(), l.fase.in_progress()), None => return }
    };
    if !still { return; }
    if !succeeded {
        update_login(number, |l| { l.fase = Phase::Errore; l.stdin = None; remove_fifo(&l.fifo); l.esito = json!({"ok": false, "msg": "Il codice non e' stato accettato, o la pagina non ha completato l'accesso. Riprova."}); });
        log_juggl("accesso: login non riuscito");
        return;
    }
    let outcome = after_login(&before);
    update_login(number, |l| { l.fase = Phase::Finito; l.stdin = None; remove_fifo(&l.fifo); l.esito = outcome.clone(); });
    if let Some(app) = crate::APP.get() {
        use tauri::Emitter;
        let _ = app.emit("accesso-finito", outcome.clone());
        if outcome["ok"].as_bool().unwrap_or(false) { crate::claude_account::notify(app, str_of(&outcome, "msg").to_string()); }
    }
    crate::bridge::broadcast("accesso-finito", &outcome.to_string());
}

/// Current account (from `claude auth status --json`, no token), limits re-read and chats to restart.
fn after_login(before: &str) -> Value {
    let Some(st) = cli_status() else { return json!({"ok": false, "msg": "Il login e' finito ma Claude dice che non c'e' un account collegato. Riprova."}) };
    // the CLI has already written ~/.claude.json and the Keychain: the credentials signature (plan) is re-read right away
    let plan_changed = crate::claude_account::refresh_signature();
    let current = crate::claude_account::current();
    let changed = !current.is_empty() && (before.is_empty() || !crate::claude_account::same_account(&current, before)) || plan_changed;
    // new token = new limits: old ones and waits dropped, read at once (even with the same account)
    crate::quota::credentials_changed();
    // re-entry after a logout (the chats had stayed with the old login): all restart, even with the same account
    let reentry = before.is_empty();
    let e = if reentry { crate::hosted::align_account(&|id: &str| format!("{}!rientro", crate::claude_account::current_for(id))) }
        else { crate::hosted::align_account(&crate::claude_account::current_for) };
    let n = e.riavviate.len() + e.a_fine_turno.len();
    log_juggl(&format!("accesso: login finito, account {} ({} chat da riavviare)", if changed { "cambiato" } else { "invariato" }, n));
    let who = if st.email.is_empty() { st.org.clone() } else { st.email.clone() };
    let msg = if !changed && !reentry { format!("Hai rifatto l'accesso con lo stesso account: {who}. Limiti riletti, le chat restano come sono.") }
        else if n == 0 { format!("Account cambiato: {who}. Nessuna chat da riavviare.") }
        else if n == 1 { format!("Account cambiato: {who}, 1 chat si riavvia da sola.") }
        else { format!("Account cambiato: {who}, {n} chat si riavviano da sole.") };
    json!({"ok": true, "cambiato": changed, "account": st.email, "organizzazione": st.org, "piano": st.piano, "chat": n, "msg": msg})
}

struct CliStatus { email: String, org: String, piano: String }

/// Account, organization and plan from `claude auth status --json` (no token); None if there is no login.
fn cli_status() -> Option<CliStatus> {
    let (prog, _) = program();
    let o = crate::new_command(&prog).args(["auth", "status", "--json"]).env("TAVOLO_INTERNO", "1").env("PATH", crate::hosted::path_login())
        .current_dir(home()).stdin(Stdio::null()).output().ok()?;
    let v: Value = serde_json::from_slice(&o.stdout).ok()?;
    if !v["loggedIn"].as_bool().unwrap_or(false) { return None; }
    let piano = [str_of(&v, "planTier"), str_of(&v, "subscriptionType")].into_iter().find(|x| !x.is_empty()).unwrap_or("").to_string();
    Some(CliStatus { email: str_of(&v, "email").to_string(), org: str_of(&v, "orgName").to_string(), piano })
}

/// Plan (e.g. "max_20x") with a small 60-second memory: `auth status` is a whole process.
fn plan_cache() -> String {
    static C: Mutex<Option<(Instant, String)>> = Mutex::new(None);
    let mut c = C.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, p)) = c.as_ref() { if t.elapsed() < Duration::from_secs(60) { return p.clone(); } }
    let p = cli_status().map(|x| x.piano).unwrap_or_default();
    *c = Some((Instant::now(), p.clone()));
    p
}

/// Log out of the account (`claude auth logout`). Explicit confirmation needed: without it, it says what would happen.
/// After the logout the chats stay idle: Juggl does not restart them until you log back in (whoever changes account restarts from `finish`).
pub fn logout(conferma: bool) -> Value {
    if !conferma {
        return json!({"ok": false, "conferma": true, "msg": "Uscendo dall'account Claude le chat di Juggl non possono rispondere finche' non rientri (Cambia account Claude). Le conversazioni restano salvate."});
    }
    cancel();
    let (prog, _) = program();
    let o = crate::new_command(&prog).args(["auth", "logout"]).env("TAVOLO_INTERNO", "1").env("PATH", crate::hosted::path_login())
        .current_dir(home()).stdin(Stdio::null()).output();
    match o {
        Ok(o) if o.status.success() => {
            log_juggl("accesso: uscito dall'account Claude");
            let _ = crate::quota::credentials_changed_no_reread();
            json!({"ok": true, "msg": "Sei uscito dall'account Claude. Le chat restano ferme finche' non rientri con Cambia account Claude."})
        }
        Ok(_) => json!({"ok": false, "msg": "Claude non e' riuscito a fare uscire l'account. Riprova."}),
        Err(e) => json!({"ok": false, "msg": format!("Non riesco ad avviare Claude: {e}")}),
    }
}

/// The code the page shows: goes only to the CLI's stdin. Waits for the outcome.
pub fn code(c: &str) -> Value {
    let c = c.trim();
    if c.is_empty() { return json!({"ok": false, "msg": "Incolla il codice che vedi nella pagina"}); }
    if c.len() > 1000 || c.chars().any(|x| x.is_control()) { return json!({"ok": false, "msg": "Quello non sembra un codice: incolla solo il codice della pagina"}); }
    let number = {
        let mut g = lock_login();
        let Some(l) = g.as_mut() else { return json!({"ok": false, "msg": "Nessun login in corso: tocca prima Cambia account"}) };
        if !l.fase.in_progress() { return json!({"ok": false, "msg": closed_msg(l)}); }
        let written = if let Some(p) = l.fifo.as_ref() {
            std::fs::OpenOptions::new().write(true).open(p).and_then(|mut f| writeln!(f, "{c}").and_then(|_| f.flush()))
        } else if let Some(si) = l.stdin.as_mut() {
            writeln!(si, "{c}").and_then(|_| si.flush())
        } else { return json!({"ok": false, "msg": "Il login non accetta il codice"}) };
        if written.is_err() { return json!({"ok": false, "msg": "Il login si e' chiuso, riprova da capo"}); }
        l.numero
    };
    let start = Instant::now();
    while start.elapsed() < CODE_WAIT {
        std::thread::sleep(Duration::from_millis(200));
        let g = lock_login();
        let Some(l) = g.as_ref().filter(|l| l.numero == number) else { return json!({"ok": false, "msg": "Login annullato"}) };
        if !l.fase.in_progress() {
            return if l.esito.is_null() { json!({"ok": false, "msg": "Login chiuso"}) } else { l.esito.clone() };
        }
    }
    json!({"ok": false, "msg": "Claude non ha ancora risposto. Controlla il codice e riprova fra un momento.", "attesa": true})
}

pub fn cancel() -> Value {
    let number = lock_login().as_ref().filter(|l| l.fase.in_progress()).map(|l| l.numero);
    if let Some(n) = number { close_login(n, Phase::Annullato, json!({"ok": false, "msg": "Login annullato"})); log_juggl("accesso: login annullato"); }
    json!({"ok": true})
}

/// Account connected now (from ~/.claude.json, no token) and login state.
pub fn status() -> Value {
    let cfg = crate::load_json(&home().join(".claude.json")).unwrap_or(Value::Null);
    let o = &cfg["oauthAccount"];
    let plan = plan_cache();
    let account = json!({"email": str_of(o, "emailAddress"), "organizzazione": str_of(o, "organizationName"), "piano": if plan.is_empty() { str_of(o, "planDisplayName").to_string() } else { plan }, "collegato": !str_of(o, "accountUuid").is_empty()});
    let g = lock_login();
    let login = match g.as_ref() {
        None => json!({"fase": "nessuno"}),
        Some(l) => {
            let rest = EXPIRY.saturating_sub(l.avviato.elapsed()).as_secs();
            let mut v = json!({"fase": l.fase.label(), "secondi": if l.fase.in_progress() { rest } else { 0 }});
            if l.fase.in_progress() { v["link"] = json!(l.link); v["manuale"] = json!(l.manuale); }
            else if !l.esito.is_null() { v["esito"] = l.esito.clone(); }
            v
        }
    };
    json!({"ok": true, "account": account, "login": login})
}


// ---------- account details (Claude Account panel, Nov 14) ----------
// All read-only. NEVER tokens: from the Keychain we only look whether the item exists (without -w / -g, so the
// password is not even printed) and when it was modified; of the credentials file only whether it exists and the date.
// The email goes only to the panel (Tauri command): it is not in the logs and not in the bridge for the iPhone.

/// Claude Code folder (CLAUDE_CONFIG_DIR if set, otherwise ~/.claude).
fn claude_dir() -> std::path::PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| home().join(".claude"))
}

/// "20261113225831Z" (Keychain date) in Unix seconds.
#[cfg(target_os = "macos")]
fn keychain_epoch(t: &str) -> Option<f64> {
    let t = t.trim();
    if t.len() < 14 || !t.is_char_boundary(14) { return None; }
    crate::quota::epoch_of(&format!("{}-{}-{}T{}:{}:{}", &t[0..4], &t[4..6], &t[6..8], &t[8..10], &t[10..12], &t[12..14]))
}

/// Where the credential is: {"dove": "portachiavi"|"file"|"nessuna", "file_esiste", "data"}. Never the content.
fn credential_source() -> Value {
    let file = claude_dir().join(".credentials.json");
    let meta = std::fs::metadata(&file).ok();
    let data_file = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64());
    let file_exists = meta.is_some();
    #[cfg(target_os = "macos")]
    {
        // no -w and no -g: only the item's attributes come out, never the password
        let o = crate::new_command("/usr/bin/security").args(["find-generic-password", "-s", "Claude Code-credentials"]).stdin(Stdio::null()).stderr(Stdio::null()).output();
        if let Ok(o) = o {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout).to_string();
                let mdat = text.lines().find(|l| l.contains("\"mdat\"")).and_then(|l| l.rsplit_once('"').and_then(|(a, _)| a.rsplit_once('"')).map(|(_, d)| d.trim_end_matches("\\000").to_string()))
                    .and_then(|d| keychain_epoch(&d));
                return json!({"dove": "portachiavi", "nome": "Portachiavi di macOS (Claude Code-credentials)", "data": mdat, "file_esiste": file_exists});
            }
        }
    }
    if file_exists { json!({"dove": "file", "nome": format!("File {}", file.display()), "data": data_file, "file_esiste": true}) }
    else { json!({"dove": "nessuna", "nome": "Nessuna credenziale trovata", "data": null, "file_esiste": false}) }
}

/// Default model from Claude Code's settings (read-only settings.json), if any.
fn default_model() -> Option<String> {
    let v = crate::load_json(&claude_dir().join("settings.json"))?;
    Some(str_of(&v, "model").to_string()).filter(|m| !m.is_empty())
}

/// All the details for the panel. `refresh` also re-reads the consumption right away (respects quota.rs's anti-429 waits).
pub fn details(refresh: bool) -> Value {
    let mut limits_error = Value::Null;
    if refresh {
        if let Err(e) = crate::quota::refresh_from_server() { limits_error = json!(e); }
    }
    let (prog, _) = program();
    let st: Option<Value> = crate::new_command(&prog).args(["auth", "status", "--json"]).env("TAVOLO_INTERNO", "1").env("PATH", crate::hosted::path_login())
        .current_dir(home()).stdin(Stdio::null()).output().ok().and_then(|o| serde_json::from_slice(&o.stdout).ok());
    let connected = st.as_ref().map(|v| v["loggedIn"].as_bool().unwrap_or(false)).unwrap_or(false);
    let limits = crate::quota::valid_limits();
    let age_min = crate::quota::limits_read_at().map(|at| {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(at);
        ((now - at) / 60.0).max(0.0).round()
    });
    // only the fields needed, one by one: the CLI's json never passes whole
    let g = |k: &str| st.as_ref().map(|v| str_of(v, k).to_string()).unwrap_or_default();
    json!({
        "ok": true,
        "collegato": connected,
        "email": if connected { g("email") } else { String::new() },
        "organizzazione": if connected { g("orgName") } else { String::new() },
        "piano": if connected { [g("planTier"), g("subscriptionType")].into_iter().find(|x| !x.is_empty()).unwrap_or_default() } else { String::new() },
        "metodo": if connected { g("authMethod") } else { String::new() },
        "scade": if connected { g("loginExpiresAt") } else { String::new() },
        "modello": default_model(),
        "credenziale": credential_source(),
        "consumi": {"cinque_ore": limits["five_hour"], "settimana": limits["seven_day"], "eta_min": age_min, "errore": limits_error},
        "piattaforma": std::env::consts::OS,
    })
}

// ---------- "/..." commands that do not exist in -p ----------

const HELP_TEXT: &str = "I comandi con la barra (/) sono del terminale di Claude Code. In Juggl: cambia account da Impostazioni (Cambia account Claude); cambia modello dal tasto del modello nella testata della chat; per alleggerire una chat lunga scrivi /compact; per ripartire da zero apri una chat nuova; /cost e /model funzionano anche qui.";

/// Juggl's message for a command that cannot be used here: (text, action). Action "cambia-account" = shows the button.
pub fn message_for(name: &str) -> (String, &'static str) {
    match name {
        "login" | "logout" => (format!("/{name} non funziona dentro le chat di Juggl (non c'e' un terminale). Per cambiare account usa il tasto Cambia account Claude, in Impostazioni."), "cambia-account"),
        "help" | "?" => (HELP_TEXT.to_string(), ""),
        "clear" => ("/clear non c'e' in Juggl. Per ripartire da zero apri una chat nuova; per alleggerire questa scrivi /compact.".into(), ""),
        "exit" | "quit" => ("Per chiudere questa chat usa Archivia dal menu della chat: la conversazione resta e la riapri quando vuoi.".into(), ""),
        "model" => ("Cambia modello dal tasto del modello nella testata della chat.".into(), ""),
        // Nov 17: before it said "scrivi /compact", i.e. to redo what had just been refused
        "compact" => ("La versione di Claude Code installata su questo Mac non accetta /compact nelle chat di Juggl. Aggiornala (nel Terminale: claude update) e riapri la chat.".into(), ""),
        other => (format!("Il comando /{other} funziona solo nel terminale di Claude Code. Da Juggl puoi cambiare account (Impostazioni), cambiare modello dalla testata, scrivere /compact o aprire una chat nuova."), ""),
    }
}

/// Command name if the text is "/name" or "/name something" (not a path like /home/x/file).
pub fn command_name(text: &str) -> Option<String> {
    let t = text.trim_start();
    let r = t.strip_prefix('/')?;
    let first = r.split_whitespace().next().unwrap_or("");
    if first.is_empty() && !r.trim().starts_with('?') { return None; }
    let first = if r.trim().starts_with('?') { "?" } else { first };
    if first != "?" && !first.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':')) { return None; }
    Some(first.to_lowercase())
}

/// Before sending: the commands Juggl handles by itself, without sending them to the model (no tokens spent).
/// Those that work in -p (verified with Claude Code 2.1.322: /model, /compact, /cost) and unknown ones
/// (they may be skills) go through.
pub fn local_command(text: &str) -> Option<Value> {
    let name = command_name(text)?;
    if !matches!(name.as_str(), "login" | "logout" | "help" | "?" | "clear" | "exit" | "quit") { return None; }
    let (msg, action) = message_for(&name);
    Some(json!({"ok": true, "esito": "gestito", "msg": msg, "comando": format!("/{name}"), "azione": action}))
}

/// From the CLI's answer "/xyz isn't available in this environment." (a command we did not know).
pub fn from_event(d: &Value) -> Option<Value> {
    if str_of(d, "type") != "assistant" || str_of(&d["message"], "model") != "<synthetic>" { return None; }
    let text: String = d["message"]["content"].as_array()?.iter().map(|c| str_of(c, "text")).collect::<Vec<_>>().join(" ");
    let t = text.trim();
    if !t.starts_with('/') || !t.contains("isn't available in this environment") { return None; }
    let name = command_name(t)?;
    let (msg, action) = message_for(&name);
    Some(json!({"comando": format!("/{name}"), "msg": msg, "azione": action}))
}

#[tauri::command(async)]
pub fn accesso_avvia() -> Value { start() }
#[tauri::command(async)]
pub fn accesso_codice(codice: String) -> Value { self::code(&codice) }
#[tauri::command(async)]
pub fn accesso_esci(conferma: bool) -> Value { logout(conferma) }
#[tauri::command(async)]
pub fn accesso_annulla() -> Value { cancel() }
#[tauri::command(async)]
pub fn accesso_dettagli(aggiorna: bool) -> Value { details(aggiorna) }
#[tauri::command(async)]
pub fn accesso_stato() -> Value { status() }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as M;
    // the tests use the same global LOGIN: one at a time
    static ONE: M<()> = M::new(());

    #[test]
    fn link_from_output() {
        let l = "https://claude.ai/oauth/authorize?code=true&client_id=abc&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&state=x";
        assert_eq!(find_link(&format!("If the browser didn't open, visit: {l}")).as_deref(), Some(l));
        assert_eq!(find_link(&format!("\u{1b}[1mVisit\u{1b}[0m \u{1b}]8;;{l}\u{7}{l}\u{1b}]8;;\u{7}.")).as_deref(), Some(l));
        assert_eq!(find_link("Opening browser to sign in…"), None);
        assert_eq!(find_link("Docs: https://docs.claude.com/en/home"), None);
        assert!(is_manual(l));
        assert!(!is_manual("https://claude.ai/oauth/authorize?client_id=a&redirect_uri=http%3A%2F%2Flocalhost%3A5555%2Fcallback"));
    }

    #[test]
    fn slash_commands() {
        for c in ["/login", "/logout", "  /help", "/?", "/clear", "/exit"] { assert_eq!(local_command(c).unwrap()["esito"], "gestito", "{c}"); }
        assert_eq!(local_command("/login").unwrap()["azione"], "cambia-account");
        // they work in -p: they go to the model
        for c in ["/model", "/model sonnet", "/compact", "/cost", "/compact tieni i file"] { assert!(local_command(c).is_none(), "{c}"); }
        // unknown (maybe skills) and paths: they go through
        for c in ["/miaskill fai questo", "/home/utente/file.txt", "/tmp", "ciao /login", "", "/"] { assert!(local_command(c).is_none(), "{c}"); }
        // uppercase
        assert!(local_command("/LOGIN").is_some());
    }

    #[test]
    fn unavailable_answer() {
        let d = json!({"type": "assistant", "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "/doctor isn't available in this environment."}]}});
        let r = from_event(&d).unwrap();
        assert_eq!(r["comando"], "/doctor");
        assert!(str_of(&r, "msg").contains("/doctor"));
        let l = json!({"type": "assistant", "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "/login isn't available in this environment."}]}});
        assert_eq!(from_event(&l).unwrap()["azione"], "cambia-account");
        // a normal model answer talking about commands does not count
        let m = json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "content": [{"type": "text", "text": "/x isn't available in this environment."}]}});
        assert!(from_event(&m).is_none());
    }

    /// Script faking `claude auth login`: prints a link, reads the code, exits 0 if it is the right one.
    #[cfg(unix)]
    fn fake_claude(folder: &std::path::Path, right_code: &str) -> std::path::PathBuf {
        let p = folder.join("claude-finto");
        let text = format!("#!/bin/sh\nif [ \"$1\" = auth ] && [ \"$2\" = status ]; then echo '{{\"loggedIn\":true,\"email\":\"prova@esempio.it\",\"orgName\":\"Org prova\"}}'; exit 0; fi\n\
            echo 'Opening browser to sign in...'\necho 'If the browser did not open, visit: https://claude.ai/oauth/authorize?code=true&client_id=prova&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Fcb'\n\
            printf 'Paste code here > '\nread c\n[ \"$c\" = '{right_code}' ] && exit 0\necho 'codice sbagliato' >&2\nexit 1\n");
        std::fs::write(&p, text).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// Details with a fake `claude auth status`: invented email, never tokens, expected fields present.
    #[test]
    #[cfg(unix)]
    fn details_with_fake_claude() {
        let _g = ONE.lock().unwrap_or_else(|e| e.into_inner());   // JUGGL_CLAUDE_FINTO applies to the whole process: one test at a time
        let folder = std::env::temp_dir().join(format!("juggl-dett-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let p = folder.join("claude-finto");
        std::fs::write(&p, "#!/bin/sh\necho '{\"loggedIn\":true,\"email\":\"prova@example.com\",\"orgName\":\"Org prova\",\"planTier\":\"max_5x\",\"loginExpiresAt\":\"2026-12-10T19:58:37.997Z\",\"accessToken\":\"NON-DEVE-USCIRE\"}'\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("JUGGL_CLAUDE_FINTO", &p);
        let d = details(false);
        assert_eq!(d["collegato"], true);
        assert_eq!(d["email"], "prova@example.com");
        assert_eq!(d["piano"], "max_5x");
        assert_eq!(d["scade"], "2026-12-10T19:58:37.997Z");
        assert!(["portachiavi", "file", "nessuna"].contains(&d["credenziale"]["dove"].as_str().unwrap()));
        assert!(!d.to_string().contains("NON-DEVE-USCIRE"), "un token e' uscito nei dettagli");
        let _ = std::fs::remove_dir_all(&folder);
    }

    fn wait_closed() {
        for _ in 0..50 { if !lock_login().as_ref().map(|l| l.fase.in_progress()).unwrap_or(false) { return; } std::thread::sleep(Duration::from_millis(100)); }
    }

    #[cfg(unix)]
    #[test]
    fn flow_with_fake_claude() {
        let _g = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("accesso-prova-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("JUGGL_CLAUDE_FINTO", fake_claude(&dir, "CODICE-GIUSTO-123"));
        let _base_lock = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (a whole-process variable)
        std::env::set_var("JUGGL_BASE", &dir);
        // no login in progress
        wait_closed();
        let r = start();
        assert_eq!(r["ok"], true, "{r}");
        assert!(str_of(&r, "link").starts_with("https://claude.ai/oauth/authorize"));
        // one login at a time: the second returns the same
        let r2 = start();
        assert_eq!(r2["gia"], true);
        assert_eq!(r2["link"], r["link"]);
        // wrong code: error, no account changed
        let e = code("CODICE-SBAGLIATO-999");
        assert_eq!(e["ok"], false, "{e}");
        // the code does not end up in the displayed state nor in the messages
        let st = status().to_string();
        assert!(!st.contains("SBAGLIATO") && !e.to_string().contains("SBAGLIATO"));
        // new login, right code
        let _ = start();
        let g = code("CODICE-GIUSTO-123");
        assert_eq!(g["ok"], true, "{g}");
        assert_eq!(g["account"], "prova@esempio.it");
        assert!(!g.to_string().contains("GIUSTO-123"));
        assert_eq!(status()["login"]["fase"], "finito");
        // cancel
        let _ = start();
        assert_eq!(cancel()["ok"], true);
        assert_eq!(status()["login"]["fase"], "annullato");
        assert_eq!(code("x")["ok"], false);
        // empty code or with newline
        let _ = start();
        assert_eq!(code("  ")["ok"], false);
        assert_eq!(code("a\nb")["ok"], false);
        cancel();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
