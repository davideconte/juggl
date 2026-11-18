// Juggl's helpers: background calls to the small model (requests, summaries, pilot, permissions, external messages, names).
// (0.1.70, "consumption reduction", requested by the user on Nov 15.) They all go through `autopilot::ask_for`, which here finds:
//  - a counter per category (file `consumi-aiutanti.jsonl`, 48 hours) feeding "Consumi di Juggl, ultime 24 h";
//  - a switch for each helper (file `aiutanti.json`);
//  - the 24-hour cap for the requests extractor: when exceeded, the request is NOT discarded (the caller postpones it);
//  - the cache by text hash: the same text is not analyzed twice.
use crate::{base_dir, unix_now};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

pub const CATEGORIES: &[&str] = &["estrattore", "riassunti", "pilota", "permessi",
    "nomi", "supporto", "altro"];
const DEFAULT_PAUSE: f64 = 150.0;
const DEFAULT_CAP: u64 = 600;
const HOURS_24: f64 = 24.0 * 3600.0;
const CACHE_TTL: f64 = 6.0 * 3600.0;
const CACHE_MAX: usize = 300;

fn settings_file() -> PathBuf { base_dir().join("aiutanti.json") }
fn log_file() -> PathBuf { base_dir().join("consumi-aiutanti.jsonl") }
fn lock() -> &'static Mutex<()> { static L: OnceLock<Mutex<()>> = OnceLock::new(); L.get_or_init(|| Mutex::new(())) }

/// Settings with default values: `attivi` (one switch per category), `pausa_estrattore_s`, `extractor_cap`.
pub fn settings() -> Value {
    let v = crate::load_json(&settings_file()).unwrap_or(json!({}));
    let mut active = serde_json::Map::new();
    for &c in CATEGORIES { active.insert(c.into(), json!(v["attivi"][c].as_bool().unwrap_or(true))); }
    json!({
        "attivi": active,
        "pausa_estrattore_s": v["pausa_estrattore_s"].as_f64().unwrap_or(DEFAULT_PAUSE).clamp(5.0, 3600.0),
        "tetto_estrattore": v["tetto_estrattore"].as_u64().unwrap_or(DEFAULT_CAP).clamp(20, 5000),
    })
}

/// Changes only the fields present (and valid). Returns the new settings.
pub fn set(new_values: &Value) -> Value {
    let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut v = settings();
    if let Some(o) = new_values["attivi"].as_object() {
        for (k, x) in o { if CATEGORIES.contains(&k.as_str()) { if let Some(b) = x.as_bool() { v["attivi"][k] = json!(b); } } }
    }
    if let Some(p) = new_values["pausa_estrattore_s"].as_f64() { v["pausa_estrattore_s"] = json!(p.clamp(5.0, 3600.0)); }
    if let Some(t) = new_values["tetto_estrattore"].as_u64() { v["tetto_estrattore"] = json!(t.clamp(20, 5000)); }
    let _ = crate::storage::write_json(&settings_file(), &v);
    v
}

pub fn is_enabled(cat: &str) -> bool { settings()["attivi"][cat].as_bool().unwrap_or(true) }
pub fn extractor_pause() -> f64 { settings()["pausa_estrattore_s"].as_f64().unwrap_or(DEFAULT_PAUSE) }
pub fn extractor_cap() -> u64 { settings()["tetto_estrattore"].as_u64().unwrap_or(DEFAULT_CAP) }

/// One line in the log: outcome = ok | errore (real calls), cache | spenta | rimandata (saved calls).
pub fn record(cat: &str, outcome: &str) {
    let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
    let p = log_file();
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; o.mode(0o600); }   // the consumption log is created with mode 600
    if let Ok(mut f) = o.open(&p) {
        let _ = writeln!(f, "{}", json!({"t": unix_now(), "c": cat, "e": outcome}));
    }
    // every now and then the log is trimmed to the last 48 hours
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 400 == 399 { trim(&p); }
}

fn log_rows() -> Vec<(f64, String, String)> {
    let t = std::fs::read_to_string(log_file()).unwrap_or_default();
    t.lines().filter_map(|r| serde_json::from_str::<Value>(r).ok())
        .filter_map(|v| Some((v["t"].as_f64()?, v["c"].as_str()?.to_string(), v["e"].as_str()?.to_string()))).collect()
}

fn trim(p: &PathBuf) {
    let limit = unix_now() - 2.0 * HOURS_24;
    let kept: Vec<String> = log_rows().into_iter().filter(|r| r.0 >= limit).map(|r| json!({"t": r.0, "c": r.1, "e": r.2}).to_string()).collect();
    let _ = crate::storage::write(p, (kept.join("\n") + "\n").as_bytes());
}

/// Real calls (ok + errore) of a category in the last 24 hours.
pub fn calls_24h(cat: &str) -> u64 {
    let since = unix_now() - HOURS_24;
    log_rows().iter().filter(|r| r.0 >= since && r.1 == cat && (r.2 == "ok" || r.2 == "errore")).count() as u64
}

/// Is the extractor cap exceeded? (The other categories have no cap.)
pub fn over_cap(cat: &str) -> bool { cat == "estrattore" && calls_24h(cat) >= extractor_cap() }

