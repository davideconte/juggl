// Automatic coordinator for each chat group (Nov 12, approved by the user).
//
// Group = the chats of the same project, as in the list (gruppiDi in index.html, "progetto" in main.rs).
// When a group has 3 or more open chats, Juggl opens by itself a chat "<group> · regia" in the project folder.
// The coordinator does no technical work: it reads the state of the group's chats and wakes up ONLY when something happens
// (a chat finishes its turn, stops on a permission, waits for the user, or wants files another chat is using).
// Never on a timer. Small model (haiku, low reasoning), no tools to modify files, no global memory.
//
// Pieces:
// - giro(): called by the Sentinel every 3 s with the list already computed. Creates the coordinators, compares the chats' state
//   with the previous pass (events), gathers them for a few seconds and sends them to the coordinator in one message when the coordinator
//   is free (if it is working they stay waiting). Writes the registry for the hook.
// - "who touches what" registry: ~/tavolo/regia/registro.json (who is in a group with a coordinator) + "cartelle" in each chat's
//   state (`juggl cartelle <dir...>`) + bin/cartelle.py as Claude Code's PreToolUse hook (Edit, Write, ...):
//   if the file is in a folder held by another working chat of the group, the edit is stopped and the coordinator is
//   notified (regia/avvisi.jsonl, read here).
// - per_elenco(): summary line per group ("2 wait for you: ..."), in the list and bridge data (iPhone).
// - impostazioni.json "regia_gruppi": {"<project>": false} turns off a group's coordinator (on by default).
// Local PC chats ("locale:") never enter: none of their content goes to the coordinator or the cloud.
use crate::{base_dir, f64_of, session_care, load_json, unix_now, str_of};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// From how many open chats in a group the coordinator is born.
pub const THRESHOLD: usize = 3;
/// Start of the automatic messages to the coordinator: so Juggl recognises them (no notices, no user requests).
pub const EVENTS_PREFIX: &str = "[Juggl · eventi";
/// Close events: wait for the group to be still for a few seconds and send them together.
const COLLECT_S: f64 = 4.0;
/// End of turn: wait (at most) for the pilot to say whether the chat needs the user, so everything arrives together.
const PILOT_WAIT_S: f64 = 25.0;
/// Creation failed (account, folder): retry after 10 minutes, not at every pass.
const RETRY_S: f64 = 600.0;
/// The same conflict is not repeated to the coordinator before 10 minutes.
const CONFLICT_REPEAT_S: f64 = 600.0;

fn coordinator_dir() -> PathBuf { base_dir().join("regia") }
pub fn registry_file() -> PathBuf { coordinator_dir().join("registro.json") }
fn notices_file() -> PathBuf { coordinator_dir().join("avvisi.jsonl") }
/// group -> id of the coordinator created (or adopted) by Juggl: used to tell whether the user deleted it.
fn coordinators_file() -> PathBuf { coordinator_dir().join("regie.json") }
fn state_file(id: &str) -> PathBuf { base_dir().join("stato").join(format!("{id}.json")) }

pub fn is_coordinator(v: &Value) -> bool { v.get("regia").and_then(|x| x.as_bool()).unwrap_or(false) }

/// Turn just finished that was an automatic Juggl message to the coordinator: no "waits for you" notice (autopilot.rs).
pub fn without_notice(v: &Value) -> bool { is_coordinator(v) && str_of(v, "chiesto").starts_with(EVENTS_PREFIX) }

/// Is the group's coordinator on? By default yes; "regia_gruppi": {"alfa": false} turns it off.
pub fn is_on(settings: &Value, gruppo: &str) -> bool { settings.get("regia_gruppi").and_then(|m| m.get(gruppo)).and_then(|x| x.as_bool()).unwrap_or(true) }

fn turn_of(v: &Value) -> &str { crate::turn_view(v) }

/// Does the chat count for its group? Open (hosted or in the terminal), never local, external messages, Juggl, coordinator, tests.
pub fn counts(v: &Value, juggl: &str) -> bool {
    let id = str_of(v, "sessione");
    let cwd = str_of(v, "cwd").trim_end_matches('/');
    !id.is_empty() && id != juggl && !id.starts_with("locale:") && !id.starts_with("wa:") && !id.starts_with("chiusa:")
        && !is_coordinator(v) && matches!(str_of(v, "vista"), "bloccata" | "aspetta" | "lavora" | "fatta" | "ferma")
        && !cwd.ends_with("/tavolo/prova") && !cwd.ends_with("generale-juggl")
}

#[derive(Clone, Debug, Default)]
pub struct Group {
    pub id: String,
    pub nome: String,
    /// project folder (where the coordinator is born)
    pub cartella: String,
    /// chats that count, in list order
    pub membri: Vec<Value>,
    /// the group's coordinator, if it is in the list (even hibernated)
    pub regia: Option<Value>,
    /// chat already named "<group> · regia" by hand (e.g. Juggl's manual coordinator): adopted instead of creating another
    pub candidata: Option<Value>,
}

fn coordinator_name(gruppo: &str) -> String { format!("{gruppo} · regia") }

fn is_candidate(v: &Value, gruppo: &str) -> bool {
    let n = str_of(v, "nome_manuale").trim().to_lowercase();
    !n.is_empty() && !gruppo.is_empty() && (n == coordinator_name(gruppo).to_lowercase() || n == format!("{} - regia", gruppo.to_lowercase()))
}

