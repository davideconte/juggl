// Oks and open items, collected from all sessions (Requests view, groups "Serve il tuo ok" and "Restano").
// The user's requests (requests.rs) say what the user asked for; this is the opposite direction: what the sessions
// ask the user for or still have open.
//
// Automatic source, without asking the sessions anything: at the end of a turn we read the session's last reply
// (transcript). Simple rules find the lines "Serve il tuo ok: ...", the block "Restano: ..." and the direct
// questions; then the small model (haiku, as in requests.rs) gives each item a title, a kind (ok, in_corso, attesa)
// and "what happens if you say yes", and recognizes items already known (no duplicates between one round and the next).
// An item that no longer appears in the session's last reply is done.
//
// Optional source, for sessions that want to be precise: ~/tavolo/stato/richieste/<session>.json with
// [{id, titolo, tipo: "ok"|"in_corso"|"attesa", progetto, creato, dettaglio, file?}]. If present, it wins over the
// automatic one. Whoever writes it uses the safe write (temp file, fsync, rename: storage.rs, comune.py).
//
// Ok, No and Reply buttons: the message "[Da «NOME», via Juggl] Ok: <title>" goes out through the same send path
// as Juggl (queued if the session is working). The item moves to "in_conferma" and becomes done only when the
// session's last reply no longer lists it among the oks.
// Registry: ~/tavolo/ok.json, saved atomically and under lock.
use crate::{base_dir, f64_of, unix_now, autopilot, str_of};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use tauri::Emitter;

pub const FROM_USER_PREFIX: &str = "[Da «NOME», via Juggl]";
const KINDS: [&str; 3] = ["ok", "in_corso", "attesa"];
/// Done items stay two days (for "recently done"), then they are deleted.
const KEEP_DONE: f64 = 2.0 * 86400.0;
/// Every 5 minutes all idle sessions are checked again (a finished turn the Sentinel did not see).
const LONG_SWEEP: f64 = 300.0;

static REG: Mutex<()> = Mutex::new(());

/// JUGGL_OK_FILE: registry elsewhere (only for tests on real data, without touching the app's one).
fn registry_path() -> PathBuf { std::env::var("JUGGL_OK_FILE").map(PathBuf::from).unwrap_or_else(|_| base_dir().join("ok.json")) }
pub fn requests_dir() -> PathBuf { base_dir().join("stato").join("richieste") }

fn normalize(mut r: Value) -> Value {
    if !r.is_object() { r = json!({}); }
    if !r["voci"].is_array() { r["voci"] = json!([]); }
    if !r["sessioni"].is_object() { r["sessioni"] = json!({}); }
    r
}

fn cleanup(r: &mut Value) {
    let now = unix_now();
    if let Some(a) = r["voci"].as_array_mut() {
        a.retain(|v| !(str_of(v, "stato") == "fatta" && now - f64_of(v, "fatta_il") > KEEP_DONE));
        // duplicate done items (same session, same title): the most recent one stays
        let mut seen: HashSet<String> = HashSet::new();
        let mut keep = vec![true; a.len()];
        let mut order: Vec<usize> = (0..a.len()).collect();
        order.sort_by(|&i, &j| f64_of(&a[j], "fatta_il").partial_cmp(&f64_of(&a[i], "fatta_il")).unwrap_or(std::cmp::Ordering::Equal));
        for i in order {
            if str_of(&a[i], "stato") != "fatta" { continue; }
            if !seen.insert(format!("{}|{}", str_of(&a[i], "sessione"), compact(str_of(&a[i], "titolo")))) { keep[i] = false; }
        }
        let mut k = 0;
        a.retain(|_| { let t = keep[k]; k += 1; t });
    }
}

/// An item the user already answered (Ok, No, Reply) does not match a session reply that arrived later:
/// there a similar ok is a new question. It matches only when re-reading the same reply or earlier replies.
fn matchable(v: &Value, when: f64) -> bool {
    let r = f64_of(v, "risposta_il");
    !(r > 0.0 && when > 0.0 && when > r)
}

/// Reads, modifies and saves the registry while holding the lock (the file lock too, as in requests.rs).
fn with_registry<R>(fun: impl FnOnce(&mut Value) -> R) -> R {
    let _g = REG.lock().unwrap_or_else(|e| e.into_inner());
    let p = registry_path();
    let _l = crate::storage::lock(&p).map_err(|e| crate::log_juggl(&format!("ok: {e}"))).ok();
    let (loaded, broken) = match crate::storage::read_json(&p) {
        crate::storage::ReadResult::Valida(v) => (v, false),
        crate::storage::ReadResult::Assente => (json!({}), false),
        crate::storage::ReadResult::Rotta => (json!({}), true),
    };
    let mut r = normalize(loaded);
    let out = fun(&mut r);
    cleanup(&mut r);
    if broken { crate::log_juggl("ok.json illeggibile: non lo riscrivo"); }
    else if let Err(e) = crate::storage::write_json(&p, &r) { crate::log_juggl(&format!("ok: scrittura non riuscita: {e}")); }
    out
}

fn registry() -> Value {
    let _g = REG.lock().unwrap_or_else(|e| e.into_inner());
    // fast chats (14/11): ok.json (0.9 MB) read once per file version
    normalize(crate::fast_open::json(&registry_path()).map(|a| (*a).clone()).unwrap_or(json!({})))
}

fn notify() { if let Some(a) = crate::APP.get() { let _ = a.emit("cambiato", ()); } }

fn new_id(r: &mut Value) -> String {
    let n = r["n"].as_u64().unwrap_or(0) + 1;
    r["n"] = json!(n);
    format!("k{n}")
}

/// Only letters and digits, lowercase: to recognize the same item written with small differences.
fn compact(t: &str) -> String { t.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect() }

fn short(t: &str, n: usize) -> String {
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.replace(['\u{2014}', '\u{2013}'], ",");
    if t.chars().count() <= n { t } else { format!("{}…", t.chars().take(n - 1).collect::<String>()) }
}

// ---------- rules: what is open in the last reply ----------

#[derive(Default, Debug, PartialEq)]
pub struct Extracted { pub ok: Vec<String>, pub restano: Vec<String>, pub domande: Vec<String> }

impl Extracted {
    pub fn is_empty(&self) -> bool { self.ok.is_empty() && self.restano.is_empty() && self.domande.is_empty() }
    fn signature(&self) -> String { compact(&format!("{:?}|{:?}|{:?}", self.ok, self.restano, self.domande)).chars().take(4000).collect() }
}

/// Line without markdown: bullet marks, bold, headings, spaces.
fn clean_line(r: &str) -> String {
    let mut t = r.trim().replace("**", "").replace("__", "");
    loop {
        let p = t.trim_start();
        let stripped = ["- ", "* ", "• ", "· ", "# ", "## ", "### ", "+ "].iter().find_map(|x| p.strip_prefix(x).map(|y| y.to_string()));
        match stripped { Some(y) => t = y, None => { t = p.to_string(); break; } }
    }
    // "1. text" / "2) text"
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() && digits.len() <= 2 {
        let rest = &t[digits.len()..];
        if let Some(x) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) { t = x.to_string(); }
    }
    t.trim().to_string()
}

