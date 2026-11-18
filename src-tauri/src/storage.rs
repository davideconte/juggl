// Safe writing of shared files (~/tavolo/stato/*.json, queue, richieste-ai.json, notifiche.json, progetti.json).
// The Python hook and the scripts in ~/tavolo/bin write the same files too: all follow the same protocol
// (the same as bin/comune.py):
// - write: unique temporary file in the same folder `<file>.tmp.<pid>.<n>`, fsync, rename over the file;
// - read, change, write: under exclusive `flock` on the `<file>.lock` next to it;
// - if the file exists but cannot be read, retry and never rewrite from scratch.
#[cfg(windows)] use crate::sys as libc;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)] use std::os::unix::io::AsRawFd;
#[cfg(windows)] use crate::sys::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// How long to wait for the lock before giving up (a stuck hook must not stop the app).
const LOCK_WAIT: Duration = Duration::from_secs(10);

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let name = p.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
    p.with_file_name(format!("{name}{suffix}"))
}

/// The lock file of `p`: `<p>.lock`.
pub fn lock_file(p: &Path) -> PathBuf { with_suffix(p, ".lock") }

/// Unique temporary name for `p`: `<p>.tmp.<pid>.<n>`.
fn temp_file(p: &Path) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    with_suffix(p, &format!(".tmp.{}.{n}", std::process::id()))
}

/// Atomic write: readers see the old file or the new one, never a half-written one.
pub fn write(p: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(d) = p.parent() { let _ = fs::create_dir_all(d); }
    let tmp = temp_file(p);
    let outcome = (|| -> std::io::Result<()> {
        let mut o = OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; o.mode(0o600); }   // new files already at 600 (review 14/11, point 3)
        let mut f = o.open(&tmp)?;
        // same permissions as the file it replaces (e.g. ponte.json at 600), as comune.py does
        if let Ok(m) = fs::metadata(p) { let _ = f.set_permissions(m.permissions()); }
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, p)?;
        // the folder: so the rename too is on disk (best effort)
        if let Some(d) = p.parent() { if let Ok(fd) = File::open(d) { let _ = fd.sync_all(); } }
        Ok(())
    })();
    if outcome.is_err() { let _ = fs::remove_file(&tmp); }
    outcome.map_err(|e| format!("{}: {e}", p.display()))
}

/// Locks down Juggl files (review 14/11, point 3): data folder at 700; ponte.json, indice.db (with -wal and -shm),
/// stato/*.json and attachments at 600; on the Mac the index is excluded from Time Machine (it holds all messages). Can be repeated:
/// changes nothing if already in order. Runs at startup.
#[cfg(unix)]
pub fn close_data_dir(base: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let set_mode = |p: &Path, m: u32| {
        if let Ok(md) = fs::metadata(p) { if md.permissions().mode() & 0o777 != m { let _ = fs::set_permissions(p, fs::Permissions::from_mode(m)); } }
    };
    set_mode(base, 0o700);
    for n in ["ponte.json", "indice.db", "indice.db-wal", "indice.db-shm", "consumi-aiutanti.jsonl", "aiutanti.json"] { set_mode(&base.join(n), 0o600); }
    for sub in ["stato", "allegati"] {
        let d = base.join(sub);
        set_mode(&d, 0o700);
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_file() { set_mode(&p, 0o600); }
        }
    }
    #[cfg(target_os = "macos")]
    for n in ["indice.db", "indice.db-wal", "indice.db-shm"] {
        let p = base.join(n);
        if p.exists() {
            let _ = crate::new_command("xattr").args(["-w", "com.apple.metadata:com_apple_backup_excludeItem", "com.apple.backupd"]).arg(&p)
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        }
    }
}
#[cfg(not(unix))]
pub fn close_data_dir(_base: &Path) {}

pub fn write_json(p: &Path, v: &Value) -> Result<(), String> {
    let t = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    write(p, t.as_bytes())
}

/// Exclusive lock (flock) on `<file>.lock`: released when it leaves scope (or if the process dies).
/// flock is per open descriptor, so it also protects between two threads of the same process.
pub struct FileLock { _f: File }

impl Drop for FileLock {
    fn drop(&mut self) { unsafe { libc::flock(self._f.as_raw_fd(), libc::LOCK_UN); } }
}

pub fn lock(p: &Path) -> Result<FileLock, String> {
    let lp = lock_file(p);
    if let Some(d) = lp.parent() { let _ = fs::create_dir_all(d); }
    let f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&lp).map_err(|e| format!("{}: {e}", lp.display()))?;
    let started = Instant::now();
    let mut pause = 2u64;
    loop {
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 { return Ok(FileLock { _f: f }); }
        if started.elapsed() > LOCK_WAIT { return Err(format!("{} occupato da troppo", lp.display())); }
        std::thread::sleep(Duration::from_millis(pause));
        pause = (pause * 2).min(50);
    }
}

