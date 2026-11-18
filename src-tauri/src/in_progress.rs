// "In corso" (In progress) view (Nov 10): all sessions together, what they are doing, on Mac and iPhone. Most of it already
// arrives with the list (view, project, sub-agents, question, last answer); here, only for sessions at work, we add to the
// list what is missing:
// - "adesso_chiaro": the current action in plain words, the same line as the chat (summaries::adesso);
// - "passi_ic": {fatti, totale} of the steps list, if the agent has one.
// One read of the transcript tail per session, kept until the transcript grows and in any case at most
// one every 2 seconds: the list is recomputed on every "cambiato".
use crate::{detail_tail, str_of, find_transcript};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

fn current_of(v: &Value) -> Value {
    static CACHE: Mutex<Option<HashMap<PathBuf, (u64, Instant, Value)>>> = Mutex::new(None);
    let Some(p) = find_transcript(v) else { return Value::Null };
    let weight = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    if let Some((q, t, x)) = CACHE.lock().ok().and_then(|c| c.as_ref().and_then(|m| m.get(&p).cloned())) {
        if q == weight || t.elapsed().as_secs_f64() < 2.0 { return x; }
    }
    let d = detail_tail(Some(p.clone()), true, 300_000, "");
    let mut o = Map::new();
    o.insert("vista".into(), json!("lavora"));
    o.insert("eventi".into(), d["eventi"].clone());
    let steps = d["passi"].as_array().cloned().unwrap_or_default();
    let x = json!({
        "adesso": crate::summaries::now_line(&o),
        "passi": if steps.is_empty() { Value::Null } else { json!({"fatti": steps.iter().filter(|p| str_of(p, "stato") == "completed").count(), "totale": steps.len()}) },
    });
    if let Ok(mut c) = CACHE.lock() {
        let m = c.get_or_insert_with(HashMap::new);
        if m.len() > 64 { m.clear(); }
        m.insert(p, (weight, Instant::now(), x.clone()));
    }
    x
}

/// In the session list (dati_tavolo, also for the iPhone): "Adesso" line and steps for those at work.
pub fn enrich(ss: &mut [Value]) {
    juggl_into_its_group(ss);   // in progress stable (Nov 12)
    mark_sections(ss);          // in progress stable (Nov 12)
    for v in ss.iter_mut() {
        if str_of(v, "vista") != "lavora" || str_of(v, "sessione").starts_with("locale:") { continue; }
        let x = current_of(v);
        // turn finished and only sub-agents at work: no "Adesso" (the last entry is the answer already given),
        // the card says how many sub-agents are working
        if crate::turn_view(v) == "lavora" { if let Some(a) = x["adesso"].as_str() { v["adesso_chiaro"] = json!(a); } }
        if !x["passi"].is_null() { v["passi_ic"] = x["passi"].clone(); }
    }
}

// ---------- in progress stable (Nov 12) ----------
// Juggl included: the general chat (juggl_id, folder ~/tavolo/generale-juggl) and the development chats in the
// work copies next to ~/tavolo (~/tavolo-windows, ~/tavolo-motori, ~/tavolo-modifiche...) without a project end up in the
// Juggl project (the one with the ~/tavolo folder), like the other development chats. Local chats stay out.
fn juggl_into_its_group(ss: &mut [Value]) {
    let base = crate::base_dir().to_string_lossy().trim_end_matches('/').to_string();
    let projects = crate::session_care::projects();
    let Some(p) = projects.iter().find(|p| { let c = str_of(p, "cartella"); !c.is_empty() && crate::session_care::is_inside(&base, c) && crate::session_care::is_inside(c, &base) }) else { return };
    let juggl_session = crate::juggl_id();
    let copy_prefix = format!("{base}-");
    for v in ss.iter_mut() {
        let id = str_of(v, "sessione").to_string();
        if id.starts_with("locale:") || id.starts_with("wa:") || id.starts_with("chiusa:") || v.get("locale").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
        if !v.get("progetto").map(|x| x.is_null()).unwrap_or(true) { continue; }
        let in_copy = [str_of(v, "cwd"), str_of(v, "cwd_origine")].iter().any(|c| c.starts_with(&copy_prefix));
        if (!juggl_session.is_empty() && id == juggl_session) || in_copy { v["progetto"] = p.clone(); }
    }
}

