// Live Activity of Juggl on iPhone (12/11): the Mac sends updates with the APNs push of type "liveactivity",
// so the lock-screen activity stays up to date even with the app closed and away from home.
// Data shared with the iPhone: ~/tavolo/piani/live-activity-contentstate.md (struct ContentState, same field names).
// - POST /api/live/token {token, attivita}: the iPhone registers the activity token ("" when it ends);
// - loop every 3 seconds: computes the state; if changed sends "update" (immediately if state or waiting count change,
//   otherwise at most one every 15 seconds); "end" when no chat has been working or waiting for 10 minutes.
// Local chats and the general Juggl chat are always excluded.
use crate::{base_dir, load_json, unix_now, str_of};
use serde_json::{json, Value};
use std::sync::Mutex;

const MIN_INTERVAL: f64 = 15.0;
const END_AFTER: f64 = 600.0;

fn token_file() -> std::path::PathBuf { base_dir().join("live-token.json") }
fn start_file() -> std::path::PathBuf { base_dir().join("live-avvio.json") }
/// Start with the app closed (push-to-start): at most one attempt every 10 minutes, if the iPhone does not register the activity.
const START_EVERY: f64 = 600.0;

fn valid_environment(a: &str) -> &'static str { match a { "sandbox" | "development" => "sandbox", "production" => "production", _ => "" } }

/// iPhone: token to start the Live Activity with the app closed ("" to remove it). POST /api/live/avvio {token, ambiente}.
pub fn register_start(token: &str, environment: &str) -> Value {
    let t: String = token.trim().chars().filter(|c| c.is_ascii_hexdigit()).take(400).collect();
    if t.is_empty() { let _ = std::fs::remove_file(start_file()); return json!({"ok": true}); }
    let v = json!({"token": t, "ambiente": valid_environment(environment), "agg": unix_now()});
    if std::fs::write(start_file(), v.to_string()).is_err() { return json!({"ok": false, "msg": "non salvato"}); }
    crate::log_juggl("live activity: token di avvio registrato");
    json!({"ok": true})
}

/// The iPhone registers (or removes, with an empty token) the Live Activity token.
pub fn register(token: &str, activity: &str, environment: &str) -> Value {
    let t: String = token.trim().chars().filter(|c| c.is_ascii_hexdigit()).take(400).collect();
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    *st = Sent::default();
    if t.is_empty() {
        let _ = std::fs::remove_file(token_file());
        crate::log_juggl("live activity: finita, token tolto");
        return json!({"ok": true});
    }
    let v = json!({"token": t, "attivita": activity.chars().take(80).collect::<String>(), "ambiente": valid_environment(environment), "agg": unix_now()});
    if std::fs::write(token_file(), v.to_string()).is_err() { return json!({"ok": false, "msg": "non salvato"}); }
    crate::log_juggl("live activity: token registrato");
    json!({"ok": true})
}

/// State to show (ContentState), from the already enriched list sessions (adesso_chiaro, passi_ic).
pub fn state_from(ss: &[Value]) -> Value {
    let juggl = crate::juggl_id();
    let good: Vec<&Value> = ss.iter().filter(|v| {
        let id = str_of(v, "sessione");
        !id.is_empty() && id != juggl && !id.starts_with("locale:")
    }).collect();
    let needs_you = |v: &Value| v.get("serve_te").map(|x| x.as_bool().unwrap_or(false) || x.as_str() == Some("True") || x.as_str() == Some("true")).unwrap_or(false);
    let latest = |v: &Value| v.get("ultimo_evento").or(v.get("agg")).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|t| t.parse().ok()))).unwrap_or(0.0);
    let working = good.iter().filter(|v| str_of(v, "vista") == "lavora").count();
    let blocked = good.iter().filter(|v| str_of(v, "vista") == "bloccata").count();
    let waiting = good.iter().filter(|v| str_of(v, "vista") == "aspetta" && needs_you(v)).count() + blocked;
    let most_recent = |f: &dyn Fn(&Value) -> bool| good.iter().filter(|v| f(v)).max_by(|a, b| latest(a).partial_cmp(&latest(b)).unwrap_or(std::cmp::Ordering::Equal)).copied();
    let ev = most_recent(&|v| str_of(v, "vista") == "bloccata")
        .or_else(|| most_recent(&|v| str_of(v, "vista") == "aspetta" && needs_you(v)))
        .or_else(|| most_recent(&|v| str_of(v, "vista") == "lavora"));
    let (title, line, state, session, pf, pt) = match ev {
        None => (String::new(), String::new(), "ferma".to_string(), String::new(), 0, 0),
        Some(v) => {
            let view = str_of(v, "vista");
            let line = match view {
                "lavora" => str_of(v, "adesso_chiaro").to_string(),
                "bloccata" => [str_of(v, "domanda"), str_of(v, "serve_te_cosa"), "Chiede un permesso"].into_iter().find(|x| !x.is_empty()).unwrap_or("").to_string(),
                _ => [str_of(v, "serve_te_cosa"), str_of(v, "domanda")].into_iter().find(|x| !x.is_empty()).unwrap_or("").to_string(),
            };
            let state = if matches!(view, "lavora" | "aspetta" | "bloccata") { view } else { "ferma" };
            (str_of(v, "nome").to_string(), line.chars().take(120).collect(), state.to_string(), str_of(v, "sessione").to_string(),
             v["passi_ic"]["fatti"].as_i64().unwrap_or(0), v["passi_ic"]["totale"].as_i64().unwrap_or(0))
        }
    };
    json!({"lavorano": working, "aspettano": waiting, "bloccate": blocked, "titolo": title, "riga": line,
           "stato": state, "sessione": session, "passiFatti": pf, "passiTotale": pt, "agg": unix_now()})
}

