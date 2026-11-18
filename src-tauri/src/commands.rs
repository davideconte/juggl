// commands (13/11): the "/" menu in Juggl chats, on Mac and iPhone.
//
// Hosted chats are `claude -p`: many terminal commands do not exist there ("/login isn't available in this
// environment") and an unknown command reaches the model as text and wastes a turn. Here:
//   - `elenco(id)`  = what to show in the menu: Juggl commands, the Claude Code ones that work in -p, skills
//                     and commands (global ~/.claude, the chat's project, plugins), with a short Italian description;
//   - `prima_dell_invio(id, text)` = called by `invia` (Mac and iPhone): Juggl runs its own commands, the ones that do not
//                     exist in -p say what to use instead, unknown ones are never sent.
//
// Claude Code list: control_request {"subtype":"initialize"} to a `claude -p` in the chat folder (no tokens,
// no saved session, TAVOLO_INTERNO = Juggl hooks do not see it). Kept in ~/tavolo/comandi/catalogo.json
// per folder and refreshed in the background when skills, commands or plugins change (or after 12 hours).
// Descriptions: Claude Code's are in English. System commands use the table below; for skills and plugins
// the small model rewrites them in Italian once (only the skill name and description, never chat text),
// in ~/tavolo/comandi/descrizioni.json.
//
// Verified with Claude Code 2.1.323 (13/11, ~/tavolo-comandi/prove/VERIFICA-2.1.323.md): in -p these work: /usage /cost
// /context /model /effort /rename /compact /config /mcp /recap, skills and commands; NOT /login /logout /status /help
// /resume /memory /permissions /fast; /clear works but opens another session under the same process (Juggl loses track).
use crate::{base_dir, home, load_json, str_of};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Juggl command: Italian name, aliases (including the Claude Code names Juggl handles instead), argument, description.
struct J { nome: &'static str, alias: &'static [&'static str], arg: &'static str, desc: &'static str }

const JUGGL: &[J] = &[
    J { nome: "nuova", alias: &["clear", "new", "reset"], arg: "[cosa ti serve]", desc: "Apri una chat nuova in questo progetto" },
    J { nome: "modello", alias: &["model"], arg: "[fable | opus | sonnet | haiku | predefinito]", desc: "Cambia il modello di questa chat" },
    J { nome: "ragionamento", alias: &["effort"], arg: "[basso | medio | alto | molto-alto | massimo]", desc: "Quanto pensa prima di rispondere" },
    J { nome: "consumi", alias: &["usage", "cost", "stats"], arg: "", desc: "Quota di Claude e consumi delle chat" },
    J { nome: "account", alias: &["login"], arg: "", desc: "Entra o cambia account Claude" },
    J { nome: "esci", alias: &["logout"], arg: "", desc: "Esci dall'account Claude" },
    J { nome: "stato", alias: &["status"], arg: "", desc: "Account, modello e cartella di questa chat" },
    J { nome: "rinomina", alias: &["rename", "name"], arg: "<nome>", desc: "Cambia il nome della chat" },
    J { nome: "sposta", alias: &[], arg: "", desc: "Sposta la chat in un altro progetto" },
    J { nome: "riprendi", alias: &["resume", "continue"], arg: "[cosa cerchi]", desc: "Ritrova e riapri una chat di prima" },
    J { nome: "eco", alias: &[], arg: "[on | off]", desc: "Modalità Eco: chat su Sonnet per risparmiare quota" },
    J { nome: "ferma", alias: &["stop"], arg: "", desc: "Ferma la risposta in corso" },
    J { nome: "archivia", alias: &["exit", "quit"], arg: "", desc: "Chiudi la chat, resta tra le archiviate" },
    J { nome: "cerca", alias: &[], arg: "[testo]", desc: "Cerca in questa conversazione" },
    J { nome: "richieste", alias: &[], arg: "", desc: "Le tue richieste aperte in tutte le chat" },
    J { nome: "permessi", alias: &["permissions"], arg: "", desc: "Cosa può fare la chat senza chiederti" },
    J { nome: "memoria", alias: &["memory"], arg: "", desc: "Apri la memoria di Claude (CLAUDE.md)" },
    J { nome: "larghezza", alias: &["width"], arg: "[stretta | normale | larga]", desc: "Larghezza della colonna dei messaggi" },   // width (16/11)
    J { nome: "impostazioni", alias: &["settings"], arg: "[cosa cambiare]", desc: "Impostazioni di Juggl: anche a parole" },   // settings as a chat (16/11)
    J { nome: "scorciatoie", alias: &["tasti", "shortcuts"], arg: "", desc: "Tasti per passare da una chat all'altra e per la chat nuova" },
    J { nome: "aiuto", alias: &["help", "?"], arg: "", desc: "Tutti i comandi" },
];

/// Juggl commands that also make sense in Codex chats and in chats with the PC model.
const JUGGL_EVERYWHERE: &[&str] = &["scorciatoie", "nuova", "consumi", "rinomina", "sposta", "riprendi", "eco", "ferma", "archivia", "cerca", "richieste", "larghezza", "impostazioni", "aiuto", "account", "esci"];

/// Claude Code commands that are missing in -p or not needed in Juggl: (name, what to say).
const NOT_HERE: &[(&str, &str)] = &[
    ("fast", "La modalità veloce c'è solo nel terminale. Per risposte più rapide: /modello sonnet, oppure /eco."),
    ("agents", "Per creare un sotto-agente chiedilo alla chat a parole: lo scrive lei in .claude/agents."),
    ("doctor", "/doctor funziona solo nel terminale di Claude Code."),
    ("color", "Il colore della casella c'è solo nel terminale."),
    ("focus", "La vista concentrata c'è solo nel terminale."),
    ("heapdump", "/heapdump funziona solo nel terminale di Claude Code."),
    ("reload-plugins", "Le estensioni nuove arrivano quando la chat riparte: apri una chat nuova o riprendi questa."),
    ("auto-mode-setup", "Il livello dei permessi lo scegli in Juggl: scrivi /permessi."),
    ("ide", "Il collegamento con l'editor c'è solo nel terminale."),
    ("terminal-setup", "Serve solo nel terminale."),
    ("vim", "Serve solo nel terminale."),
    ("bug", "Per segnalare un problema di Claude Code usa il terminale (/bug)."),
    ("feedback", "Per mandare un parere su Claude Code usa il terminale (/feedback)."),
    ("upgrade", "Il piano si cambia da claude.ai, in Impostazioni."),
    ("privacy-settings", "Le impostazioni privacy sono su claude.ai, in Impostazioni."),
    ("export", "Per salvare la conversazione chiedilo alla chat: la scrive in un file."),
    ("hooks", "Gli hook si cambiano in ~/.claude/settings.json: chiedilo alla chat (skill update-config)."),
    ("add-dir", "Per lavorare in un'altra cartella apri una chat nuova lì (/nuova)."),
    ("extra-usage", "Si chiama /usage-credits."),
];

/// Claude Code internals: never shown.
const HIDDEN: &[&str] = &["__remote-workflow", "workflow-launch-exec", "design-consent", "design-revoke", "extra-usage", "heapdump", "color", "focus", "auto-mode-setup", "agents"];

/// Italian descriptions of Claude Code system commands (the others go through the small model, once).
const SYSTEM_DESC_IT: &[(&str, &str)] = &[
    ("compact", "Riassume la conversazione per liberare memoria"),
    ("context", "Quanta memoria della chat è usata"),
    ("config", "Cambia un'impostazione di Claude Code (chiave=valore)"),
    ("mcp", "Stato dei server MCP collegati"),
    ("init", "Crea CLAUDE.md, la guida del progetto per Claude"),
    ("recap", "Riassunto della chat in una riga"),
    ("code-review", "Controlla le modifiche e trova gli errori"),
    ("simplify", "Ripulisce e semplifica il codice cambiato"),
    ("security-review", "Controllo di sicurezza delle modifiche"),
    ("verify", "Prova davvero che la modifica funziona"),
    ("debug", "Accende i log di debug per capire un problema"),
    ("batch", "Grande modifica divisa tra tanti agenti in parallelo"),
    ("loop", "Ripete un compito a intervalli"),
    ("schedule", "Compiti automatici a orario, nel cloud"),
    ("run", "Avvia l'app del progetto per vederla funzionare"),
    ("run-skill-generator", "Crea la skill che spiega come avviare il progetto"),
    ("advisor", "Fa consultare un modello più forte nei momenti chiave"),
    ("autocompact", "Quando compattare da sola la chat"),
    ("output-style", "Stile delle risposte"),
    ("reload-skills", "Ricarica le skill cambiate su disco"),
    ("ultrareview", "Revisione profonda nel cloud, a pagamento"),
    ("usage-credits", "Crediti in più quando finisce la quota"),
    ("insights", "Rapporto sulle tue sessioni di Claude Code"),
    ("skill-doctor", "Quali skill non usi e pesano sulla memoria"),
    ("goal", "Continua a lavorare finché non raggiunge l'obiettivo"),
    ("list-agents", "Agenti e chat a cui si può scrivere"),
    ("team-onboarding", "Guida per far partire i colleghi con Claude Code"),
    ("import", "Importa le impostazioni da un altro agente"),
    ("fewer-permission-prompts", "Meno richieste di permesso per i comandi sicuri"),
    ("update-config", "Cambia le impostazioni di Claude Code (permessi, hook)"),
    ("claude-api", "Guida alle API di Claude e ai modelli"),
    ("workflow-authoring", "Guida per scrivere un workflow con più agenti"),
    ("plugin-authoring", "Crea o modifica un'estensione di Claude"),
    ("design", "Accesso di Claude ai progetti Design"),
    ("design-sync", "Manda un design system a Claude Design"),
    ("dataviz", "Grafici e visualizzazioni fatti bene"),
    ("deep-research", "Ricerca approfondita sul web, con le fonti"),
    ("doctor", "Controllo e pulizia della configurazione di Claude Code"),
    ("pr-comments", "Leggi i commenti di una pull request"),
    ("review", "Controlla le modifiche e trova gli errori"),
    ("statusline", "Imposta la riga di stato del terminale"),
    ("memory", "Apri la memoria di Claude"),
    ("todos", "Elenco delle cose da fare della chat"),
    ("tasks", "Lavori in sottofondo della chat"),
];

fn system_desc_it(name: &str) -> Option<&'static str> { SYSTEM_DESC_IT.iter().find(|(n, _)| *n == name).map(|(_, d)| *d) }

// ---------------------------------------------------------------------------------------------------------------
// Claude Code catalog per folder

static CACHE: Mutex<Option<Map<String, Value>>> = Mutex::new(None);
static IN_PROGRESS: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static TRANSLATING: Mutex<bool> = Mutex::new(false);
/// Last error per folder and for descriptions: no bursts of retries if Claude Code does not start or is down.
static FAILED: Mutex<Option<std::collections::HashMap<String, f64>>> = Mutex::new(None);
static TRANSLATION_FAILED: Mutex<f64> = Mutex::new(0.0);
const LIFETIME: f64 = 24.0 * 3600.0;
const ERROR_PAUSE: f64 = 600.0;
const TRANSLATION_ERROR_PAUSE: f64 = 3600.0;

fn commands_dir() -> PathBuf { base_dir().join("comandi") }
fn catalog_file() -> PathBuf { commands_dir().join("catalogo.json") }
fn it_file() -> PathBuf { commands_dir().join("descrizioni.json") }
fn now_secs() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

fn mtime(p: &Path) -> u64 { std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0) }

/// Fingerprint of what decides the list: if a skill, command or plugin changes, it is rebuilt.
fn fingerprint(cwd: &Path) -> String {
    let h = home().join(".claude");
    let mut v: Vec<u64> = ["skills", "commands", "plugins", "plugins/installed_plugins.json", "settings.json", "agents"].iter().map(|x| mtime(&h.join(x))).collect();
    for x in [".claude", ".claude/skills", ".claude/commands", ".claude/settings.json"] { v.push(mtime(&cwd.join(x))); }
    v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(".")
}

fn with_cache<R>(f: impl FnOnce(&mut Map<String, Value>) -> R) -> R {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() { *g = Some(load_json(&catalog_file()).and_then(|v| v.as_object().cloned()).unwrap_or_default()); }
    f(g.as_mut().unwrap())
}

fn save_cache() {
    let m = with_cache(|m| m.clone());
    let _ = std::fs::create_dir_all(commands_dir());
    let tmp = catalog_file().with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec(&Value::Object(m)).unwrap_or_default()).is_ok() { let _ = std::fs::rename(&tmp, catalog_file()); }
}