fn is_bullet(r: &str) -> bool {
    let t = r.trim_start();
    ["- ", "* ", "• ", "· ", "+ "].iter().any(|x| t.starts_with(x))
        || { let c: String = t.chars().take_while(|c| c.is_ascii_digit()).collect(); !c.is_empty() && c.len() <= 2 && (t[c.len()..].starts_with(". ") || t[c.len()..].starts_with(") ")) }
}

/// "a, b (x, y), c." -> ["a", "b (x, y)", "c"]: commas and semicolons outside parentheses.
pub fn split_list(t: &str) -> Vec<String> {
    let mut out = vec![]; let mut cur = String::new(); let mut depth = 0i32;
    for c in t.chars() {
        match c {
            '(' | '[' => { depth += 1; cur.push(c); }
            ')' | ']' => { depth -= 1; cur.push(c); }
            ',' | ';' if depth <= 0 => { out.push(std::mem::take(&mut cur)); }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|x| x.trim().trim_end_matches(['.', ';']).trim().to_string())
        .map(|x| x.strip_prefix("e ").map(String::from).unwrap_or(x)).filter(|x| x.chars().count() > 1).collect()
}

/// After "Restano" / "Serve il tuo ok": the rest of the line after the colon (None if the line does not start that way).
fn after_label(p: &str, labels: &[&str]) -> Option<String> {
    let low = p.to_lowercase();
    for e in labels {
        if let Some(rest) = low.strip_prefix(e) {
            let rest = rest.trim_start();
            if let Some(x) = rest.strip_prefix(':') {
                let n = p.chars().count() - x.chars().count();
                return Some(p.chars().skip(n).collect::<String>().trim().to_string());
            }
        }
    }
    None
}

/// Direct questions to the user in the final part: sentences ending with "?" or asking for an ok ("dimmi se").
fn questions_in(r: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let close = |cur: &mut String, out: &mut Vec<String>, end: char| {
        let t = cur.trim().to_string(); cur.clear();
        let low = t.to_lowercase();
        let asks = end == '?' || ["dimmi se", "fammi sapere se", "mi dai l'ok", "aspetto il tuo", "serve il tuo"].iter().any(|x| low.contains(x));
        if asks && t.chars().count() >= 8 { out.push(if end == '?' { format!("{t}?") } else { t }); }
    };
    for c in r.chars() {
        match c {
            '?' => close(&mut cur, &mut out, '?'),
            '.' | '!' => close(&mut cur, &mut out, c),
            _ => cur.push(c),
        }
    }
    close(&mut cur, &mut out, ' ');
    out
}

pub fn extract_from_rules(text: &str) -> Extracted {
    let mut e = Extracted::default();
    // drop code blocks and quotes (drafts of emails and messages: their questions are not for the user)
    // except the question of a ```juggl-scelta block: it is a request to the user like "Serve il tuo ok:" (with buttons in chat)
    let mut lines: Vec<&str> = vec![];
    let mut in_code = false;
    let mut in_choice = false;
    for r in text.lines() {
        let t = r.trim_start();
        if t.starts_with("```") { in_code = !in_code; in_choice = in_code && t.trim_end() == "```juggl-scelta"; lines.push(""); continue; }
        if in_code && in_choice {
            if let Some(d) = t.strip_prefix("domanda:") { let d = d.trim(); if !d.is_empty() && !e.ok.iter().any(|x| x == d) { e.ok.push(d.to_string()); } }
        }
        if in_code || t.starts_with('>') { lines.push(""); continue; }
        lines.push(r);
    }
    let mut taken: HashSet<usize> = HashSet::new();
    let mut i = 0;
    while i < lines.len() {
        let p = clean_line(lines[i]);
        if let Some(x) = after_label(&p, &["serve il tuo ok", "serve il tuo si", "serve il tuo sì", "mi serve il tuo ok"]) {
            taken.insert(i);
            if !x.is_empty() { e.ok.push(x); }
            else {
                // "Serve il tuo ok:" followed by a list
                // (12/11: also with blank lines between the label and the items or between items: same rule as ui/index.html)
                let mut j = i + 1;
                while j < lines.len() && (is_bullet(lines[j]) || (lines[j].trim().is_empty() && j + 1 < lines.len() && is_bullet(lines[j + 1]))) {
                    if !lines[j].trim().is_empty() { e.ok.push(clean_line(lines[j])); }
                    taken.insert(j); j += 1;
                }
                i = j; continue;
            }
        } else if let Some(x) = after_label(&p, &["restano", "resta", "rimangono", "ancora aperti", "ancora aperte"]) {
            taken.insert(i);
            e.restano = split_list(&x);
            let mut j = i + 1;
            while j < lines.len() && (is_bullet(lines[j]) || (lines[j].trim().is_empty() && j + 1 < lines.len() && is_bullet(lines[j + 1]) && j == i + 1)) {
                if !lines[j].trim().is_empty() { e.restano.push(clean_line(lines[j]).trim_end_matches(['.', ';']).to_string()); }
                taken.insert(j); j += 1;
            }
            i = j; continue;
        }
        i += 1;
    }
    // questions: only in the last written lines (the rest of the reply is an account of the work done)
    let filled: Vec<usize> = (0..lines.len()).filter(|i| !lines[*i].trim().is_empty() && !taken.contains(i)).collect();
    for &k in filled.iter().skip(filled.len().saturating_sub(8)) {
        for d in questions_in(&clean_line(lines[k])) {
            if !e.domande.contains(&d) && e.domande.len() < 6 { e.domande.push(d); }
        }
    }
    e.restano.retain(|x| !x.is_empty());
    e
}

/// Kind from the words alone (if the model does not answer).
fn kind_from_words(t: &str) -> (&'static str, bool) {
    let l = t.to_lowercase();
    let by_user = ["lo fai tu", "la fai tu", "li fai tu", "le fai tu", "da fare tu", "(tu)", "tua prova", "da te", "solo tu"].iter().any(|x| l.contains(x));
    if by_user { return ("ok", true); }
    if ["ok tuo", "tuo ok", "tuo sì", "tuo si", "serve il tuo", "decidi tu", "decidere", "confermare", "approva", "scegli", "?"].iter().any(|x| l.contains(x)) { return ("ok", false); }
    if ["in corso", "sto ", "lavoro", "sotto-agente", "in background"].iter().any(|x| l.contains(x)) { return ("in_corso", false); }
    if ["aspetto", "attesa", "da apple", "revisione", "verifica apple", "dal cliente", "sessione "].iter().any(|x| l.contains(x)) || waiting_on_person(&l) { return ("attesa", false); }
    ("in_corso", false)
}

fn without_notes(t: &str) -> String {
    // "send the draft to a client (you do it)" -> title without the final note in parentheses
    let t = t.trim();
    if t.ends_with(')') { if let Some(i) = t.rfind(" (") { if i > 6 { return t[..i].trim().to_string(); } } }
    t.to_string()
}

fn capitalize(t: &str) -> String { let mut c = t.chars(); match c.next() { Some(x) => x.to_uppercase().collect::<String>() + c.as_str(), None => String::new() } }

/// Items from the rules alone: title = the session's sentence.
pub fn items_from_rules(e: &Extracted) -> Vec<Value> {
    let mut out = vec![];
    for x in &e.ok { out.push(json!({"titolo": short(&capitalize(x), 110), "tipo": "ok", "fai_tu": false, "testo": x})); }
    for x in &e.restano {
        let (kind, by_user) = kind_from_words(x);
        out.push(json!({"titolo": short(&capitalize(&without_notes(x)), 110), "tipo": kind, "fai_tu": by_user, "testo": x}));
    }
    for x in &e.domande { out.push(json!({"titolo": short(x, 110), "tipo": "ok", "fai_tu": false, "testo": x})); }
    out
}

const PROMPT: &str = "Sei l'assistente di «NOME». «NOME» segue molte sessioni di agenti AI di programmazione insieme. \
Alla fine di ogni risposta le sessioni scrivono cosa resta aperto (\"Restano: ...\") e cosa aspetta un suo ok (\"Serve il tuo ok: ...\", domande dirette). \
«NOME» vuole vederlo raccolto in un posto solo, diviso in tre tipi:\n\
- \"ok\": senza «NOME» resta fermo. Decisioni e approvazioni prima di agire: invii a persone vere (clienti, colleghi, messaggi, email), spese, rilasci e pubblicazioni (App Store, produzione, sito), testi da approvare, azioni irreversibili, domande dirette a lui, scelte che spettano a lui. \
Anche le cose che deve fare lui con le sue mani (\"lo fai tu\", \"tu\", \"tua prova\"): allora \"fai_tu\" e' true.\n\
- \"in_corso\": la sessione o un suo sotto-agente ci sta lavorando o la fara' da sola, senza «NOME».\n\
- \"attesa\": aspetta altre persone (cliente, collega, Apple, un'altra sessione) o un evento; \"chi\" dice chi o cosa (1-3 parole).\n\
Regole: solo cose scritte nel testo, non inventare. Una voce per ogni cosa distinta (al massimo 14). \
Ignora gli extra facoltativi (\"se vuoi posso...\", \"vuoi che...\") quando non bloccano niente e la sessione ha gia' finito. \
\"titolo\": massimo 10 parole, concreto, dice la cosa (es. \"Pubblicare su /updates i testi delle sette funzioni\", \"Invio della bozza ad Alberto\"). \
\"se_si\": solo per ok, cosa fa la sessione se «NOME» dice si' (massimo 14 parole, es. \"Pubblica i testi sul sito\"); per fai_tu cosa sblocca quando «NOME» l'ha fatto. \
\"id\": se e' la stessa cosa di una delle voci gia' aperte qui sotto (anche detta con parole diverse), il suo id; altrimenti \"\". \
Scrivi in italiano semplice, senza trattini lunghi ne' emoji.";

fn ask_model(name: &str, e: &Extracted, tail_text: &str, open: &[Value]) -> Option<Vec<Value>> {
    let bullets = |l: &[String]| if l.is_empty() { "(nessuna)".to_string() } else { l.iter().map(|x| format!("- {x}")).collect::<Vec<_>>().join("\n") };
    let open_list = if open.is_empty() { "(nessuna)".to_string() } else {
        open.iter().map(|v| format!("- {} [{}] {}", str_of(v, "id"), str_of(v, "tipo"), str_of(v, "titolo"))).collect::<Vec<_>>().join("\n")
    };
    let question = format!("{PROMPT}\n\nSessione: {name}\n\nVoci gia' aperte per questa sessione (id, tipo, titolo):\n{open_list}\n\n\
Righe \"Serve il tuo ok\":\n{}\n\nVoci del blocco \"Restano\":\n{}\n\nDomande dirette trovate nella parte finale:\n{}\n\n\
Fine dell'ultima risposta della sessione, per il contesto:\n\"\"\"\n{tail_text}\n\"\"\"\n\n\
Rispondi SOLO con un oggetto JSON, senza altro testo: {{\"voci\": [{{\"id\": \"\", \"titolo\": \"...\", \"tipo\": \"ok\", \"fai_tu\": false, \"se_si\": \"...\", \"chi\": \"\", \"testo\": \"la frase originale della sessione\"}}]}}",
        bullets(&e.ok), bullets(&e.restano), bullets(&e.domande));
    // "Serve il tuo ok" items are turns waiting for the user: the cap does not apply (urgent), the cache avoids re-analyzing the same text
    let r = autopilot::ask_for("estrattore", &question, None, autopilot::Cache::SoloUna, true)?;
    let a = r["voci"].as_array()?.clone();
    Some(a.into_iter().filter(|x| !str_of(x, "titolo").trim().is_empty()).take(14).collect())
}

/// Ok buttons only for what the session really asks: the "Serve il tuo ok" lines and the direct questions.
/// A "Restano" item the model put among the oks (e.g. "notify a collaborator", already decided by the user and that
/// the session does on its own) goes back to "in_corso"; only hands-on things ("fai_tu") stay oks.
fn only_requested_oks(v: Vec<Value>, e: &Extracted) -> Vec<Value> {
    let asked: Vec<String> = e.ok.iter().chain(e.domande.iter()).map(|x| compact(x)).filter(|x| x.len() > 5).collect();
    let similar = |t: &str| { let k = compact(t); k.len() > 5 && asked.iter().any(|c| c.contains(&k) || k.contains(c.as_str())) };
    v.into_iter().map(|mut x| {
        if str_of(&x, "tipo") == "ok" && !x["fai_tu"].as_bool().unwrap_or(false) && !similar(str_of(&x, "testo")) && !similar(str_of(&x, "titolo")) {
            x["tipo"] = json!("in_corso"); x["se_si"] = json!("");
        }
        x
    }).collect()
}

#[cfg(test)]
#[test]
fn ok_only_if_asked() {
    let e = extract_from_rules("Lo mando io a Paolo.\n\nServe il tuo ok: pubblicare la 1.2 su App Store\n\nRestano: avvisare Paolo, build iPhone (tu)");
    let v = vec![
        json!({"titolo": "Avvisare Paolo", "tipo": "ok", "fai_tu": false, "testo": "avvisare Paolo"}),
        json!({"titolo": "Pubblicare la 1.2", "tipo": "ok", "fai_tu": false, "testo": "pubblicare la 1.2 su App Store"}),
        json!({"titolo": "Build iPhone", "tipo": "ok", "fai_tu": true, "testo": "build iPhone (tu)"}),
    ];
    let out = only_requested_oks(v, &e);
    assert_eq!(str_of(&out[0], "tipo"), "in_corso");
    assert_eq!(str_of(&out[1], "tipo"), "ok");
    assert_eq!(str_of(&out[2], "tipo"), "ok");
}

#[cfg(test)]
#[test]
fn choice_is_an_ok() {
    let e = extract_from_rules("Pronte le due versioni.\n\n```juggl-scelta\ndomanda: Quale logo mando ad Alberto?\n- A: Monogramma\n- B: Scritta\n```\n\n```juggl-stato\ndomanda: non conta\n```\n\nRestano: export finale");
    assert_eq!(e.ok, vec!["Quale logo mando ad Alberto?".to_string()]);
    assert!(e.domande.is_empty());
    assert_eq!(e.restano, vec!["export finale".to_string()]);
    assert_eq!(text_for_raw("fatto", "A; B", false, "").unwrap(), format!("{FROM_USER_PREFIX} Fatto: A; B"));
}

// ---------- merging into the registry ----------

/// Puts a session's open items from a source ("auto" or "file") into the registry: known ones are updated,
/// new ones are added, the ones no longer there become done.
pub fn apply_items(r: &mut Value, sid: &str, name: &str, source: &str, new_items: &[Value], msg_t: &str, when: f64) -> usize {
    let now = unix_now();
    let mut changed = 0;
    let mut taken: HashSet<String> = HashSet::new();
    let mut added: Vec<Value> = vec![];
    for x in new_items {
        let kind = if KINDS.contains(&str_of(x, "tipo")) { str_of(x, "tipo") } else { "in_corso" };
        let title = short(str_of(x, "titolo").trim().trim_end_matches('.'), 120);
        if title.is_empty() { continue; }
        let k = compact(&title); let kt = compact(str_of(x, "testo"));
        let fid = str_of(x, "fid");
        let found = r["voci"].as_array().and_then(|a| a.iter().position(|v| {
            str_of(v, "sessione") == sid && str_of(v, "fonte") == source && str_of(v, "stato") != "fatta" && !taken.contains(str_of(v, "id")) && matchable(v, when) && (
                (!fid.is_empty() && str_of(v, "fid") == fid)
                || (fid.is_empty() && !str_of(x, "id").is_empty() && str_of(v, "id") == str_of(x, "id"))
                || (fid.is_empty() && compact(str_of(v, "titolo")) == k)
                || (fid.is_empty() && !kt.is_empty() && compact(str_of(v, "testo")) == kt))
        }));
        let fill_fields = |v: &mut Value| {
            v["titolo"] = json!(title); v["tipo"] = json!(kind);
            v["fai_tu"] = json!(x["fai_tu"].as_bool().unwrap_or(false) && kind == "ok");
            for c in ["se_si", "chi", "testo", "dettaglio", "progetto_file"] { v[c] = json!(short(str_of(x, c), if c == "dettaglio" || c == "testo" { 600 } else { 200 })); }
            v["file"] = json!(str_of(x, "file").trim());
            v["vista"] = json!(now);
            if !msg_t.is_empty() { v["msg_t"] = json!(msg_t); }
        };
        match found {
            Some(i) => {
                let v = &mut r["voci"][i];
                let before = (str_of(v, "titolo").to_string(), str_of(v, "tipo").to_string(), str_of(v, "stato").to_string());
                fill_fields(v);
                // ok already given and the session now reports it as work in progress or waiting: the ok arrived
                if str_of(v, "stato") == "in_conferma" && kind != "ok" { v["stato"] = json!("aperta"); v["ok_arrivato_il"] = json!(now); }
                if (str_of(v, "titolo").to_string(), str_of(v, "tipo").to_string(), str_of(v, "stato").to_string()) != before { changed += 1; }
                taken.insert(str_of(v, "id").to_string());
            }
            None => {
                let id = new_id(r);
                let created = { let c = f64_of(x, "creato"); if c > 0.0 { c } else if when > 0.0 { when } else { now } };
                let mut v = json!({"id": id, "sessione": sid, "nome": name, "fonte": source, "fid": fid, "stato": "aperta", "creata": created});
                fill_fields(&mut v);
                taken.insert(id);
                added.push(v);
                changed += 1;
            }
        }
    }
    if let Some(a) = r["voci"].as_array_mut() {
        for v in a.iter_mut() {
            if str_of(v, "sessione") != sid || str_of(v, "fonte") != source || str_of(v, "stato") == "fatta" || taken.contains(str_of(v, "id")) { continue; }
            v["stato"] = json!("fatta"); v["fatta_il"] = json!(now); changed += 1;
        }
        a.extend(added);
    }
    changed
}

// ---------- the session's last reply ----------

/// The last text written by the agent in the transcript (whole, not cut) and its ISO time.
fn last_reply(p: &Path) -> Option<(String, String)> {
    for amount in [1_500_000u64, 12_000_000] {
        let (lines, size) = crate::tail_file(p, amount);
        for r in lines.iter().rev() {
            if !r.contains("\"assistant\"") { continue; }
            let Ok(d) = serde_json::from_str::<Value>(r) else { continue };
            if str_of(&d, "type") != "assistant" || d["isSidechain"].as_bool().unwrap_or(false) { continue; }
            let Some(a) = d["message"]["content"].as_array() else { continue };
            let t: Vec<&str> = a.iter().filter(|x| str_of(x, "type") == "text").map(|x| str_of(x, "text")).filter(|x| !x.trim().is_empty()).collect();
            if t.is_empty() { continue; }
            return Some((t.join("\n\n"), str_of(&d, "timestamp").to_string()));
        }
        if amount >= size { break; }
    }
    None
}

// ---------- optional source: the session's file ----------

fn file_of(sid: &str) -> PathBuf { requests_dir().join(format!("{sid}.json")) }

/// The items of the session's file (None if missing or unreadable: then what was there stays).
fn read_file(p: &Path) -> Option<Vec<Value>> {
    let crate::storage::ReadResult::Valida(v) = crate::storage::read_json(p) else { return None };
    let a = if v.is_array() { v } else { v["voci"].clone() };
    let a = a.as_array()?;
    Some(a.iter().filter(|x| !str_of(x, "titolo").trim().is_empty()).take(40).map(|x| {
        let id = match &x["id"] { Value::String(t) => t.clone(), Value::Number(n) => n.to_string(), _ => compact(str_of(x, "titolo")).chars().take(40).collect() };
        let created = match &x["creato"] { Value::Number(n) => n.as_f64().unwrap_or(0.0), Value::String(t) => crate::requests::from_iso(t), _ => 0.0 };
        let created = if created > 1e12 { created / 1000.0 } else { created };
        json!({"fid": id, "titolo": str_of(x, "titolo"), "tipo": str_of(x, "tipo"), "progetto_file": str_of(x, "progetto"), "creato": created,
            "dettaglio": str_of(x, "dettaglio"), "file": str_of(x, "file"), "se_si": str_of(x, "se_si"), "chi": str_of(x, "chi"), "fai_tu": x["fai_tu"].as_bool().unwrap_or(false)})
    }).collect())
}

fn import_file(sid: &str) {
    let Some(items) = read_file(&file_of(sid)) else { return };
    let name = crate::find_session(sid).map(|v| str_of(&v, "nome").to_string()).unwrap_or_default();
    let n = with_registry(|r| {
        // the file wins: drop this session's automatic items
        let before = r["voci"].as_array().map(|a| a.len()).unwrap_or(0);
        if let Some(a) = r["voci"].as_array_mut() { a.retain(|v| !(str_of(v, "sessione") == sid && str_of(v, "fonte") == "auto")); }
        let removed = before - r["voci"].as_array().map(|a| a.len()).unwrap_or(0);
        removed + apply_items(r, sid, &name, "file", &items, "", 0.0)
    });
    if n > 0 { notify(); }
}

/// Session file removed: its items go away and the automatic source comes back.
fn file_removed(sid: &str) {
    with_registry(|r| {
        if let Some(a) = r["voci"].as_array_mut() { a.retain(|v| !(str_of(v, "sessione") == sid && str_of(v, "fonte") == "file")); }
        if let Some(o) = r["sessioni"].as_object_mut() { o.remove(sid); }
    });
    put_items(sid);
    notify();
}

// ---------- analysis of one session ----------

pub fn analyze(sid: &str) {
    if file_of(sid).exists() { return import_file(sid); }
    let Some(v) = crate::find_session(sid) else { return };
    if crate::turn_view(&v) == "lavora" { return; }   // at the end of the turn the Sentinel queues it again
    let Some(p) = crate::find_transcript(&v) else { return };
    // transcript same as last time: nothing to re-read
    let fingerprint = std::fs::metadata(&p).map(|m| format!("{}:{:?}", m.len(), m.modified().ok())).unwrap_or_default();
    let reg = registry();
    let st = reg["sessioni"][sid].clone();
    if !fingerprint.is_empty() && str_of(&st, "impronta") == fingerprint { return; }
    let Some((text, msg_t)) = last_reply(&p) else { return };
    let key = format!("{msg_t}:{}", text.len());
    if str_of(&st, "chiave") == key { with_registry(|r| { r["sessioni"][sid]["impronta"] = json!(fingerprint); }); return; }
    let name = str_of(&v, "nome").to_string();
    let when = crate::requests::from_iso(&msg_t);
    let e = extract_from_rules(&text);
    let sig = e.signature();
    let mark = |r: &mut Value, key: &str, sig: &str| { r["sessioni"][sid] = json!({"chiave": key, "firma": sig, "t": msg_t, "impronta": fingerprint, "analizzata": unix_now()}); };
    // same lines as before in a new reply: the items stay, only the position in the chat changes
    if !e.is_empty() && str_of(&st, "firma") == sig {
        with_registry(|r| {
            if let Some(a) = r["voci"].as_array_mut() { for x in a.iter_mut() { if str_of(x, "sessione") == sid && str_of(x, "fonte") == "auto" && str_of(x, "stato") != "fatta" { x["msg_t"] = json!(msg_t); x["vista"] = json!(unix_now()); } } }
            mark(r, &key, &sig);
        });
        return;
    }
    let items = if e.is_empty() { vec![] } else {
        let open: Vec<Value> = reg["voci"].as_array().cloned().unwrap_or_default().into_iter()
            .filter(|x| str_of(x, "sessione") == sid && str_of(x, "fonte") == "auto" && str_of(x, "stato") != "fatta" && matchable(x, when)).collect();
        let n = text.chars().count();
        let tail_text: String = text.chars().skip(n.saturating_sub(3500)).collect();
        { in_analysis().lock().unwrap_or_else(|e| e.into_inner()).insert(sid.to_string()); }
        notify();
        let r = ask_model(&name, &e, &tail_text, &open);
        in_analysis().lock().unwrap_or_else(|e| e.into_inner()).remove(sid);
        match r { Some(v) => only_requested_oks(v, &e), None => items_from_rules(&e) }
    };
    with_registry(|r| { apply_items(r, sid, &name, "auto", &items, &msg_t, when); mark(r, &key, &sig); });
    notify();
}

// ---------- queue: one session at a time, on a separate thread ----------

fn in_analysis() -> &'static Mutex<HashSet<String>> { static A: OnceLock<Mutex<HashSet<String>>> = OnceLock::new(); A.get_or_init(|| Mutex::new(HashSet::new())) }

struct Queue { q: VecDeque<String>, dentro: HashSet<String>, vivo: bool }
fn queue() -> &'static (Mutex<Queue>, Condvar) {
    static C: OnceLock<(Mutex<Queue>, Condvar)> = OnceLock::new();
    C.get_or_init(|| (Mutex::new(Queue { q: VecDeque::new(), dentro: HashSet::new(), vivo: false }), Condvar::new()))
}

