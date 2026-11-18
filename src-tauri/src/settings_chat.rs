// chat-style settings (Nov 16): the Rust side of the word-based Settings (ui/settings-chat.js).
//  - night notices: quiet hours for Mac notices and iPhone pushes (silenzio());
//  - model of new chats: key "modello_nuove" in impostazioni.json (fable|opus|sonnet|haiku, empty = as today),
//    read by /api/nuova (main.rs, account-switch chat) and by the app's "nuova" command (modello_nuove());
//  - change log (~/tavolo/regolazioni-storia.jsonl): every change made from Settings leaves one line;
//  - phrases the interface rules do not understand: the small model says ONLY which setting and which value
//    (regolazioni_capisci). It never changes anything: the interface decides and applies, asking confirmation for delicate things.
// The keys are written with profilo_imposta (profile.rs, block "impostazioni come chat (16/11)").
use serde_json::{json, Value};
use std::process::Stdio;
#[cfg(windows)] use crate::sys as libc;

fn base_dir() -> std::path::PathBuf { crate::profile::settings_file().parent().map(|p| p.to_path_buf()).unwrap_or_default() }
fn settings() -> Value { std::fs::read(crate::profile::settings_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({})) }

pub const MODELS: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

/// Model of new chats chosen in Settings, if valid.
pub fn model_for_new() -> Option<String> {
    let m = settings().get("modello_nuove").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if MODELS.contains(&m.as_str()) { Some(m) } else { None }
}

/// "23:00" -> minutes since midnight.
pub fn minutes_of(t: &str) -> Option<u32> {
    let (h, m) = t.trim().split_once(':').unwrap_or((t.trim(), "0"));
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    if h > 23 || m > 59 { return None; }
    Some(h * 60 + m)
}

/// Minute inside the from-to interval (also across midnight: 23:00-08:00).
pub fn in_window(now: u32, start: u32, a: u32) -> bool {
    if start == a { return false; }
    if start < a { now >= start && now < a } else { now >= start || now < a }
}

fn local_minute() -> u32 {
    let t = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    (tm.tm_hour as u32) * 60 + tm.tm_min as u32
}

/// Night notices: true if Mac notices and iPhone pushes must be held back right now (they stay in Juggl's list).
pub fn is_quiet() -> bool {
    let n = settings().get("avvisi_notte").cloned().unwrap_or(Value::Null);
    if !n.get("acceso").and_then(|v| v.as_bool()).unwrap_or(false) { return false; }
    let start = n.get("da").and_then(|v| v.as_str()).and_then(minutes_of).unwrap_or(23 * 60);
    let a = n.get("a").and_then(|v| v.as_str()).and_then(minutes_of).unwrap_or(8 * 60);
    in_window(local_minute(), start, a)
}

fn history_file() -> std::path::PathBuf { base_dir().join("regolazioni-storia.jsonl") }

#[tauri::command(async)]
pub fn regolazioni_stato() -> Value {
    let i = settings();
    let n = i.get("avvisi_notte").cloned().unwrap_or(Value::Null);
    let history: Vec<Value> = std::fs::read_to_string(history_file()).unwrap_or_default().lines().rev().take(30)
        .filter_map(|r| serde_json::from_str(r).ok()).collect();
    json!({
        "avvisi_notte": {"acceso": n.get("acceso").and_then(|v| v.as_bool()).unwrap_or(false),
                          "da": n.get("da").and_then(|v| v.as_str()).unwrap_or("23:00"), "a": n.get("a").and_then(|v| v.as_str()).unwrap_or("08:00")},
        "silenzio_adesso": is_quiet(),
        "modello_nuove": model_for_new().unwrap_or_default(),
        "storia": history,
    })
}