/// Asks Claude Code in the folder for the command list ("initialize" handshake: no message, no tokens).
fn ask_cli(cwd: &Path) -> Result<Value, String> {
    let mut c = crate::new_command(crate::profile::claude_cmd());
    use crate::NoConsoleWindow;
    c.no_console_window()
        .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--strict-mcp-config", "--no-session-persistence"])
        .env("TAVOLO_INTERNO", "1").current_dir(if cwd.is_dir() { cwd.to_path_buf() } else { home() })
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn().map_err(|e| format!("Claude Code non parte: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("stdin")?;
    let out = child.stdout.take().ok_or("stdout")?;
    let _ = writeln!(stdin, "{}", json!({"type": "control_request", "request_id": "juggl-comandi", "request": {"subtype": "initialize"}}));
    let _ = stdin.flush();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if !line.contains("control_response") { continue; }
            if let Ok(d) = serde_json::from_str::<Value>(&line) {
                if str_of(&d, "type") == "control_response" { let _ = tx.send(d); return; }
            }
        }
    });
    let r = rx.recv_timeout(Duration::from_secs(40));
    drop(stdin);
    let _ = child.kill(); let _ = child.wait();
    let d = r.map_err(|_| "Claude Code non ha risposto".to_string())?;
    let resp = &d["response"]["response"];
    let commands = resp["commands"].as_array().cloned().ok_or("risposta senza comandi")?;
    Ok(json!({"comandi": commands, "versione": str_of(resp, "claude_code_version"), "modelli": resp["models"].clone()}))
}