pub fn put_items(sid: &str) {
    if sid.is_empty() || sid.starts_with("locale:") { return; }
    let (m, c) = queue();
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if g.dentro.insert(sid.to_string()) { g.q.push_back(sid.to_string()); }
    if !g.vivo {
        g.vivo = true;
        std::thread::spawn(|| loop {
            let sid = {
                let (m, c) = queue();
                let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
                while g.q.is_empty() { g = c.wait(g).unwrap_or_else(|e| e.into_inner()); }
                let x = g.q.pop_front().unwrap_or_default();
                g.dentro.remove(&x);
                x
            };
            crate::run_guarded("ok", || analyze(&sid));
        });
    }
    c.notify_all();
}

struct Sweep { prima: HashMap<String, (String, f64)>, file: HashMap<String, u128>, lungo: f64 }
fn sweep_state() -> &'static Mutex<Option<Sweep>> { static G: OnceLock<Mutex<Option<Sweep>>> = OnceLock::new(); G.get_or_init(|| Mutex::new(None)) }

/// Every 3 s from the Sentinel: turns just finished, session files new or changed, and every 5 minutes a check round.
pub fn tick(ss: &[Value]) {
    let mut g = sweep_state().lock().unwrap_or_else(|e| e.into_inner());
    let first = g.is_none();
    let gg = g.get_or_insert_with(|| Sweep { prima: HashMap::new(), file: HashMap::new(), lungo: unix_now() });
    let long_due = unix_now() - gg.lungo > LONG_SWEEP;
    if long_due { gg.lungo = unix_now(); }
    for v in ss {
        let sid = str_of(v, "sessione").to_string();
        if sid.is_empty() { continue; }
        let now_state = (str_of(v, "stato").to_string(), f64_of(v, "da"));
        let p = gg.prima.insert(sid.clone(), now_state.clone());
        let idle = crate::turn_view(v) != "lavora";
        if first || long_due { if idle { put_items(&sid); } continue; }
        // turn finished: the state goes back to "aspetta" (or stays, with a new end time)
        let finished = now_state.0 == "aspetta" && p.map(|p| p != now_state).unwrap_or(true);
        if finished { put_items(&sid); }
    }
    // session files: new, changed or removed
    let mut seen: HashMap<String, u128> = HashMap::new();
    for e in std::fs::read_dir(requests_dir()).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(sid) = name.strip_suffix(".json") else { continue };
        if !crate::storage::is_valid_id(sid) { continue; }
        let t = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis()).unwrap_or(0);
        seen.insert(sid.to_string(), t);
    }
    let changed: Vec<String> = seen.iter().filter(|(k, t)| gg.file.get(*k) != Some(t)).map(|(k, _)| k.clone()).collect();
    let removed: Vec<String> = gg.file.keys().filter(|k| !seen.contains_key(*k)).cloned().collect();
    gg.file = seen;
    drop(g);
    for sid in changed { std::thread::spawn(move || crate::run_guarded("ok-file", || import_file(&sid))); }
    for sid in removed { std::thread::spawn(move || crate::run_guarded("ok-file", || file_removed(&sid))); }
}