#[derive(Default)]
struct Sent { ultimo: Option<Value>, quando: f64, vuoto_da: f64, finita: bool, avvio_t: f64 }
static STATE: Mutex<Sent> = Mutex::new(Sent { ultimo: None, quando: 0.0, vuoto_da: 0.0, finita: false, avvio_t: 0.0 });

fn without_agg(v: &Value) -> Value { let mut x = v.clone(); if let Some(o) = x.as_object_mut() { o.remove("agg"); } x }

/// What to do now: None, Some(("update", priority)) or Some(("end", 5)). Pure logic, tested.
fn decide(prima: Option<&Value>, when: f64, empty_since: f64, new_state: &Value, now: f64) -> Option<(&'static str, u8)> {
    let empty = new_state["lavorano"].as_i64() == Some(0) && new_state["aspettano"].as_i64() == Some(0);
    if empty && empty_since > 0.0 && now - empty_since >= END_AFTER { return Some(("end", 5)); }
    let urgent = matches!(str_of(new_state, "stato"), "bloccata" | "aspetta");
    let pr = if urgent { 10 } else { 5 };
    match prima {
        None => Some(("update", pr)),
        Some(p) if without_agg(p) == without_agg(new_state) => None,
        Some(p) => {
            let immediately = str_of(p, "stato") != str_of(new_state, "stato") || p["aspettano"] != new_state["aspettano"];
            if immediately || now - when >= MIN_INTERVAL { Some(("update", pr)) } else { None }
        }
    }
}

/// Loop (every 3 seconds from the startup thread): sends the update if needed.
pub fn tick(ss: &[Value]) {
    let Some(reg) = load_json(&token_file()) else { start_if_needed(ss); return };
    let token = str_of(&reg, "token").to_string();
    if token.is_empty() || !crate::push::config_available() { return; }
    // the "Adesso" lines and the steps are only needed here: computed only if there is an activity to update
    let mut ss = ss.to_vec();
    crate::in_progress::enrich(&mut ss);
    let new_state = state_from(&ss);
    let now = unix_now();
    let decision = {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if st.finita { return; }
        let empty = new_state["lavorano"].as_i64() == Some(0) && new_state["aspettano"].as_i64() == Some(0);
        if empty { if st.vuoto_da == 0.0 { st.vuoto_da = now; } } else { st.vuoto_da = 0.0; }
        decide(st.ultimo.as_ref(), st.quando, st.vuoto_da, &new_state, now)
    };
    let Some((event, priority)) = decision else { return };
    let mut aps = json!({"timestamp": now as i64, "event": event, "content-state": new_state});
    if event == "end" { aps["dismissal-date"] = json!(now as i64 + 60); }
    match crate::push::send_live(&token, &json!({"aps": aps}), priority, str_of(&reg, "ambiente")) {
        Ok(()) => {
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            st.ultimo = Some(new_state); st.quando = now;
            if event == "end" { st.finita = true; crate::log_juggl("live activity: finita (nessuna chat da 10 minuti)"); }
        }
        Err(e) => {
            crate::log_juggl(&format!("live activity: push non riuscito: {e}"));
            if e.contains("non piu' valido") { let _ = std::fs::remove_file(token_file()); }
            // retry on the next loop, but not before 15 seconds
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            st.quando = now;
        }
    }
}

