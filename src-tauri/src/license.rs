// Juggl account and license (Nov 11). Single contract server / app / site: ~/tavolo/account/CONTRATTO.md
//
// Juggl is used only with an account the user has activated. The app signs in with email + 6-digit code and receives a device token
// (in the Keychain, service "Juggl account") and, if the account is active, a license signed by the server
// with Ed25519: `<payload>.<signature>` in base64url without "=". The license is not secret: it sits in cache in
// <Juggl folder>/account.json and is checked without network (signature, active state, expiry, account email).
// Renewal with POST /api/licenza at startup and every 6 hours; without network it stays valid until expiry; if the server says
// the account is not active, or 401 (unknown or revoked token), it blocks at once and the cached license is deleted.
//
// Without a valid license: the interface shows only "Entra in Juggl" (ui/sign-in.js), the iPhone bridge answers 403
// {"errore":"licenza"} (bridge.rs), new chats do not start (hosted.rs). Chats already at work do not stop.
//
// HTTPS calls go through the system curl (ureq here has no TLS), with the whole configuration (address,
// headers with the token, body) on stdin: never the token or the code on the command line, which ps shows.
//
// Tests (debug builds only): JUGGL_ACCOUNT_URL=http://127.0.0.1:<port> for a fake server (and the Keychain
// uses the service "Juggl account (prova)"), JUGGL_SENZA_LICENZA=1 skips the check, JUGGL_LICENZA_OGNI=<seconds>
// shortens the renewal round. In release builds these variables are ignored.
use crate::{base_dir, load_json, log_juggl, unix_now, str_of};
use base64::Engine;
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Server's Ed25519 public key (standard base64, 32 raw bytes). The private one lives only on the server and on the owner's Mac.
pub const PUBLIC_KEY: &str = "uwdZsRXq7szgKZ8qCk8xCkzhFC7Z05a/b8ot4Kz5KVU=";
const SERVICE: &str = "Juggl account";
const ENTRY: &str = "dispositivo";
const ADDRESS: &str = "https://juggl.dev";
/// Renewal succeeded: the next in 6 hours. Network absent or server down: retry in 15 minutes.
const EVERY: f64 = 6.0 * 3600.0;
const RETRY: f64 = 15.0 * 60.0;
/// Account pending: check every 10 minutes, so when the owner activates it Juggl unlocks by itself.
const PENDING_EVERY: f64 = 10.0 * 60.0;

/// Called when the license goes from invalid to valid (main.rs: resumes the chats and the welcome).
pub static ON_UNLOCK: OnceLock<fn(&AppHandle)> = OnceLock::new();
static APP: OnceLock<AppHandle> = OnceLock::new();
/// Content of account.json in memory (None = not read yet).
static DATA: Mutex<Option<Value>> = Mutex::new(None);
/// Fingerprint of the last state announced to the interface (to send "licenza" only when it changes).
static LAST: Mutex<String> = Mutex::new(String::new());

fn lock_mutex<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

// ---------- test settings (debug only) ----------

fn var_debug(_name: &str) -> Option<String> {
    #[cfg(debug_assertions)]
    { return std::env::var(_name).ok().filter(|x| !x.trim().is_empty()); }
    #[cfg(not(debug_assertions))]
    { None }
}

/// Account service address: https://juggl.dev (JUGGL_ACCOUNT_URL only in debug builds).
pub fn service_url() -> String {
    var_debug("JUGGL_ACCOUNT_URL").map(|u| u.trim_end_matches('/').to_string()).unwrap_or_else(|| ADDRESS.to_string())
}

fn service_name() -> &'static str { if var_debug("JUGGL_ACCOUNT_URL").is_some() { "Juggl account (prova)" } else { SERVICE } }

/// Check skipped: only in debug builds with JUGGL_SENZA_LICENZA=1.
pub fn is_skipped() -> bool { var_debug("JUGGL_SENZA_LICENZA").as_deref() == Some("1") }

fn every_secs() -> f64 { var_debug("JUGGL_LICENZA_OGNI").and_then(|x| x.parse::<f64>().ok()).filter(|x| *x >= 1.0).unwrap_or(EVERY) }

// ---------- license: signature verification ----------

fn b64url(t: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(t.trim().trim_end_matches('=')).ok()
}

/// Ed25519 signature over the exact bytes of the payload. Returns the payload read as JSON.
pub fn verify_signature(license: &str, key_bytes: &[u8]) -> Result<Value, String> {
    let (p, f) = license.trim().split_once('.').ok_or("licenza senza firma")?;
    let payload = b64url(p).ok_or("payload non leggibile")?;
    let signature = b64url(f).ok_or("firma non leggibile")?;
    if signature.len() != 64 { return Err("firma della lunghezza sbagliata".into()); }
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key_bytes)
        .verify(&payload, &signature).map_err(|_| "firma non valida".to_string())?;
    serde_json::from_slice(&payload).map_err(|_| "payload non e' JSON".to_string())
}

fn pubkey_bytes() -> Vec<u8> { base64::engine::general_purpose::STANDARD.decode(PUBLIC_KEY).unwrap_or_default() }