// ---------- commands ----------

/// Pending oks (open, of live sessions): the badge number.
pub fn n_ok(ss: &[Value]) -> usize {
    let alive: HashSet<&str> = ss.iter().map(|v| str_of(v, "sessione")).collect();
    registry()["voci"].as_array().map(|a| a.iter().filter(|v| str_of(v, "tipo") == "ok" && str_of(v, "stato") == "aperta" && alive.contains(str_of(v, "sessione"))).count()).unwrap_or(0)
}

/// All items, with name, project and state of the live sessions.
pub fn all_items() -> Value {
    let alive: HashMap<String, Value> = crate::list_sessions().into_iter().map(|v| (str_of(&v, "sessione").to_string(), v)).collect();
    let mut items: Vec<Value> = registry()["voci"].as_array().cloned().unwrap_or_default();
    for x in items.iter_mut() {
        if let Some(v) = alive.get(str_of(x, "sessione")) {
            x["nome"] = json!(str_of(v, "nome")); x["progetto"] = v.get("progetto").cloned().unwrap_or(Value::Null);
            x["vista_sessione"] = json!(str_of(v, "vista")); x["viva"] = json!(true);
        } else { x["viva"] = json!(false); x["progetto"] = Value::Null; }
    }
    items.retain(|x| x["viva"] == json!(true));
    items.sort_by(|a, b| f64_of(a, "creata").partial_cmp(&f64_of(b, "creata")).unwrap_or(std::cmp::Ordering::Equal));
    let n = items.iter().filter(|v| str_of(v, "tipo") == "ok" && str_of(v, "stato") == "aperta").count();
    let analyzing: Vec<String> = in_analysis().lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect();
    json!({"voci": items, "ora": unix_now(), "n_ok": n, "in_analisi": analyzing})
}