/// The groups from the sessions list (the one from sessioni() / dati_tavolo, with "vista" and "progetto").
pub fn groups(ss: &[Value], juggl: &str) -> BTreeMap<String, Group> {
    let mut out: BTreeMap<String, Group> = BTreeMap::new();
    for v in ss {
        let id = str_of(v, "sessione");
        if id.is_empty() || id.starts_with("locale:") || id == juggl { continue; }
        let p = v.get("progetto").cloned().unwrap_or(Value::Null);
        let gid = if is_coordinator(v) && !str_of(v, "regia_gruppo").is_empty() { str_of(v, "regia_gruppo").to_string() } else { str_of(&p, "id").to_string() };
        if gid.is_empty() || gid == "locale" { continue; }
        let g = out.entry(gid.clone()).or_insert_with(|| Group { id: gid.clone(), ..Default::default() });
        if g.nome.is_empty() { g.nome = if !str_of(&p, "nome").is_empty() { str_of(&p, "nome").to_string() } else { str_of(v, "regia_nome").to_string() }; }
        if g.cartella.is_empty() && !str_of(&p, "cartella").is_empty() { g.cartella = session_care::expand_path(str_of(&p, "cartella")); }
        if is_coordinator(v) {
            // one coordinator per group: if by mistake there are two, the oldest (born first) wins
            if g.regia.as_ref().map(|r| f64_of(r, "regia_nata") > f64_of(v, "regia_nata")).unwrap_or(true) { g.regia = Some(v.clone()); }
        } else if counts(v, juggl) {
            if g.candidata.is_none() && is_candidate(v, &g.nome) { g.candidata = Some(v.clone()); } else { g.membri.push(v.clone()); }
        }
    }
    for g in out.values_mut() {
        // project without folder: the one where the first chat started
        if g.cartella.is_empty() || !Path::new(&g.cartella).is_dir() {
            if let Some(m) = g.membri.iter().find(|m| Path::new(str_of(m, "cwd_origine")).is_dir()) { g.cartella = str_of(m, "cwd_origine").to_string(); }
        }
        if g.nome.is_empty() { g.nome = crate::path_tail(&g.cartella); }
    }
    out
}

/// Name of the chat without "Project · " in front (like nomeCorto in the interface).
fn short_name(v: &Value, gruppo: &str) -> String {
    let n = str_of(v, "nome").to_string();
    if !gruppo.is_empty() && n.to_lowercase().starts_with(&gruppo.to_lowercase()) {
        let r = n[gruppo.len()..].trim_start_matches(|c: char| c.is_whitespace() || "·-:|".contains(c)).trim().to_string();
        if !r.is_empty() { return r; }
    }
    n
}

fn truncate_text(t: &str, n: usize) -> String {
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= n { t } else { format!("{}…", t.chars().take(n.saturating_sub(1)).collect::<String>()) }
}

fn link(v: &Value) -> String { format!("[{}]({})", str_of(v, "nome").replace(['[', ']'], ""), format!("juggl://sessione/{}", str_of(v, "sessione"))) }

// ---------- summary line (Mac and iPhone) ----------

/// Who in the group waits for the user: stopped on a permission, or finished with "needs you" (pilot). The coordinator itself too.
fn waiting(g: &Group) -> Vec<Value> {
    g.regia.iter().chain(g.membri.iter()).filter(|v| {
        let t = turn_of(v);
        t == "bloccata" || (t == "aspetta" && v.get("serve_te").and_then(|x| x.as_bool()) == Some(true))
    }).map(|v| {
        let what = if !str_of(v, "serve_te_cosa").trim().is_empty() { str_of(v, "serve_te_cosa").trim().to_string() }
            else if turn_of(v) == "bloccata" { format!("permesso a {}", short_name(v, &g.nome)) } else { short_name(v, &g.nome) };
        json!({"id": str_of(v, "sessione"), "nome": short_name(v, &g.nome), "cosa": truncate_text(&what, 48)})
    }).collect()
}

/// "2 wait for you: App Store privacy, voice test" (empty if nobody waits).
pub fn digest(list: &[Value]) -> String {
    if list.is_empty() { return String::new(); }
    let things: Vec<String> = list.iter().take(3).map(|x| str_of(x, "cosa").to_string()).collect();
    let others = list.len().saturating_sub(3);
    format!("{} {} te: {}{}", list.len(), if list.len() == 1 { "aspetta" } else { "aspettano" }, things.join(", "),
        if others > 0 { format!(" e altre {others}") } else { String::new() })
}

/// For the list (dati_tavolo, so also /api/tavolo for the iPhone): one object per group with the coordinator or with at least
/// SOGLIA chats. {nome, regia, regia_vista, accesa, n, aspettano: [{id, nome, cosa}], sintesi}
pub fn for_list(ss: &[Value]) -> Value {
    let juggl = crate::juggl_id();
    let settings = session_care::settings();
    let mut out = Map::new();
    for (gid, g) in groups(ss, &juggl) {
        if g.regia.is_none() && g.membri.len() < THRESHOLD { continue; }
        let a = waiting(&g);
        out.insert(gid.clone(), json!({
            "nome": g.nome, "regia": g.regia.as_ref().map(|r| str_of(r, "sessione").to_string()),
            "regia_vista": g.regia.as_ref().map(|r| str_of(r, "vista").to_string()), "accesa": is_on(&settings, &gid),
            "n": g.membri.len(), "sintesi": digest(&a), "aspettano": a,
        }));
    }
    Value::Object(out)
}

// ---------- events ----------

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub t: f64,
    pub sessione: String,
    /// "finito", "bloccata", "serve_te", "conflitto", "nascita"
    pub tipo: String,
    /// key against duplicates (kind, session, turn start)
    pub chiave: String,
    /// ready-made text (conflicts)
    pub testo: String,
}

#[derive(Default)]
pub struct Round {
    partito: bool,
    /// id -> (turn, needs you, turn start)
    prima: HashMap<String, (String, bool, f64)>,
    /// group -> events to send to the coordinator
    pub attesa: HashMap<String, Vec<Event>>,
    /// key -> when it was sent (no duplicates)
    mandati: HashMap<String, f64>,
    /// group -> last attempt to create the coordinator
    tentativi: HashMap<String, f64>,
    registro: String,
    hook_controllato: bool,
}

impl Round {
    fn already(&self, chiave: &str, gruppo: &str) -> bool {
        self.mandati.contains_key(chiave) || self.attesa.get(gruppo).map(|l| l.iter().any(|e| e.chiave == chiave)).unwrap_or(false)
    }