/// "Consumi di Juggl, ultime 24 h": calls per category and savings, cap and warning.
pub fn usage_24h() -> Value {
    let since = unix_now() - HOURS_24;
    let mut per_cat: HashMap<String, [u64; 5]> = HashMap::new();
    for (t, c, e) in log_rows() {
        if t < since { continue; }
        let x = per_cat.entry(c).or_insert([0; 5]);
        match e.as_str() { "ok" => x[0] += 1, "errore" => x[1] += 1, "cache" => x[2] += 1, "spenta" => x[3] += 1, "rimandata" => x[4] += 1, _ => {} }
    }
    let settings = settings();
    let mut cat = serde_json::Map::new();
    let (mut total, mut saved) = (0u64, 0u64);
    for &c in CATEGORIES {
        let x = per_cat.get(c).copied().unwrap_or([0; 5]);
        total += x[0] + x[1]; saved += x[2] + x[3] + x[4];
        cat.insert(c.into(), json!({"chiamate": x[0] + x[1], "errori": x[1], "cache": x[2], "spenta": x[3], "rimandate": x[4], "attivo": settings["attivi"][c]}));
    }
    let cap = settings["tetto_estrattore"].as_u64().unwrap_or(DEFAULT_CAP);
    let extractor = per_cat.get("estrattore").copied().unwrap_or([0; 5]);
    let above = extractor[0] + extractor[1] >= cap;
    json!({
        "categorie": cat, "totale_chiamate": total, "risparmiate": saved, "tetto_estrattore": cap, "sopra_tetto": above,
        "avviso": if above { format!("Tetto di {cap} chiamate in 24 ore raggiunto: le richieste nuove vengono analizzate piu' tardi, nessuna si perde.") } else { String::new() },
    })
}

// ---------- cache by hash ----------

fn hash_cache() -> &'static Mutex<HashMap<u64, (f64, Option<Value>)>> { static C: OnceLock<Mutex<HashMap<u64, (f64, Option<Value>)>>> = OnceLock::new(); C.get_or_init(|| Mutex::new(HashMap::new())) }

pub fn hash(cat: &str, text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cat.hash(&mut h); text.hash(&mut h); h.finish()
}

/// Already analyzed recently? `Some(remembered answer)` if so (the answer is `None` for those that do not remember it).
pub fn seen_recently(h: u64) -> Option<Option<Value>> {
    let mut c = hash_cache().lock().unwrap_or_else(|e| e.into_inner());
    let now_ = unix_now();
    c.retain(|_, v| now_ - v.0 < CACHE_TTL);
    c.get(&h).map(|v| v.1.clone())
}

pub fn remember(h: u64, answer: Option<Value>) {
    let mut c = hash_cache().lock().unwrap_or_else(|e| e.into_inner());
    if c.len() >= CACHE_MAX { if let Some(k) = c.iter().min_by(|a, b| a.1.0.partial_cmp(&b.1.0).unwrap_or(std::cmp::Ordering::Equal)).map(|x| *x.0) { c.remove(&k); } }
    c.insert(h, (unix_now(), answer));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_counters_cap_and_cache() {
        let _base_lock = crate::test_data_lock();
        let d = std::env::temp_dir().join(format!("juggl-aiutanti-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap();
        std::env::set_var("JUGGL_BASE", &d);
        // defaults: everything on, pause 150 s, cap 600
        let i = settings();
        assert_eq!(i["attivi"]["estrattore"], true);
        assert_eq!(extractor_pause(), 150.0);
        assert_eq!(extractor_cap(), 600);
        // change only some fields, out-of-range values clamped, unknown categories ignored
        set(&json!({"attivi": {"riassunti": false, "inventata": false}, "pausa_estrattore_s": 99999.0, "tetto_estrattore": 10}));
        assert!(!is_enabled("riassunti")); assert!(is_enabled("estrattore"));
        assert_eq!(extractor_pause(), 3600.0);
        assert_eq!(extractor_cap(), 20);
        set(&json!({"pausa_estrattore_s": 150.0, "tetto_estrattore": 3, "attivi": {"riassunti": true}}));
        // the cap: below no, at the cap yes; only for the extractor
        for _ in 0..19 { record("estrattore", "ok"); }
        assert!(!over_cap("estrattore"));
        record("estrattore", "errore");
        assert!(over_cap("estrattore"), "20 chiamate (ok + errore) = tetto minimo raggiunto");
        for _ in 0..30 { record("riassunti", "ok"); }
        assert!(!over_cap("riassunti"), "le altre categorie non hanno tetto");
        // saved calls do not count toward the cap but show in the consumption
        record("estrattore", "cache"); record("estrattore", "rimandata"); record("pilota", "spenta");
        let c = usage_24h();
        assert_eq!(c["categorie"]["estrattore"]["chiamate"], 20);
        assert_eq!(c["categorie"]["estrattore"]["cache"], 1);
        assert_eq!(c["categorie"]["estrattore"]["rimandate"], 1);
        assert_eq!(c["categorie"]["pilota"]["spenta"], 1);
        assert_eq!(c["totale_chiamate"], 50);
        assert_eq!(c["sopra_tetto"], true);
        assert!(!c["avviso"].as_str().unwrap().is_empty());
        // the cache by hash
        let h = hash("estrattore", "stesso testo");
        assert!(seen_recently(h).is_none());
        remember(h, Some(json!({"ok": 1})));
        assert_eq!(seen_recently(h), Some(Some(json!({"ok": 1}))));
        assert_ne!(hash("estrattore", "stesso testo"), hash("riassunti", "stesso testo"), "categorie diverse = hash diversi");
        let _ = std::fs::remove_dir_all(&d);
    }
}