/// Text to send to the session for a button.
/// People something can depend on ("da Mario"): personale.json {"persone": ["mario", ...]}.
fn waiting_on_person(l: &str) -> bool {
    crate::profile::personal().get("persone").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str())
        .any(|n| !n.is_empty() && l.contains(&format!("da {}", n.to_lowercase())))).unwrap_or(false)
}

/// Codes like CO-13E5 / HF-A1B2: tokens with uppercase letters, a dash and digits (the collab/hf tools look for them in the ok).
fn codes(t: &str) -> Vec<String> {
    t.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-').to_string())
        .filter(|w| w.contains('-') && w.chars().any(|c| c.is_ascii_uppercase()) && w.chars().any(|c| c.is_ascii_digit())).collect()
}
/// The line to send to the session for an ok: the one written by the agent (`testo`), not the title rewritten by the model
/// ("Send the link to a collaborator's agent": it lost the code CO-13E5, 12/11). Every code present in title or text stays.
pub fn ok_line(title: &str, text: &str) -> String {
    let (title, text) = (title.trim(), text.trim());
    let mut base = if text.is_empty() { title.to_string() } else { text.to_string() };
    for x in codes(title).into_iter().chain(codes(text)) { if !base.contains(&x) { base = format!("{base} ({x})"); } }
    base
}