/// No open activity: starts it with the app closed (push-to-start) when something works or waits and the user
/// is not at the Mac (same rule as notifications, push::serve). Then the iPhone registers the token with /api/live/token.
fn start_if_needed(ss: &[Value]) {
    let Some(av) = load_json(&start_file()) else { return };
    let token = str_of(&av, "token").to_string();
    if token.is_empty() || !crate::push::is_needed() { return; }
    let now = unix_now();
    {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if now - st.avvio_t < START_EVERY { return; }
    }
    let mut ss = ss.to_vec();
    crate::in_progress::enrich(&mut ss);
    let new_state = state_from(&ss);
    if new_state["lavorano"].as_i64() == Some(0) && new_state["aspettano"].as_i64() == Some(0) { return; }
    let body = if new_state["aspettano"].as_i64().unwrap_or(0) > 0 { "Una chat aspetta te" } else { "Le chat sono al lavoro" };
    let aps = json!({"timestamp": now as i64, "event": "start", "content-state": new_state, "attributes-type": "JugglAttivita",
                     "attributes": {"nome": "Juggl"}, "alert": {"title": "Juggl", "body": body}});
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.avvio_t = now;
    match crate::push::send_live(&token, &json!({"aps": aps}), 10, str_of(&av, "ambiente")) {
        Ok(()) => { *st = Sent { avvio_t: now, ..Default::default() }; crate::log_juggl("live activity: avviata ad app chiusa"); }
        Err(e) => {
            crate::log_juggl(&format!("live activity: avvio non riuscito: {e}"));
            if e.contains("non piu' valido") { let _ = std::fs::remove_file(start_file()); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(id: &str, view: &str, extra: Value) -> Value {
        let mut v = json!({"sessione": id, "nome": format!("chat {id}"), "vista": view, "ultimo_evento": 100.0});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) { for (k, x) in e { o.insert(k.clone(), x.clone()); } }
        v
    }

    #[test]
    fn featured_chat_and_counts() {
        let ss = vec![
            make_session("a", "lavora", json!({"adesso_chiaro": "Compila", "passi_ic": {"fatti": 2, "totale": 5}, "ultimo_evento": 300.0})),
            make_session("b", "aspetta", json!({"serve_te": "True", "serve_te_cosa": "Serve il tuo ok"})),
            make_session("locale:x", "bloccata", json!({})),
        ];
        let st = state_from(&ss);
        assert_eq!((st["lavorano"].as_i64(), st["aspettano"].as_i64(), st["bloccate"].as_i64()), (Some(1), Some(1), Some(0)));
        assert_eq!(st["sessione"], json!("b"));       // first the waiting one, then the working one; the local chat does not count
        assert_eq!(st["riga"], json!("Serve il tuo ok"));
        let st2 = state_from(&ss[..1]);
        assert_eq!((st2["stato"].as_str(), st2["passiFatti"].as_i64(), st2["riga"].as_str()), (Some("lavora"), Some(2), Some("Compila")));
        let st3 = state_from(&[make_session("c", "bloccata", json!({"domanda": "Posso eseguire rm?"})), ss[1].clone()]);
        assert_eq!((st3["sessione"].as_str(), st3["aspettano"].as_i64(), st3["bloccate"].as_i64()), (Some("c"), Some(2), Some(1)));
    }

    #[test]
    fn when_to_send() {
        let a = json!({"lavorano": 1, "aspettano": 0, "stato": "lavora", "riga": "x", "agg": 1.0});
        let b = json!({"lavorano": 1, "aspettano": 0, "stato": "lavora", "riga": "y", "agg": 2.0});
        let c = json!({"lavorano": 1, "aspettano": 1, "stato": "aspetta", "riga": "y", "agg": 3.0});
        let empty = json!({"lavorano": 0, "aspettano": 0, "stato": "ferma", "agg": 4.0});
        assert_eq!(decide(None, 0.0, 0.0, &a, 100.0), Some(("update", 5)));
        assert_eq!(decide(Some(&a), 100.0, 0.0, &a, 101.0), None);                 // equal: nothing
        assert_eq!(decide(Some(&a), 100.0, 0.0, &b, 105.0), None);                 // only the line changes: wait 15 s
        assert_eq!(decide(Some(&a), 100.0, 0.0, &b, 116.0), Some(("update", 5)));
        assert_eq!(decide(Some(&b), 100.0, 0.0, &c, 101.0), Some(("update", 10))); // state changes: immediately, high priority
        assert_eq!(decide(Some(&c), 100.0, 200.0, &empty, 500.0), Some(("update", 5)));
        assert_eq!(decide(Some(&empty), 500.0, 200.0, &empty, 801.0), Some(("end", 5)));
    }
}
