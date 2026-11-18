// Claude account (Nov 9). Hosted sessions are always-on `claude -p` processes that read the token from the
// Keychain only at startup: if the user switches account (/login) they keep the old one and, when that one has used up
// its quota, they answer "You've hit your weekly limit" even if the new one is free (incident at 23:04 on Nov 9).
//
// Here: who is logged in now (oauthAccount in ~/.claude.json: account + organization) and the loop every 15 seconds
// that, when the account changes, restarts the hosted sessions with the new one (`hosted::align_account`). The
// Keychain is no use as a signal: the item's date also changes at every token refresh, without an account change.
//
// Tests without touching the real sessions: ~/tavolo/prova/account-finto.json
//   {"account": "finto-1", "sessioni": ["<id>", ...], "limite": ["<id>"], "scade": <epoch, at most one hour ahead>}
// shows a different account ONLY to the listed sessions; "limite" makes the next end of turn of those
// sessions be treated as "quota used up". Expired or more than one hour ahead: ignored.
use crate::{base_dir, home, load_json, log_juggl, unix_now, str_of};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Instant, SystemTime};
use tauri::{AppHandle, Emitter};

/// "accountUuid/organizationUuid" from the content of ~/.claude.json; empty if there is no login.
pub fn from_config(v: &Value) -> String {
    let o = &v["oauthAccount"];
    let a = str_of(o, "accountUuid");
    if a.is_empty() { return String::new(); }
    format!("{a}/{}", str_of(o, "organizationUuid"))
}

/// Account logged in now. The file (230 KB, written often by every session) is re-read only if it changed;
/// if it is half-written and cannot be read, the last value stays valid.
pub fn current() -> String {
    let b = current_base();
    if b.is_empty() { return b; }
    let f = SIGNATURE.lock().unwrap_or_else(|e| e.into_inner());
    if f.is_empty() { b } else { format!("{b}~{f}") }
}

// credentials change (Nov 12): same account but a different plan (e.g. Pro -> Max) or a new token after a login. The signature
// (plan and tier, never the token) is appended to the account: "uuid/org~max/default_claude_max_20x". This way the
// mechanism that restarts chats on account change also covers this case.
static SIGNATURE: Mutex<String> = Mutex::new(String::new());

/// Two values of `current()` are the same account if equal, or if one has no signature (chat started with an older
/// Juggl) and the part before the signature matches: this way a Juggl update does not restart every chat.
pub fn same_account(a: &str, b: &str) -> bool {
    if a == b { return true; }
    let (ba, fa) = a.split_once('~').unwrap_or((a, ""));
    let (bb, fb) = b.split_once('~').unwrap_or((b, ""));
    ba == bb && (fa.is_empty() || fb.is_empty())
}

/// Re-reads the credentials signature. True if it CHANGED since before (not on the first read after startup).
pub fn refresh_signature() -> bool {
    let new_sig = crate::quota::credentials_signature();
    if new_sig.is_empty() { return false; }
    let mut f = SIGNATURE.lock().unwrap_or_else(|e| e.into_inner());
    if *f == new_sig { return false; }
    let changed = !f.is_empty();
    *f = new_sig;
    changed
}

fn current_base() -> String {
    static CACHE: Mutex<Option<(SystemTime, u64, String)>> = Mutex::new(None);
    let p = home().join(".claude.json");
    let Ok(m) = std::fs::metadata(&p) else { return String::new() };
    let (t, l) = (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len());
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((ct, cl, v)) = c.as_ref() { if *ct == t && *cl == l { return v.clone(); } }
    let Some(v) = load_json(&p) else { return c.as_ref().map(|x| x.2.clone()).unwrap_or_default() };
    let a = from_config(&v);
    *c = Some((t, l, a.clone()));
    a
}

fn fake_account() -> Option<Value> {
    let v = load_json(&base_dir().join("prova").join("account-finto.json"))?;
    let expires = v.get("scade").and_then(|x| x.as_f64()).unwrap_or(0.0);
    if expires < unix_now() || expires > unix_now() + 3600.0 { return None; }
    Some(v)
}

fn is_listed(v: &Value, key: &str, id: &str) -> bool {
    v[key].as_array().map(|a| a.iter().any(|x| x.as_str() == Some(id))).unwrap_or(false)
}

/// Current account as seen by a session (the same for all, except in tests).
pub fn current_for(id: &str) -> String {
    if let Some(v) = fake_account() { if is_listed(&v, "sessioni", id) && !str_of(&v, "account").is_empty() { return str_of(&v, "account").to_string(); } }
    current()
}

/// Tests only: the end of turn of this session must be treated as "quota used up".
pub fn fake_limit(id: &str) -> bool { fake_account().map(|v| is_listed(&v, "limite", id)).unwrap_or(false) }

/// Short form for the log: the first digits of the account.
pub fn short(a: &str) -> String { if a.is_empty() { "?".into() } else { a.chars().take(8).collect() } }