fn refresh_folder(cwd: &str) -> Option<Value> {
    let last = FAILED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).get(cwd).copied().unwrap_or(0.0);
    if now_secs() - last < ERROR_PAUSE { return None; }
    {
        let mut g = IN_PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
        let set = g.get_or_insert_with(HashSet::new);
        if !set.insert(cwd.to_string()) { return None; }
    }
    let r = ask_cli(Path::new(cwd));
    IN_PROGRESS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).remove(cwd);
    match r {
        Ok(mut v) => {
            v["firma"] = json!(fingerprint(Path::new(cwd))); v["t"] = json!(now_secs());
            with_cache(|m| { m.insert(cwd.to_string(), v.clone()); if m.len() > 80 { keep_recent(m, 60); } });
            save_cache();
            translate_in_background();
            Some(v)
        }
        Err(e) => {
            FAILED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(cwd.to_string(), now_secs());
            crate::log_juggl(&format!("comandi: elenco di Claude Code non letto in {cwd}: {e} (riprovo tra 10 min)"));
            None
        }
    }
}

fn keep_recent(m: &mut Map<String, Value>, n: usize) {
    let mut v: Vec<(String, f64)> = m.iter().map(|(k, x)| (k.clone(), x["t"].as_f64().unwrap_or(0.0))).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (k, _) in v.into_iter().skip(n) { m.remove(&k); }
}

/// Claude Code list for the folder. `wait` = if it is not there yet, ask for it right away (up to 40 s);
/// otherwise return what is there (even if old) and refresh it in the background.
fn catalog(cwd: &str, wait: bool) -> Option<Value> {
    let c = with_cache(|m| m.get(cwd).cloned());
    let fresh = c.as_ref().map(|v| str_of(v, "firma") == fingerprint(Path::new(cwd)) && now_secs() - v["t"].as_f64().unwrap_or(0.0) < LIFETIME).unwrap_or(false);
    if fresh { return c; }
    if c.is_some() || !wait {
        let w = cwd.to_string();
        std::thread::spawn(move || { refresh_folder(&w); });
        return c;
    }
    refresh_folder(cwd).or_else(|| with_cache(|m| m.get(cwd).cloned()))
}

// ---------------------------------------------------------------------------------------------------------------
// Italian descriptions for skills and plugins

fn it_key(name: &str, desc: &str) -> String {
    // name + description fingerprint: if the skill text changes, it is rewritten
    let mut h: u64 = 1469598103934665603;
    for b in desc.bytes() { h ^= b as u64; h = h.wrapping_mul(1099511628211); }
    format!("{name}|{h:x}")
}

fn it_descriptions() -> Map<String, Value> { load_json(&it_file()).and_then(|v| v.as_object().cloned()).unwrap_or_default() }

/// Strips "(user)", "(project)", "(plugin:x)", "(claude.ai sync)" at the end and tells where it comes from.
fn source_and_text(desc: &str) -> (String, &'static str) {
    let d = desc.trim();
    if let Some(i) = d.rfind(" (") {
        if d.ends_with(')') {
            let f = &d[i + 2..d.len() - 1];
            let source = if f == "user" { "tue" } else if f == "project" { "progetto" } else if f.starts_with("plugin") { "plugin" } else if f.contains("sync") { "claude.ai" } else if f.contains("workflow") { "" } else { "" };
            if !source.is_empty() || f.contains("workflow") { return (d[..i].trim().to_string(), source); }
        }
    }
    (d.to_string(), "")
}

/// First sentence, short: while the Italian one is missing.
fn short(d: &str) -> String {
    let p = d.split(['\n']).next().unwrap_or("").trim();
    let end = p.find(". ").map(|i| i + 1).unwrap_or(p.len());
    let mut t: String = p[..end].chars().take(90).collect();
    if t.chars().count() == 90 { t.push('…'); }
    t
}

/// Names of the user's projects and clients (~/tavolo/progetti.json: id, name, keywords, folder): never sent to the model.
fn reserved_words() -> Vec<String> {
    let mut v: Vec<String> = vec![];
    let d = load_json(&base_dir().join("progetti.json")).unwrap_or(Value::Null);
    for p in d["progetti"].as_array().cloned().unwrap_or_default() {
        let mut add = |x: &str| for w in x.split(|c: char| !c.is_alphanumeric()) { let w = w.to_lowercase(); if w.chars().count() >= 4 && !matches!(w.as_str(), "juggl" | "studio" | "progetto" | "project" | "scale" | "garage" | "backstage" | "documents" | "workspace" | "users") { v.push(w); } };
        add(str_of(&p, "id")); add(str_of(&p, "nome"));
        if let Some(f) = Path::new(str_of(&p, "cartella")).file_name() { add(&f.to_string_lossy()); }
        for k in p["parole"].as_array().cloned().unwrap_or_default() { if let Some(k) = k.as_str() { add(k); } }
    }
    v.sort(); v.dedup(); v
}

fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back().map(|c| c.is_alphanumeric()).unwrap_or(false);
        let after = text[i + word.len()..].chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false);
        !before && !after
    })
}

fn translate_in_background() {
    if now_secs() - *TRANSLATION_FAILED.lock().unwrap_or_else(|e| e.into_inner()) < TRANSLATION_ERROR_PAUSE { return; }
    {
        let mut g = TRANSLATING.lock().unwrap_or_else(|e| e.into_inner());
        if *g { return; }
        *g = true;
    }
    std::thread::spawn(|| {
        if translate().is_none() { *TRANSLATION_FAILED.lock().unwrap_or_else(|e| e.into_inner()) = now_secs(); }
        *TRANSLATING.lock().unwrap_or_else(|e| e.into_inner()) = false;
    });
}