    /// Compares the current state with the previous pass and queues the new events of the `live` groups
    /// (coordinator on, existing and with at least SOGLIA chats). The first pass only looks.
    pub fn observe(&mut self, gg: &BTreeMap<String, Group>, live: &[String], now: f64) {
        for (gid, g) in gg {
            for v in &g.membri {
                let id = str_of(v, "sessione").to_string();
                let t = turn_of(v).to_string();
                let st = v.get("serve_te").and_then(|x| x.as_bool()).unwrap_or(false);
                let since = f64_of(v, "da");
                let prima = self.prima.insert(id.clone(), (t.clone(), st, since));
                let Some((pt, pst, _)) = prima else { continue };
                if !self.partito || !live.contains(gid) { continue; }
                let mut new_events = vec![];
                if pt == "lavora" && matches!(t.as_str(), "aspetta" | "fatta" | "ferma") { new_events.push(("finito", format!("finito:{id}:{since}"))); }
                if t == "bloccata" && pt != "bloccata" { new_events.push(("bloccata", format!("bloccata:{id}:{since}"))); }
                // "needs you" decided after the end of the turn: if the end of turn is still waiting it goes into it
                if st && !pst {
                    let pending = self.attesa.get(gid).map(|l| l.iter().any(|e| e.sessione == id && e.tipo == "finito")).unwrap_or(false);
                    if !pending { new_events.push(("serve_te", format!("serve_te:{id}:{since}"))); }
                }
                for (tipo, chiave) in new_events {
                    if self.already(&chiave, gid) { continue; }
                    self.attesa.entry(gid.clone()).or_default().push(Event { t: now, sessione: id.clone(), tipo: tipo.into(), chiave, testo: String::new() });
                }
            }
        }
        // chats gone from the list: also removed from memory
        let present: std::collections::HashSet<&str> = gg.values().flat_map(|g| g.membri.iter().map(|v| str_of(v, "sessione"))).collect();
        self.prima.retain(|k, _| present.contains(k.as_str()));
        self.mandati.retain(|_, t| now - *t < 86400.0);
        self.partito = true;
    }

    /// Conflict reported by the hook or by `juggl cartelle` (a line of regia/avvisi.jsonl).
    pub fn conflict(&mut self, gg: &BTreeMap<String, Group>, a: &Value, now: f64) {
        let gid = str_of(a, "gruppo").to_string();
        let Some(g) = gg.get(&gid) else { return };
        let (sid, theirs) = (str_of(a, "sessione"), str_of(a, "tenuta_da"));
        let what = if !str_of(a, "cartella").is_empty() { str_of(a, "cartella") } else { str_of(a, "file") };
        let chiave = format!("conflitto:{sid}:{theirs}:{what}");
        if self.mandati.get(&chiave).map(|t| now - *t < CONFLICT_REPEAT_S).unwrap_or(false) || self.already(&chiave, &gid) { return; }
        self.mandati.remove(&chiave);
        let find_chat = |id: &str, nome: &str| g.membri.iter().find(|v| str_of(v, "sessione") == id).map(link)
            .unwrap_or_else(|| format!("[{}](juggl://sessione/{id})", if nome.is_empty() { "chat" } else { nome }));
        let action = if str_of(a, "tipo") == "dichiarazione" { format!("voleva dichiarare la cartella {what}") } else { format!("voleva modificare {}", str_of(a, "file")) };
        let testo = format!("Conflitto: {} {action}, ma {} e' tenuta da {}, che sta lavorando. Juggl l'ha fermata.",
            find_chat(sid, str_of(a, "nome")), if str_of(a, "cartella").is_empty() { "quel file".to_string() } else { format!("la cartella {}", str_of(a, "cartella")) }, find_chat(theirs, str_of(a, "tenuta_nome")));
        self.attesa.entry(gid).or_default().push(Event { t: now, sessione: sid.to_string(), tipo: "conflitto".into(), chiave, testo });
    }

    /// Groups with events ready to send: all ready (the pilot decided or enough time passed) and no
    /// new event for COLLECT_S seconds.
    pub fn ready(&self, gg: &BTreeMap<String, Group>, now: f64) -> Vec<String> {
        let mut out = vec![];
        for (gid, ev) in &self.attesa {
            if ev.is_empty() { continue; }
            let last = ev.iter().map(|e| e.t).fold(0.0, f64::max);
            if now - last < COLLECT_S { continue; }
            let ok = ev.iter().all(|e| {
                if e.tipo != "finito" || now - e.t >= PILOT_WAIT_S { return true; }
                let Some(v) = gg.get(gid).and_then(|g| g.membri.iter().find(|v| str_of(v, "sessione") == e.sessione)) else { return true };
                // the pilot already looked at this turn, or the chat no longer waits (restarted or read)
                (f64_of(v, "pilota_su_da") == f64_of(v, "da") && v.get("serve_te").is_some()) || turn_of(v) != "aspetta"
            });
            if ok { out.push(gid.clone()); }
        }
        out.sort();
        out
    }

    /// Removes the group's events from the queue and marks them as sent.
    pub fn take(&mut self, gid: &str, now: f64) -> Vec<Event> {
        let ev = self.attesa.remove(gid).unwrap_or_default();
        for e in &ev { self.mandati.insert(e.chiave.clone(), now); }
        ev
    }
}

/// The message for the coordinator: one line per event, with the chat link and the current state.
pub fn compose(g: &Group, ev: &[Event]) -> String {
    let mut lines = vec![format!("{EVENTS_PREFIX} del gruppo {}]", g.nome)];
    // one line per chat: end of turn and "needs you" of the same chat go together
    let mut done: Vec<String> = vec![];
    for e in ev {
        if e.tipo == "conflitto" { lines.push(format!("- {}", e.testo)); continue; }
        if done.contains(&e.sessione) { continue; }
        done.push(e.sessione.clone());
        let Some(v) = g.membri.iter().find(|v| str_of(v, "sessione") == e.sessione) else { continue };
        let kinds: Vec<&str> = ev.iter().filter(|x| x.sessione == e.sessione && x.tipo != "conflitto").map(|x| x.tipo.as_str()).collect();
        let t = turn_of(v);
        let mut r = format!("- {}: ", link(v));
        if t == "bloccata" || (kinds.contains(&"bloccata") && !kinds.contains(&"finito")) {
            r.push_str(&format!("e' ferma su un permesso: «{}»", truncate_text(str_of(v, "domanda"), 160)));
        } else {
            r.push_str(if kinds.contains(&"finito") { "ha finito il turno" } else { "ha un aggiornamento" });
            if v.get("serve_te").and_then(|x| x.as_bool()) == Some(true) {
                let what = str_of(v, "serve_te_cosa").trim();
                r.push_str(&if what.is_empty() { " e aspetta «NOME»".to_string() } else { format!(" e aspetta «NOME»: «{}»", truncate_text(what, 80)) });
            }
            if t == "lavora" { r.push_str(" (adesso lavora di nuovo)"); }
            let reply = str_of(v, "risposta");
            if !reply.trim().is_empty() { r.push_str(&format!(". Ultima risposta: «{}»", truncate_text(reply, 220))); }
        }
        lines.push(r);
    }
    lines.push("(Messaggio automatico di Juggl, non di «NOME». Rispondi come dicono le tue istruzioni: breve.)".into());
    crate::profile::with_name(&lines.join("\n"))
}