pub fn text_for(action: &str, title: &str, by_user: bool, text: &str) -> Option<String> { text_for_raw(action, title, by_user, text).map(|t| crate::profile::with_name(&t)) }
fn text_for_raw(action: &str, title: &str, by_user: bool, text: &str) -> Option<String> {
    match action {
        "ok" => Some(format!("{FROM_USER_PREFIX} {}: {title}", if by_user { "Fatto" } else { "Ok" })),
        "no" => Some(format!("{FROM_USER_PREFIX} No: {title}")),
        // checks of the ```juggl-checklist checklists (title = items joined by "; ")
        "fatto" => Some(format!("{FROM_USER_PREFIX} Fatto: {title}")),
        "non_fatto" => Some(format!("{FROM_USER_PREFIX} Non ancora fatto (tolta la spunta): {title}")),
        "rispondi" if !text.trim().is_empty() => Some(format!("{FROM_USER_PREFIX} Su \"{title}\": {}", text.trim())),
        _ => None,
    }
}

/// Buttons on an item: ok, no, reply (with text) send the answer to the session; remove drops it without writing.
pub fn action(id: &str, action: &str, text: &str) -> Value {
    let Some(v) = registry()["voci"].as_array().and_then(|a| a.iter().find(|v| str_of(v, "id") == id).cloned()) else { return json!({"ok": false, "msg": "voce non trovata"}) };
    if action == "togli" {
        with_registry(|r| { if let Some(a) = r["voci"].as_array_mut() { for x in a.iter_mut() { if str_of(x, "id") == id { x["stato"] = json!("fatta"); x["fatta_il"] = json!(unix_now()); x["tolta"] = json!(true); } } } });
        notify();
        return json!({"ok": true});
    }
    if str_of(&v, "stato") == "fatta" { return json!({"ok": false, "msg": "non e' piu' aperta"}) }
    if matches!(action, "ok" | "no") && (str_of(&v, "tipo") != "ok" || str_of(&v, "stato") != "aperta") { return json!({"ok": false, "msg": "risposta gia' mandata"}) }
    let line = ok_line(str_of(&v, "titolo"), str_of(&v, "testo"));
    let Some(msg) = text_for(action, &line, v["fai_tu"].as_bool().unwrap_or(false), text) else { return json!({"ok": false, "msg": "azione sconosciuta"}) };
    let sid = str_of(&v, "sessione").to_string();
    // Ok and No are not new requests (invia_interno); the written reply is, and goes through the requests like any message
    let r = if action == "rispondi" { crate::invia(sid.clone(), msg.clone(), None, None) } else { crate::send_internal(sid.clone(), msg.clone()) };
    if r["ok"].as_bool().unwrap_or(false) {
        with_registry(|reg| {
            if let Some(a) = reg["voci"].as_array_mut() {
                for x in a.iter_mut() {
                    if str_of(x, "id") != id { continue; }
                    if str_of(x, "tipo") == "ok" { x["stato"] = json!("in_conferma"); }
                    x["risposta"] = json!(action); x["risposta_testo"] = json!(msg); x["risposta_il"] = json!(unix_now()); x["esito"] = json!(str_of(&r, "esito"));
                }
            }
        });
        notify();
    }
    let mut out = r;
    out["inviato"] = json!(msg);
    out
}

/// Oks of a session waiting for the user or for their confirmation (to link the requests in requests.rs to them).
pub fn oks_of(sid: &str) -> Vec<Value> {
    registry()["voci"].as_array().map(|a| a.iter().filter(|v| str_of(v, "sessione") == sid && str_of(v, "tipo") == "ok" && matches!(str_of(v, "stato"), "aperta" | "in_conferma")).cloned().collect()).unwrap_or_default()
}
/// All oks still open or awaiting confirmation, by id.
pub fn open_oks() -> HashMap<String, Value> {
    // fast chats (14/11): from the cached registry, without copying it all (reads a whole version of the file)
    let reg = crate::fast_open::json(&registry_path()).unwrap_or_else(|| std::sync::Arc::new(json!({})));
    reg["voci"].as_array().map(|a| a.iter().filter(|v| str_of(v, "tipo") == "ok" && matches!(str_of(v, "stato"), "aperta" | "in_conferma")).map(|v| (str_of(v, "id").to_string(), v.clone())).collect()).unwrap_or_default()
}