/// One line in the change log: {t, id, da, a, frase}. Keeps the last 300.
#[tauri::command(async)]
pub fn regolazioni_registra(riga: Value) -> Value {
    let mut r = riga; r["t"] = json!(crate::unix_now());
    let p = history_file();
    let mut lines: Vec<String> = std::fs::read_to_string(&p).unwrap_or_default().lines().map(String::from).collect();
    lines.push(r.to_string());
    let keep = lines.len().saturating_sub(300);
    let tmp = p.with_extension("jsonl.tmp");
    let ok = std::fs::write(&tmp, lines[keep..].join("\n") + "\n").is_ok() && std::fs::rename(&tmp, &p).is_ok();
    json!({"ok": ok})
}

/// Phrase not understood by the rules: the small model picks ONE entry of the list and a value among the allowed ones.
/// voci = [{id, nome, valori:[..]}]. Replies {ok, id, valore} or {ok:false}. Never actions: only the choice.
#[tauri::command(async)]
pub fn regolazioni_capisci(frase: String, voci: Value) -> Value {
    let frase: String = frase.chars().take(300).collect();
    let list: String = voci.as_array().cloned().unwrap_or_default().iter().take(60).map(|v| {
        let vals: Vec<String> = v.get("valori").and_then(|x| x.as_array()).cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).collect();
        format!("- {}: {} (valori: {})\n", v.get("id").and_then(|x| x.as_str()).unwrap_or(""), v.get("nome").and_then(|x| x.as_str()).unwrap_or(""), vals.join(" | "))
    }).collect();
    let question = format!("Sei il pannello Impostazioni di un'app. L'utente ha scritto: «{frase}».\nScegli al massimo UNA impostazione da questo elenco e UN valore tra quelli ammessi:\n{list}\nRispondi solo con JSON: {{\"id\": \"...\", \"valore\": \"...\"}}. Se nessuna impostazione corrisponde rispondi {{}}. Non spiegare.");
    let claude = crate::profile::claude_cmd();
    let empty_dir = base_dir().join("comandi").join("vuota");
    let _ = std::fs::create_dir_all(&empty_dir);
    use crate::NoConsoleWindow;
    let o = match crate::new_command(&claude).no_console_window()
        .args(["-p", "--model", "haiku", "--setting-sources", "project,local", "--strict-mcp-config", "--tools", "", "--output-format", "text", &question])
        .env("TAVOLO_INTERNO", "1").current_dir(&empty_dir).stdin(Stdio::null()).output() { Ok(o) => o, Err(_) => return json!({"ok": false}) };
    let text = String::from_utf8_lossy(&o.stdout);
    let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) else { return json!({"ok": false}) };
    if b < a { return json!({"ok": false}); }
    let v: Value = serde_json::from_str(&text[a..=b]).unwrap_or(json!({}));
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
    let value = v.get("valore").and_then(|x| x.as_str()).unwrap_or("");
    // only entries and values from the list: an invented answer does not pass
    let valid = voci.as_array().map(|vv| vv.iter().any(|x| x.get("id").and_then(|i| i.as_str()) == Some(id)
        && x.get("valori").and_then(|l| l.as_array()).map(|l| l.iter().any(|y| y.as_str() == Some(value))).unwrap_or(false))).unwrap_or(false);
    if !valid { return json!({"ok": false}); }
    json!({"ok": true, "id": id, "valore": value})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn times() {
        assert_eq!(minutes_of("23:00"), Some(1380));
        assert_eq!(minutes_of("8"), Some(480));
        assert_eq!(minutes_of("24:00"), None);
        // across midnight
        assert!(in_window(23 * 60 + 30, 23 * 60, 8 * 60));
        assert!(in_window(3 * 60, 23 * 60, 8 * 60));
        assert!(!in_window(8 * 60, 23 * 60, 8 * 60));
        assert!(!in_window(12 * 60, 23 * 60, 8 * 60));
        // within the same day
        assert!(in_window(14 * 60, 13 * 60, 15 * 60));
        assert!(!in_window(15 * 60, 13 * 60, 15 * 60));
        // from = to: never
        assert!(!in_window(10, 600, 600));
    }
}