/// First message of the newly born coordinator.
pub fn birth_message(g: &Group) -> String {
    let who = g.membri.iter().map(link).collect::<Vec<_>>().join(", ");
    crate::profile::with_name(&format!("{EVENTS_PREFIX} del gruppo {}] Sei appena nata: il gruppo ha {} chat aperte ({who}). \
Fai il punto per «NOME» in poche righe: chi lavora, chi aspetta lui e per cosa, cosa e' bloccato. Poi aspetta i prossimi eventi.", g.nome, g.membri.len()))
}

// ---------- instructions and start of the coordinator ----------

fn slug(t: &str) -> String { t.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect() }

/// Coordinator instructions (--append-system-prompt via the state's "contesto", hosted.rs). Plain Italian.
pub fn instructions(g: &Group) -> String {
    let juggl = base_dir().join("bin/juggl").to_string_lossy().to_string();
    let state_dir = base_dir().join("stato").to_string_lossy().to_string();
    crate::profile::with_name(&format!("Sei la REGIA del gruppo «{n}» in Juggl: la chat che tiene il quadro delle altre chat di questo gruppo \
(cartella del progetto: {c}). Non fai lavoro tecnico e non modifichi mai file: il lavoro lo fanno le chat del gruppo.

Come lavori
- Ti svegli solo quando Juggl ti manda un messaggio che inizia con \"{EVENTS_PREFIX}\": una chat del gruppo ha finito il turno, si e' fermata \
su un permesso, aspetta «NOME», oppure due chat volevano gli stessi file. Non controllare niente a tempo, niente sleep, niente attese.
- Per vedere lo stato usa solo: {j} sessioni (le chat del progetto «{n}»), {j} leggi <chat> [n], {j} cartelle --elenco, e i file {st}/<id>.json. \
Guarda solo le chat del tuo gruppo. Mai le chat con id che inizia con \"locale:\".
- Per smistare: {j} manda <chat> \"testo\" oppure {j} nuova {c} \"testo\" --nome \"{n} · <cosa>\". Quando passi un lavoro a una chat, \
chiedile di dichiarare prima le cartelle che toccherà con: {j} cartelle <cartella...>
- Messaggi di «NOME» senza destinatario: capisci a quale chat del gruppo spettano e passali con juggl manda (o apri una chat nuova se \
quella giusta non c'e'). Poi rispondi in una riga a chi li hai passati, con il link della chat ({j} link <chat>).

Cosa rispondi a ogni evento: poche righe, italiano semplice, niente codice, niente tabelle.
- Una chat aspetta «NOME»: una riga con cosa deve fare lui e il link della chat.
- Due chat volevano gli stessi file: la seconda e' gia' stata fermata da Juggl. Scrivile con juggl manda di aspettare che l'altra finisca \
(o di fare altro) e dillo a «NOME» in una riga.
- Una chat ha finito e non serve niente: rispondi solo \"ok\", al massimo una riga sul prossimo passo. Non disturbare «NOME» per cose \
che le chat sanno fare da sole.
- Quando serve una decisione di «NOME» scrivila in fondo esattamente cosi': \"Serve il tuo ok: ...\".
Non dare istruzioni tecniche dettagliate alle chat e non approvare al posto di «NOME» azioni verso clienti, soldi, produzione o cose irreversibili.",
        n = g.nome, c = g.cartella, j = juggl, st = state_dir))
}

/// Extra arguments for Claude when the chat is a coordinator (hosted.rs, avvia_con): few read-only tools,
/// no global memory nor MCP services (about 6k tokens per turn instead of 120k), with Juggl's hooks for the state.
pub fn extra_args(st: &Value) -> Vec<String> {
    if !is_coordinator(st) { return vec![]; }
    let mut a: Vec<String> = ["--tools", "Bash,Read,Grep,Glob", "--disallowedTools", "Edit,Write,NotebookEdit,MultiEdit",
        "--setting-sources", "project,local", "--strict-mcp-config"].iter().map(|x| x.to_string()).collect();
    // without the user settings Juggl's hooks (reply, title) would be missing: I pass them separately
    if let Some((py, _)) = crate::profile::python() {
        let script = crate::profile::script_hook();   // rename (Nov 14): the earlier bin/tavolo or bin/juggl-hook
        if script.is_file() {
            let q = |p: &Path| { let t = p.to_string_lossy().to_string(); if t.contains(' ') { format!("'{t}'") } else { t } };
            let cmd = format!("{} {} hook", q(&py), q(&script));
            let mut h = Map::new();
            for ev in ["UserPromptSubmit", "Stop", "Notification", "SessionStart", "SessionEnd"] {
                h.insert(ev.into(), json!([{"hooks": [{"type": "command", "command": cmd, "timeout": 10, "async": true}]}]));
            }
            a.extend(["--settings".to_string(), json!({"hooks": h}).to_string()]);
        }
    }
    a
}

/// What Juggl does on its own initiative: open a coordinator and write to it. In tests it is replaced with a fake.
pub trait Hands {
    fn create(&self, g: &Group, id: &str, first_msg: &str) -> Result<(), String>;
    fn send(&self, id: &str, testo: &str) -> Result<(), String>;
}

struct RealHands(tauri::AppHandle);
impl Hands for RealHands {
    fn create(&self, g: &Group, id: &str, first_msg: &str) -> Result<(), String> {
        crate::hosted::start_with(&self.0, &g.cartella, first_msg, Some(id), Some("haiku")).map(|_| ())
    }
    fn send(&self, id: &str, testo: &str) -> Result<(), String> {
        let (id, testo) = (id.to_string(), testo.to_string());
        // outside the Sentinel's thread: a hibernated coordinator restarts, and it takes a few moments
        std::thread::spawn(move || {
            let r = crate::send_internal(id.clone(), testo);
            if !r["ok"].as_bool().unwrap_or(false) { crate::log_juggl(&format!("regia {id}: evento non consegnato: {}", str_of(&r, "msg"))); }
        });
        Ok(())
    }
}