fn same_email(a: &str, b: &str) -> bool { !a.trim().is_empty() && a.trim().eq_ignore_ascii_case(b.trim()) }

/// License good for this account now: signature, active state, expiry in the future, same email.
/// Err with the reason ("firma", "stato", "scaduta", "email").
pub fn check(license: &str, email: &str, now: f64, key_bytes: &[u8]) -> Result<Value, &'static str> {
    let p = verify_signature(license, key_bytes).map_err(|_| "firma")?;
    if p.get("v").and_then(|x| x.as_i64()) != Some(1) { return Err("firma"); }
    if str_of(&p, "stato") != "attivo" { return Err("stato"); }
    if p.get("exp").and_then(|x| x.as_f64()).unwrap_or(0.0) <= now { return Err("scaduta"); }
    if !same_email(str_of(&p, "email"), email) { return Err("email"); }
    Ok(p)
}

// ---------- cached data (account.json) ----------

fn account_file() -> std::path::PathBuf { base_dir().join("account.json") }

fn cached_data() -> Value {
    let mut d = lock_mutex(&DATA);
    if d.is_none() { *d = Some(load_json(&account_file()).unwrap_or(json!({}))); }
    d.clone().unwrap_or(json!({}))
}

fn save(v: Value) {
    if let Err(e) = crate::storage::write_json(&account_file(), &v) { log_juggl(&format!("account: salvataggio non riuscito ({e})")); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(account_file(), std::fs::Permissions::from_mode(0o600)); }
    *lock_mutex(&DATA) = Some(v);
}

/// Read, change, write under a single lock (background renewal and sign-in from the interface at the same time).
fn change(f: impl FnOnce(&mut Value)) {
    static WRITING: Mutex<()> = Mutex::new(());
    let _l = lock_mutex(&WRITING);
    let mut v = cached_data(); f(&mut v); save(v);
}

/// License valid now (or check skipped in debug).
pub fn is_valid() -> bool {
    if is_skipped() { return true; }
    let d = cached_data();
    check(str_of(&d, "licenza"), str_of(&d, "email"), unix_now(), &pubkey_bytes()).is_ok()
}

// ---------- permissions (Nov 12): administrator account ----------

/// Licenses signed from here on (2026-11-12 02:03, server publication) have "admin": true only for the administrator
/// account. Before, the field did not exist: an older license says nothing.
pub const ADMIN_SINCE: f64 = 1_794_445_400.0;

/// From the payload of a valid license: Some(true) administrator, Some(false) not, None unknown (old license).
pub fn admin_from(p: &Value) -> Option<bool> {
    if p.get("admin").and_then(|x| x.as_bool()) == Some(true) { return Some(true); }
    if p.get("iat").and_then(|x| x.as_f64()).unwrap_or(0.0) >= ADMIN_SINCE { return Some(false); }
    None
}

/// Tests only: content of account.json in memory (permissions.rs, update test).
#[cfg(test)]
pub fn test_data(v: Value) { *lock_mutex(&DATA) = Some(v); }

/// Is the signed-in account the administrator? Only from the valid license in cache (permissions.rs, first start).
pub fn admin() -> Option<bool> {
    let d = cached_data();
    check(str_of(&d, "licenza"), str_of(&d, "email"), unix_now(), &pubkey_bytes()).ok().and_then(|p| admin_from(&p))
}

/// State for the interface and for Settings.
pub fn status_json() -> Value {
    let d = cached_data();
    let email = str_of(&d, "email").to_string();
    let lic = str_of(&d, "licenza");
    let outcome = if lic.is_empty() { Err("nessuna") } else { check(lic, &email, unix_now(), &pubkey_bytes()) };
    let p = verify_signature(lic, &pubkey_bytes()).unwrap_or(json!({}));
    let state = str_of(&d, "stato").to_string();
    let signed_in = !email.is_empty() && token().is_some();
    // reason for the block, in the order that matters to whoever looks at the screen
    let reason = match &outcome {
        Ok(_) => "",
        Err(_) if !signed_in => "esci",
        Err(_) if state == "in_attesa" || state == "sospeso" || state == "rifiutato" => state.as_str(),
        Err(m) => *m,
    };
    json!({
        "valida": is_skipped() || outcome.is_ok(), "saltato": is_skipped(), "entrato": signed_in,
        "email": email, "email_ultima": str_of(&d, "email_ultima"), "stato": state, "motivo": reason,
        "scadenza": p.get("exp").cloned().unwrap_or(Value::Null), "emessa": p.get("iat").cloned().unwrap_or(Value::Null),
        "controllo": d.get("controllo").cloned().unwrap_or(Value::Null), "errore": str_of(&d, "errore"),
    })
}

