// Messages for people using Juggl (notice scripts, sessions): they arrive in Juggl instead of an external chat service.
// They come in via POST /api/messaggio, end up in the notifications area (type "messaggio"), reach the iPhone as SSE
// event "messaggio" and, only if the user is at the Mac, as a native notification. The notice script reads the reply (iphone, al_mac)
// to decide whether a fallback notice outside Juggl is needed.
use crate::{f64_of, juggl_id, unix_now, str_of};
use serde_json::{json, Value};
use std::sync::Mutex;

/// Last signal from the iPhone app: time, state declared by the app ("attiva", "sfondo", "sospesa") and when it changed.
struct Iphone { t: f64, stato: String, cambio: f64 }
static IPHONE: Mutex<Iphone> = Mutex::new(Iphone { t: 0.0, stato: String::new(), cambio: 0.0 });

const ALIVE_FOR: f64 = 90.0;      // with no signals for 90 s the iPhone is not connected
const BACKGROUND_FOR: f64 = 30.0;    // in background iOS keeps the app alive only briefly: after 30 s I no longer count on the banner

/// Does the request come from the iPhone app? (not from the Mac itself: notice scripts call from 127.0.0.1)
pub fn from_phone(req: &tiny_http::Request) -> bool {
    let remote = req.remote_addr().map(|a| !a.ip().is_loopback()).unwrap_or(false);
    remote && req.headers().iter().any(|x| {
        (x.field.equiv("X-Juggl-Client")) || (x.field.equiv("User-Agent") && x.value.as_str().contains("CFNetwork"))
    })
}

/// Every request from the iPhone app (also opening the event channel) is a sign of life.
pub fn heartbeat() {
    if let Ok(mut i) = IPHONE.lock() { i.t = unix_now(); }
}

/// POST /api/iphone {stato}: the app says whether it is in foreground, background or being stopped by iOS.
pub fn app_state(b: &Value) -> Value {
    let state_name = match str_of(b, "stato") { "sfondo" => "sfondo", "sospesa" => "sospesa", _ => "attiva" };
    if let Ok(mut i) = IPHONE.lock() {
        let now = unix_now();
        i.t = now;
        if i.stato != state_name { i.stato = state_name.to_string(); i.cambio = now; }
    }
    status()
}

/// True if the iPhone showed a sign of life less than 90 s ago and is not stopped in background.
pub fn iphone_connected() -> bool {
    let Ok(i) = IPHONE.lock() else { return false };
    let now = unix_now();
    if now - i.t > ALIVE_FOR { return false; }
    match i.stato.as_str() {
        "sospesa" => false,
        "sfondo" => now - i.cambio < BACKGROUND_FOR,
        _ => true,
    }
}

/// GET /api/messaggio/stato: for the notice scripts and for the summary when the user steps away.
pub fn status() -> Value {
    let (t, state_name) = IPHONE.lock().map(|i| (i.t, i.stato.clone())).unwrap_or((0.0, String::new()));
    json!({"ok": true, "iphone": iphone_connected(), "al_mac": crate::user_at_mac(), "iphone_visto": t, "iphone_stato": state_name, "non_letti": unread_count()})
}

/// Messages (type "messaggio") not yet read in the notifications area.
fn unread_count() -> usize {
    let l = crate::load_json(&crate::base_dir().join("notifiche.json")).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let read_at = crate::load_json(&crate::base_dir().join("notifiche-lette.json")).map(|v| f64_of(&v, "t")).unwrap_or(0.0);
    l.iter().filter(|x| str_of(x, "tipo") == "messaggio" && f64_of(x, "t") > read_at).count()
}

/// The session the tap on the message leads to: the project's one (the most recent, preferably not hibernated).
/// "generale" = Juggl, the general assistant. Destinations that are only external point to their project.
fn session_for_project(p: &str) -> String {
    if p.is_empty() { return String::new(); }
    if p == "generale" { return juggl_id(); }
    let mut id = p.to_string();
    if let Some(v) = crate::load_json(&crate::base_dir().join("progetti.json")) {
        if let Some(d) = v.get("destinazioni").and_then(|x| x.as_array()).and_then(|a| a.iter().find(|x| str_of(x, "id") == p).cloned()) {
            if !str_of(&d, "sessione").is_empty() { id = str_of(&d, "sessione").to_string(); }
        }
    }
    let all_sessions = crate::list_sessions();
    let mut best: Option<&Value> = None;
    for v in all_sessions.iter().filter(|v| v.get("progetto").map(|x| str_of(x, "id") == id).unwrap_or(false)) {
        let score = |x: &Value| (if str_of(x, "vista") == "ibernata" { 0 } else { 1 }, f64_of(x, "da"));
        if best.map(|m| { let (a, b) = (score(v), score(m)); a.0 > b.0 || (a.0 == b.0 && a.1 > b.1) }).unwrap_or(true) { best = Some(v); }
    }
    best.map(|v| str_of(v, "sessione").to_string()).unwrap_or_default()
}

