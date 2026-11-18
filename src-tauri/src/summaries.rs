// Plain-language explanations of command groups in chat (Nov 10, 2026, "it would be nice if it explained here what it is doing").
// - errore_chiaro: tool errors put in one understandable line; wanted blocks (foreground waits
//   moved to background, files to re-read) are "neutral": grey, they do not count as errors.
// - adesso: for a working session, the current action in plain words ("adesso_chiaro" field of the detail, also used by
//   the "In corso" view; the "adesso" field is something else: an object with tool and time for the Now panel).
// - riassunto_azioni: one or two sentences on what it is doing and why, from the small model, only from descriptions and outcomes.
//   Cache in ~/tavolo/riassunti-azioni.json by group key; redone only when the group has grown a lot.
use crate::{base_dir, load_json, unix_now, str_of};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());
static IN_PROGRESS: Mutex<Option<HashSet<String>>> = Mutex::new(None);
const MAX_ENTRIES: usize = 3000;

fn file() -> std::path::PathBuf { base_dir().join("riassunti-azioni.json") }

/// (understandable text, neutral). Neutral = wanted block or hiccup the agent solves by itself: not a red error.
pub fn clear_error(t: &str) -> (String, bool) {
    let clean = t.replace("<tool_use_error>", "").replace("</tool_use_error>", "");
    let low = clean.to_lowercase();
    let has = |x: &str| low.contains(x);
    if has("blocked: sleep") || (has("blocked") && has("sleep")) {
        return ("Attesa spostata in background: le attese lunghe non bloccano la chat".into(), true);
    }
    if has("has not been read yet") || has("read it first") { return ("Doveva prima leggere il file: lo legge e riprova".into(), true); }
    if has("file has been modified since") || has("modified since read") { return ("Il file era cambiato nel frattempo: lo rilegge e riprova".into(), true); }
    if has("string to replace not found") || has("old_string") { return ("Modifica non applicata: il testo da sostituire non c'era più, riprova".into(), false); }
    if has("doesn't want to proceed") || has("user rejected") || has("permission denied by user") || has("was rejected") {
        return ("Comando non permesso".into(), false);
    }
    if has("timed out") || has("timeout") { return ("Tempo scaduto: il comando ci ha messo troppo".into(), false); }
    if has("command not found") { return ("Programma non trovato sul Mac".into(), false); }
    if has("no such file or directory") { return ("File o cartella non trovati".into(), false); }
    if has("permission denied") || has("operation not permitted") { return ("Permesso negato dal sistema".into(), false); }
    if has("inputvalidationerror") || has("invalid input") { return ("Chiamata con parametri sbagliati: la rifà".into(), true); }
    if let Some(i) = low.find("exit code ") {
        let code: String = low[i + 10..].chars().take_while(|c| c.is_ascii_digit()).collect();
        if !code.is_empty() { return (format!("Il comando è finito con un errore (codice {code})"), false); }
    }
    let line = clean.lines().map(|r| r.trim()).find(|r| !r.is_empty()).unwrap_or("").to_string();
    (line.chars().take(160).collect(), false)
}

fn file_name(p: &str) -> String { p.trim().trim_end_matches('/').rsplit('/').next().unwrap_or(p).to_string() }

/// The current action in plain words, for a working session.
pub fn now_line(o: &Map<String, Value>) -> Option<String> {
    let works = o.get("vista").and_then(|x| x.as_str()) == Some("lavora");
    if !works { return None; }
    let ev = o.get("eventi")?.as_array()?;
    for e in ev.iter().rev() {
        match str_of(e, "tipo") {
            "agente" => return Some("Scrive la risposta".into()),
            "tu" => return Some("Legge il tuo messaggio".into()),
            "strumento" => {
                let (name, text) = (str_of(e, "nome"), str_of(e, "testo").trim());
                if name == "avviso" { continue; }
                let t = match name {
                    "Read" => format!("Legge {}", file_name(text)),
                    "Edit" | "MultiEdit" => format!("Modifica {}", file_name(text)),
                    "Write" => format!("Scrive {}", file_name(text)),
                    "Grep" | "Glob" => format!("Cerca {}", text),
                    "Agent" | "Task" => format!("Sotto-agente: {text}"),
                    "WebSearch" => format!("Cerca sul web: {text}"),
                    "WebFetch" => format!("Legge una pagina web: {text}"),
                    _ => text.to_string(),
                };
                let t: String = t.chars().take(140).collect();
                if !t.trim().is_empty() { return Some(t); }
            }
            _ => {}
        }
    }
    None
}