fn translate() -> Option<()> {
    let done = it_descriptions();
    let mut missing: Vec<(String, String, String)> = vec![];   // (key, name, description)
    let mut seen = HashSet::new();
    let reserved = reserved_words();
    for (_, v) in with_cache(|m| m.clone()) {
        for c in v["comandi"].as_array().cloned().unwrap_or_default() {
            let name = str_of(&c, "name").to_string();
            if name.is_empty() || system_desc_it(&name).is_some() || HIDDEN.contains(&name.as_str()) { continue; }
            let (d, source) = source_and_text(str_of(&c, "description"));
            if d.is_empty() || source == "progetto" { continue; }   // project skills stay on the Mac
            // client or project names (progetti.json) in the name or description: they do not leave, English stays
            let l = format!("{name} {d}").to_lowercase();
            if reserved.iter().any(|p| contains_word(&l, p)) { continue; }
            let k = it_key(&name, &d);
            if done.contains_key(&k) || !seen.insert(k.clone()) { continue; }
            missing.push((k, name, d.chars().take(300).collect()));
        }
    }
    if missing.is_empty() { return Some(()); }
    let claude = crate::profile::claude_cmd();
    // empty folder: no project CLAUDE.md in the prompt
    let empty_dir = commands_dir().join("vuota");
    let _ = std::fs::create_dir_all(&empty_dir);
    for group in missing.chunks(40) {
        let list: String = group.iter().enumerate().map(|(i, (_, n, d))| format!("{}. /{n}: {}\n", i + 1, d.replace('\n', " "))).collect();
        let prompt = format!("Per ogni comando qui sotto scrivi una descrizione brevissima in italiano semplice, che capisca anche chi non programma: massimo 55 caratteri, verbo alla terza persona all'inizio (Crea, Apre, Controlla), niente punto finale, niente virgolette. Scrivi per un utente qualsiasi: non nominare mai persone, clienti o progetti, anche se li conosci.\nRispondi SOLO con un oggetto JSON {{\"1\": \"descrizione\", \"2\": \"...\"}}, una voce per numero.\n\n{list}");
        use crate::NoConsoleWindow;
        let o = crate::new_command(&claude).no_console_window()
            .args(["-p", "--model", "haiku", "--setting-sources", "project,local", "--strict-mcp-config", "--tools", "", "--output-format", "text", &prompt])
            .env("TAVOLO_INTERNO", "1").current_dir(&empty_dir).stdin(Stdio::null()).output().ok()?;
        // limit reached, account expired, odd answer: stop here (retry in an hour)
        let text = String::from_utf8_lossy(&o.stdout);
        let (a, b) = (text.find('{')?, text.rfind('}')?);
        if b <= a { return None; }
        let v = serde_json::from_str::<Value>(&text[a..=b]).ok()?;
        let mut all = it_descriptions();
        for (i, (k, _, _)) in group.iter().enumerate() {
            let t = str_of(&v, &(i + 1).to_string()).trim().trim_end_matches('.').to_string();
            let l = t.to_lowercase();
            if t.is_empty() || t.chars().count() > 80 || l.contains(&crate::profile::with_name("«NOME»").to_lowercase()) || reserved.iter().any(|p| contains_word(&l, p)) { continue; }
            all.insert(k.clone(), json!(t));
        }
        let _ = std::fs::create_dir_all(commands_dir());
        let tmp = it_file().with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(&Value::Object(all)).unwrap_or_default()).is_ok() { let _ = std::fs::rename(&tmp, it_file()); }
    }
    crate::log_juggl(&format!("comandi: {} descrizioni in italiano", missing.len()));
    Some(())
}

// ---------------------------------------------------------------------------------------------------------------
// List for the menu

#[derive(Clone, Copy, PartialEq)]
enum ChatKind { Ospitata, Terminale, Codex, Locale }

fn kind_of(id: &str, st: &Value) -> ChatKind {
    if id.starts_with("locale:") { ChatKind::Locale }
    else if crate::engines::engine_of(st) == "codex" { ChatKind::Codex }
    else if crate::hosted::is_hosted(id) || str_of(st, "surface").is_empty() { ChatKind::Ospitata }
    else { ChatKind::Terminale }
}

fn state_of(id: &str) -> Value { if id.starts_with("locale:") { json!({}) } else { crate::find_session(id).unwrap_or(json!({})) } }

fn folder_of(st: &Value) -> String {
    let c = str_of(st, "cwd");
    if c.is_empty() || c == "/" { home().to_string_lossy().to_string() } else { c.to_string() }
}

fn juggl_entry(j: &J) -> Value {
    json!({"nome": j.nome, "alias": j.alias, "arg": j.arg, "desc": j.desc, "tipo": "juggl", "gruppo": "Juggl"})
}

/// Claude Code entries for the folder (without the ones Juggl handles and without internals).
fn claude_entries(cat: &Value) -> Vec<Value> {
    let it = it_descriptions();
    let done_by_juggl: HashSet<&str> = JUGGL.iter().flat_map(|j| j.alias.iter().copied()).collect();
    let mut out = vec![];
    let mut seen = HashSet::new();
    for c in cat["comandi"].as_array().cloned().unwrap_or_default() {
        let name = str_of(&c, "name").to_string();
        if name.is_empty() || HIDDEN.contains(&name.as_str()) || done_by_juggl.contains(name.as_str()) || NOT_HERE.iter().any(|(n, _)| *n == name) { continue; }
        if !seen.insert(name.to_lowercase()) { continue; }
        let (d, source) = source_and_text(str_of(&c, "description"));
        if d.starts_with("(removed)") || d.starts_with("Renamed to") { continue; }
        let is_builtin = c["builtin"].as_bool().unwrap_or(false) && source.is_empty();
        let desc = system_desc_it(&name).map(String::from)
            .or_else(|| it.get(&it_key(&name, &d)).and_then(|x| x.as_str()).map(String::from))
            .unwrap_or_else(|| short(&d));
        let alias: Vec<String> = c["aliases"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| !x.contains(':') || !name.contains(':')).filter(|x| *x != name).map(String::from).collect()).unwrap_or_default();
        let group = if is_builtin { "Claude Code" } else if source == "progetto" { "Del progetto" } else if name.contains(':') { "Altri comandi" } else { "Skill e comandi" };
        out.push(json!({"nome": name, "alias": alias, "arg": str_of(&c, "argumentHint"), "desc": desc, "tipo": if is_builtin { "claude" } else { "skill" }, "gruppo": group, "fonte": source}));
    }
    let order = |g: &str| match g { "Claude Code" => 0, "Del progetto" => 1, "Skill e comandi" => 2, _ => 3 };
    out.sort_by(|a, b| order(str_of(a, "gruppo")).cmp(&order(str_of(b, "gruppo"))).then_with(|| str_of(a, "nome").to_lowercase().cmp(&str_of(b, "nome").to_lowercase())));
    out
}

/// "/" menu entries for chat `id` (Mac: Tauri command; iPhone: GET /api/comandi?id=).
pub fn list(id: &str) -> Value {
    let st = state_of(id);
    let kind = kind_of(id, &st);
    let mut entries: Vec<Value> = JUGGL.iter().filter(|j| matches!(kind, ChatKind::Ospitata | ChatKind::Terminale) || JUGGL_EVERYWHERE.contains(&j.nome)).map(juggl_entry).collect();
    let mut loading = false;
    let mut note = String::new();
    match kind {
        ChatKind::Codex => note = "Chat Codex: qui ci sono solo i comandi di Juggl.".into(),
        ChatKind::Locale => note = "Chat col modello del PC: qui ci sono solo i comandi di Juggl.".into(),
        _ => {
            let cwd = folder_of(&st);
            match catalog(&cwd, false) {
                Some(c) => entries.extend(claude_entries(&c)),
                None => loading = true,
            }
        }
    }
    json!({"ok": true, "voci": entries, "caricamento": loading, "nota": note, "terminale": kind == ChatKind::Terminale})
}

// ---------------------------------------------------------------------------------------------------------------
// Before sending

pub enum Outcome { Passa, Gestito(Value) }

fn handled(command: &str, msg: &str, action: &str, extra: Value) -> Outcome {
    let mut v = json!({"ok": true, "esito": "gestito", "comando": command, "msg": msg, "azione": action});
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) { for (k, x) in e { o.insert(k.clone(), x.clone()); } }
    Outcome::Gestito(v)
}