/// Notifies the interface if the state changed; on the switch to valid it restarts what was stopped.
fn announce() {
    let st = status_json();
    let fingerprint = format!("{}|{}|{}|{}|{}|{}", st["valida"], st["stato"], st["motivo"], st["scadenza"], st["email"], st["errore"]);
    let previous = std::mem::replace(&mut *lock_mutex(&LAST), fingerprint.clone());
    if previous == fingerprint { return; }
    let was_valid = previous.starts_with("true|");
    let Some(app) = APP.get() else { return };
    let _ = app.emit("licenza", st.clone());
    if !was_valid && st["valida"] == json!(true) && !previous.is_empty() {
        log_juggl("account: licenza valida, Juggl sbloccato");
        if let Some(f) = ON_UNLOCK.get() { let (a, f) = (app.clone(), *f); std::thread::spawn(move || f(&a)); }
    }
    if was_valid && st["valida"] != json!(true) { log_juggl(&format!("account: Juggl bloccato ({})", st["motivo"])); }
}

// ---------- Keychain (security command, with the secret, if any, only on stdin) ----------

fn is_clean_base64url(t: &str) -> bool { !t.is_empty() && t.len() <= 200 && t.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') }

/// Device token from the Keychain.
pub fn token() -> Option<String> {
    #[cfg(target_os = "linux")] { let t = linux::read(service_name())?; return is_clean_base64url(&t).then_some(t); }
    // Windows: Credential Manager (sys.rs), entry "Juggl/<service>/dispositivo"
    #[cfg(windows)] { let t = crate::sys::vault_read(&format!("{}/{ENTRY}", service_name()))?; return is_clean_base64url(&t).then_some(t); }
    #[allow(unreachable_code)]
    let o = crate::new_command("/usr/bin/security")
        .args(["find-generic-password", "-s", service_name(), "-a", ENTRY, "-w"])
        .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    if !o.status.success() { return None; }
    let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
    is_clean_base64url(&t).then_some(t)
}

fn save_token(t: &str) -> Result<(), String> {
    // the token ends up inside a `security -i` command: I accept only base64url characters (no quotes or spaces)
    if !is_clean_base64url(t) { return Err("token del server non valido".into()); }
    #[cfg(target_os = "linux")]
    { return if linux::save(service_name(), t) && token().as_deref() == Some(t) { Ok(()) } else { Err("token non salvato".into()) }; }
    #[cfg(windows)]
    { return if crate::sys::vault_save(&format!("{}/{ENTRY}", service_name()), t) && token().as_deref() == Some(t) { Ok(()) } else { Err("Gestione credenziali: token non salvato".into()) }; }
    #[allow(unreachable_code)]
    use std::io::Write;
    let mut c = crate::new_command("/usr/bin/security").arg("-i")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped())
        .spawn().map_err(|e| format!("Portachiavi non disponibile ({e})"))?;
    if let Some(mut i) = c.stdin.take() {
        let _ = writeln!(i, "add-generic-password -U -s \"{0}\" -l \"{0}\" -a {ENTRY} -w {t}", service_name());
    }
    let o = c.wait_with_output().map_err(|e| e.to_string())?;
    if token().as_deref() != Some(t) {
        return Err(format!("Portachiavi: token non salvato ({})", String::from_utf8_lossy(&o.stderr).trim().chars().take(80).collect::<String>()));
    }
    Ok(())
}

fn remove_token() {
    #[cfg(target_os = "linux")] { linux::remove(service_name()); return; }
    #[cfg(windows)] { crate::sys::vault_remove(&format!("{}/{ENTRY}", service_name())); return; }
    #[allow(unreachable_code)]
    let _ = crate::new_command("/usr/bin/security").args(["delete-generic-password", "-s", service_name(), "-a", ENTRY])
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
}

// ---------- HTTPS with curl ----------

/// Quoted string for curl's configuration file (stdin): \ and " escaped, no newlines.
fn quote(t: &str) -> String {
    let mut o = String::from("\"");
    for c in t.chars() { match c { '\\' => o.push_str("\\\\"), '"' => o.push_str("\\\""), '\n' => o.push_str("\\n"), '\r' => o.push_str("\\r"), '\t' => o.push_str("\\t"), c => o.push(c) } }
    o.push('"');
    o
}

/// POST JSON to the account service. Ok((HTTP code, JSON answer)); Err if there is no network or curl does not start.
pub(crate) fn call(path: &str, body: &Value, token: Option<&str>) -> Result<(u16, Value), String> {
    use std::io::Write;
    let mut conf = format!("url = {}\nrequest = \"POST\"\nheader = \"Content-Type: application/json\"\nheader = \"Accept: application/json\"\n",
        quote(&format!("{}{path}", service_url())));
    if let Some(t) = token { conf.push_str(&format!("header = {}\n", quote(&format!("Authorization: Bearer {t}")))); }
    conf.push_str(&format!("data-binary = {}\nwrite-out = \"\\n%{{http_code}}\"\n", quote(&body.to_string())));
    let mut c = curl_command()
        .args(["-sS", "--connect-timeout", "8", "--max-time", "20", "-K", "-"])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .spawn().map_err(|e| format!("curl non parte ({e})"))?;
    if let Some(mut i) = c.stdin.take() { let _ = i.write_all(conf.as_bytes()); }
    let o = c.wait_with_output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("juggl.dev non risponde ({})", String::from_utf8_lossy(&o.stderr).trim().chars().take(80).collect::<String>()));
    }
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    let (body, codice) = out.rsplit_once('\n').unwrap_or(("", out.as_str()));
    let codice: u16 = codice.trim().parse().unwrap_or(0);
    Ok((codice, serde_json::from_str(body.trim()).unwrap_or(json!({}))))
}

