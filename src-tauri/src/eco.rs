// Eco mode (Nov 11): fewer tokens without touching the important work.
// Turned on by hand (Usage view) or automatically, if the user allows it, when at the last hour's pace the weekly limit
// would run out before the reset (and usage is already over 50%). What changes:
// - hosted chats that are NOT pinned (pinned ones are the important work, Juggl stays as it is): Sonnet with medium reasoning,
//   at the end of the turn if they are working (same mechanism as the model switch, `regola`); the previous model stays in
//   "eco_prima" in the state and comes back when Eco is turned off;
// - every chat that restarts gets the Eco rules appended to its system instructions (short answers, essential checks,
//   sub-agents only if indispensable): no extra messages, so no turn spent;
// - new chats start on Sonnet.
// State in impostazioni.json: "eco": {attiva, auto, dal, motivo}.
use crate::{storage, base_dir, load_json, unix_now, str_of};
use serde_json::{json, Value};

pub const RULES: &str = "Modalita' Eco di Juggl attiva (limite settimanale di Claude): risposte brevi e al punto, verifiche essenziali, \
niente sotto-agenti se non indispensabili (al massimo 2, model sonnet o haiku), immagini solo ridotte e solo se servono, output dei comandi filtrati. \
Codice importante, deploy e decisioni restano fatti con la cura di sempre.";

fn settings_file() -> std::path::PathBuf { base_dir().join("impostazioni.json") }

pub fn status() -> Value { load_json(&settings_file()).map(|v| v["eco"].clone()).filter(|v| v.is_object()).unwrap_or(json!({"attiva": false, "auto": false})) }
pub fn is_on() -> bool { status()["attiva"].as_bool().unwrap_or(false) }

/// Rules to append to the system instructions of a chat that starts (hosted.rs), if Eco is on.
pub fn rules() -> Option<&'static str> { if is_on() { Some(RULES) } else { None } }

/// Model for a new chat with no chosen model: Sonnet when Eco is on.
pub fn model_for_new() -> Option<&'static str> { if is_on() { Some("sonnet") } else { None } }

/// Chats Eco applies to: hosted inside Juggl, not pinned, not Juggl itself, not local, not archived.
fn eligible_chats() -> Vec<Value> {
    let juggl = crate::juggl_id();
    crate::list_sessions().into_iter().filter(|v| {
        let id = str_of(v, "sessione");
        str_of(v, "ospite") == "tavolo" && !id.is_empty() && id != juggl && !id.starts_with("locale:")
            && !v.get("fissata").and_then(|x| x.as_bool()).unwrap_or(false) && !matches!(str_of(v, "vista"), "archiviata" | "chiusa")
    }).collect()
}

/// Turns Eco on or off. Returns how many chats it changed and what changed.
pub fn set(on: bool, auto: Option<bool>, reason: &str) -> Value {
    let before = status();
    let r = storage::modify(&settings_file(), Some(json!({})), |v| {
        let e = &mut v["eco"];
        if !e.is_object() { *e = json!({}); }
        e["attiva"] = json!(on);
        if let Some(a) = auto { e["auto"] = json!(a); }
        if on && !before["attiva"].as_bool().unwrap_or(false) { e["dal"] = json!(unix_now()); e["motivo"] = json!(reason); }
        if !on { e["dal"] = Value::Null; e["motivo"] = Value::Null; }
        // hysteresis (Nov 12): turned off by hand = the automatic mode does not turn it back on until the weekly reset (saved in the state)
        if !on && reason == "a mano" { e["spenta_a_mano"] = json!(unix_now()); }
        if on && reason == "a mano" { e["spenta_a_mano"] = Value::Null; }
        true
    });
    if let Err(e) = r { return json!({"ok": false, "msg": e}); }
    let changed = if on != before["attiva"].as_bool().unwrap_or(false) { apply_to_chats(on) } else { 0 };
    crate::log_juggl(&format!("eco {}: {reason} ({changed} chat)", if on { "accesa" } else { "spenta" }));
    json!({"ok": true, "attiva": on, "auto": status()["auto"], "cambiate": changed})
}