fn find_juggl(name: &str) -> Option<&'static J> {
    JUGGL.iter().find(|j| j.nome == name).or_else(|| JUGGL.iter().find(|j| j.alias.contains(&name)))
}

fn model_from(a: &str) -> Option<&'static str> {
    Some(match a.trim().to_lowercase().as_str() {
        "opus" => "opus", "sonnet" => "sonnet", "haiku" => "haiku", "fable" => "fable",
        "predefinito" | "default" | "auto" | "normale" | "" => "",
        _ => return None,
    })
}

fn effort_from(a: &str) -> Option<&'static str> {
    Some(match a.trim().to_lowercase().replace(' ', "-").as_str() {
        "basso" | "low" | "poco" => "low", "medio" | "medium" => "medium", "alto" | "high" => "high",
        "molto-alto" | "moltoalto" | "xhigh" => "xhigh", "massimo" | "max" => "max",
        "predefinito" | "default" | "auto" | "" => "",
        _ => return None,
    })
}

fn model_name(m: &str) -> &str { match m { "" => "predefinito", "opus" => "Opus", "sonnet" => "Sonnet", "haiku" => "Haiku", "fable" => "Fable", x => x } }
/// Real name from the CLI list ("Fable 5.2", "Predefinito (Opus 5.5)"), otherwise the short name.
fn model_display_name(m: &str) -> String {
    models()["modelli"].as_array().and_then(|a| a.iter().find(|x| str_of(x, "v") == m).map(|x| str_of(x, "nome").to_string())).unwrap_or_else(|| model_name(m).to_string())
}
fn effort_name(e: &str) -> &str { match e { "" => "predefinito", "low" => "basso", "medium" => "medio", "high" => "alto", "xhigh" => "molto alto", "max" => "massimo", x => x } }

fn run_juggl(j: &J, typed: &str, id: &str, arg: &str, st: &Value, kind: ChatKind) -> Outcome {
    let cmd = format!("/{}", j.nome);
    let cwd = folder_of(st);
    match j.nome {
        "nuova" => handled(&cmd, if typed == "clear" || typed == "reset" || typed == "new" { "In Juggl /clear apre una chat nuova nello stesso progetto: questa resta com'è." } else { "" }, "nuova-chat", json!({"arg": arg, "cartella": cwd})),
        "modello" => {
            if kind == ChatKind::Locale || kind == ChatKind::Codex { return handled(&cmd, "Il modello di questa chat si cambia dal tasto del modello in alto.", "scegli-modello", json!({})); }
            if arg.is_empty() { return handled(&cmd, &format!("Modello attuale: {}. Scrivi /modello fable, opus, sonnet, haiku o predefinito (anche una versione, es. opus 4.8).", model_display_name(str_of(st, "modello"))), "scegli-modello", json!({})); }
            let m = match resolve_model(arg) { Some(m) => m, None => return handled(&cmd, &format!("Non conosco il modello «{arg}». Puoi scegliere fable, opus, sonnet, haiku o predefinito; l'elenco completo è nel tasto del modello."), "scegli-modello", json!({})) };
            let r = crate::regola(id.to_string(), m.clone(), str_of(st, "effort").to_string());
            let ok = r["ok"].as_bool().unwrap_or(false);
            handled(&cmd, &if ok { format!("Modello: {}. {}", model_display_name(&m), explain_setting(str_of(&r, "msg"))) } else { format!("Modello non cambiato: {}", str_of(&r, "msg")) }, "fatto", json!({"ok": ok, "aggiorna": true}))
        }
        "ragionamento" => {
            if kind == ChatKind::Locale || kind == ChatKind::Codex { return handled(&cmd, "Qui il ragionamento si regola dal tasto del modello in alto.", "scegli-modello", json!({})); }
            if arg.is_empty() { return handled(&cmd, &format!("Ragionamento attuale: {}. Scrivi /ragionamento basso, medio, alto, molto-alto, massimo o predefinito.", effort_name(str_of(st, "effort"))), "scegli-modello", json!({})); }
            let Some(e) = effort_from(arg) else { return handled(&cmd, &format!("Non conosco «{arg}». Puoi scegliere basso, medio, alto, molto-alto, massimo o predefinito."), "nota", json!({})) };
            let r = crate::regola(id.to_string(), str_of(st, "modello").to_string(), e.to_string());
            let ok = r["ok"].as_bool().unwrap_or(false);
            handled(&cmd, &if ok { format!("Ragionamento: {}. {}", effort_name(e), explain_setting(str_of(&r, "msg"))) } else { format!("Non cambiato: {}", str_of(&r, "msg")) }, "fatto", json!({"ok": ok, "aggiorna": true}))
        }
        "consumi" => handled(&cmd, "Quota e consumi sono nella barra della quota in fondo, e in Consumi.", "consumi", json!({})),
        "account" => handled(&cmd, "Per entrare o cambiare account Claude usa il pannello Account Claude.", "login", json!({})),
        "esci" => handled(&cmd, "Per uscire dall'account Claude usa il pannello Account Claude.", "logout", json!({})),
        "stato" => handled(&cmd, &status_text(id, st, kind), "nota", json!({})),
        "rinomina" => {
            if arg.is_empty() { return handled(&cmd, "Scrivi /rinomina e il nome nuovo.", "rinomina", json!({})); }
            if id.starts_with("locale:") { return handled(&cmd, "Questa chat si rinomina dal suo nome in alto.", "rinomina", json!({})); }
            let r = crate::rinomina(id.to_string(), arg.chars().take(80).collect());
            let ok = r["ok"].as_bool().unwrap_or(false);
            handled(&cmd, &if ok { format!("Rinominata: {}", arg.trim()) } else { format!("Non rinominata: {}", str_of(&r, "msg")) }, "fatto", json!({"ok": ok, "aggiorna": true}))
        }
        "sposta" => handled(&cmd, "Per spostarla usa Sposta nel menu della chat.", "sposta", json!({})),
        "riprendi" => handled(&cmd, "Le chat di prima sono in Archiviate, in fondo all'elenco, e nella ricerca.", "riprendi", json!({"arg": arg})),
        // width (16/11): the UI does it (ui/typography.js); here only the value check
        "larghezza" => {
            let v = arg.trim().to_lowercase();
            if !v.is_empty() && !matches!(v.as_str(), "stretta" | "normale" | "larga") { return handled(&cmd, "Scrivi /larghezza stretta, normale o larga (senza niente passa alla successiva).", "nota", json!({})); }
            handled(&cmd, "", "larghezza", json!({"arg": v}))
        }
        "eco" => {
            let current = crate::eco::is_on();
            let on = match arg.trim().to_lowercase().as_str() { "" => !current, "on" | "si" | "sì" | "accendi" | "accesa" | "1" => true, "off" | "no" | "spegni" | "spenta" | "0" => false, _ => return handled(&cmd, "Scrivi /eco on oppure /eco off (da solo la accende o la spegne).", "nota", json!({})) };
            let r = crate::eco_imposta(on, None);
            let n = r["cambiate"].as_u64().unwrap_or(0);
            let msg = if !r["ok"].as_bool().unwrap_or(false) { format!("Eco non cambiata: {}", str_of(&r, "msg")) }
                else if on { format!("Eco accesa{}. La spegni con /eco off.", if n > 0 { format!(": {n} chat su Sonnet") } else { String::new() }) }
                else { format!("Eco spenta{}.", if n > 0 { format!(": {n} chat tornano come prima") } else { String::new() }) };
            handled(&cmd, &msg, "fatto", json!({"ok": r["ok"].as_bool().unwrap_or(false), "aggiorna": true}))
        }
        "ferma" => {
            let r = crate::interrompi(id.to_string());
            handled(&cmd, if r["ok"].as_bool().unwrap_or(false) { "Fermata." } else { "Non c'era niente da fermare." }, "fatto", json!({"ok": true, "aggiorna": true}))
        }
        "archivia" => handled(&cmd, "Archiviare questa chat? Resta tra le archiviate e la riprendi quando vuoi.", "archivia", json!({})),
        "cerca" => handled(&cmd, "Per cercare nella chat usa ⌘F.", "cerca", json!({"arg": arg})),
        "richieste" => handled(&cmd, "Le richieste sono nella vista Richieste (⌘R).", "richieste", json!({})),
        "scorciatoie" => handled(&cmd, "Passare da una chat all'altra: ⌘[ e ⌘] indietro e avanti tra le chat aperte (come in Safari); ⌃Tab e ⌃⇧Tab chat successiva e precedente nell'ordine dell'elenco. Sono anche nel menu Vista. Chat nuova: ⌘N (nella finestra ⌘P cambia il progetto, ⌘Invio apre una chat nuova anche se ce n'è una simile).", "", json!({})),
        "permessi" => handled(&cmd, "I permessi delle chat sono in Impostazioni, alla voce Permessi.", "permessi", json!({})),
        "memoria" => {
            let project = Path::new(&cwd).join("CLAUDE.md");
            let global = home().join(".claude").join("CLAUDE.md");
            let files: Vec<String> = [project, global].iter().filter(|p| p.exists()).map(|p| p.to_string_lossy().to_string()).collect();
            handled(&cmd, "La memoria di Claude sta nei file CLAUDE.md: quello del progetto e quello in ~/.claude, che vale per tutti i progetti. Per aggiungere una cosa basta chiederla alla chat.", "memoria", json!({"file": files}))
        }
        "impostazioni" => handled(&cmd, "", "impostazioni", json!({"arg": arg})),   // settings as a chat (16/11): /impostazioni <phrase>
        _ => handled(&cmd, "", "aiuto", json!({})),
    }
}