/// System curl: on the Mac /usr/bin/curl, on Windows the one in System32 (Windows 10 and 11), without a console window.
pub(crate) fn curl_command() -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let mut c = crate::new_command(format!(r"{sys}\System32\curl.exe"));
        c.creation_flags(crate::sys::CREATE_NO_WINDOW);
        return c;
    }
    #[allow(unreachable_code)]
    crate::new_command("/usr/bin/curl")
}

/// Platform for the accounts server.
fn platform() -> &'static str { if cfg!(windows) { "windows" } else if cfg!(target_os = "linux") { "linux" } else { "mac" } }

/// Linux: token in the desktop keyring (Secret Service, secret-tool command of libsecret-tools). If secret-tool
/// is not there (servers, desktops without a keyring) the token goes in a file readable only by the user, as gh and docker do.
#[cfg(target_os = "linux")]
mod linux {
    use std::io::Write;
    use std::process::{Command, Stdio};
    fn token_file() -> std::path::PathBuf { crate::base_dir().join(".token-dispositivo") }
    fn has_secret_tool() -> bool { crate::new_command("secret-tool").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok() }
    pub fn read(serv: &str) -> Option<String> {
        if has_secret_tool() {
            if let Ok(o) = crate::new_command("secret-tool").args(["lookup", "service", serv, "account", super::ENTRY]).stdin(Stdio::null()).stderr(Stdio::null()).output() {
                let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if o.status.success() && !t.is_empty() { return Some(t); }
            }
        }
        std::fs::read_to_string(token_file()).ok().map(|x| x.trim().to_string()).filter(|x| !x.is_empty())
    }
    pub fn save(serv: &str, t: &str) -> bool {
        if has_secret_tool() {
            // the secret goes through stdin, never on the command line
            if let Ok(mut c) = crate::new_command("secret-tool").args(["store", "--label", serv, "service", serv, "account", super::ENTRY])
                .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                if let Some(mut i) = c.stdin.take() { let _ = i.write_all(t.as_bytes()); }
                if c.wait().map(|s| s.success()).unwrap_or(false) { let _ = std::fs::remove_file(token_file()); return true; }
            }
        }
        use std::os::unix::fs::OpenOptionsExt;
        let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(token_file()) else { return false };
        f.write_all(t.as_bytes()).is_ok()
    }
    pub fn remove(serv: &str) {
        if has_secret_tool() { let _ = crate::new_command("secret-tool").args(["clear", "service", serv, "account", super::ENTRY]).status(); }
        let _ = std::fs::remove_file(token_file());
    }
}

/// Device name: the computer's real one; if it cannot be read, "Mac" on the Mac and "Computer" elsewhere.
pub(crate) fn device_name() -> String {
    #[cfg(target_os = "linux")] { return std::fs::read_to_string("/etc/hostname").ok().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).or_else(|| std::env::var("HOSTNAME").ok().filter(|x| !x.is_empty())).unwrap_or_else(|| "Computer".into()); }
    #[cfg(windows)] { return std::env::var("COMPUTERNAME").ok().filter(|x| !x.is_empty()).unwrap_or_else(|| "Computer".into()); }
    #[allow(unreachable_code)]
    crate::new_command("/usr/sbin/scutil").args(["--get", "ComputerName"]).output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|x| !x.is_empty()).unwrap_or_else(|| "Mac".into())
}

fn version() -> &'static str { env!("CARGO_PKG_VERSION") }

// ---------- actions ----------

/// Renewal from the server. Returns the new state.
pub fn renew() -> Value {
    let Some(tok) = token() else { announce(); return status_json() };
    let outcome = call("/api/licenza", &json!({"versione": version(), "piattaforma": platform()}), Some(&tok));
    let now = unix_now();
    match outcome {
        Err(e) => {
            log_juggl(&format!("account: rinnovo non riuscito ({e})"));
            change(|d| { d["errore"] = json!("rete"); d["tentativo"] = json!(now); });
        }
        Ok((401, _)) => {
            // unknown or revoked token (disconnected device): block and sign in again with email and code
            log_juggl("account: token rifiutato dal server, si rientra");
            remove_token();
            change(|d| {
                let e = str_of(d, "email").to_string();
                *d = json!({"email_ultima": e, "errore": "scollegato", "tentativo": now});
            });
        }
        Ok((200, v)) => {
            let state = str_of(&v, "stato").to_string();
            change(|d| {
                d["tentativo"] = json!(now); d["controllo"] = json!(now); d["errore"] = json!("");
                if str_of(d, "email").is_empty() && !str_of(&v, "email").is_empty() { d["email"] = json!(str_of(&v, "email")); }
                d["stato"] = json!(state);
                if state == "attivo" {
                    let lic = str_of(&v, "licenza");
                    match check(lic, str_of(d, "email"), now, &pubkey_bytes()) {
                        Ok(_) => d["licenza"] = json!(lic),
                        // new license that does not check out (signature, email): the previous one stays until its expiry
                        Err(m) => { log_juggl(&format!("account: licenza dal server non valida ({m})")); d["errore"] = json!(format!("licenza {m}")); }
                    }
                } else if let Some(o) = d.as_object_mut() {
                    // pending, suspended, rejected: block at once, the cached license no longer counts
                    o.remove("licenza");
                }
            });
        }
        Ok((c, v)) => {
            log_juggl(&format!("account: rinnovo, risposta {c} {}", v.to_string().chars().take(80).collect::<String>()));
            change(|d| { d["errore"] = json!("server"); d["tentativo"] = json!(now); });
        }
    }
    announce();
    status_json()
}