pub enum ReadResult { Assente, Valida(Value), Rotta }

/// Reads a JSON distinguishing "absent" from "present but unreadable" (retries a few times: it may be an old
/// writer, without rename, halfway through writing).
pub fn read_json(p: &Path) -> ReadResult {
    for attempt in 0..4 {
        match fs::read_to_string(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadResult::Assente,
            Ok(t) => if let Ok(v) = serde_json::from_str::<Value>(&t) { return ReadResult::Valida(v) },
            Err(_) => {}
        }
        if attempt < 3 { std::thread::sleep(Duration::from_millis(40)); }
    }
    ReadResult::Rotta
}

/// Read, change, write under lock. `empty`: the value to start from if the file is absent (None = do not create it).
/// The function returns false to write nothing. If the file exists but is unreadable it is never written.
/// Returns the final value (even when no write was needed).
pub fn modify(p: &Path, empty: Option<Value>, f: impl FnOnce(&mut Value) -> bool) -> Result<Value, String> {
    let _l = lock(p)?;
    let mut v = match read_json(p) {
        ReadResult::Valida(v) => v,
        ReadResult::Assente => match empty { Some(v) => v, None => return Err(format!("{} non c'e'", p.display())) },
        ReadResult::Rotta => return Err(format!("{} illeggibile: non lo riscrivo", p.display())),
    };
    if f(&mut v) { write_json(p, &v)?; }
    Ok(v)
}

/// Removes a file and its lock (only if nobody holds it).
pub fn remove(p: &Path) {
    let _ = fs::remove_file(p);
    let lp = lock_file(p);
    if let Ok(f) = File::open(&lp) {
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 && !p.exists() { let _ = fs::remove_file(&lp); }
    }
}

/// Cleanup of leftover service files in a folder: old temporaries (writer died midway) and locks
/// of files that no longer exist.
pub fn clean_service_dir(dir: &Path, older_than: Duration) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map(|d| d > older_than).unwrap_or(false);
        if !old { continue; }
        if name.contains(".tmp.") || name.ends_with(".tmp") { let _ = fs::remove_file(&p); }
        else if let Some(base) = name.strip_suffix(".lock") {
            let data = dir.join(base);
            if !data.exists() { remove(&data); }
        }
    }
}