fn registered_coordinators() -> Map<String, Value> { load_json(&coordinators_file()).and_then(|v| v.as_object().cloned()).unwrap_or_default() }
fn register_coordinator(gid: &str, id: Option<&str>) {
    let _ = crate::storage::modify(&coordinators_file(), Some(json!({})), |v| {
        match id { Some(i) => { v[gid] = json!(i); } None => { if let Some(o) = v.as_object_mut() { o.remove(gid); } } }
        true
    });
}

/// Turns a group's coordinator off or on (impostazioni.json "regia_gruppi").
pub fn set(gruppo: &str, on: bool) -> Result<(), String> {
    crate::storage::modify(&base_dir().join("impostazioni.json"), Some(json!({})), |v| {
        if !v["regia_gruppi"].is_object() { v["regia_gruppi"] = json!({}); }
        if on { if let Some(o) = v["regia_gruppi"].as_object_mut() { o.remove(gruppo); } } else { v["regia_gruppi"][gruppo] = json!(false); }
        true
    }).map(|_| ())
}

/// The group's coordinator is born: state ready before starting (name, regia, model, instructions), then the chat.
fn create_coordinator(g: &Group, hands: &dyn Hands) -> Result<String, String> {
    if !Path::new(&g.cartella).is_dir() { return Err(format!("cartella del progetto non trovata: {}", g.cartella)); }
    let id = crate::new_uuid();
    let ctx = base_dir().join("contesti").join(format!("regia-{}.md", slug(&g.id)));
    crate::storage::write(&ctx, instructions(g).as_bytes())?;
    let p = state_file(&id);
    crate::storage::write_json(&p, &json!({"sessione": id, "nome_manuale": coordinator_name(&g.nome), "regia": true, "regia_gruppo": g.id,
        "regia_nome": g.nome, "regia_nata": unix_now(), "modello": "haiku", "effort": "low", "contesto": ctx.to_string_lossy(), "pilota": false,
        "cwd": g.cartella}))?;
    register_coordinator(&g.id, Some(&id));
    if let Err(e) = hands.create(g, &id, &birth_message(g)) {
        crate::storage::remove(&p);
        register_coordinator(&g.id, None);
        return Err(e);
    }
    Ok(id)
}

/// Chat already named "<group> · regia": becomes the group's coordinator (model and tools stay its own until resumed).
fn adopt(g: &Group, v: &Value) {
    let id = str_of(v, "sessione").to_string();
    crate::update_json(&state_file(&id), |x| {
        x["regia"] = json!(true); x["regia_gruppo"] = json!(g.id); x["regia_nome"] = json!(g.nome); x["regia_nata"] = json!(unix_now()); x["pilota"] = json!(false);
        true
    });
    register_coordinator(&g.id, Some(&id));
    crate::log_juggl(&format!("regia: {id} adottata come regia del gruppo {}", g.id));
}

/// One pass: called by the Sentinel every 3 s. Never blocking.
pub fn tick(app: &tauri::AppHandle, ss: &[Value]) {
    static G: Mutex<Option<Round>> = Mutex::new(None);
    let mut gg = G.lock().unwrap_or_else(|e| e.into_inner());
    let g = gg.get_or_insert_with(Round::default);
    step(g, ss, &RealHands(app.clone()), unix_now());
}

/// The real pass, separated from the app for the tests.
pub fn step(gr: &mut Round, ss: &[Value], hands: &dyn Hands, now: f64) {
    let juggl = crate::juggl_id();
    let settings = session_care::settings();
    let gg = groups(ss, &juggl);
    let registered = registered_coordinators();
    // 1) birth (or adoption) of the coordinators
    for (gid, g) in &gg {
        if g.regia.is_some() || !is_on(&settings, gid) { continue; }
        if let Some(c) = &g.candidata { adopt(g, c); continue; }
        if g.membri.len() < THRESHOLD { continue; }
        if let Some(r) = registered.get(gid).and_then(|x| x.as_str()) {
            let p = state_file(r);
            if p.exists() { continue; }   // it exists but is archived or closed: leave it as it is, the user resumes it
            // deleted by the user: this group's coordinator is turned off (it turns on again from Settings or from the group)
            let _ = set(gid, false);
            register_coordinator(gid, None);
            crate::log_juggl(&format!("regia del gruppo {gid} eliminata: regia automatica spenta per questo gruppo"));
            continue;
        }
        if now - gr.tentativi.get(gid).copied().unwrap_or(0.0) < RETRY_S { continue; }
        gr.tentativi.insert(gid.clone(), now);
        match create_coordinator(g, hands) {
            Ok(id) => { crate::log_juggl(&format!("regia: nata {id} per il gruppo {gid} ({} chat)", g.membri.len())); gr.hook_controllato = false; }
            Err(e) => crate::log_juggl(&format!("regia del gruppo {gid} non creata: {e}")),
        }
    }
    // the coordinator's reply to an automatic message that asks nothing of the user: read by itself (it does not stay
    // "to read" at every event). If the user is needed it stays "waits for you" and goes into the summary line.
    for g in gg.values() {
        if let Some(r) = &g.regia {
            if without_notice(r) && str_of(r, "vista") == "fatta" {
                let since = f64_of(r, "da");
                crate::update_json(&state_file(str_of(r, "sessione")), |x| { if f64_of(x, "da") != since { return false; } x["letta_da"] = json!(since); true });
            }
        }
    }
    // 2) events: only groups with the coordinator on and at least SOGLIA chats (below the threshold the coordinator stays still)
    let live: Vec<String> = gg.iter().filter(|(gid, g)| g.regia.is_some() && is_on(&settings, gid) && g.membri.len() >= THRESHOLD).map(|(k, _)| k.clone()).collect();
    gr.observe(&gg, &live, now);
    for a in read_notices() {
        let gid = str_of(&a, "gruppo");
        if gg.get(gid).map(|g| g.regia.is_some() && is_on(&settings, gid)).unwrap_or(false) { gr.conflict(&gg, &a, now); }
    }
    for gid in gr.ready(&gg, now) {
        let Some(g) = gg.get(&gid) else { gr.attesa.remove(&gid); continue };
        let Some(r) = g.regia.as_ref() else { gr.attesa.remove(&gid); continue };
        // coordinator at work: events wait for the end of its turn (no wake-ups halfway)
        if matches!(turn_of(r), "lavora" | "bloccata") { continue; }
        let ev = gr.take(&gid, now);
        if let Err(e) = hands.send(str_of(r, "sessione"), &compose(g, &ev)) { crate::log_juggl(&format!("regia {gid}: {e}")); }
    }
    // groups that are off or without coordinator: no pending events
    gr.attesa.retain(|gid, _| gg.get(gid).map(|g| g.regia.is_some() && is_on(&settings, gid)).unwrap_or(false));
    // 3) registry for the "who touches what" hook
    write_registry(gr, &gg, &settings);
    if !gr.hook_controllato && !gr.registro.is_empty() {
        gr.hook_controllato = true;
        match ensure_hook() { Ok(true) => crate::log_juggl("regia: hook delle cartelle aggiunto a ~/.claude/settings.json"), Ok(false) => {}, Err(e) => crate::log_juggl(&format!("regia: hook delle cartelle non aggiunto: {e}")) }
    }
}

