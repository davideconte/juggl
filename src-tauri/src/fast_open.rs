//! fast (Nov 14): open a chat without waiting.
//!
//! Three caches, all in memory (empty after an app restart) and all with the same file key:
//! size + modification time in nanoseconds + inode. A file that grows, gets rewritten (rename = new inode),
//! truncated or corrupted changes at least one of the three: the entry is no longer valid and is re-read.
//! - JSON registries (richieste-ai.json of 3.8 MB, ok.json): read once per file version, shared in an Arc
//! - chat detail (transcript -> events): 24 MB cap in total, no entries above 4 MB, emptied when free
//!   memory is low, expires after 60 s (the file cards also look at the disk), and saved only if the transcript
//!   did not change while I was reading it
//! - processes (`ps`): 2 s, only for reading a single chat

use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Fingerprint of a file: (size, modification time in ns, inode). None = file missing.
pub fn fingerprint(p: &Path) -> Option<(u64, i128, u64)> {
    let m = std::fs::metadata(p).ok()?;
    #[cfg(unix)]
    let ino = { use std::os::unix::fs::MetadataExt; m.ino() };
    #[cfg(not(unix))]
    let ino = 0u64;
    let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i128).unwrap_or(-1);
    Some((m.len(), t, ino))
}

// ---------- free memory of the Mac ----------

/// Percentage of memory available according to macOS (kern.memorystatus_level), read at most every 5 s.
fn free_memory_pct() -> Option<u32> {
    static LAST: Mutex<Option<(Instant, Option<u32>)>> = Mutex::new(None);
    let mut u = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, v)) = *u { if t.elapsed() < Duration::from_secs(5) { return v; } }
    let v = read_level();
    *u = Some((Instant::now(), v));
    v
}
#[cfg(target_os = "macos")]
fn read_level() -> Option<u32> {
    let mut v: u32 = 0;
    let mut n = std::mem::size_of::<u32>();
    let name = b"kern.memorystatus_level\0";
    let r = unsafe { libc::sysctlbyname(name.as_ptr() as *const libc::c_char, &mut v as *mut u32 as *mut libc::c_void, &mut n, std::ptr::null_mut(), 0) };
    if r == 0 { Some(v) } else { None }
}
#[cfg(not(target_os = "macos"))]
fn read_level() -> Option<u32> { None }

/// Low free memory: the caches are emptied and keep nothing new.
pub fn low_memory() -> bool { free_memory_pct().map(|p| p < 12).unwrap_or(false) }

// ---------- JSON registries read once per file version ----------

type Registries = HashMap<PathBuf, ((u64, i128, u64), Arc<Value>)>;
static REGISTRIES: Mutex<Option<Registries>> = Mutex::new(None);

/// The file's JSON, rebuilt only if the file changed. None = missing or unreadable (like `leggi`).
pub fn json(p: &Path) -> Option<Arc<Value>> {
    let f = fingerprint(p)?;
    if let Some((k, v)) = REGISTRIES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(p) {
        if *k == f { return Some(v.clone()); }
    }
    // same read as storage::leggi_json: retry if it finds a half-written file (old writers without rename)
    let crate::storage::ReadResult::Valida(v) = crate::storage::read_json(p) else { return None };
    // the file changed while I was reading it: the result is fine now, but I do not keep it
    if fingerprint(p) != Some(f) { return Some(Arc::new(v)); }
    let v = Arc::new(v);
    let mut g = REGISTRIES.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if m.len() > 16 { m.clear(); }
    m.insert(p.to_path_buf(), (f, v.clone()));
    Some(v)
}

// ---------- chat detail ----------

const CAP: usize = 24 * 1024 * 1024;
const MAX_ENTRY: usize = 4 * 1024 * 1024;
const EXPIRES: Duration = Duration::from_secs(60);

struct Entry { file: PathBuf, firma: (u64, i128, u64), dal: String, prima: usize, v: Value, peso: usize, nata: Instant, usata: Instant }
static DETAILS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// The chat detail from transcript `p`: from the cache if the file is identical, otherwise `calcola`.
pub fn cached_detail(p: Option<&Path>, dal: &str, prima: usize, compute: impl FnOnce() -> Value) -> Value {
    let Some(p) = p else { return compute() };
    let Some(f) = fingerprint(p) else { return compute() };
    if low_memory() { clear(); return compute(); }
    {
        let mut c = DETAILS.lock().unwrap_or_else(|e| e.into_inner());
        c.retain(|x| x.nata.elapsed() < EXPIRES);
        if let Some(x) = c.iter_mut().find(|x| x.file == p && x.dal == dal && x.prima == prima) {
            if x.firma == f { x.usata = Instant::now(); return x.v.clone(); }
        }
        c.retain(|x| x.file != p || x.firma == f);   // old version of the file: drop all its entries
    }
    let v = compute();
    // grown, rewritten or truncated while I was reading it: fine to show it, but I do not keep it
    if fingerprint(p) != Some(f) { return v; }
    let peso = serde_json::to_vec(&v).map(|b| b.len()).unwrap_or(usize::MAX);
    if peso > MAX_ENTRY { return v; }
    let mut c = DETAILS.lock().unwrap_or_else(|e| e.into_inner());
    c.retain(|x| !(x.file == p && x.dal == dal && x.prima == prima));
    let now = Instant::now();
    c.push(Entry { file: p.to_path_buf(), firma: f, dal: dal.to_string(), prima, v: v.clone(), peso, nata: now, usata: now });
    while c.iter().map(|x| x.peso).sum::<usize>() > CAP {
        let Some(i) = c.iter().enumerate().min_by_key(|(_, x)| x.usata).map(|(i, _)| i) else { break };
        c.remove(i);
    }
    v
}