fn explain_setting(m: &str) -> String {
    match m { "Applicato" => "La chat riparte con tutta la conversazione.".into(), "" => String::new(), x => format!("{x}.").replace("..", ".") }
}

fn status_text(id: &str, st: &Value, kind: ChatKind) -> String {
    let a = crate::claude_login::status();
    let acc = &a["account"];
    let plan = match str_of(acc, "piano").to_lowercase().as_str() { "max_20x" => "Max 20x".to_string(), "max_5x" => "Max 5x".into(), "max" => "Max".into(), "pro" => "Pro".into(), "team" => "Team".into(), "enterprise" => "Enterprise".into(), "free" => "Gratis".into(), _ => str_of(acc, "piano").to_string() };
    let account = if acc["collegato"].as_bool().unwrap_or(false) { format!("{}{}", str_of(acc, "email"), if plan.is_empty() { String::new() } else { format!(" ({plan})") }) } else { "nessuno collegato".into() };
    let mut lines = vec![format!("Account Claude: {account}")];
    match kind {
        ChatKind::Codex => lines.push("Motore: Codex".into()),
        ChatKind::Locale => lines.push("Motore: modello del PC dello studio".into()),
        _ => {
            lines.push(format!("Modello: {} · ragionamento {}", model_display_name(str_of(st, "modello")), effort_name(str_of(st, "effort"))));
            if let Some(v) = with_cache(|m| m.get(&folder_of(st)).cloned()) { let ver = str_of(&v, "versione"); if !ver.is_empty() { lines.push(format!("Claude Code {ver}")); } }
        }
    }
    lines.push(format!("Cartella: {}", folder_of(st).replace(&home().to_string_lossy().to_string(), "~")));
    lines.push(if kind == ChatKind::Terminale { "Gira nel terminale".into() } else { "Gira dentro Juggl".into() });
    if crate::eco::is_on() { lines.push("Eco accesa".into()); }
    let _ = id;
    lines.join("\n")
}

/// Distance between two names (for "did you mean").
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut r: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = r[0]; r[0] = i;
        for j in 1..=b.len() {
            let t = r[j];
            r[j] = (r[j] + 1).min(r[j - 1] + 1).min(prev + if a[i - 1] == b[j - 1] { 0 } else { 1 });
            prev = t;
        }
    }
    r[b.len()]
}

fn similar(name: &str, names: &[String]) -> Vec<String> {
    let mut v: Vec<(usize, &String)> = names.iter().filter_map(|n| {
        let l = n.to_lowercase();
        let d = if l.starts_with(name) { 0 } else if l.contains(name) { 1 } else { distance(name, &l) + 1 };
        (d <= 3).then_some((d, n))
    }).collect();
    v.sort_by_key(|x| (x.0, x.1.len()));
    let mut seen = HashSet::new();
    v.into_iter().filter(|x| seen.insert(x.1.to_lowercase())).take(3).map(|x| format!("/{}", x.1)).collect()
}