/// regia/registro.json: who is in a group with the coordinator on. Read by the hook (bin/cartelle.py): whoever is not there passes
/// at once. With no groups the file disappears, and the hook does not even start Python.
fn write_registry(gr: &mut Round, gg: &BTreeMap<String, Group>, settings: &Value) {
    let (mut sess, mut grs) = (Map::new(), Map::new());
    for (gid, g) in gg {
        let Some(r) = g.regia.as_ref() else { continue };
        if !is_on(settings, gid) { continue; }
        grs.insert(gid.clone(), json!({"nome": g.nome, "regia": str_of(r, "sessione")}));
        for m in &g.membri { sess.insert(str_of(m, "sessione").to_string(), json!({"g": gid, "nome": str_of(m, "nome")})); }
    }
    let p = registry_file();
    if grs.is_empty() {
        if !gr.registro.is_empty() || p.exists() { let _ = std::fs::remove_file(&p); }
        gr.registro.clear();
        return;
    }
    let testo = serde_json::to_string(&json!({"gruppi": grs, "sessioni": sess})).unwrap_or_default();
    if testo != gr.registro || !p.exists() {
        match crate::storage::write(&p, testo.as_bytes()) { Ok(_) => gr.registro = testo, Err(e) => crate::log_juggl(&format!("regia: registro non scritto: {e}")) }
    }
}

/// The new lines of regia/avvisi.jsonl (written by the hook and `juggl cartelle`): the file is moved before reading it,
/// so a line written now ends up in the new file and is not lost.
fn read_notices() -> Vec<Value> {
    let p = notices_file();
    if !p.exists() { return vec![]; }
    let tmp = coordinator_dir().join(format!("avvisi.jsonl.lettura.{}", std::process::id()));
    if std::fs::rename(&p, &tmp).is_err() { return vec![]; }
    let t = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    t.lines().filter_map(|r| serde_json::from_str::<Value>(r).ok()).collect()
}

// ---------- "who touches what" hook in ~/.claude/settings.json ----------

const MATCHER: &str = "Edit|Write|MultiEdit|NotebookEdit";

/// The hook command: if there is no group with a coordinator (registry absent) it exits at once without starting Python.
pub fn hook_command() -> Option<String> {
    // Windows and Linux: no Python hook (not guaranteed, and a hook that fails at every edit fills the chats with
    // errors). The coordinator works all the same, without "who touches what", until the hook is ported to Rust.
    if !cfg!(target_os = "macos") { return None; }
    let (py, _) = crate::profile::python()?;
    let script = base_dir().join("bin/cartelle.py");
    if !script.is_file() { return None; }
    let q = |p: &Path| { let t = p.to_string_lossy().to_string(); if t.contains(' ') { format!("'{t}'") } else { t } };
    Some(format!("[ -s {} ] || exit 0; exec {} -I -S {} hook", q(&registry_file()), q(&py), q(&script)))
}

/// Adds the folders' PreToolUse hook if missing. Only if Juggl's link with Claude Code is already there
/// (Juggl's other hooks): nothing new in settings.json without that consent. Ok(true) = added now.
pub fn ensure_hook() -> Result<bool, String> {
    if crate::profile::hook_status()["collegati"].as_u64().unwrap_or(0) == 0 { return Ok(false); }
    let p = crate::profile::settings();
    let mut v: Value = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or("settings.json non si legge")?;
    if !v.is_object() { return Err("settings.json non e' un oggetto".into()); }
    let already = v.pointer("/hooks/PreToolUse").and_then(|x| x.as_array()).map(|l| l.iter().any(|g| g.get("hooks").and_then(|h| h.as_array())
        .map(|hh| hh.iter().any(|x| str_of(x, "command").contains("/bin/cartelle.py "))).unwrap_or(false))).unwrap_or(false);
    if already { return Ok(false); }
    let cmd = hook_command().ok_or("manca Python 3 o bin/cartelle.py")?;
    if !v["hooks"].is_object() { v["hooks"] = json!({}); }
    if !v["hooks"]["PreToolUse"].is_array() { v["hooks"]["PreToolUse"] = json!([]); }
    v["hooks"]["PreToolUse"].as_array_mut().unwrap().push(json!({"matcher": MATCHER, "hooks": [{"type": "command", "command": cmd, "timeout": 1}]}));
    crate::profile::save_settings(&v).map(|_| true)
}

// ---------- commands for the interface ----------