/// Like `dimentica`, from the chat id (keeper events: the transcript is named <id>.jsonl).
pub fn forget_id(id: &str) {
    let name = format!("{id}.jsonl");
    DETAILS.lock().unwrap_or_else(|e| e.into_inner()).retain(|x| x.file.file_name().and_then(|n| n.to_str()) != Some(name.as_str()));
}

/// fast (Nov 15): the path is on a network disk (smb, afp, nfs, webdav...). On a mounted network disk that
/// does not answer, a file check hangs for tens of seconds (a chat that cited files on a network disk: 54 s to
/// open). The disk table is read with MNT_NOWAIT, which does not touch the disks and does not block; cached for 10 s.
pub fn is_network(p: &str) -> bool {
    if !p.starts_with('/') { return false; }
    let mounts = network_mounts();
    mounts.iter().any(|m| p == m.as_str() || (p.starts_with(m.as_str()) && p.as_bytes().get(m.len()) == Some(&b'/')))
}
fn network_mounts() -> Arc<Vec<String>> {
    static P: Mutex<Option<(Instant, Arc<Vec<String>>)>> = Mutex::new(None);
    let mut g = P.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, v)) = g.as_ref() { if t.elapsed() < Duration::from_secs(10) { return v.clone(); } }
    let v = Arc::new(read_network_mounts());
    *g = Some((Instant::now(), v.clone()));
    v
}
#[cfg(target_os = "macos")]
fn read_network_mounts() -> Vec<String> {
    const NETWORK_FS: [&str; 8] = ["smbfs", "afpfs", "nfs", "webdav", "ftp", "cifs", "macfuse", "osxfuse"];
    let n = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if n <= 0 { return vec![]; }
    let mut buf: Vec<libc::statfs> = Vec::with_capacity(n as usize + 8);
    let size = (buf.capacity() * std::mem::size_of::<libc::statfs>()) as libc::c_int;
    let n = unsafe { libc::getfsstat(buf.as_mut_ptr(), size, libc::MNT_NOWAIT) };
    if n <= 0 { return vec![]; }
    unsafe { buf.set_len(n as usize) };
    let to_text = |a: &[libc::c_char]| -> String { let b: Vec<u8> = a.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect(); String::from_utf8_lossy(&b).to_string() };
    buf.iter().filter(|s| NETWORK_FS.contains(&to_text(&s.f_fstypename).as_str())).map(|s| to_text(&s.f_mntonname)).filter(|m| m != "/").collect()
}
#[cfg(not(target_os = "macos"))]
fn read_network_mounts() -> Vec<String> { vec![] }

pub fn clear() { DETAILS.lock().unwrap_or_else(|e| e.into_inner()).clear(); }

/// How much the chat cache holds now: (entries, bytes).
pub fn status() -> (usize, usize) { let c = DETAILS.lock().unwrap_or_else(|e| e.into_inner()); (c.len(), c.iter().map(|x| x.peso).sum()) }

// ---------- live changes with the summary only ----------

/// Removes the diff lines from live changes (file, state, +/- stay): "leggero" tells the interface to
/// ask for them with `modifiche_pezzi` when the card is opened.
pub fn slim_changes(d: &mut Value) {
    let Some(ev) = d.get_mut("eventi").and_then(|x| x.as_array_mut()) else { return };
    for e in ev.iter_mut().filter(|e| e.get("tipo").and_then(|t| t.as_str()) == Some("modifiche")) {
        let Some(el) = e.get_mut("elenco").and_then(|x| x.as_array_mut()) else { continue };
        let mut removed = false;
        for f in el.iter_mut() {
            if let Some(o) = f.as_object_mut() {
                if o.get("pezzi").and_then(|p| p.as_array()).map(|a| !a.is_empty()).unwrap_or(false) { o.remove("pezzi"); o.insert("leggero".into(), Value::Bool(true)); removed = true; }
            }
        }
        if removed { e["leggero"] = Value::Bool(true); }
    }
}

// ---------- processes ----------