/// Session id accepted by all commands: uuid (`^[0-9a-f-]{36}$`) or `locale:<hex>`.
pub fn is_valid_id(id: &str) -> bool {
    let uuid = id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') && id.matches('-').count() == 4;
    let local_id = id.strip_prefix("locale:").map(|l| !l.is_empty() && l.len() <= 32 && l.chars().all(|c| c.is_ascii_hexdigit())).unwrap_or(false);
    uuid || local_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("juggl-prova-{name}-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parallel_changes_are_not_lost() {
        let d = make_dir("parallelo");
        let p = d.join("stato.json");
        write_json(&p, &json!({"n": 0, "voci": []})).unwrap();
        let threads: Vec<_> = (0..16).map(|k| {
            let p = p.clone();
            std::thread::spawn(move || {
                for i in 0..50 {
                    modify(&p, None, |v| {
                        v["n"] = json!(v["n"].as_i64().unwrap() + 1);
                        v["voci"].as_array_mut().unwrap().push(json!(format!("{k}-{i}")));
                        true
                    }).unwrap();
                }
            })
        }).collect();
        for f in threads { f.join().unwrap(); }
        let v = match read_json(&p) { ReadResult::Valida(v) => v, _ => panic!("file rotto") };
        assert_eq!(v["n"].as_i64().unwrap(), 800);
        assert_eq!(v["voci"].as_array().unwrap().len(), 800);
        // no temporaries left around
        let leftovers: Vec<String> = fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.contains(".tmp.")).collect();
        assert!(leftovers.is_empty(), "temporanei rimasti: {leftovers:?}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn different_processes_respect_the_lock() {
        // another process (like the Python hook) holds the lock: I wait and do not write over it
        let d = make_dir("processi");
        let p = d.join("x.json");
        write_json(&p, &json!({"chi": "nessuno"})).unwrap();
        let lp = lock_file(&p);
        let script = format!("import fcntl,time,json,os\nf=open({lp:?},'a+')\nfcntl.flock(f,fcntl.LOCK_EX)\ntime.sleep(0.6)\nt={:?}+'.tmp.'+str(os.getpid())+'.0'\nopen(t,'w').write(json.dumps({{'chi':'python'}}))\nos.replace(t,{:?})\n",
            p.to_string_lossy(), p.to_string_lossy());
        let mut py = match crate::new_command("python3").arg("-c").arg(&script).spawn() { Ok(c) => c, Err(_) => return };
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        let v = modify(&p, None, |v| { v["anche"] = json!("rust"); true }).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(250), "non ha aspettato il lucchetto");
        let _ = py.wait();
        assert_eq!(v["chi"], json!("python"));
        assert_eq!(v["anche"], json!("rust"));
        let _ = fs::remove_dir_all(&d);
    }

    /// Rust and the real ~/tavolo/bin/comune.py (hook and scripts) modify the same file together: no step lost.
    #[test]
    fn together_with_comune_py() {
        let common = crate::base_dir().join("bin/comune.py");   // rename (14/11)
        if !common.exists() { return; }
        let d = make_dir("comune");
        let p = d.join("02ced450-9036-453a-897c-818b366c2ea0.json");
        write_json(&p, &json!({"rust": 0, "python": 0, "nome_manuale": "Progetto A"})).unwrap();
        let script = format!("import sys\nsys.path.insert(0, {:?})\nimport comune\ndef f(d):\n    d['python'] += 1\nfor _ in range(300):\n    comune.aggiorna({:?}, f)\n",
            common.parent().unwrap().to_string_lossy(), p.to_string_lossy());
        let mut py = crate::new_command("python3").arg("-I").arg("-c").arg(&script).spawn().unwrap();
        for _ in 0..300 { modify(&p, None, |v| { v["rust"] = json!(v["rust"].as_i64().unwrap() + 1); true }).unwrap(); }
        assert!(py.wait().unwrap().success());
        let v = match read_json(&p) { ReadResult::Valida(v) => v, _ => panic!("file rotto") };
        assert_eq!((v["rust"].as_i64(), v["python"].as_i64()), (Some(300), Some(300)));
        assert_eq!(v["nome_manuale"], json!("Progetto A"));
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn permissions_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let d = make_dir("permessi");
        let p = d.join("ponte.json");
        write_json(&p, &json!({"a": 1})).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        write_json(&p, &json!({"a": 2})).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn broken_file_is_not_rewritten() {
        let d = make_dir("rotto");
        let p = d.join("s.json");
        fs::write(&p, "{\"nome_manuale\": \"Progetto A\", ").unwrap();
        let r = modify(&p, Some(json!({})), |v| { v["stato"] = json!("lavora"); true });
        assert!(r.is_err());
        assert_eq!(fs::read_to_string(&p).unwrap(), "{\"nome_manuale\": \"Progetto A\", ");
        // absent: starts from empty
        let q = d.join("nuovo.json");
        let v = modify(&q, Some(json!({"sessione": "a"})), |v| { v["stato"] = json!("ferma"); true }).unwrap();
        assert_eq!(v["stato"], json!("ferma"));
        assert!(modify(&d.join("mai.json"), None, |_| true).is_err());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn reads_during_writes_are_always_whole() {
        let d = make_dir("letture");
        let p = d.join("r.json");
        let big: Vec<String> = (0..3000).map(|i| format!("voce numero {i}")).collect();
        write_json(&p, &json!({"voci": big})).unwrap();
        let p2 = p.clone();
        let writer = std::thread::spawn(move || for i in 0..100 { write_json(&p2, &json!({"i": i, "voci": (0..3000).map(|k| format!("v{k}")).collect::<Vec<_>>()})).unwrap(); });
        for _ in 0..300 { assert!(matches!(read_json(&p), ReadResult::Valida(_))); }
        writer.join().unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn ids_are_checked() {
        assert!(is_valid_id("02ced450-9036-453a-897c-818b366c2ea0"));
        assert!(is_valid_id("locale:1a2b"));
        for bad in ["", "../x", "02ced450-9036-453a-897c-818b366c2ea", "locale:", "locale:zz", "02ced450/9036-453a-897c-818b366c2ea0"] { assert!(!is_valid_id(bad), "{bad}"); }
    }
    #[cfg(unix)]
    #[test]
    fn lock_down_data_folder_sets_600_and_700() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("juggl-prova-perm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("stato")).unwrap(); fs::create_dir_all(d.join("allegati")).unwrap();
        for f in ["ponte.json", "indice.db", "stato/a.json", "allegati/x.png", "altro.txt"] { fs::write(d.join(f), "x").unwrap(); fs::set_permissions(d.join(f), fs::Permissions::from_mode(0o644)).unwrap(); }
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
        close_data_dir(&d);
        let m = |p: &str| fs::metadata(d.join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
        for f in ["ponte.json", "indice.db", "stato/a.json", "allegati/x.png"] { assert_eq!(m(f), 0o600, "{f}"); }
        assert_eq!(m("altro.txt"), 0o644, "i file che non sono nell'elenco non si toccano");
        // a new file written by storage::scrivi is born at 600
        write(&d.join("nuovo.json"), b"{}").unwrap();
        assert_eq!(m("nuovo.json"), 0o600);
        let _ = fs::remove_dir_all(&d);
    }
}