/// Section of the "In corso" view the session belongs to ("" = not shown).
fn section_of(v: &Value) -> &'static str {
    match str_of(v, "vista") { "aspetta" | "bloccata" => "aspetta", "lavora" => "lavora", "fatta" | "ferma" => "ferma", _ => "" }
}

/// For a stable order (Mac and iPhone): "ic_sezione" (aspetta, lavora, ferma), "ic_dal" (since when the session is in
/// that section: changes only when the section changes) and "ic_gruppo" (the card: "p:<project>" or "s:<session>").
/// Within a section, sort by ic_dal ascending: whoever arrives goes last and no card moves on its own.
fn mark_sections(ss: &mut [Value]) {
    static SINCE: Mutex<Option<HashMap<String, (&'static str, f64)>>> = Mutex::new(None);
    let now = crate::unix_now();
    let Ok(mut g) = SINCE.lock() else { return };
    let first_time = g.is_none();
    let m = g.get_or_insert_with(HashMap::new);
    let mut seen = std::collections::HashSet::new();
    for v in ss.iter_mut() {
        let sect = section_of(v);
        let id = str_of(v, "sessione").to_string();
        if sect.is_empty() || id.is_empty() || id.starts_with("locale:") { continue; }
        // the first time (Juggl just opened) the time of the last state change counts; afterwards the time the section changes
        let since = match m.get(&id) { Some((prev_z, t)) if *prev_z == sect => *t, _ => if first_time { num_field(v, "da") } else { now } };
        m.insert(id.clone(), (sect, since));
        seen.insert(id.clone());
        let project_id = v.get("progetto").map(|p| str_of(p, "id").to_string()).unwrap_or_default();
        v["ic_sezione"] = json!(sect);
        v["ic_dal"] = json!(since);
        v["ic_gruppo"] = json!(if project_id.is_empty() || project_id == "locale" { format!("s:{id}") } else { format!("p:{project_id}") });
    }
    m.retain(|k, _| seen.contains(k));
}

fn num_field(v: &Value, k: &str) -> f64 { v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) }

#[cfg(test)]
mod tests {
    /// in progress stable (Nov 12): ic_dal stays put until the session changes section, then becomes the time of the change.
    #[test]
    fn sections_stay_put() {
        use serde_json::json;
        let mut ss = vec![
            json!({"sessione": "a", "vista": "lavora", "da": 100.0, "progetto": {"id": "tavolo"}}),
            json!({"sessione": "b", "vista": "fatta", "da": 50.0, "progetto": null}),
            json!({"sessione": "locale:x", "vista": "lavora", "da": 10.0}),
        ];
        super::mark_sections(&mut ss);
        assert_eq!(ss[0]["ic_dal"], json!(100.0));
        assert_eq!(ss[0]["ic_gruppo"], json!("p:tavolo"));
        assert_eq!(ss[1]["ic_sezione"], json!("ferma"));
        assert_eq!(ss[1]["ic_gruppo"], json!("s:b"));
        assert!(ss[2].get("ic_dal").is_none());
        // "a" still working (with a new "da"): same place; "b" goes to waiting: time of the change
        ss[0]["da"] = json!(200.0);
        ss[1]["vista"] = json!("aspetta");
        super::mark_sections(&mut ss);
        assert_eq!(ss[0]["ic_dal"], json!(100.0));
        assert_eq!(ss[1]["ic_sezione"], json!("aspetta"));
        assert!(ss[1]["ic_dal"].as_f64().unwrap() > 1_000_000.0);
    }

    /// On real sessions (Nov 10): those at work have the "Adesso" line in the list.
    #[test]
    #[ignore]
    fn real_list() {
        let t = crate::desk_data();
        for v in t["sessioni"].as_array().unwrap() {
            if crate::str_of(v, "vista") == "lavora" { println!("{} | {} | {}", crate::str_of(v, "nome"), v["adesso_chiaro"], v["passi_ic"]); }
        }
    }
}