/// `ps` costs ~30 ms: for reading a single chat the one from 2 s ago is enough.
pub fn processes() -> Arc<crate::session_care::Processes> {
    static P: Mutex<Option<(Instant, Arc<crate::session_care::Processes>)>> = Mutex::new(None);
    let mut g = P.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, p)) = g.as_ref() { if t.elapsed() < Duration::from_secs(2) { return p.clone(); } }
    let p = Arc::new(crate::session_care::processes());
    *g = Some((Instant::now(), p.clone()));
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("veloci-prova-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d.join("t.jsonl")
    }
    fn count_lines(p: &Path) -> Value { json!({"righe": std::fs::read_to_string(p).unwrap_or_default().lines().count()}) }

    #[test]
    fn cache_grows_rewritten_truncated_broken() {
        let p = tmp("vita");
        std::fs::write(&p, "a\nb\n").unwrap();
        let mut n = 0;
        let mut read_it = || { let v = cached_detail(Some(&p), "", 6, || { n += 1; count_lines(&p) }); v };
        assert_eq!(read_it()["righe"], 2);
        assert_eq!(read_it()["righe"], 2);
        drop(read_it);
        assert_eq!(n, 1, "seconda lettura dalla cache");
        // grows
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(b"c\n").unwrap();
        assert_eq!(cached_detail(Some(&p), "", 6, || count_lines(&p))["righe"], 3);
        // rewritten with the same length (rename: new inode)
        let q = p.with_extension("nuovo"); std::fs::write(&q, "x\ny\nz\n").unwrap(); std::fs::rename(&q, &p).unwrap();
        let v = cached_detail(Some(&p), "", 6, || json!({"testo": std::fs::read_to_string(&p).unwrap()}));
        assert_eq!(v["testo"], "x\ny\nz\n");
        // truncated
        std::fs::write(&p, "solo\n").unwrap();
        assert_eq!(cached_detail(Some(&p), "", 6, || count_lines(&p))["righe"], 1);
        // corrupted (same length, different content, written in place: the date changes)
        std::thread::sleep(Duration::from_millis(5));
        std::fs::write(&p, "ROTT\n").unwrap();
        assert_eq!(cached_detail(Some(&p), "", 6, || json!({"testo": std::fs::read_to_string(&p).unwrap()}))["testo"], "ROTT\n");
    }

    #[test]
    fn changed_during_read_is_not_kept() {
        let p = tmp("durante");
        std::fs::write(&p, "a\n").unwrap();
        let v = cached_detail(Some(&p), "", 6, || {
            let v = count_lines(&p);
            std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(b"b\n").unwrap();
            v
        });
        assert_eq!(v["righe"], 1);
        let mut n = 0;
        let v = cached_detail(Some(&p), "", 6, || { n += 1; count_lines(&p) });
        assert_eq!((v["righe"].as_u64(), n), (Some(2), 1), "rilegge: la versione letta a meta' non era stata tenuta");
    }

    #[test]
    fn cap_and_large_entries() {
        let p = tmp("grande");
        std::fs::write(&p, "a\n").unwrap();
        let big = "x".repeat(MAX_ENTRY + 10);
        let mut n = 0;
        for _ in 0..2 { cached_detail(Some(&p), "grande", 6, || { n += 1; json!({"t": big}) }); }
        assert_eq!(n, 2, "le voci sopra 4 MB non si tengono");
        let (_, bytes_) = status();
        assert!(bytes_ <= CAP);
    }

    #[test]
    fn changes_without_lines() {
        let mut d = json!({"eventi": [{"tipo": "modifiche", "elenco": [{"path": "/a", "piu": 3, "pezzi": [{"a": 1, "b": 1, "righe": ["+x"]}]}, {"path": "/b", "nascosto": true}]}, {"tipo": "agente", "testo": "ok"}]});
        slim_changes(&mut d);
        let e = &d["eventi"][0];
        assert!(e["elenco"][0].get("pezzi").is_none());
        assert_eq!((e["elenco"][0]["piu"].as_u64(), e["elenco"][0]["leggero"].as_bool(), e["leggero"].as_bool()), (Some(3), Some(true), Some(true)));
        assert!(e["elenco"][1].get("leggero").is_none());
    }

    #[test]
    fn network_disk_recognized() {
        assert!(!is_network("relativo/file.png"));
        assert!(!is_network("/"));
        // the real network mounts of the Mac (if any): the mount and the files under it yes, a name starting the same no
        for m in network_mounts().iter() {
            assert!(is_network(m) && is_network(&format!("{m}/x/y.png")));
            assert!(!is_network(&format!("{m}altro/y.png")));
        }
        assert!(!is_network(&std::env::temp_dir().join("x.png").to_string_lossy()));
    }

    #[test]
    fn json_registry_follows_file() {
        let p = tmp("reg").with_extension("json");
        std::fs::write(&p, r#"{"voci":[1]}"#).unwrap();
        assert_eq!(json(&p).unwrap()["voci"], json!([1]));
        let q = p.with_extension("tmp"); std::fs::write(&q, r#"{"voci":[2]}"#).unwrap(); std::fs::rename(&q, &p).unwrap();
        assert_eq!(json(&p).unwrap()["voci"], json!([2]));
        std::fs::write(&p, "{rotto").unwrap();
        assert!(json(&p).is_none(), "illeggibile = come leggi(): None");
        std::fs::remove_file(&p).unwrap();
        assert!(json(&p).is_none());
    }
}
