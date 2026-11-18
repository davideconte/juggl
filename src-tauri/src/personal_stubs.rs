// Placeholders for the personal functions, used when the "personale" cargo feature is off (public tree).
// Commands have the same name and arguments as the originals, so the tauri::generate_handler! list
// does not change and the UI always gets an answer: "not available".
#![allow(unused_variables, dead_code)]

/// External messaging ("Incoming" view): not present in this version.
pub mod wa {
    use serde_json::{json, Value};
    use tauri::AppHandle;

    fn unavailable() -> Value { json!({"ok": false, "msg": "non disponibile"}) }

    #[tauri::command(async)]
    pub fn wa_dati() -> Value { json!({"ok": false, "msg": "non disponibile", "spenta": true, "chat": [], "aspettano": []}) }
    #[tauri::command(async)]
    pub fn wa_sposta(jid: String, progetto: String) -> Value { unavailable() }
    #[tauri::command(async)]
    pub fn wa_livello(jid: String, livello: i64) -> Value { unavailable() }
    #[tauri::command(async)]
    pub fn wa_fatto(jid: String) -> Value { unavailable() }
    #[tauri::command(async)]
    pub fn wa_trascrivi(jid: String, id: String) -> Value { unavailable() }
    #[tauri::command(async)]
    pub fn wa_silenzia(jid: String) -> Value { unavailable() }
    #[tauri::command(async)]
    pub fn wa_apri(jid: String) -> Value { unavailable() }
    #[tauri::command(async)]
    pub fn wa_nuova_bozza(jid: String, testo: String) -> Value { unavailable() }
    #[tauri::command(async)]
    pub fn wa_consegna(accesa: bool) -> Value { unavailable() }
    #[tauri::command(async)]
    pub fn wa_bozza(app: AppHandle, id: String, azione: String, valore: Option<String>) -> Value { unavailable() }
}

/// State and "Stop" of a PC's local model with its guardian: not present in this version.
pub mod pc_local_engine {
    use serde_json::{json, Value};

    #[tauri::command(async)]
    pub fn locale_stato() -> Value { json!({"ok": false, "raggiungibile": false, "msg": "non disponibile"}) }
    #[tauri::command(async)]
    pub fn locale_ferma() -> Value { json!({"ok": false, "msg": "non disponibile"}) }
}

/// Agent with tools on the PC (local chats of type "agent"): not present in this version.
pub mod pc_agent {
    use crate::{load_json, unix_now, write_state};
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};
    use tauri::{AppHandle, Emitter};

    pub fn is_available() -> bool { false }

    pub fn screen() -> Result<Vec<u8>, String> { Err("non disponibile".into()) }

    pub fn answer(p: &Path, n: &str, testo: Option<&str>) -> Value { json!({"ok": false, "msg": "non disponibile"}) }

    /// The turn ends immediately with a reply that explains it, so the chat does not stay waiting.
    pub fn turn(id: String, p: PathBuf, testo: String, app: AppHandle, should_stop: impl Fn() -> bool) {
        if let Some(mut c) = load_json(&p) {
            let mut msgs = c["messaggi"].as_array().cloned().unwrap_or_default();
            msgs.push(json!({"ruolo": "assistant", "testo": "L'agente del PC non è disponibile in questa versione.", "t": unix_now()}));
            c["messaggi"] = json!(msgs);
            write_state(&p, &c);
        }
        let _ = app.emit("parziale", json!({"id": format!("locale:{id}"), "testo": ""}));
        let _ = app.emit("cambiato", Value::Null);
    }
}