/// Items of a session (open and awaiting confirmation), for the block at the bottom of the reply in the chat.
pub fn of_session(sid: &str) -> Value {
    let reg = crate::fast_open::json(&registry_path()).unwrap_or_else(|| std::sync::Arc::new(json!({})));   // fast chats (14/11)
    json!(reg["voci"].as_array().map(|a| a.iter().filter(|v| str_of(v, "sessione") == sid && str_of(v, "stato") != "fatta").cloned().collect::<Vec<_>>()).unwrap_or_default())
}

/// Buttons of the in-chat block for a line that is not (yet) in the registry: same text and same path as `action`.
pub fn send_answer(sid: &str, action: &str, title: &str, text: &str) -> Value {
    if !crate::storage::is_valid_id(sid) { return json!({"ok": false, "msg": "sessione non valida"}) }
    let title = short(title.trim(), 300);
    if title.is_empty() { return json!({"ok": false, "msg": "manca il testo"}) }
    let Some(msg) = text_for(action, &title, false, text) else { return json!({"ok": false, "msg": "azione sconosciuta"}) };
    let mut r = if action == "rispondi" { crate::invia(sid.to_string(), msg.clone(), None, None) } else { crate::send_internal(sid.to_string(), msg.clone()) };
    r["inviato"] = json!(msg);
    r
}