/// In a session's detail: "adesso_chiaro" and, on error events, "chiaro" and "neutro". A single call point.
pub fn enrich(o: &mut Map<String, Value>) {
    if let Some(Value::Array(ev)) = o.get_mut("eventi") {
        for e in ev.iter_mut() {
            if str_of(e, "tipo") != "errore" { continue; }
            let (c, n) = clear_error(str_of(e, "testo"));
            e["chiaro"] = json!(c); e["neutro"] = json!(n);
        }
    }
    // "adesso" already exists (object with tool and time, Now panel): the plain sentence goes in its own field
    if let Some(a) = now_line(o) { o.insert("adesso_chiaro".into(), json!(a)); }
}

/// How many more actions are needed to redo the summary of a group already summarised with n actions.
fn threshold(n: usize) -> usize { n + std::cmp::max(4, n / 3) }

/// Summary of a group of actions. Immediate answer if cached (and the group has not grown too much), otherwise asks
/// the small model (2-10 s). Two equal requests at once: the second gets the old cache with "in_corso".
pub fn summary(chiave: &str, righe: &[String], richiesta: &str) -> Value {
    let chiave: String = chiave.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.')).take(120).collect();
    if chiave.is_empty() || righe.len() < 3 || chiave.starts_with("locale:") { return json!({"ok": false}); }   // local chats: never to the cloud
    let n = righe.len();
    let cache = { let _g = LOCK.lock(); load_json(&file()).unwrap_or(json!({})) };
    let old = cache.get(&chiave).cloned();
    if let Some(v) = &old {
        let nv = v["n"].as_u64().unwrap_or(0) as usize;
        if !str_of(v, "testo").is_empty() && n < threshold(nv) { return json!({"ok": true, "testo": str_of(v, "testo"), "n": nv}); }
    }
    {
        let mut g = IN_PROGRESS.lock().unwrap();
        let set = g.get_or_insert_with(HashSet::new);
        if !set.insert(chiave.clone()) { return json!({"ok": true, "testo": old.as_ref().map(|v| str_of(v, "testo")).unwrap_or(""), "in_corso": true}); }
    }
    let list: String = righe.iter().rev().take(60).collect::<Vec<_>>().into_iter().rev().map(|r| format!("- {}", r.chars().take(200).collect::<String>())).collect::<Vec<_>>().join("\n");
    let question = format!("Sei dentro un'app che mostra a «NOME» il lavoro di un agente di programmazione. Ecco le azioni dell'agente in questo pezzo di lavoro \
(descrizioni tecniche ed esiti, anche in inglese){}.\n\nRichiesta di «NOME» a cui sta lavorando: {}\n\nAzioni:\n{}\n\n\
Scrivi in italiano semplice, per chi non legge il codice, UNA o DUE frasi brevi (al massimo 220 caratteri in tutto) su cosa sta facendo e perché, \
in terza persona e al presente (Sta controllando..., Ha corretto... e ora prova...), mai in prima persona, \
e se qualcosa non tornava cosa sta provando adesso. Niente nomi di file o di funzioni, niente elenchi, niente virgolette, non citare «NOME». \
Le attese spostate in background e i file da rileggere non sono problemi. Rispondi solo con JSON: {{\"testo\": \"...\"}}",
        if n > 60 { format!(" (ultime 60 di {n})") } else { String::new() },
        if richiesta.trim().is_empty() { "(non nota)".to_string() } else { richiesta.chars().take(400).collect() }, list);
    let r = crate::autopilot::ask_for("riassunti", &question, None, crate::autopilot::Cache::Risposta, true);
    IN_PROGRESS.lock().unwrap().as_mut().map(|x| x.remove(&chiave));
    let text: String = r.as_ref().map(|v| str_of(v, "testo").trim().to_string()).unwrap_or_default().chars().take(260).collect();
    if text.is_empty() { return json!({"ok": false, "testo": old.as_ref().map(|v| str_of(v, "testo")).unwrap_or("")}); }
    {
        let _g = LOCK.lock();
        let mut c = load_json(&file()).unwrap_or(json!({}));
        if !c.is_object() { c = json!({}); }
        c[&chiave] = json!({"n": n, "testo": text, "t": unix_now()});
        if let Some(o) = c.as_object_mut() {
            if o.len() > MAX_ENTRIES {
                let mut v: Vec<(String, f64)> = o.iter().map(|(k, x)| (k.clone(), x["t"].as_f64().unwrap_or(0.0))).collect();
                v.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                for (k, _) in v.into_iter().take(o.len() - MAX_ENTRIES) { o.remove(&k); }
            }
        }
        let _ = crate::storage::write_json(&file(), &c);
    }
    json!({"ok": true, "testo": text, "n": n})
}

