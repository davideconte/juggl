// Commands launched with run_in_background that do not yet have a completion notice (0.1.74, bug "Stop for 13 min" while a
// 57-minute transcription ran in the background). Lives in each session's state as `in_background`: a list of
// {id, kind, description, since}. The state text does not change here: the clear list shows it.
// How it works: in the transcript the call (Bash with run_in_background, or Monitor) and its reply "... with ID: x"
// give the job id; the final <task-notification> (the one `task_outcomes` reads) closes it. Chunked reading like esiti_task.
use crate::unix_now;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

type Launches = HashMap<String, (String, String, f64)>;   // job id -> (kind, description, when)

#[derive(Clone, Default)]
struct State { da: u64, in_attesa: HashMap<String, (String, String, f64)>, lanci: Launches }

static CACHE: Mutex<Option<HashMap<PathBuf, State>>> = Mutex::new(None);
const FIRST_READ: u64 = 8 * 1024 * 1024;
const CHUNK: u64 = 4 * 1024 * 1024;

fn text_of(c: &Value) -> String {
    if let Some(t) = c.as_str() { return t.to_string(); }
    c.as_array().map(|a| a.iter().filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

fn job_id(t: &str) -> Option<String> {
    let after = t.split("with ID: ").nth(1).or_else(|| t.split("Monitor started (task ").nth(1))?;
    let id: String = after.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if id.is_empty() { None } else { Some(id) }
}

fn read_line(r: &str, st: &mut State) {
    let Ok(v) = serde_json::from_str::<Value>(r) else { return };
    let when = v["timestamp"].as_str().map(crate::requests::from_iso).unwrap_or(0.0);
    let Some(c) = v["message"]["content"].as_array() else { return };
    for x in c {
        match x["type"].as_str().unwrap_or("") {
            "tool_use" => {
                let name = x["name"].as_str().unwrap_or("");
                let i = &x["input"];
                let background = (name == "Bash" && i["run_in_background"].as_bool().unwrap_or(false)) || name == "Monitor";
                if !background { continue; }
                let d = i["description"].as_str().filter(|d| !d.trim().is_empty()).map(|d| d.to_string())
                    .unwrap_or_else(|| i["command"].as_str().unwrap_or("").chars().take(80).collect());
                let kind = if name == "Monitor" { "monitor" } else { "comando" };
                if let Some(id) = x["id"].as_str() { st.in_attesa.insert(id.to_string(), (kind.into(), d.chars().take(160).collect(), when)); }
            }
            "tool_result" => {
                let Some(uid) = x["tool_use_id"].as_str() else { continue };
                let Some(l) = st.in_attesa.remove(uid) else { continue };
                if let Some(id) = job_id(&text_of(&x["content"])) { st.lanci.insert(id, l); }
            }
            _ => {}
        }
    }
}

/// Background jobs launched in transcript `t` (including already finished ones: the caller removes them).
fn launches_of(t: &Path) -> Launches {
    let size = std::fs::metadata(t).map(|m| m.len()).unwrap_or(0);
    let mut st = {
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let m = c.get_or_insert_with(HashMap::new);
        if m.len() > 64 { m.clear(); }
        m.get(t).cloned().unwrap_or_default()
    };
    if size < st.da { st = State::default(); }
    if size == st.da { return st.lanci; }
    let mut skip_partial = false;
    if st.da == 0 && size > FIRST_READ { st.da = size - FIRST_READ; skip_partial = true; }
    if let Ok(mut fh) = std::fs::File::open(t) {
        while st.da < size {
            let mut b = vec![];
            if fh.seek(SeekFrom::Start(st.da)).is_err() || (&mut fh).take(CHUNK.min(size - st.da)).read_to_end(&mut b).is_err() || b.is_empty() { break; }
            let mut start = 0;
            if skip_partial {
                match b.iter().position(|x| *x == b'\n') { Some(k) => start = k + 1, None => { st.da += b.len() as u64; continue; } }
                skip_partial = false;
            }
            let Some(upto) = b.iter().rposition(|x| *x == b'\n').map(|k| k + 1).filter(|&f| f > start) else {
                if b.len() as u64 >= CHUNK { st.da += b.len() as u64; skip_partial = true; continue; }
                break;
            };
            for r in String::from_utf8_lossy(&b[start..upto]).lines() {
                // huge lines (images, long results) without background calls: not even read
                if r.len() > 512 * 1024 || !(r.contains("run_in_background") || r.contains("\"Monitor\"") || r.contains("running in background") || r.contains("Monitor started")) { continue; }
                read_line(r, &mut st);
            }
            st.da += upto as u64;
            if (upto as u64) < b.len() as u64 && (b.len() as u64) < CHUNK { break; }
        }
    }
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    c.get_or_insert_with(HashMap::new).insert(t.to_path_buf(), st.clone());
    st.lanci
}

/// Those without a completion notice, oldest first. `esiti` = the transcript's completion notices (esiti_task).
fn without_end(lanci: &Launches, outcomes: &HashMap<String, (String, f64)>, now: f64) -> Vec<Value> {
    let mut v: Vec<(&String, &(String, String, f64))> = lanci.iter()
        .filter(|(id, l)| !outcomes.contains_key(*id) && (l.2 == 0.0 || now - l.2 < 86400.0)).collect();
    v.sort_by(|a, b| a.1.2.partial_cmp(&b.1.2).unwrap_or(std::cmp::Ordering::Equal));
    v.into_iter().take(12).map(|(id, l)| json!({"id": id, "tipo": l.0, "descrizione": l.1, "da": l.2})).collect()
}

/// `in_background` of a session: empty list if there are none or if the Claude process has no live commands left.
pub fn in_background(v: &Value) -> Vec<Value> {
    let Some(t) = crate::find_transcript(v) else { return vec![] };
    let list = without_end(&launches_of(&t), &crate::task_outcomes(&t), unix_now());
    if list.is_empty() { return list; }
    // if it can be verified and the process has no background commands left, the jobs ended without notice (stopped session)
    let pid = v["pid"].as_u64().unwrap_or(0) as u32;
    if pid > 0 && crate::hosted::background_commands(pid) == Some(false) { return vec![]; }
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn background_jobs_without_end_notice() {
        let d = std::env::temp_dir().join(format!("juggl-sfondo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap();
        let t = d.join("t.jsonl");
        let row = |v: Value| v.to_string();
        let usage = |id: &str, name: &str, input: Value| row(json!({"timestamp": "2026-11-16T10:00:00.000Z", "message": {"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}}));
        let res = |id: &str, text: &str| row(json!({"timestamp": "2026-11-16T10:00:01.000Z", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}}));
        let rows = vec![
            usage("u1", "Bash", json!({"command": "trascrivi video.mp4", "description": "Trascrizione del montaggio", "run_in_background": true})),
            res("u1", "Command running in background with ID: bx1. Output is being written to: /tmp/x.output"),
            usage("u2", "Bash", json!({"command": "sleep 5", "description": "Attesa breve", "run_in_background": true})),
            res("u2", "Command running in background with ID: bx2. Output is being written to: /tmp/y.output"),
            usage("u3", "Bash", json!({"command": "ls", "description": "Non in sottofondo"})),
            res("u3", "file.txt"),
            usage("u4", "Monitor", json!({"command": "until false; do sleep 1; done", "description": "Aspetto la fine", "timeout_ms": 1000})),
            res("u4", "Monitor started (task bm1, expires in 5m unless the source ends first)"),
        ];
        std::fs::write(&t, rows.join("\n") + "\n").unwrap();
        let lanci = launches_of(&t);
        assert_eq!(lanci.len(), 3, "bx1, bx2 e il monitor bm1; il comando senza sottofondo non c'e'");
        // bx2 has the completion notice: it disappears; the others stay, oldest first
        let mut outcomes = HashMap::new();
        outcomes.insert("bx2".to_string(), ("completed".to_string(), 1.0));
        let now = crate::requests::from_iso("2026-11-16T10:30:00.000Z");
        let l = without_end(&lanci, &outcomes, now);
        let ids: Vec<&str> = l.iter().map(|x| x["id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), 2); assert!(ids.contains(&"bx1")); assert!(ids.contains(&"bm1")); assert!(!ids.contains(&"bx2"));
        let first = l.iter().find(|x| x["id"] == "bx1").unwrap();
        assert_eq!(first["descrizione"], "Trascrizione del montaggio");
        assert_eq!(first["tipo"], "comando");
        // after one day they are no longer shown
        assert!(without_end(&lanci, &outcomes, now + 90000.0).is_empty());
        // incremental read: an appended line shows up without rereading everything
        std::fs::OpenOptions::new().append(true).open(&t).unwrap().write_all((usage("u5", "Bash", json!({"command": "x", "description": "Nuovo", "run_in_background": true})) + "\n" + &res("u5", "Command running in background with ID: bx5. Output") + "\n").as_bytes()).unwrap();
        assert_eq!(launches_of(&t).len(), 4);
        let _ = std::fs::remove_dir_all(&d);
    }
}