/// Delete chat: its items go away too.
pub fn remove_session(sid: &str) {
    with_registry(|r| {
        if let Some(a) = r["voci"].as_array_mut() { a.retain(|v| str_of(v, "sessione") != sid); }
        if let Some(o) = r["sessioni"].as_object_mut() { o.remove(sid); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Ok, chiuso anche Bisc.\n\nServe il tuo ok: pubblico su /updates i testi delle sette funzioni accese.\nServe il tuo ok: tengo l'esclusione di Windows Defender su D:\\android, sul PC.\n\
Serve il tuo ok: dopo la tua prova della 1.1.59 mando l'app ad Apple e la metto su Play.\n\n\
Restano: rigenerare il token API di PROGETTO su Meta (lo fai tu), correzioni contro i riavvii, server e app (in corso), prova della release Android firmata dal PC, tre bozze video in inglese (in corso), tua prova e quella di Anna sulla 1.1.59 e poi la 1.1.60, prova di stampa di Luca, T5 oggi.";

    #[test]
    fn codes_stay_in_the_ok() {
        assert_eq!(ok_line("Mandare all'agente di Marco il link", "mando all'agente di Marco il messaggio CO-13E5"), "mando all'agente di Marco il messaggio CO-13E5");
        assert_eq!(ok_line("Mandare la richiesta", "mando la richiesta a Higgsfield (HF-A1B2)"), "mando la richiesta a Higgsfield (HF-A1B2)");
        assert_eq!(ok_line("Mandare CO-13E5 a Marco", ""), "Mandare CO-13E5 a Marco");
        assert_eq!(ok_line("Pubblicare la 1.2", "pubblico la 1.2 su App Store"), "pubblico la 1.2 su App Store");
    }

    #[test]
    fn rules_sample() {
        let e = extract_from_rules(SAMPLE);
        assert_eq!(e.ok.len(), 3, "{e:?}");
        assert!(e.ok[0].starts_with("pubblico su /updates"));
        // the comma inside an item ("riavvii, server e app") splits it: then the model joins them back
        assert_eq!(e.restano.len(), 8, "{:?}", e.restano);
        assert_eq!(e.restano[0], "rigenerare il token API di PROGETTO su Meta (lo fai tu)");
        assert_eq!(e.restano[7], "T5 oggi");
        let v = items_from_rules(&e);
        assert!(v.iter().filter(|x| str_of(x, "tipo") == "ok").count() >= 4, "{v:?}");
        assert_eq!(str_of(&v[3], "titolo"), "Rigenerare il token API di PROGETTO su Meta");
        assert_eq!(v[3]["fai_tu"], json!(true));
    }

    #[test]
    fn rules_blocks() {
        let t = "Fatto il lavoro.\n\n> Ciao Alberto, mi confermi l'indirizzo?\n\n```\nperche?\n```\n\n**Restano:**\n- studio del gruppo Sud dev (in corso);\n- regola nel CLAUDE.md globale (serve il tuo sì);\n- provare la build 202611081354 (tu).\n\nL'app è ancora tolta dagli Stati Uniti: dimmi se vuoi riattivarla.";
        let e = extract_from_rules(t);
        assert_eq!(e.restano, vec!["studio del gruppo Sud dev (in corso)", "regola nel CLAUDE.md globale (serve il tuo sì)", "provare la build 202611081354 (tu)"]);
        assert!(e.ok.is_empty());
        assert_eq!(e.domande.len(), 1, "{:?}", e.domande);
        assert!(e.domande[0].contains("dimmi se vuoi"));
        assert!(extract_from_rules("Tutto fatto, niente da segnalare.").is_empty());
    }

    #[test]
    fn merge_and_close() {
        let mut r = normalize(json!({}));
        let a = vec![json!({"titolo": "Pubblicare i testi", "tipo": "ok", "testo": "pubblico i testi"}), json!({"titolo": "Bozze video", "tipo": "in_corso"})];
        assert_eq!(apply_items(&mut r, "s1", "PROGETTO", "auto", &a, "2026-11-08T10:00:00.000Z", 10.0), 2);
        let id = str_of(&r["voci"][0], "id").to_string();
        // same round: no duplicates
        assert_eq!(apply_items(&mut r, "s1", "PROGETTO", "auto", &a, "2026-11-08T11:00:00.000Z", 20.0), 0);
        assert_eq!(r["voci"].as_array().unwrap().len(), 2);
        // ok given: awaiting confirmation as long as the session lists it among the oks
        r["voci"][0]["stato"] = json!("in_conferma");
        apply_items(&mut r, "s1", "PROGETTO", "auto", &[json!({"id": id, "titolo": "Pubblicare i testi su updates", "tipo": "ok"}), a[1].clone()], "", 0.0);
        assert_eq!(str_of(&r["voci"][0], "stato"), "in_conferma");
        // no longer appears: done
        apply_items(&mut r, "s1", "PROGETTO", "auto", &[a[1].clone()], "", 0.0);
        assert_eq!(str_of(&r["voci"][0], "stato"), "fatta");
        assert_eq!(str_of(&r["voci"][1], "stato"), "aperta");
        assert_eq!(f64_of(&r["voci"][1], "creata"), 10.0);
    }

    /// Real case (08/11): Ok pressed on "Pubblicare su /updates…", then the session asks for a NEW ok
    /// that looks similar ("Pubblico /updates cosi'…"). It must be a new item, open, with the buttons; the old one becomes done.
    #[test]
    fn new_ok_after_the_user_reply() {
        let mut r = normalize(json!({}));
        let t0 = 1794151000.0;   // reply with the ok
        apply_items(&mut r, "s1", "PROGETTO", "auto", &[json!({"titolo": "Pubblicare su /updates i testi delle sette funzioni", "tipo": "ok", "testo": "pubblico su /updates i testi delle sette funzioni accese."})], "2026-11-08T15:38:24.452Z", t0);
        let id = str_of(&r["voci"][0], "id").to_string();
        // re-reading the same reply (before the button): same item
        assert_eq!(apply_items(&mut r, "s1", "PROGETTO", "auto", &[json!({"id": id, "titolo": "Pubblicare su /updates i testi delle sette funzioni", "tipo": "ok"})], "2026-11-08T15:38:24.452Z", t0), 0);
        // the user presses Ok at 15:56
        r["voci"][0]["stato"] = json!("in_conferma"); r["voci"][0]["risposta"] = json!("ok"); r["voci"][0]["risposta_il"] = json!(t0 + 1100.0);
        // next reply of the session (16:40) with a new similar ok, and the model sends back the old id
        let t1 = t0 + 3800.0;
        apply_items(&mut r, "s1", "PROGETTO", "auto", &[json!({"id": id, "titolo": "Pubblicare su /updates i testi delle sette funzioni", "tipo": "ok", "testo": "pubblico /updates così, come negli screenshot."})], "2026-11-08T16:40:53.417Z", t1);
        let vv = r["voci"].as_array().unwrap();
        assert_eq!(vv.len(), 2, "{vv:?}");
        let old = vv.iter().find(|v| str_of(v, "id") == id).unwrap();
        let new_item = vv.iter().find(|v| str_of(v, "id") != id).unwrap();
        assert_eq!(str_of(old, "stato"), "fatta");
        assert_eq!(str_of(new_item, "stato"), "aperta");
        assert!(new_item.get("risposta").is_none());
        assert_eq!(f64_of(new_item, "creata"), t1);
        // the same reply re-read: the new item stays a single one
        apply_items(&mut r, "s1", "PROGETTO", "auto", &[json!({"titolo": "Pubblicare su /updates i testi delle sette funzioni", "tipo": "ok", "testo": "pubblico /updates così, come negli screenshot."})], "2026-11-08T16:40:53.417Z", t1);
        assert_eq!(r["voci"].as_array().unwrap().iter().filter(|v| str_of(v, "stato") == "aperta").count(), 1);
        // duplicate done items: one stays
        r["voci"].as_array_mut().unwrap().push(json!({"id": "kx", "sessione": "s1", "titolo": "Pubblicare su /updates i testi delle sette funzioni", "stato": "fatta", "fatta_il": unix_now() - 10.0, "tipo": "ok"}));
        cleanup(&mut r);
        assert_eq!(r["voci"].as_array().unwrap().iter().filter(|v| str_of(v, "stato") == "fatta").count(), 1);
    }

    #[test]
    fn button_texts() {
        assert_eq!(text_for("ok", "Pubblicare i testi", false, "").unwrap(), crate::profile::with_name("[Da «NOME», via Juggl] Ok: Pubblicare i testi"));
        assert_eq!(text_for("ok", "Rigenerare il token", true, "").unwrap(), crate::profile::with_name("[Da «NOME», via Juggl] Fatto: Rigenerare il token"));
        assert_eq!(text_for("no", "X", false, "").unwrap(), crate::profile::with_name("[Da «NOME», via Juggl] No: X"));
        assert_eq!(text_for("rispondi", "X", false, " solo domani ").unwrap(), crate::profile::with_name("[Da «NOME», via Juggl] Su \"X\": solo domani"));
        assert!(text_for("rispondi", "X", false, "  ").is_none());
    }

    /// On real data, read-only: reads every session's last reply and writes the registry to JUGGL_OK_FILE.
    /// cargo test --release approvals::tests::real_analysis -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_analysis() {
        assert!(std::env::var("JUGGL_OK_FILE").is_ok(), "serve JUGGL_OK_FILE");
        for v in crate::list_sessions() { let sid = str_of(&v, "sessione").to_string(); let t = unix_now(); analyze(&sid); eprintln!("{} {} {:.1}s", &sid[..8], str_of(&v, "nome"), unix_now() - t); }
        let t = all_items();
        for v in t["voci"].as_array().unwrap() { eprintln!("[{}|{}{}] {} :: {} :: {}", str_of(v, "tipo"), str_of(v, "stato"), if v["fai_tu"] == json!(true) { "|tu" } else { "" }, str_of(v, "nome"), str_of(v, "titolo"), str_of(v, "se_si")); }
        eprintln!("ok aperti: {}", t["n_ok"]);
    }

    /// Full test on a fake home (HOME in a test folder): a fake session hosted by Juggl,
    /// the Ok button puts the message in its queue and the item moves to "in_conferma". No real session.
    #[test]
    fn ok_button_on_fake_session() {
        let home_dir = std::env::var("OK_PROVA_HOME").unwrap_or_else(|_| format!("{}/tavolo/prova/ok-casa", std::env::var("HOME").unwrap()));
        let _ = std::fs::remove_dir_all(&home_dir);
        std::fs::create_dir_all(format!("{home_dir}/tavolo/stato/richieste")).unwrap();
        let _base_lock = crate::test_data_lock(); // HOME also decides the data folder: one test at a time
        let _env = crate::EnvGuard::new(&["HOME"]);
        std::env::set_var("HOME", &home_dir);
        let sid = "00000000-0000-4000-8000-0000000000ab";
        crate::storage::write_json(&base_dir().join("stato").join(format!("{sid}.json")), &json!({"sessione": sid, "stato": "aspetta", "da": unix_now(), "agg": unix_now(), "ripresa_il": unix_now(), "ospite": "tavolo", "titolo": "Sessione finta", "cwd": home_dir})).unwrap();
        // optional source: the session's file
        crate::storage::write_json(&file_of(sid), &json!([{"id": "a1", "titolo": "Mando il messaggio a Luca", "tipo": "ok", "creato": "2026-11-08T09:00:00Z", "dettaglio": "bozza pronta"}, {"id": "a2", "titolo": "Build iPhone", "tipo": "in_corso"}])).unwrap();
        import_file(sid);
        let t = all_items();
        assert_eq!(t["n_ok"], json!(1), "{t}");
        let v = t["voci"].as_array().unwrap().iter().find(|v| str_of(v, "tipo") == "ok").unwrap().clone();
        assert!(f64_of(&v, "creata") > 1.79e9);
        let r = action(str_of(&v, "id"), "ok", "");
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(str_of(&r, "esito"), "in_coda");
        let queued = crate::read_queue(sid);
        assert_eq!(str_of(&queued[0], "testo"), crate::profile::with_name("[Da «NOME», via Juggl] Ok: Mando il messaggio a Luca"));
        let after = registry()["voci"].as_array().unwrap().iter().find(|x| str_of(x, "id") == str_of(&v, "id")).unwrap().clone();
        assert_eq!(str_of(&after, "stato"), "in_conferma");
        // second time: no double send
        assert_eq!(action(str_of(&v, "id"), "ok", "")["ok"], json!(false));
        // the session updates the file without that item: done
        crate::storage::write_json(&file_of(sid), &json!([{"id": "a2", "titolo": "Build iPhone", "tipo": "in_corso"}])).unwrap();
        import_file(sid);
        let final_item = registry()["voci"].as_array().unwrap().iter().find(|x| str_of(x, "id") == str_of(&v, "id")).unwrap().clone();
        assert_eq!(str_of(&final_item, "stato"), "fatta");
        assert_eq!(n_ok(&crate::list_sessions()), 0);
    }
}