#[tauri::command(async)]
pub fn riassunto_azioni(chiave: String, righe: Vec<String>, richiesta: Option<String>) -> Value {
    summary(&chiave, &righe, &richiesta.unwrap_or_default())
}

/// iPhone voice mode (Nov 11): from the agent's finished reply, 1-3 sentences to say aloud.
/// Never for local chats (locale:): refused here, the iPhone reads the first sentences by itself.
pub fn short_voice(id: &str, text: &str, richiesta: &str) -> Value {
    if id.trim().is_empty() || id.starts_with("locale:") { return json!({"ok": false, "msg": "chat locale: niente cloud"}); }
    let text: String = text.chars().take(6000).collect();
    if text.trim().is_empty() { return json!({"ok": false}); }
    let question = format!("«NOME» sta parlando a voce con un agente di programmazione, dal telefono, senza guardare lo schermo. \
Ecco la risposta scritta dell'agente. Scrivi cosa dirgli a voce, in italiano parlato e naturale, in prima persona come se fossi l'agente: \
da UNA a TRE frasi brevi, al massimo 320 caratteri in tutto. Di' l'essenziale: cosa e' stato fatto o trovato, e se l'agente chiede una decisione, \
un ok o qualcosa da fare a «NOME», dillo chiaro come domanda alla fine. Niente nomi di file, percorsi, codice, link, numeri di versione, elenchi, \
simboli o emoji; niente \"Restano:\". Non cominciare con Ecco o Ok.{}\n\nRisposta dell'agente:\n{}\n\n\
Rispondi solo con JSON: {{\"testo\": \"...\"}}",
        if richiesta.trim().is_empty() { String::new() } else { format!("\n\nCosa aveva chiesto «NOME»: {}", richiesta.chars().take(400).collect::<String>()) },
        text);
    let r = crate::autopilot::ask_for("riassunti", &question, None, crate::autopilot::Cache::No, true);
    let t: String = r.as_ref().map(|v| str_of(v, "testo").trim().to_string()).unwrap_or_default().chars().take(400).collect();
    if t.is_empty() { json!({"ok": false}) } else { json!({"ok": true, "testo": t}) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_never_local() {
        assert_eq!(short_voice("locale:abc", "ciao a tutti", "")["ok"], json!(false));
        assert_eq!(short_voice("", "ciao", "")["ok"], json!(false));
    }

    #[test]
    fn clear_errors() {
        let (t, n) = clear_error("<tool_use_error>Blocked: sleep 45 followed by: cat /tmp/x.log</tool_use_error>");
        assert!(n && t.starts_with("Attesa spostata"));
        let (t, n) = clear_error("Exit code 2\nls: x: No such file");
        assert!(!n && t.contains("codice 2"));
        assert!(clear_error("<tool_use_error>File has not been read yet. Read it first before writing to it.</tool_use_error>").1);
    }

    #[test]
    fn now_in_plain() {
        let o = json!({"vista": "lavora", "eventi": [
            {"tipo": "tu", "testo": "fai"}, {"tipo": "strumento", "nome": "Bash", "testo": "Controllo il log della prova"},
            {"tipo": "strumento", "nome": "Read", "testo": "/a/b/notte.py"}]});
        assert_eq!(now_line(o.as_object().unwrap()).as_deref(), Some("Legge notte.py"));
        let idle = json!({"vista": "aspetta", "eventi": []});
        assert!(now_line(idle.as_object().unwrap()).is_none());
    }
}