fn email_ok(e: &str) -> bool {
    let e = e.trim();
    e.len() <= 200 && e.split_once('@').map(|(a, b)| !a.is_empty() && b.contains('.') && !b.starts_with('.') && !b.ends_with('.')).unwrap_or(false)
        && !e.chars().any(|c| c.is_whitespace())
}

/// Asks for the 6-digit code by email.
pub fn request_code(email: &str) -> Value {
    let email = email.trim().to_lowercase();
    if !email_ok(&email) { return json!({"ok": false, "msg": "Controlla l'indirizzo email."}); }
    match call("/api/account/codice", &json!({"email": email, "lingua": "it"}), None) {
        Ok((200, v)) if v["ok"] == json!(true) => { change(|d| d["email_ultima"] = json!(email)); json!({"ok": true}) }
        Ok((429, _)) => json!({"ok": false, "msg": "Troppe richieste: aspetta qualche minuto e riprova."}),
        Ok((c, _)) => json!({"ok": false, "msg": format!("Il server non ha mandato il codice (errore {c}). Riprova fra poco.")}),
        Err(_) => json!({"ok": false, "msg": "Juggl non riesce a raggiungere juggl.dev. Controlla la connessione."}),
    }
}

/// Signs in with email and code: token in the Keychain, state and license (if the account is active) in cache.
pub fn sign_in(email: &str, codice: &str) -> Value {
    let email = email.trim().to_lowercase();
    let codice: String = codice.chars().filter(|c| c.is_ascii_digit()).collect();
    if !email_ok(&email) { return json!({"ok": false, "msg": "Controlla l'indirizzo email."}); }
    if codice.len() != 6 { return json!({"ok": false, "msg": "Il codice ha 6 cifre."}); }
    let body = json!({"email": email, "codice": codice, "dispositivo": {"nome": device_name(), "piattaforma": platform(), "versione": version()}});
    let (c, v) = match call("/api/account/entra", &body, None) {
        Ok(x) => x,
        Err(_) => return json!({"ok": false, "msg": "Juggl non riesce a raggiungere juggl.dev. Controlla la connessione."}),
    };
    if c != 200 || v["ok"] != json!(true) {
        let msg = match str_of(&v, "errore") {
            "codice" => "Codice sbagliato. Controlla l'email e riprova.",
            "scaduto" => "Codice scaduto: chiedine uno nuovo.",
            "troppi" => "Troppi tentativi: chiedi un codice nuovo fra qualche minuto.",
            _ => "Non sono riuscito a entrare. Riprova fra poco.",
        };
        return json!({"ok": false, "msg": msg, "errore": str_of(&v, "errore")});
    }
    let tok = str_of(&v, "token");
    if let Err(e) = save_token(tok) { log_juggl(&format!("account: {e}")); return json!({"ok": false, "msg": "Non riesco a salvare l'accesso nel Portachiavi."}); }
    let state = str_of(&v, "stato").to_string();
    let now = unix_now();
    let mut d = json!({"email": email, "email_ultima": email, "stato": state, "controllo": now, "tentativo": now, "errore": ""});
    let lic = str_of(&v, "licenza");
    if !lic.is_empty() {
        match check(lic, &email, now, &pubkey_bytes()) {
            Ok(_) => d["licenza"] = json!(lic),
            Err(m) => { log_juggl(&format!("account: licenza all'entrata non valida ({m})")); d["errore"] = json!(format!("licenza {m}")); }
        }
    }
    change(|x| *x = d);
    log_juggl(&format!("account: entrato, stato {state}"));
    announce();
    let mut st = status_json();
    st["ok"] = json!(true);
    st
}

/// Signs out: token removed from the Keychain, license deleted. Only the last email remains, to fill the field.
pub fn sign_out() -> Value {
    remove_token();
    change(|d| {
        let e = if str_of(d, "email").is_empty() { str_of(d, "email_ultima").to_string() } else { str_of(d, "email").to_string() };
        *d = json!({"email_ultima": e});
    });
    log_juggl("account: uscito");
    announce();
    status_json()
}