/// Called by `invia` for every message. Passa = sent as is; Riscrivi = sent with another text; Gestito = not sent.
pub fn before_send(id: &str, text: &str) -> Outcome {
    let Some(name) = crate::claude_login::command_name(text) else { return Outcome::Passa };
    // "/tmp", "/Applications guarda qui": a path that exists on disk is text, not a command
    if let Some(first) = text.trim_start().split_whitespace().next() { if first.len() > 1 && Path::new(first).exists() && find_juggl(&name).is_none() { return Outcome::Passa; } }
    let rest = {
        let t = text.trim_start().trim_start_matches('/');
        t.split_once(char::is_whitespace).map(|x| x.1.trim()).unwrap_or("").to_string()
    };
    let st = state_of(id);
    let kind = kind_of(id, &st);
    // Juggl commands. In a real terminal the Claude Code names (model, clear, login...) are handled by Claude Code itself.
    if let Some(j) = find_juggl(&name) {
        let is_alias = j.nome != name;
        if !(kind == ChatKind::Terminale && is_alias) {
            if matches!(kind, ChatKind::Codex | ChatKind::Locale) && !JUGGL_EVERYWHERE.contains(&j.nome) && !matches!(j.nome, "modello" | "ragionamento") {
                return handled(&format!("/{}", j.nome), "Qui questo comando non c'è: vale solo nelle chat di Claude.", "nota", json!({}));
            }
            return run_juggl(j, &name, id, &rest, &st, kind);
        }
    }
    match kind {
        ChatKind::Terminale => return Outcome::Passa,
        ChatKind::Codex | ChatKind::Locale => {
            let place = if kind == ChatKind::Codex { "Codex" } else { "col modello del PC" };
            return handled(&format!("/{name}"), &format!("Nelle chat {place} i comandi di Claude Code non ci sono. Scrivi / per vedere quelli di Juggl."), "nota", json!({}));
        }
        ChatKind::Ospitata => {}
    }
    if let Some((_, msg)) = NOT_HERE.iter().find(|(n, _)| *n == name) { return handled(&format!("/{name}"), msg, "nota", json!({})); }
    let Some(cat) = catalog(&folder_of(&st), true) else {
        // list not readable (Claude Code does not start?): better let it through than block a real skill
        return Outcome::Passa;
    };
    let commands = cat["comandi"].as_array().cloned().unwrap_or_default();
    let found = commands.iter().find(|c| str_of(c, "name").eq_ignore_ascii_case(&name) || c["aliases"].as_array().map(|a| a.iter().any(|x| x.as_str().map(|x| x.eq_ignore_ascii_case(&name)).unwrap_or(false))).unwrap_or(false));
    if let Some(c) = found {
        if HIDDEN.contains(&str_of(c, "name")) { return handled(&format!("/{name}"), &format!("/{name} non funziona dentro Juggl."), "nota", json!({})); }
        return Outcome::Passa;
    }
    // unknown: must never reach the agent
    let mut names: Vec<String> = JUGGL.iter().flat_map(|j| std::iter::once(j.nome.to_string()).chain(j.alias.iter().filter(|a| **a != "?").map(|a| a.to_string()))).collect();
    names.extend(commands.iter().map(|c| str_of(c, "name").to_string()).filter(|n| !HIDDEN.contains(&n.as_str())));
    let sim = similar(&name, &names);
    let msg = if sim.is_empty() { format!("Il comando /{name} non c'è. Scrivi / per vedere tutti i comandi; se volevi scrivere un testo che inizia con la barra, mettici uno spazio davanti.") }
        else { format!("Il comando /{name} non c'è. Forse cercavi {}.", sim.join(", ")) };
    handled(&format!("/{name}"), &msg, "sconosciuto", json!({"simili": sim, "testo": text}))   // the text goes back into the input box
}

/// For the menu: the same check, but also says when the text can be sent (outcome "passa").
pub fn run(id: &str, text: &str) -> Value {
    match before_send(id, text) {
        Outcome::Passa => json!({"ok": true, "esito": "passa", "testo": text}),
        Outcome::Gestito(v) => v,
    }
}

/// GB of free Mac memory (same measure as the Mac panel).
fn free_gb() -> f64 {
    let m = crate::session_care::mac_memory();
    m["totale_gb"].as_f64().unwrap_or(0.0) * m["libera_pct"].as_f64().unwrap_or(100.0) / 100.0
}

/// At app start: prepares the list for open chat folders that do not have it (or have it older than 24 hours).
/// One at a time, 10 s apart, only with more than 1 GB free (coordinator 13/11: no memory spikes).
pub fn prepare(folders: Vec<String>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(45));
        let mut seen = HashSet::new();
        let mut done = 0;
        for c in folders {
            if done >= 8 || !seen.insert(c.clone()) { continue; }
            let age = with_cache(|m| m.get(&c).map(|v| now_secs() - v["t"].as_f64().unwrap_or(0.0)));
            if age.map(|e| e < LIFETIME).unwrap_or(false) { continue; }
            if free_gb() < 1.0 { crate::log_juggl("comandi: meno di 1 GB libero, elenchi rimandati al primo \"/\""); return; }
            if refresh_folder(&c).is_none() { return; }   // Claude Code does not answer: no more attempts now
            done += 1;
            std::thread::sleep(Duration::from_secs(10));
        }
    });
}

// ---------------------------------------------------------------------------------------------------------------
// models (14/11): full list of the account's models, from the same "initialize" handshake (models field)

/// Italian model descriptions (Claude Code gives them in English); unknown ones stay as they are.
fn model_desc_it(d: &str) -> String {
    let (head, rest) = match d.split_once(" · ") { Some((a, b)) => (Some(a), b), None => (None, d) };
    let it = match rest.trim() {
        "Best for everyday, complex tasks" => "Il migliore per il lavoro di ogni giorno e le cose complesse",
        "For complex work and everyday tasks" => "Per lavori complessi e di tutti i giorni",
        "For your toughest challenges" => "Per i lavori più difficili",
        "Most efficient for simpler tasks" => "Il più efficiente per le cose semplici",
        "Fastest for quick answers" => "Il più veloce, per risposte rapide",
        "Most capable for your hardest and longest-running tasks" => "Il più capace, per lavori difficili e lunghi",
        "Efficient for routine tasks" => "Efficiente per i lavori di routine",
        x => x,
    };
    match head { Some(p) => format!("{p} · {it}"), None => it.to_string() }
}

/// Aliases that Claude Code always keeps on the latest version: these are the "main" ones.
const MAIN_ALIASES: &[&str] = &["default", "fable", "opus", "sonnet", "haiku"];

fn models_from(cat: &Value) -> Vec<Value> {
    let mut out = vec![];
    for m in cat["modelli"].as_array().cloned().unwrap_or_default() {
        let v = str_of(&m, "value").to_string();
        if v.is_empty() { continue; }
        let is_main = MAIN_ALIASES.contains(&v.as_str());
        let efforts: Vec<Value> = if m["supportsEffort"].as_bool().unwrap_or(false) { m["supportedEffortLevels"].as_array().cloned().unwrap_or_default() } else { vec![] };
        let name = if v == "default" { format!("Predefinito ({})", str_of(&m, "description").split(" · ").next().unwrap_or("")) } else { str_of(&m, "displayName").to_string() };
        out.push(json!({
            "v": if v == "default" { String::new() } else { v.clone() },   // "" = like regola(): no --model
            "nome": name, "corto": if v == "default" { str_of(&m, "description").split(" · ").next().unwrap_or("Predefinito").to_string() } else { str_of(&m, "displayName").to_string() },
            "desc": model_desc_it(str_of(&m, "description")), "risolto": str_of(&m, "resolvedModel"),
            "sforzi": efforts, "principale": is_main,
        }));
    }
    out
}

/// Account models (Mac: Tauri command; iPhone: GET /api/modelli). Takes the most recent list in memory;
/// if there is none, asks Claude Code in the home folder (handshake, no tokens).
pub fn models() -> Value {
    let recent = with_cache(|m| m.values().filter(|v| v["modelli"].as_array().map(|a| !a.is_empty()).unwrap_or(false)).max_by(|a, b| a["t"].as_f64().unwrap_or(0.0).total_cmp(&b["t"].as_f64().unwrap_or(0.0))).cloned());
    let cat = recent.or_else(|| catalog(&home().to_string_lossy(), true));
    match cat {
        Some(c) => { let v = models_from(&c); json!({"ok": !v.is_empty(), "modelli": v, "versione": str_of(&c, "versione")}) }
        None => json!({"ok": false, "modelli": [], "msg": "Claude Code non ha dato l'elenco dei modelli"}),
    }
}