/// Moves the chats to Sonnet (saving the previous model) or puts them back as they were.
fn apply_to_chats(on: bool) -> usize {
    let mut n = 0;
    for v in eligible_chats() {
        let id = str_of(&v, "sessione").to_string();
        let p = base_dir().join("stato").join(format!("{id}.json"));
        let st = load_json(&p).unwrap_or(json!({}));
        if on {
            let (m, e) = (str_of(&st, "modello").to_string(), str_of(&st, "effort").to_string());
            if m.contains("sonnet") || m.contains("haiku") { continue; }   // already light
            let _ = crate::update_json(&p, |x| { x["eco_prima"] = json!({"modello": m, "effort": e}); true });
            crate::regola(id, "sonnet".into(), "medium".into());
            n += 1;
        } else if let Some(pr) = st.get("eco_prima").filter(|x| x.is_object()).cloned() {
            let _ = crate::update_json(&p, |x| { if let Some(o) = x.as_object_mut() { o.remove("eco_prima"); } true });
            crate::regola(id, str_of(&pr, "modello").into(), str_of(&pr, "effort").into());
            n += 1;
        }
    }
    n
}

/// Automatic Eco (only if the user allowed it): on when at the last hour's pace the limit runs out before
/// the reset and usage is already over 50%; off when the week restarts (below 20%).
pub fn check_auto(analysis: &Value) -> Option<String> {
    let st = status();
    // emergency (Nov 12): over 90% of the week Eco turns itself on even without the "auto" permission
    // (the user's decision), and turns itself off when the week restarts
    // turned off by hand this week (after the last reset): no automatic re-enabling until the next one
    let blocked = match (st["spenta_a_mano"].as_f64(), analysis["limite"]["reset"].as_f64()) {
        (Some(m), Some(reset)) => m > reset - 7.0 * 86400.0,
        (Some(m), None) => unix_now() - m < 24.0 * 3600.0,
        _ => false,
    };
    if let Some(pct) = analysis["limite"]["pct"].as_f64() {
        let on = st["attiva"].as_bool().unwrap_or(false);
        // above 90 it turns on, it turns off only below 20 (reset): no oscillation around the threshold
        if !on && pct >= 90.0 && !blocked {
            set(true, None, "automatica: settimana di Claude oltre il 90%");
            return Some(format!("Modalità Eco accesa da sola: settimana di Claude al {pct:.0}%"));
        }
        if on && pct < 20.0 && str_of(&st, "motivo").contains("oltre il 90%") {
            set(false, None, "automatica: settimana ripartita");
            return Some("Modalità Eco spenta: la settimana è ripartita".into());
        }
    }
    // turned off by hand but week over 97%: ONE warning per week only (never turns itself back on)
    if blocked {
        if let (Some(pct), Some(reset)) = (analysis["limite"]["pct"].as_f64(), analysis["limite"]["reset"].as_f64()) {
            if pct >= 97.0 && !st["attiva"].as_bool().unwrap_or(false) && (st["avviso97"].as_f64().unwrap_or(0.0) - reset).abs() > 3600.0 {
                let _ = storage::modify(&settings_file(), Some(json!({})), |v| { v["eco"]["avviso97"] = json!(reset); true });
                return Some(format!("Settimana di Claude al {pct:.0}%: la Modalità Eco è spenta. Riaccendila dalla vista Consumi (tasto Accendi)."));
            }
        }
    }
    if !st["auto"].as_bool().unwrap_or(false) || blocked { return None; }
    let l = &analysis["limite"];
    let (pct, reset) = (l["pct"].as_f64()?, l["reset"].as_f64()?);
    let on = st["attiva"].as_bool().unwrap_or(false);
    if !on {
        let hours_left = analysis["fine_ore_ritmo_ora"].as_f64()?;
        let hours_to_reset = (reset - unix_now()) / 3600.0;
        if pct >= 50.0 && hours_left < hours_to_reset {
            let reason = format!("automatica: al ritmo dell'ultima ora il limite finirebbe tra {:.0} ore, si azzera tra {:.0} ore", hours_left.max(1.0), hours_to_reset);
            set(true, None, &reason);
            return Some(format!("Modalità Eco accesa da sola: {}", &reason[12..]));
        }
    } else if pct < 20.0 && str_of(&st, "motivo").starts_with("automatica") {
        set(false, None, "automatica: settimana ripartita");
        return Some("Modalità Eco spenta: la settimana è ripartita".into());
    }
    None
}