/// Background round: renewal at startup and every 6 hours (15 minutes if the network is missing, 10 if the account is pending);
/// every minute it rechecks the cached license, so expiry without network blocks even without renewal.
pub fn start(app: AppHandle) {
    let _ = APP.set(app);
    announce();
    std::thread::spawn(|| {
        let mut next_at = 0.0;
        loop {
            if unix_now() >= next_at {
                let st = renew();
                let d = cached_data();
                next_at = unix_now() + if token().is_none() { 60.0 }
                    else if !str_of(&d, "errore").is_empty() { RETRY.min(every_secs()) }
                    else if str_of(&st, "stato") == "in_attesa" { PENDING_EVERY.min(every_secs()) }
                    else { every_secs() };
            } else {
                announce();
            }
            std::thread::sleep(Duration::from_secs(if var_debug("JUGGL_LICENZA_OGNI").is_some() { 1 } else { 60 }));
        }
    });
}

// ---------- Tauri commands ----------

#[tauri::command]
pub fn licenza_stato() -> Value { status_json() }

#[tauri::command(async)]
pub fn licenza_rinnova() -> Value { renew() }

#[tauri::command(async)]
pub fn account_codice(email: String) -> Value { request_code(&email) }

#[tauri::command(async)]
pub fn account_entra(email: String, codice: String) -> Value { sign_in(&email, &codice) }