/// "/modello X": alias, exact CLI value or displayed name ("opus 4.8", "Fable 5.1"); None if not found.
fn resolve_model(arg: &str) -> Option<String> {
    if let Some(m) = model_from(arg) { return Some(m.to_string()); }
    let a = arg.trim().to_lowercase();
    let norm = |x: &str| x.to_lowercase().replace([' ', '.', '_'], "-");
    let list = models();
    list["modelli"].as_array()?.iter().find(|m| {
        let v = str_of(m, "v");
        !v.is_empty() && (v.eq_ignore_ascii_case(&a) || norm(str_of(m, "corto")) == norm(&a) || norm(v).trim_start_matches("claude-") == norm(&a))
    }).map(|m| str_of(m, "v").to_string())
}

#[tauri::command(async)]
pub fn modelli_claude() -> Value { models() }

#[tauri::command(async)]
pub fn comandi_elenco(id: String) -> Value { list(&id) }
#[tauri::command(async)]
pub fn comando_esegui(id: String, testo: String) -> Value { run(&id, &testo) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_from_description() {
        assert_eq!(source_and_text("Fa una cosa. (user)"), ("Fa una cosa.".into(), "tue"));
        assert_eq!(source_and_text("Fa una cosa (plugin:vercel)"), ("Fa una cosa".into(), "plugin"));
        assert_eq!(source_and_text("Fa (cose) bene"), ("Fa (cose) bene".into(), ""));
        assert_eq!(source_and_text("Ricerca (dynamic workflow)"), ("Ricerca".into(), ""));
    }

    #[test]
    fn juggl_and_aliases() {
        assert_eq!(find_juggl("login").unwrap().nome, "account");
        assert_eq!(find_juggl("clear").unwrap().nome, "nuova");
        assert_eq!(find_juggl("usage").unwrap().nome, "consumi");
        assert_eq!(find_juggl("cost").unwrap().nome, "consumi");
        assert_eq!(find_juggl("?").unwrap().nome, "aiuto");
        assert!(find_juggl("compact").is_none());
        assert!(find_juggl("code-review").is_none());
        // no duplicate alias
        let mut v: Vec<&str> = JUGGL.iter().flat_map(|j| std::iter::once(j.nome).chain(j.alias.iter().copied())).collect();
        let n = v.len(); v.sort(); v.dedup(); assert_eq!(v.len(), n);
    }

    #[test]
    fn models_and_efforts() {
        assert_eq!(model_from("Sonnet"), Some("sonnet"));
        assert_eq!(model_from("predefinito"), Some(""));
        assert_eq!(model_from("gpt"), None);
        assert_eq!(effort_from("molto alto"), Some("xhigh"));
        assert_eq!(effort_from("basso"), Some("low"));
        assert_eq!(effort_from("tanto"), None);
    }

    #[test]
    fn did_you_mean() {
        let n: Vec<String> = ["modello", "model", "compact", "context", "code-review"].iter().map(|x| x.to_string()).collect();
        assert!(similar("modelo", &n).contains(&"/modello".to_string()));
        assert!(similar("mod", &n).contains(&"/model".to_string()));
        assert!(similar("zzzzzzzz", &n).is_empty());
        let dupes: Vec<String> = ["model", "model", "modello"].iter().map(|x| x.to_string()).collect();
        assert_eq!(similar("modelo", &dupes).len(), 2);
    }

    /// Real test (launches Claude Code and the small model): JUGGL_BASE=<test folder> CARTELLA=<cwd> OUT=<file.json>
    /// cargo test --bin tavolo commands::tests::real_list -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_list() {
        let cwd = std::env::var("CARTELLA").unwrap_or_else(|_| crate::base_dir().to_string_lossy().to_string())   /* rename (14/11) */;
        let t0 = std::time::Instant::now();
        let c = catalog(&cwd, true).expect("elenco di Claude Code");
        eprintln!("elenco in {:?}: {} comandi, Claude Code {}", t0.elapsed(), c["comandi"].as_array().map(|a| a.len()).unwrap_or(0), str_of(&c, "versione"));
        let t1 = std::time::Instant::now();
        translate();
        eprintln!("descrizioni in {:?}", t1.elapsed());
        let mut entries: Vec<Value> = JUGGL.iter().map(juggl_entry).collect();
        entries.extend(claude_entries(&c));
        let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp/elenco-comandi.json".into());
        std::fs::write(&out, serde_json::to_vec_pretty(&json!({"ok": true, "voci": entries, "caricamento": false, "nota": ""})).unwrap()).unwrap();
        // again: it must come from memory, right away
        let t2 = std::time::Instant::now();
        assert!(catalog(&cwd, false).is_some());
        assert!(t2.elapsed() < Duration::from_millis(200));
    }

    #[test]
    #[cfg(unix)]   // /tmp and /Applications only exist on Mac (and /tmp on Linux)
    fn paths_pass_through() {
        assert!(matches!(before_send("id-che-non-esiste", "/tmp"), Outcome::Passa));
        #[cfg(target_os = "macos")]
        assert!(matches!(before_send("id-che-non-esiste", "/Applications guarda qui"), Outcome::Passa));
        assert!(matches!(before_send("id-che-non-esiste", "/home/x/y e /tmp/a"), Outcome::Passa));
    }

    #[test]
    fn models_from_cli() {
        let cat = json!({"modelli": [
            {"value": "default", "resolvedModel": "claude-opus-5-5", "displayName": "Default (recommended)", "description": "Opus 5.5 · Best for everyday, complex tasks", "supportsEffort": true, "supportedEffortLevels": ["low", "max"]},
            {"value": "fable", "resolvedModel": "claude-fable-5-2", "displayName": "Fable 5.2", "description": "For your toughest challenges", "supportsEffort": true, "supportedEffortLevels": ["low"]},
            {"value": "claude-haiku-4-5-20251001", "displayName": "Haiku 4.5", "description": "Fastest for quick answers", "supportsThinkingOff": true}]});
        let m = models_from(&cat);
        assert_eq!(m.len(), 3);
        assert_eq!(str_of(&m[0], "v"), ""); assert_eq!(str_of(&m[0], "nome"), "Predefinito (Opus 5.5)"); assert_eq!(str_of(&m[0], "corto"), "Opus 5.5");
        assert_eq!(str_of(&m[1], "desc"), "Per i lavori più difficili"); assert!(m[1]["principale"].as_bool().unwrap());
        assert!(!m[2]["principale"].as_bool().unwrap()); assert!(m[2]["sforzi"].as_array().unwrap().is_empty());
    }

    #[test]
    fn whole_words() {
        assert!(contains_word("deploy di alfa sul server", "alfa"));
        assert!(!contains_word("alfaapp", "alfa"));
        assert!(contains_word("/alfa-deploy: fa", "alfa"));
    }

    #[test]
    fn short_description() {
        assert_eq!(short("Fa una cosa. Poi un'altra."), "Fa una cosa.");
        assert!(short(&"a".repeat(200)).ends_with('…'));
    }
}