/// POST /api/messaggio {progetto, testo, etichetta?, link?, file?, prova?}
/// With "prova": true it replies only with the state (iphone, al_mac) without recording anything: for dry runs.
pub fn receive(b: &Value) -> Value {
    let project = str_of(b, "progetto").trim().to_string();
    let text = str_of(b, "testo").trim().to_string();
    let file = str_of(b, "file").trim().to_string();
    let at_mac = crate::user_at_mac();
    let iphone = iphone_connected();
    if b.get("prova").and_then(|x| x.as_bool()).unwrap_or(false) {
        return json!({"ok": true, "prova": true, "iphone": iphone, "al_mac": at_mac});
    }
    if text.is_empty() && file.is_empty() { return json!({"ok": false, "msg": "testo vuoto", "iphone": iphone, "al_mac": at_mac}); }
    let label = if str_of(b, "etichetta").is_empty() { project.clone() } else { str_of(b, "etichetta").to_string() };
    let title = if label.is_empty() { "Messaggio".to_string() } else { label.clone() };
    // link: the one passed in, or else the first found in the text
    let link = if !str_of(b, "link").is_empty() { str_of(b, "link").to_string() } else {
        text.split_whitespace().find(|w| w.starts_with("http://") || w.starts_with("https://"))
            .map(|w| w.trim_end_matches(|c: char| ")]>.,;:!?\"'".contains(c)).to_string()).unwrap_or_default()
    };
    let session = session_for_project(&project);
    let entry = crate::session_care::record_with(&title, &text, &session, "messaggio", json!({
        "id": format!("m{}", (unix_now() * 1000.0) as u64), "progetto": project, "etichetta": label, "link": link, "file": file,
    }));
    crate::bridge::broadcast("messaggio", &entry.to_string());
    // push to the iPhone if the user is not at the Mac and the app is not open: the reply tells the notice script whether it went out
    // (the external notice stays the fallback only when the push does not go out)
    let pushed = if crate::push::is_needed() {
        let body = if text.is_empty() { file.rsplit('/').next().unwrap_or("").to_string() } else { text.clone() };
        match crate::push::send(&title, &body, &session, &project) { Ok(()) => true, Err(e) => { crate::log_juggl(&format!("push non partita: {e}")); false } }
    } else { false };
    let quiet = crate::settings_chat::is_quiet();   // chat-style settings (Nov 16): night notices
    if at_mac && !quiet {
        use tauri_plugin_notification::NotificationExt;
        if let Some(app) = crate::APP.get() {
            let body = if text.is_empty() { file.rsplit('/').next().unwrap_or("").to_string() } else { text.chars().take(240).collect() };
            let _ = app.notification().builder().title(&title).body(&body).show();
        }
    }
    json!({"ok": true, "iphone": iphone, "al_mac": at_mac, "push": pushed, "silenzio": quiet, "id": entry.get("id").cloned().unwrap_or(Value::Null), "sessione": session})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn iphone_connection() {
        { let mut i = IPHONE.lock().unwrap(); i.t = 0.0; i.stato.clear(); }
        assert!(!iphone_connected());
        heartbeat();
        assert!(iphone_connected());
        app_state(&json!({"stato": "sfondo"}));
        assert!(iphone_connected());
        { let mut i = IPHONE.lock().unwrap(); i.cambio -= 31.0; }
        assert!(!iphone_connected());
        app_state(&json!({"stato": "sospesa"}));
        assert!(!iphone_connected());
        app_state(&json!({"stato": "attiva"}));
        assert!(iphone_connected());
        { let mut i = IPHONE.lock().unwrap(); i.t -= 91.0; }
        assert!(!iphone_connected());
    }
}