#[tauri::command(async)]
pub fn account_esci() -> Value { sign_out() }

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn keypair() -> Ed25519KeyPair { Ed25519KeyPair::from_seed_unchecked(&[7u8; 32]).unwrap() }

    fn signature(k: &Ed25519KeyPair, payload: &Value) -> String {
        let p = payload.to_string();
        let e = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}", e.encode(p.as_bytes()), e.encode(k.sign(p.as_bytes()).as_ref()))
    }

    fn payload(state: &str, exp: f64) -> Value {
        json!({"v": 1, "acc": "a1", "email": "Prova@Esempio.it", "disp": "d1", "stato": state, "iat": exp - 7.0 * 86400.0, "exp": exp})
    }

    #[test]
    fn good_license() {
        let k = keypair();
        let now = 1_800_000_000.0;
        let l = signature(&k, &payload("attivo", now + 3600.0));
        assert!(check(&l, "prova@esempio.it", now, k.public_key().as_ref()).is_ok());
    }

    #[test]
    fn tampered_signature_rejected() {
        let k = keypair();
        let now = 1_800_000_000.0;
        let l = signature(&k, &payload("attivo", now + 3600.0));
        let pk = k.public_key().as_ref().to_vec();
        // payload changed (expiry extended) with the old signature
        let (_, f) = l.split_once('.').unwrap();
        let other = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload("attivo", now + 999_999.0).to_string());
        assert_eq!(check(&format!("{other}.{f}"), "prova@esempio.it", now, &pk), Err("firma"));
        // one bit of the signature changed
        let mut b = b64url(f).unwrap(); b[10] ^= 1;
        let (p, _) = l.split_once('.').unwrap();
        let l2 = format!("{p}.{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&b));
        assert_eq!(check(&l2, "prova@esempio.it", now, &pk), Err("firma"));
        // signed by another key
        let another_key = Ed25519KeyPair::from_seed_unchecked(&[9u8; 32]).unwrap();
        assert_eq!(check(&signature(&another_key, &payload("attivo", now + 3600.0)), "prova@esempio.it", now, &pk), Err("firma"));
        // the contract's real key does not accept licenses signed by others
        assert_eq!(check(&l, "prova@esempio.it", now, &pubkey_bytes()), Err("firma"));
        // garbage
        assert!(check("niente", "prova@esempio.it", now, &pk).is_err());
        assert!(check("", "prova@esempio.it", now, &pk).is_err());
    }

    #[test]
    fn expired_state_email() {
        let k = keypair();
        let pk = k.public_key().as_ref().to_vec();
        let now = 1_800_000_000.0;
        assert_eq!(check(&signature(&k, &payload("attivo", now - 1.0)), "prova@esempio.it", now, &pk), Err("scaduta"));
        assert_eq!(check(&signature(&k, &payload("sospeso", now + 3600.0)), "prova@esempio.it", now, &pk), Err("stato"));
        assert_eq!(check(&signature(&k, &payload("attivo", now + 3600.0)), "altro@esempio.it", now, &pk), Err("email"));
        assert_eq!(check(&signature(&k, &payload("attivo", now + 3600.0)), "", now, &pk), Err("email"));
    }

    #[test]
    fn contract_key() {
        assert_eq!(pubkey_bytes().len(), 32);
        // if the Mac has the contract's private key, the public one must match
        let p = crate::home().join(".config/juggl/account/licenza-ed25519.key");
        if let Ok(t) = std::fs::read_to_string(p) {
            let seed = base64::engine::general_purpose::STANDARD.decode(t.trim()).unwrap();
            let k = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
            assert_eq!(k.public_key().as_ref(), pubkey_bytes().as_slice());
            let now = unix_now();
            let l = signature(&k, &payload("attivo", now + 3600.0));
            assert!(check(&l, "prova@esempio.it", now, &pubkey_bytes()).is_ok());
        }
    }

    /// Full round with the fake server (~/tavolo/account/prova-server-finto.py), the real curl and Keychain
    /// (service "Juggl account (prova)") and the real bridge on a test port. Temporary Juggl folder.
    ///   cargo test full_round_fake_server -- --ignored --nocapture
    /// Sign in with the account on THIS Mac before installing the first version with accounts (Nov 11), so Juggl does not
    /// block: JUGGL_ENTRA_EMAIL=.. cargo test sign_in_on_this_mac -- --ignored (asks for the code), then with JUGGL_ENTRA_CODICE=.. (signs in).
    #[test]
    #[ignore]
    fn sign_in_on_this_mac() {
        let Ok(email) = std::env::var("JUGGL_ENTRA_EMAIL") else { return };
        match std::env::var("JUGGL_ENTRA_CODICE") {
            Err(_) => println!("codice: {}", request_code(&email)),
            Ok(c) => { let r = sign_in(&email, &c); println!("entra: ok={} stato={} valida={}", r["ok"], r["stato"], r["valida"]); }
        }
    }


    #[test]
    #[ignore]
    fn full_round_fake_server() {
        let srv_port = 47990;
        let dir = std::env::temp_dir().join(format!("juggl-licenza-prova-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _base_lock = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (a whole-process variable)
        std::env::set_var("JUGGL_BASE", &dir);
        std::env::set_var("JUGGL_ACCOUNT_URL", format!("http://127.0.0.1:{srv_port}"));
        std::env::set_var("JUGGL_PORTA", "47931");
        let script = crate::home().join("tavolo/account/prova-server-finto.py");
        let start_srv = || {
            let c = crate::new_command("/usr/bin/python3").arg(&script).arg(srv_port.to_string())
                .stdout(std::process::Stdio::null()).spawn().unwrap();
            for _ in 0..50 { if std::net::TcpStream::connect(("127.0.0.1", srv_port)).is_ok() { break; } std::thread::sleep(Duration::from_millis(100)); }
            c
        };
        let poke = |path: &str, body: Value| {
            let o = crate::new_command("/usr/bin/curl").args(["-s", "-X", "POST", "-d", &body.to_string(), &format!("http://127.0.0.1:{srv_port}{path}")]).output().unwrap();
            assert!(o.status.success());
        };
        let bridge_call = |path: &str| -> (u16, String) {
            let k = crate::bridge::key();
            let o = crate::new_command("/usr/bin/curl").args(["-s", "-H", &format!("Authorization: Bearer {k}"), "-w", "\n%{http_code}", &format!("http://127.0.0.1:47931{path}")]).output().unwrap();
            let t = String::from_utf8_lossy(&o.stdout).to_string();
            let (b, c) = t.rsplit_once('\n').unwrap();
            (c.trim().parse().unwrap(), b.to_string())
        };
        let mut srv = start_srv();
        remove_token();
        let email = "prova@esempio.it";

        // 1. without account: blocked, the bridge answers 403 licenza except for connecting
        assert!(!is_valid());
        assert_eq!(status_json()["motivo"], json!("esci"));
        crate::bridge::start();
        for _ in 0..50 { if std::net::TcpStream::connect(("127.0.0.1", 47931)).is_ok() { break; } std::thread::sleep(Duration::from_millis(100)); }
        let (c, b) = bridge_call("/api/tavolo");
        assert_eq!((c, b.as_str()), (403, r#"{"errore":"licenza"}"#));
        assert_eq!(bridge_call("/api/licenza").0, 200);
        println!("1. senza account: bloccato, ponte 403 {b}");

        // 2. code -> sign in: new account pending, still blocked
        assert_eq!(request_code(email)["ok"], json!(true));
        let r = sign_in(email, "000000");
        assert_eq!(r["ok"], json!(false));
        println!("2a. codice sbagliato: {}", r["msg"]);
        let r = sign_in(email, "123456");
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(r["stato"], json!("in_attesa"));
        assert!(token().is_some());
        assert!(!is_valid());
        assert_eq!(status_json()["motivo"], json!("in_attesa"));
        assert_eq!(bridge_call("/api/tavolo").0, 403);
        println!("2b. entrato: in attesa, ancora bloccato");

        // 3. the owner activates: at renewal the license is valid, bridge open
        poke("/prova/stato", json!({"email": email, "stato": "attivo"}));
        let st = renew();
        assert_eq!(st["valida"], json!(true), "{st}");
        assert!(is_valid());
        assert_eq!(bridge_call("/api/tavolo").0, 200);
        println!("3. attivato: valida fino a {}, ponte 200", st["scadenza"]);

        // 4. suspension: block at renewal, cached license deleted
        poke("/prova/stato", json!({"email": email, "stato": "sospeso"}));
        let st = renew();
        assert_eq!(st["valida"], json!(false));
        assert_eq!(st["motivo"], json!("sospeso"));
        assert!(load_json(&account_file()).unwrap().get("licenza").is_none());
        assert_eq!(bridge_call("/api/tavolo").0, 403);
        println!("4. sospeso: bloccato al rinnovo, licenza cancellata");

        // 5. reactivated with a 3-second license, then server off: without network valid until exp, then block
        poke("/prova/stato", json!({"email": email, "stato": "attivo"}));
        poke("/prova/durata", json!({"secondi": 3}));
        assert_eq!(renew()["valida"], json!(true));
        let _ = srv.kill(); let _ = srv.wait();
        let st = renew();
        assert_eq!(st["errore"], json!("rete"));
        assert_eq!(st["valida"], json!(true), "senza rete resta valida fino a exp");
        std::thread::sleep(Duration::from_millis(3500));
        let st = status_json();
        assert_eq!(st["valida"], json!(false));
        assert_eq!(st["motivo"], json!("scaduta"));
        assert_eq!(bridge_call("/api/tavolo").0, 403);
        println!("5. senza rete: valida fino alla scadenza, poi bloccata ({})", st["motivo"]);

        // 6. server restarted without this device: 401, token removed, sign in again
        srv = start_srv();
        let st = renew();
        assert_eq!(st["motivo"], json!("esci"));
        assert!(token().is_none());
        assert_eq!(st["email_ultima"], json!(email));
        println!("6. 401: token tolto, schermata di entrata con l'email gia' scritta");

        // 7. account rejected
        assert_eq!(request_code(email)["ok"], json!(true));
        assert_eq!(sign_in(email, "123456")["ok"], json!(true));
        poke("/prova/stato", json!({"email": email, "stato": "rifiutato"}));
        assert_eq!(renew()["motivo"], json!("rifiutato"));
        println!("7. rifiutato: bloccato");

        // 8. active at sign-in (license at once), then Sign out
        poke("/prova/durata", json!({"secondi": 7 * 86400}));
        poke("/prova/stato", json!({"email": email, "stato": "attivo"}));
        let r = sign_in(email, "123456");
        assert_eq!(r["valida"], json!(true), "{r}");
        let st = sign_out();
        assert_eq!(st["valida"], json!(false));
        assert!(token().is_none());
        println!("8. attivo subito all'entrata, poi Esci: bloccato e token tolto");

        // the token never goes through the command line: the fake server receives the right device and headers
        let o = crate::new_command("/usr/bin/curl").args(["-s", &format!("http://127.0.0.1:{srv_port}/prova/registro")]).output().unwrap();
        let reg: Value = serde_json::from_slice(&o.stdout).unwrap();
        let d = reg.as_array().unwrap().iter().find(|x| x["dispositivo"].is_object()).cloned().unwrap();
        assert_eq!(d["dispositivo"]["piattaforma"], json!("mac"));
        assert_eq!(d["dispositivo"]["versione"], json!(version()));
        println!("dispositivo mandato: {}", d["dispositivo"]);

        let _ = srv.kill();
        remove_token();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn curl_config() {
        assert_eq!(quote(r#"{"a":"b\c"}"#), r#""{\"a\":\"b\\c\"}""#);
        assert!(is_clean_base64url("abc-_09XY"));
        assert!(!is_clean_base64url("abc def"));
        assert!(!is_clean_base64url("a\"b"));
        assert!(!is_clean_base64url(""));
        assert!(email_ok("nome@esempio.it"));
        assert!(!email_ok("nome@esempio"));
        assert!(!email_ok("me @x.it"));
    }
}

// permissions (Nov 12): "admin" field of the license
#[cfg(test)]
mod admin_tests {
    use super::*;

    #[test]
    fn admin_from_payload() {
        assert_eq!(admin_from(&json!({"v": 1, "admin": true, "iat": 1.0})), Some(true));
        assert_eq!(admin_from(&json!({"v": 1, "iat": ADMIN_SINCE + 10.0})), Some(false));
        assert_eq!(admin_from(&json!({"v": 1, "iat": ADMIN_SINCE - 10.0})), None, "licenza di prima: non si sa");
    }

    /// Real licenses downloaded from the server (prova_licenza.py in /tmp/permessi-prove): the new format passes the usual
    /// check (that of the apps already installed), and admin is read only where present.
    #[test]
    fn real_server_licenses() {
        let Ok(l) = std::fs::read_to_string("/tmp/permessi-prove/licenza-prova.txt") else { return };
        let p = verify_signature(l.trim(), &pubkey_bytes()).expect("firma");
        assert!(check(l.trim(), str_of(&p, "email"), unix_now(), &pubkey_bytes()).is_ok(), "account normale: valida come prima");
        assert_eq!(admin_from(&p), Some(false));
        if let Ok(t) = std::fs::read_to_string("/tmp/permessi-prove/licenza-admin.txt") {
            let mut r = t.lines();
            let (lic, email) = (r.next().unwrap_or(""), r.next().unwrap_or(""));
            let p = check(lic, email, unix_now(), &pubkey_bytes()).expect("licenza amministratore valida col controllo di sempre");
            assert_eq!(admin_from(&p), Some(true));
        }
    }
}
