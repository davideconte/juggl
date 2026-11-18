// emergency (Nov 12): when Claude hits the weekly limit, work does not stop.
// Minimal part (decided with the coordination chat, week at 92%):
// 1) Automatic Eco mode above 90% of the week (eco.rs, "emergenza (12/11)" block).
// 2) A Claude chat that receives "limit reached" is flagged (`limite_claude` in the state). The chat shows the
//    "Continua su Codex" button (ui/engines.js), which opens a Codex chat in the same folder with a SHORT summary (Codex is
//    on the Free plan, low quota): name, the user's latest requests, last answer.
// 3) Fallback chats are "cautious": fixed rule in the system instructions (engines.rs, REGOLE_PRUDENTI), no outward
//    actions without the user's explicit ok; a visible "prudente" mark at the top of the chat.
use crate::{storage, base_dir, str_of};
use serde_json::{json, Value};

/// Claude quota error (not login): 429, "rate_limit", "You've hit your ... limit", "usage limit".
pub fn limit_reached(d: &Value) -> bool {
    let kind = str_of(d, "type");
    let api_error = match kind {
        "assistant" => d.get("is_api_error_message").or_else(|| d.get("isApiErrorMessage")).and_then(|x| x.as_bool()).unwrap_or(false),
        "result" => d.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false),
        _ => false,
    };
    if !api_error { return false; }
    let status = d.get("api_error_status").or_else(|| d.get("apiErrorStatus")).and_then(|x| x.as_i64()).unwrap_or(0);
    if status == 429 || str_of(d, "error") == "rate_limit" { return true; }
    let text = if kind == "result" { str_of(d, "result").to_string() } else {
        d["message"]["content"].as_array().map(|a| a.iter().map(|c| str_of(c, "text")).collect::<Vec<_>>().join(" ")).unwrap_or_default()
    };
    let t = text.to_lowercase();
    (t.contains("hit your") && t.contains("limit")) || t.contains("usage limit") || t.contains("limit reached")
}

fn state_file(id: &str) -> std::path::PathBuf { base_dir().join("stato").join(format!("{id}.json")) }

/// At end of turn (hosted.rs): sets or clears `limite_claude` in the Claude chat's state.
pub fn after_turn(id: &str, limit: bool) {
    if !storage::is_valid_id(id) { return; }
    let p = state_file(id);
    if !p.exists() { return; }
    let _ = storage::modify(&p, None, |v| {
        if crate::engines::engine_of(v) != "claude" { return false; }
        if limit { v["limite_claude"] = json!(crate::unix_now()); true }
        else if v.get("limite_claude").is_some() { if let Some(o) = v.as_object_mut() { o.remove("limite_claude"); } true }
        else { false }
    });
}

fn shorten(t: &str, n: usize) -> String {
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= n { t } else { format!("{}…", t.chars().take(n).collect::<String>()) }
}

/// Short summary of the chat for the fallback: the user's last 3 requests and the last answer, from the transcript.
pub fn summary(id: &str) -> (Vec<String>, String) {
    let Some(p) = crate::find_transcript(&json!({"sessione": id})) else { return (vec![], String::new()) };
    let (lines, _) = crate::tail_file(&p, 1_500_000);
    let (mut requests, mut last) = (vec![], String::new());
    for r in lines {
        let Ok(d) = serde_json::from_str::<Value>(&r) else { continue };
        if d.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) || d.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
        let c = &d["message"]["content"];
        let text = match c { Value::String(t) => t.clone(), Value::Array(a) => a.iter().filter(|x| str_of(x, "type") == "text").map(|x| str_of(x, "text")).collect::<Vec<_>>().join("\n"), _ => String::new() };
        let text = text.trim().to_string();
        if text.is_empty() || text.starts_with('<') || text.starts_with("[Request interrupted") { continue; }
        match str_of(&d, "type") {
            "user" => requests.push(shorten(&text, 300)),
            "assistant" if !d.get("isApiErrorMessage").and_then(|x| x.as_bool()).unwrap_or(false) => last = shorten(&text, 600),
            _ => {}
        }
    }
    let n = requests.len();
    (requests.into_iter().skip(n.saturating_sub(3)).collect(), last)
}

/// Text of the first message of the fallback chat (short: Codex is on the Free plan).
pub fn fallback_message(name: &str, requests: &[String], last: &str) -> String {
    let mut t = format!("Riprendi il lavoro della chat \"{name}\": Claude si e' fermato per il limite settimanale e continui tu nella stessa cartella. Prima guarda lo stato dei file, poi continua da dove era arrivato.\n");
    if !requests.is_empty() { t.push_str(&crate::profile::with_name("\nUltime richieste di «NOME»:\n")); for r in requests { t.push_str(&format!("- {r}\n")); } }
    if !last.is_empty() { t.push_str(&format!("\nUltima risposta di Claude:\n{last}\n")); }
    t
}

/// "Continua su Codex": Codex chat in the same folder, cautious, with the summary. Returns the new id.
pub fn continue_on_codex_with(app: &tauri::AppHandle, id: &str) -> Result<String, String> {
    if !storage::is_valid_id(id) { return Err("id non valido".into()); }
    let st = crate::load_json(&state_file(id)).unwrap_or(json!({}));
    let name = [str_of(&st, "nome_manuale"), str_of(&st, "titolo")].into_iter().find(|x| !x.is_empty()).unwrap_or("chat").to_string();
    let cwd = crate::hosted::resume_dir(id);
    let (requests, last) = summary(id);
    let text = fallback_message(&name, &requests, &last);
    // state of the new chat before starting: cautious from the first turn (engines::conf_codex reads "prudente")
    let new_id = crate::hosted::uuid();
    crate::write_state(&state_file(&new_id), &json!({"sessione": new_id, "motore": "codex", "prudente": true, "da_chat": id, "nome_manuale": format!("{name} · Codex")}));
    crate::hosted::start_engine(app, &cwd, &text, Some(&new_id), None, Some("codex"))
}

#[tauri::command(async)]
pub fn continua_su_codex(app: tauri::AppHandle, id: String) -> Value {
    match continue_on_codex_with(&app, &id) { Ok(n) => json!({"ok": true, "id": n}), Err(e) => json!({"ok": false, "msg": e}) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limit() {
        assert!(limit_reached(&json!({"type": "result", "is_error": true, "result": "You've hit your weekly limit · resets Sun 4:59am"})));
        assert!(limit_reached(&json!({"type": "result", "is_error": true, "api_error_status": 429, "result": "x"})));
        assert!(!limit_reached(&json!({"type": "result", "is_error": true, "api_error_status": 401, "result": "Please run /login"})));
        assert!(!limit_reached(&json!({"type": "result", "is_error": false, "result": "parliamo del limite: hit your limit"})));
    }
    #[test]
    fn short_message() {
        let m = fallback_message("Progetto A", &["fai il deploy".into()], "ho finito i test");
        assert!(m.contains("\"Progetto A\"") && m.contains("- fai il deploy") && m.contains("ho finito i test"));
        assert!(shorten(&"a ".repeat(500), 300).chars().count() <= 301);
    }
}