/// Switch for a group's coordinator (Settings and the group's menu).
#[tauri::command(async)]
pub fn regia_imposta(gruppo: String, accesa: bool) -> Value {
    if gruppo.trim().is_empty() { return json!({"ok": false, "msg": "gruppo mancante"}); }
    // turned back on after the user had deleted it: the memory of the old coordinator is no longer needed
    if accesa {
        if let Some(r) = registered_coordinators().get(&gruppo).and_then(|x| x.as_str()) { if !state_file(r).exists() { register_coordinator(&gruppo, None); } }
    }
    let r = match set(&gruppo, accesa) { Ok(_) => json!({"ok": true}), Err(e) => json!({"ok": false, "msg": e}) };
    crate::notify_changed();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str, prog: &str, vista: &str) -> Value {
        json!({"sessione": id, "nome": format!("{} · {id}", prog.to_uppercase()), "vista": vista, "da": 100.0, "cwd": format!("/tmp/{prog}"),
               "progetto": {"id": prog, "nome": prog.to_uppercase(), "cartella": format!("/tmp/{prog}")}})
    }
    struct Fakes { create: Mutex<Vec<(String, String)>>, mandati: Mutex<Vec<(String, String)>> }
    impl Hands for Fakes {
        fn create(&self, g: &Group, id: &str, _primo: &str) -> Result<(), String> { self.create.lock().unwrap().push((g.id.clone(), id.to_string())); Ok(()) }
        fn send(&self, id: &str, testo: &str) -> Result<(), String> { self.mandati.lock().unwrap().push((id.to_string(), testo.to_string())); Ok(()) }
    }

    #[test]
    fn groups_count_only_right_chats() {
        let mut ss = vec![chat("a", "alfa", "lavora"), chat("b", "alfa", "aspetta"), chat("c", "alfa", "ibernata"),
            chat("locale:1", "alfa", "lavora"), chat("j", "alfa", "lavora"), chat("d", "beta", "ferma")];
        let mut r = chat("r", "alfa", "aspetta"); r["regia"] = json!(true); r["regia_gruppo"] = json!("alfa"); ss.push(r);
        let mut p = chat("p", "alfa", "lavora"); p["cwd"] = json!("/home/x/tavolo/prova"); ss.push(p);
        let gg = groups(&ss, "j");
        let ok = &gg["alfa"];
        assert_eq!(ok.membri.iter().map(|v| str_of(v, "sessione")).collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(ok.regia.as_ref().map(|v| str_of(v, "sessione")), Some("r"));
        assert_eq!(gg["beta"].membri.len(), 1);
    }

    #[test]
    fn short_summary() {
        assert_eq!(digest(&[]), "");
        let l = vec![json!({"cosa": "privacy App Store"}), json!({"cosa": "prova voce"})];
        assert_eq!(digest(&l), "2 aspettano te: privacy App Store, prova voce");
        assert_eq!(digest(&l[..1]), "1 aspetta te: privacy App Store");
    }

    #[test]
    fn events_collected_and_sent_together() {
        let mut gr = Round::default();
        let mut ss = vec![chat("a", "alfa", "lavora"), chat("b", "alfa", "lavora"), chat("c", "alfa", "ferma")];
        let mut r = chat("r", "alfa", "aspetta"); r["regia"] = json!(true); ss.push(r);
        let live = vec!["alfa".to_string()];
        gr.observe(&groups(&ss, ""), &live, 1000.0);      // first pass: only looks
        assert!(gr.attesa.is_empty());
        ss[0]["vista"] = json!("aspetta"); ss[0]["da"] = json!(1001.0); ss[0]["risposta"] = json!("Fatto il logo");
        ss[1]["vista"] = json!("bloccata"); ss[1]["domanda"] = json!("Posso cancellare la cartella?");
        let gg = groups(&ss, "");
        gr.observe(&gg, &live, 1003.0);
        assert_eq!(gr.attesa["alfa"].len(), 2);
        // the pilot has not decided yet for "a": wait
        assert!(gr.ready(&gg, 1010.0).is_empty());
        ss[0]["pilota_su_da"] = json!(1001.0); ss[0]["serve_te"] = json!(true); ss[0]["serve_te_cosa"] = json!("Scegli il logo");
        let gg = groups(&ss, "");
        gr.observe(&gg, &live, 1012.0);                   // "needs you" inside the end of turn, no extra event
        assert_eq!(gr.attesa["alfa"].len(), 2);
        let pronti = gr.ready(&gg, 1014.0);              // pilot decided and group calm for more than COLLECT_S
        assert_eq!(pronti, vec!["alfa".to_string()]);
        let ev = gr.take("alfa", 1016.5);
        let t = compose(&gg["alfa"], &ev);
        assert!(t.starts_with(EVENTS_PREFIX), "{t}");
        assert!(t.contains("ha finito il turno e aspetta") && t.contains("Scegli il logo") && t.contains("Fatto il logo"), "{t}");
        assert!(t.contains("ferma su un permesso") && t.contains("Posso cancellare"), "{t}");
        assert!(t.contains("juggl://sessione/a"), "{t}");
        // no duplicates for the same state
        gr.observe(&gg, &live, 1020.0);
        assert!(gr.attesa.get("alfa").map(|l| l.is_empty()).unwrap_or(true));
    }

    #[test]
    fn off_or_small_group_no_events() {
        let mut gr = Round::default();
        let mut ss = vec![chat("a", "alfa", "lavora"), chat("b", "alfa", "lavora"), chat("c", "alfa", "ferma")];
        gr.observe(&groups(&ss, ""), &[], 1.0);
        ss[0]["vista"] = json!("aspetta");
        gr.observe(&groups(&ss, ""), &[], 5.0);
        assert!(gr.attesa.is_empty());
    }

    /// The whole pass on real files (temp folder): birth, event, conflict from the hook, switch off.
    #[test]
    fn full_round_on_files() {
        let d = std::env::temp_dir().join(format!("juggl-regia-prova-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("stato")).unwrap();
        std::fs::create_dir_all(d.join("prog")).unwrap();
        let _base_guard = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (a variable of the whole process)
        std::env::set_var("JUGGL_BASE", &d);
        let prog = d.join("prog").to_string_lossy().to_string();
        let ch = |id: &str, vista: &str| json!({"sessione": id, "nome": format!("PROVA · {id}"), "vista": vista, "da": 10.0, "cwd": prog,
            "progetto": {"id": "prova", "nome": "PROVA", "cartella": prog}});
        let hands = Fakes { create: Mutex::new(vec![]), mandati: Mutex::new(vec![]) };
        let mut gr = Round::default();
        // two chats: no coordinator
        let mut ss = vec![ch("11111111-1111-1111-1111-111111111111", "lavora"), ch("22222222-2222-2222-2222-222222222222", "lavora")];
        step(&mut gr, &ss, &hands, 1000.0);
        assert!(hands.create.lock().unwrap().is_empty());
        // the third: the coordinator is born, with the state ready
        ss.push(ch("33333333-3333-3333-3333-333333333333", "ferma"));
        step(&mut gr, &ss, &hands, 1003.0);
        let (gid, rid) = hands.create.lock().unwrap()[0].clone();
        assert_eq!(gid, "prova");
        let st = load_json(&state_file(&rid)).unwrap();
        assert_eq!(str_of(&st, "nome_manuale"), "PROVA · regia");
        assert_eq!(st["regia"], json!(true));
        assert_eq!(str_of(&st, "modello"), "haiku");
        assert!(Path::new(str_of(&st, "contesto")).is_file());
        // the coordinator shows up in the list: no second coordinator, registry written for the hook
        let mut r = st.clone(); r["vista"] = json!("aspetta"); r["nome"] = json!("PROVA · regia"); r["progetto"] = ss[0]["progetto"].clone();
        ss.push(r.clone());
        step(&mut gr, &ss, &hands, 1006.0);
        assert_eq!(hands.create.lock().unwrap().len(), 1);
        let reg = load_json(&registry_file()).unwrap();
        assert_eq!(reg["sessioni"]["11111111-1111-1111-1111-111111111111"]["g"], json!("prova"));
        assert!(reg["sessioni"].get(&rid).is_none());
        // a chat finishes its turn: after the gathering the event reaches the coordinator
        ss[0]["vista"] = json!("aspetta"); ss[0]["da"] = json!(1007.0); ss[0]["risposta"] = json!("Fatto");
        ss[0]["pilota_su_da"] = json!(1007.0); ss[0]["serve_te"] = json!(false);
        step(&mut gr, &ss, &hands, 1008.0);
        assert!(hands.mandati.lock().unwrap().is_empty());
        step(&mut gr, &ss, &hands, 1013.0);
        {
            let m = hands.mandati.lock().unwrap();
            assert_eq!(m.len(), 1);
            assert_eq!(m[0].0, rid);
            assert!(m[0].1.contains("ha finito il turno") && m[0].1.contains("PROVA · 11111111"), "{}", m[0].1);
        }
        // conflict written by the hook: it reaches the coordinator (only once)
        let row = json!({"t": 1014.0, "gruppo": "prova", "sessione": "22222222-2222-2222-2222-222222222222", "nome": "PROVA · 2222",
            "tenuta_da": "11111111-1111-1111-1111-111111111111", "tenuta_nome": "PROVA · 1111", "cartella": prog, "file": format!("{prog}/a.txt"), "tipo": "modifica"});
        std::fs::write(notices_file(), format!("{row}\n{row}\n")).unwrap();
        step(&mut gr, &ss, &hands, 1014.0);
        step(&mut gr, &ss, &hands, 1019.0);
        {
            let m = hands.mandati.lock().unwrap();
            assert_eq!(m.len(), 2, "{m:?}");
            assert!(m[1].1.contains("Conflitto") && m[1].1.contains("e' tenuta da"), "{}", m[1].1);
            assert_eq!(m[1].1.matches("Conflitto").count(), 1);
        }
        // coordinator at work: the event waits for the end of its turn
        ss[3]["vista"] = json!("lavora");
        ss[1]["vista"] = json!("aspetta"); ss[1]["da"] = json!(1020.0); ss[1]["pilota_su_da"] = json!(1020.0); ss[1]["serve_te"] = json!(true); ss[1]["serve_te_cosa"] = json!("Prova la voce");
        step(&mut gr, &ss, &hands, 1020.0);
        step(&mut gr, &ss, &hands, 1030.0);
        assert_eq!(hands.mandati.lock().unwrap().len(), 2);
        // summary line with who is waiting
        let listing = for_list(&ss);
        assert_eq!(str_of(&listing["prova"], "sintesi"), "1 aspetta te: Prova la voce");
        ss[3]["vista"] = json!("aspetta");
        step(&mut gr, &ss, &hands, 1033.0);
        assert_eq!(hands.mandati.lock().unwrap().len(), 3);
        // the coordinator's reply to an event that asks nothing: read by itself
        ss[3]["vista"] = json!("fatta"); ss[3]["chiesto"] = json!(format!("{EVENTS_PREFIX} del gruppo PROVA]")); ss[3]["da"] = json!(1034.0);
        crate::update_json(&state_file(&rid), |x| { x["da"] = json!(1034.0); true });
        step(&mut gr, &ss, &hands, 1034.0);
        assert_eq!(load_json(&state_file(&rid)).unwrap()["letta_da"], json!(1034.0));
        ss[3]["vista"] = json!("aspetta");
        // turned off from Settings: no events and registry removed (the hook no longer checks anyone)
        set("prova", false).unwrap();
        ss[2]["vista"] = json!("lavora");
        step(&mut gr, &ss, &hands, 1040.0);
        ss[2]["vista"] = json!("aspetta"); ss[2]["da"] = json!(1041.0);
        step(&mut gr, &ss, &hands, 1041.0);
        step(&mut gr, &ss, &hands, 1080.0);
        assert_eq!(hands.mandati.lock().unwrap().len(), 3);
        assert!(!registry_file().exists());
        // off: a new group with 3 chats gives birth to nothing
        set("altro", false).unwrap();
        let other = |id: &str| { let mut x = ch(id, "ferma"); x["progetto"]["id"] = json!("altro"); x };
        let ss2 = vec![other("44444444-4444-4444-4444-444444444444"), other("55555555-5555-5555-5555-555555555555"), other("66666666-6666-6666-6666-666666666666")];
        step(&mut gr, &ss2, &hands, 1100.0);
        assert_eq!(hands.create.lock().unwrap().len(), 1);
        // coordinator deleted by the user (state gone) with the group turned back on: it is not reborn, the group's coordinator turns off
        set("prova", true).unwrap();
        std::fs::remove_file(state_file(&rid)).unwrap();
        let without: Vec<Value> = ss.iter().filter(|v| str_of(v, "sessione") != rid).cloned().collect();
        step(&mut gr, &without, &hands, 1200.0);
        assert_eq!(hands.create.lock().unwrap().len(), 1);
        assert!(!is_on(&session_care::settings(), "prova"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