/// Claude event (stream-json) saying the problem is the account: quota used up (429, "rate_limit",
/// "You've hit your ... limit") or invalid login (401/403). Only real API errors: an agent text that
/// talks about limits does not count.
pub fn is_account_error(d: &Value) -> bool {
    let kind = str_of(d, "type");
    let is_api_error = match kind {
        "assistant" => d.get("is_api_error_message").or_else(|| d.get("isApiErrorMessage")).and_then(|x| x.as_bool()).unwrap_or(false),
        "result" => d.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false),
        _ => false,
    };
    if !is_api_error { return false; }
    let status = d.get("api_error_status").or_else(|| d.get("apiErrorStatus")).and_then(|x| x.as_i64()).unwrap_or(0);
    if matches!(status, 401 | 403 | 429) { return true; }
    if matches!(str_of(d, "error"), "rate_limit" | "authentication_failed" | "billing_error") { return true; }
    let text = if kind == "result" { str_of(d, "result").to_string() } else {
        d["message"]["content"].as_array().map(|a| a.iter().map(|c| str_of(c, "text")).collect::<Vec<_>>().join(" ")).unwrap_or_default()
    };
    let t = text.to_lowercase();
    (t.contains("hit your") && t.contains("limit")) || t.contains("weekly limit") || t.contains("/login") || t.contains("oauth token")
}

/// Loop guard: the account really changes only if two reads in a row (15 s) give the same new value.
#[derive(Default)]
pub struct AccountGuard { candidato: String, noto: String, avvisato: Option<Instant>, giri: u32 }

impl AccountGuard {
    pub fn tick(&mut self, app: &AppHandle) {
        // credentials changed with the same account (different plan): old limits dropped and re-read right away. Checked
        // every minute (one Keychain call); the first read after startup only sets the starting point.
        if self.giri % 4 == 0 && refresh_signature() {
            log_juggl("credenziali Claude cambiate (stesso account, piano diverso)");
            crate::quota::credentials_changed();
        }
        self.giri = self.giri.wrapping_add(1);
        let a = current();
        if a.is_empty() { return; }
        if a != self.candidato { self.candidato = a; return; }
        let changed = !self.noto.is_empty() && !same_account(&self.noto, &a);
        if changed {
            log_juggl(&format!("account Claude cambiato: {} -> {}", short(&self.noto), short(&a)));
            // limits belong to the account: re-read them right away (Nov 11)
            std::thread::spawn(|| { let _ = crate::quota::refresh_from_server(); });
        }
        self.noto = a;
        let e = crate::hosted::align_account(&current_for);
        if e.riavviate.is_empty() && e.a_fine_turno.is_empty() { return; }
        let mut r = format!("account cambiato: {} chat riavviate", e.riavviate.len());
        if !e.a_fine_turno.is_empty() { r += &format!(", {} ripartono a fine turno", e.a_fine_turno.len()); }
        log_juggl(&r);
        // one notice per wave only (sessions postponed for background commands arrive later, a few at a time)
        if self.avvisato.map(|t| t.elapsed().as_secs() > 120).unwrap_or(true) {
            self.avvisato = Some(Instant::now());
            notify(app, if e.a_fine_turno.is_empty() { "Account cambiato: chat riavviate".to_string() }
                else { format!("Account cambiato: chat riavviate ({} a fine turno)", e.a_fine_turno.len()) });
        }
    }
}

/// Discreet notice in the app (toast) and on the paired iPhone.
pub fn notify(app: &AppHandle, text: String) {
    let p = json!({"testo": text});
    let _ = app.emit("account-cambiato", p.clone());
    crate::bridge::broadcast("account-cambiato", &p.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_from_config() {
        assert_eq!(from_config(&json!({"oauthAccount": {"accountUuid": "a1", "organizationUuid": "o1"}})), "a1/o1");
        assert_eq!(from_config(&json!({"oauthAccount": {}})), "");
        assert_eq!(from_config(&json!({})), "");
    }

    #[test]
    fn signature_and_comparison() {
        assert!(same_account("a/o", "a/o"));
        assert!(same_account("a/o", "a/o~max"));      // chat started before the signature
        assert!(same_account("a/o~max", "a/o"));
        assert!(same_account("a/o~max", "a/o~max"));
        assert!(!same_account("a/o~pro", "a/o~max"));  // plan changed
        assert!(!same_account("a/o", "b/o~max"));
    }

    #[test]
    fn account_errors() {
        // quota used up (as in the Nov 9 transcripts)
        let a = json!({"type": "assistant", "is_api_error_message": true, "error": "rate_limit", "api_error_status": 429,
            "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "You've hit your weekly limit · resets 9am (Europe/Rome)"}]}});
        assert!(is_account_error(&a));
        let r = json!({"type": "result", "is_error": true, "api_error_status": 429, "result": "You've hit your weekly limit"});
        assert!(is_account_error(&r));
        // text only, no status
        let r2 = json!({"type": "result", "is_error": true, "result": "You've hit your session limit · resets 5:10am"});
        assert!(is_account_error(&r2));
        // expired login
        assert!(is_account_error(&json!({"type": "result", "is_error": true, "api_error_status": 401, "result": "Please run /login"})));
        // limit inside the compaction error (Nov 12)
        let c = json!({"type": "result", "is_error": true, "result": "Prompt is too long · automatic compaction failed: You have hit your weekly limit"});
        assert!(is_account_error(&c));
        let c2 = json!({"type": "assistant", "is_api_error_message": true, "message": {"content": [{"type": "text", "text": "Prompt is too long \u{b7} automatic compaction failed: You have hit your weekly limit"}]}});
        assert!(is_account_error(&c2));
        // wrong model: an error, but not an account one
        assert!(!is_account_error(&json!({"type": "result", "is_error": true, "api_error_status": 404, "result": "There's an issue with the selected model"})));
        // the agent TALKING about the limit does not count
        assert!(!is_account_error(&json!({"type": "result", "is_error": false, "result": "Juggl dava 'You've hit your weekly limit'"})));
        assert!(!is_account_error(&json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "You've hit your weekly limit"}]}})));
    }
}
