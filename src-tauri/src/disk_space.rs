// disk space for everyone (Nov 14, 0.1.65): Space engine built into the app, for those without the personal engine
// bin/mac in the data folder (almost everyone). Same JSON as the personal engine: the view (ui/disk-space.js) is the same.
//
// Coordinator rules for other people's Macs (Nov 14):
// 1) everything with confirmation and summary (what, how much, where); nothing automatic;
// 2) only Trash with Undo: never direct deletion, not even of builds (the user empties it, in two steps);
// 3) proposes ONLY from a closed list of things that come back: cargo target/, node_modules, Xcode DerivedData,
//    npm, pip and Homebrew caches; the Trash; old Downloads and large files only as a suggestion (Show in Finder);
//    iOS simulators just for information;
// 4) never cloud folders (iCloud, Dropbox, CloudStorage), never files not downloaded (SF_DATALESS), never other disks;
// 5) no path or file name leaves the Mac toward the internet, no statistics (the view on the iPhone goes through the local
//    bridge, with the key, like the rest of Juggl);
// 7) texts in Italian and English (Mac language).
// macOS only for now: on Windows and Linux it answers "non disponibile".
use crate::{base_dir, home, load_json, unix_now, str_of};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

const GB: f64 = 1e9;

/// English if the Mac language is not Italian (read once).
fn en() -> bool {
    static EN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *EN.get_or_init(|| {
        if let Ok(x) = std::env::var("JUGGL_LINGUA") { return !x.starts_with("it"); }
        let o = crate::new_command("/usr/bin/defaults").args(["read", "-g", "AppleLanguages"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        let first = o.split(|c: char| c == '"' || c == '(' || c == ',' || c.is_whitespace()).find(|x| x.len() >= 2).unwrap_or("it");
        !first.starts_with("it")
    })
}
fn t(it_text: &str, en_text: &str) -> String { if en() { en_text.to_string() } else { it_text.to_string() } }
const DAY: f64 = 86400.0;
static STOP: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);
static BYTES: AtomicU64 = AtomicU64::new(0);
static FILES: AtomicU64 = AtomicU64::new(0);
static SCAN_STATE: Mutex<(f64, String)> = Mutex::new((0.0, String::new()));
static ACTION_LOCK: Mutex<()> = Mutex::new(());

fn dir() -> PathBuf { base_dir().join("spazio") }
fn file(n: &str) -> PathBuf { dir().join(n) }
fn write_json(p: &Path, v: &Value) {
    let _ = std::fs::create_dir_all(dir());
    if let Err(e) = crate::storage::write_json(p, v) { crate::log_juggl(&format!("spazio: scrittura non riuscita: {e}")); }
}
fn short_path(p: &str) -> String {
    let h = home().to_string_lossy().to_string();
    if p == h { "~".into() } else if let Some(r) = p.strip_prefix(&format!("{h}/")) { format!("~/{r}") } else { p.to_string() }
}
fn is_under(p: &str, b: &str) -> bool { p == b || p.starts_with(&format!("{}/", b.trim_end_matches('/'))) }
fn age_days(t: f64) -> f64 { ((unix_now() - t) / DAY * 10.0).round() / 10.0 }
fn mtime(m: &std::fs::Metadata) -> f64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}
fn hash(p: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    format!("{:010x}", h.finish() & 0xff_ffff_ffff)
}

#[cfg(target_os = "macos")]
fn bytes_on_disk(m: &std::fs::Metadata) -> u64 { use std::os::macos::fs::MetadataExt; m.st_blocks() * 512 }
#[cfg(target_os = "macos")]
fn not_downloaded(m: &std::fs::Metadata) -> bool { use std::os::macos::fs::MetadataExt; m.st_flags() & 0x4000_0000 != 0 }
#[cfg(target_os = "macos")]
fn disk_id(m: &std::fs::Metadata) -> u64 { use std::os::macos::fs::MetadataExt; m.st_dev() as u64 }
#[cfg(not(target_os = "macos"))]
fn bytes_on_disk(m: &std::fs::Metadata) -> u64 { m.len() }
#[cfg(not(target_os = "macos"))]
fn not_downloaded(_m: &std::fs::Metadata) -> bool { false }
#[cfg(not(target_os = "macos"))]
fn disk_id(_m: &std::fs::Metadata) -> u64 { 0 }

/// The user's Trash: always ~/.Trash of the current home (no variable can move it, outside tests).
fn trash_dir() -> PathBuf {
    #[cfg(test)]
    if let Ok(x) = std::env::var("JUGGL_SPAZIO_CESTINO") { return PathBuf::from(x); }
    home().join(".Trash")
}

/// The home must be a real, deep folder (/Users/<name>): with an empty HOME or "/" nothing is done.
fn home_is_valid() -> bool { let h = home(); h.is_absolute() && h.components().count() >= 3 && h.is_dir() }

/// The REAL path (review Nov 15): no "..", no link along the way, inside the real home, outside
/// the skipped folders, outside .app bundles and .git repositories, on the same disk as the home. Also works for things
/// that no longer exist (Undo): the parent is checked.
fn safe_path(pa: &Path) -> Result<(), String> {
    use std::path::Component;
    let no = |it_text: &str, en_text: &str| Err(t(it_text, en_text));
    if !pa.is_absolute() || pa.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) { return no("percorso non valido", "invalid path"); }
    let h = home();
    let hc = std::fs::canonicalize(&h).map_err(|_| t("cartella utente non trovata", "home folder not found"))?;
    let (Some(parent), Some(name)) = (pa.parent(), pa.file_name()) else { return no("percorso non valido", "invalid path") };
    let Ok(rel) = parent.strip_prefix(&h) else { return no("fuori dalla tua cartella", "outside your folder") };
    let pc = std::fs::canonicalize(parent).map_err(|_| t("la cartella che lo contiene non c'è più", "the containing folder is gone"))?;
    if pc != hc.join(rel) { return no("passa da un collegamento", "goes through a link"); }
    let real = pc.join(name);
    let (rs, hs) = (real.to_string_lossy().to_string(), hc.to_string_lossy().to_string());
    if !is_under(&rs, &hs) || real == hc { return no("fuori dalla tua cartella", "outside your folder"); }
    for x in skipped_dirs() {
        let xc = std::fs::canonicalize(&x).map(|y| y.to_string_lossy().to_string()).unwrap_or_else(|_| x.clone());
        if is_under(&rs, &xc) || is_under(&pa.to_string_lossy(), &x) { return no("cartella protetta", "protected folder"); }
    }
    if pa.components().any(|c| { let n = c.as_os_str().to_string_lossy(); n == ".git" || n.ends_with(".app") }) { return no("dentro un'app o un archivio git", "inside an app or a git repository"); }
    let dh = std::fs::symlink_metadata(&hc).map(|m| disk_id(&m)).unwrap_or(0);
    if std::fs::symlink_metadata(&pc).map(|m| disk_id(&m)).unwrap_or(1) != dh { return no("sta su un altro disco", "it's on another disk"); }
    Ok(())
}

/// Folders never read: cloud (reading them would download the files), Trash, Juggl data, and other apps' containers
/// (Library/Containers and Group Containers: on macOS 14 and later reading them triggers "access data from other apps").
fn skipped_dirs() -> Vec<String> {
    let h = home();
    ["Library/CloudStorage", "Library/Mobile Documents", "Dropbox", ".dropbox", "Library/Application Support/FileProvider",
     "Library/Containers", "Library/Group Containers", ".Trash", ".claude", "Library/Messages",
     "Library/Mail", "Pictures/Photos Library.photoslibrary", "Google Drive", "OneDrive"]
        .iter().map(|x| h.join(x).to_string_lossy().to_string()).chain([base_dir().to_string_lossy().to_string()]).collect()
}

/// Folders that come back on their own: (kind, short name, how it comes back).
/// The closed list: only these folders are proposed (kind). Apps (.app) are only there to avoid counting their content.
fn unit_kind(p: &str, name: &str, names: &HashSet<String>, parent_names: &HashSet<String>) -> Option<&'static str> {
    if name.ends_with(".app") && names.contains("Contents") { return Some("app"); }
    if name == "node_modules" && parent_names.contains("package.json") { return Some("node_modules"); }
    if name == "target" && (names.contains(".rustc_info.json") || names.contains("CACHEDIR.TAG")) && parent_names.contains("Cargo.toml") { return Some("rust"); }
    let h = home().to_string_lossy().to_string();
    if p == format!("{h}/Library/Developer/Xcode/DerivedData") { return Some("xcode"); }
    if p == format!("{h}/.npm/_cacache") { return Some("npm"); }
    if p == format!("{h}/Library/Caches/pip") { return Some("pip"); }
    if p == format!("{h}/Library/Caches/Homebrew") { return Some("homebrew"); }
    None
}

/// (short name, how it comes back), in the Mac language.
fn describe_kind(kind: &str) -> (String, String) {
    match kind {
        "node_modules" => ("node_modules".into(), t("si reinstallano con npm install", "reinstall with npm install")),
        "rust" => (t("Build Rust", "Rust build"), t("si rifà alla prossima build", "rebuilt on the next build")),
        "xcode" => (t("Build di Xcode (DerivedData)", "Xcode builds (DerivedData)"), t("Xcode le rifà alla prossima build", "Xcode rebuilds them on the next build")),
        "npm" => (t("Cache di npm", "npm cache"), t("npm la riscarica quando serve", "npm downloads it again when needed")),
        "pip" => (t("Cache di pip", "pip cache"), t("pip la riscarica quando serve", "pip downloads it again when needed")),
        "homebrew" => (t("Cache di Homebrew", "Homebrew cache"), t("Homebrew la riscarica quando serve", "Homebrew downloads it again when needed")),
        _ => (kind.into(), String::new()),
    }
}

#[derive(Default)]
struct Collection { unita: Vec<Value>, grandi: Vec<Value>, cartelle: Vec<(String, u64, f64)>, negate: Vec<String> }

struct Explorer { salta: Vec<String>, dev: u64, r: Mutex<Collection> }

impl Explorer {
    /// (bytes, last change) of a folder, recursive. `inside`: we are inside a unit or a bundle.
    fn measure(&self, d: &Path, dir_meta: &std::fs::Metadata, parent_names: &HashSet<String>, inside: bool, depth: usize) -> (u64, f64) {
        if STOP.load(Ordering::Relaxed) || depth > 80 { return (0, 0.0); }
        let rd = match std::fs::read_dir(d) {
            Ok(rd) => rd,
            Err(e) => {
                // permission denied by macOS (Desktop, Documents, Downloads...): the folder is skipped and the view says so
                if e.kind() == std::io::ErrorKind::PermissionDenied && depth <= 2 { self.r.lock().unwrap().negate.push(short_path(&d.to_string_lossy())); }
                return (0, mtime(dir_meta));
            }
        };
        let entries: Vec<(PathBuf, String, std::fs::Metadata)> = rd.filter_map(|e| e.ok()).filter_map(|e| {
            let m = std::fs::symlink_metadata(e.path()).ok()?;
            Some((e.path(), e.file_name().to_string_lossy().to_string(), m))
        }).collect();
        let names: HashSet<String> = entries.iter().map(|x| x.1.clone()).collect();
        let ps = d.to_string_lossy().to_string();
        let name = d.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        let unit = if inside { None } else { unit_kind(&ps, &name, &names, parent_names) };
        let inside2 = inside || unit.is_some() || name == ".git";
        let (mut b, mut latest, mut nf) = (0u64, mtime(dir_meta), 0u64);
        for (p, n, m) in &entries {
            if not_downloaded(m) || m.file_type().is_symlink() { continue; }
            if m.is_dir() {
                let pp = p.to_string_lossy().to_string();
                if disk_id(m) != self.dev || self.salta.iter().any(|x| is_under(&pp, x)) { continue; }
                let (sb, sr) = self.measure(p, m, &names, inside2, depth + 1);
                b += sb;
                if sr > latest { latest = sr; }
            } else if m.is_file() {
                let sz = bytes_on_disk(m);
                b += sz;
                nf += 1;
                let t = mtime(m);
                if t > latest { latest = t; }
                if !inside2 && sz >= 500_000_000 {
                    let pp = p.to_string_lossy().to_string();
                    self.r.lock().unwrap().grandi.push(json!({"percorso": pp, "byte": sz, "mtime": t, "nome": n}));
                }
            }
        }
        BYTES.fetch_add(entries.iter().filter(|x| x.2.is_file()).map(|x| bytes_on_disk(&x.2)).sum::<u64>(), Ordering::Relaxed);
        FILES.fetch_add(nf, Ordering::Relaxed);
        if depth <= 3 { if let Ok(mut a) = SCAN_STATE.lock() { a.1 = short_path(&ps); } }
        let mut r = self.r.lock().unwrap();
        if let Some(kind) = unit {
            if kind != "app" && b >= 50_000_000 {
                r.unita.push(json!({"percorso": ps, "tipo": kind, "byte": b, "recente": latest}));
            }
        }
        if depth <= 2 && b >= 100_000_000 { r.cartelle.push((ps, b, latest)); }
        (b, latest)
    }
}

/// Full scan of the home in the background (low priority thread, with Stop). Returns immediately.
pub fn scan_start() -> Value {
    if RUNNING.swap(true, Ordering::SeqCst) { return json!({"ok": true, "gia": true, "avanza": progress()}); }
    STOP.store(false, Ordering::SeqCst);
    BYTES.store(0, Ordering::SeqCst);
    FILES.store(0, Ordering::SeqCst);
    *SCAN_STATE.lock().unwrap() = (unix_now(), String::new());
    struct Done;
    impl Drop for Done { fn drop(&mut self) { RUNNING.store(false, Ordering::SeqCst); } }
    let spawned = std::thread::Builder::new().name("spazio".into()).stack_size(64 << 20).spawn(|| {
        let _done = Done;   // even if the thread crashes, the scan does not stay "running" forever
        // only this thread in "background" (low priority CPU and disk): the app stays responsive
        #[cfg(target_os = "macos")]
        unsafe { libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG); }
        let t0 = unix_now();
        let h = home();
        let dev = std::fs::symlink_metadata(&h).map(|m| disk_id(&m)).unwrap_or(0);
        let ex = Explorer { salta: skipped_dirs(), dev, r: Mutex::new(Collection::default()) };
        if let Ok(m) = std::fs::symlink_metadata(&h) { ex.measure(&h, &m, &HashSet::new(), false, 0); }
        let interrupted = STOP.load(Ordering::SeqCst);
        let r = ex.r.into_inner().unwrap();
        // old Downloads (suggestion only): folders and files at the first level of ~/Downloads
        let dl = h.join("Downloads").to_string_lossy().to_string();
        let downloads: Vec<Value> = r.cartelle.iter().filter(|(p, b, _)| Path::new(p).parent().map(|x| x.to_string_lossy() == dl.as_str()).unwrap_or(false) && *b >= 100_000_000)
            .map(|(p, b, rec)| json!({"percorso": p, "byte": b, "mtime": rec})).collect();
        let out = json!({"versione": 1, "motore": "juggl", "ts": unix_now(), "durata_s": ((unix_now() - t0) * 10.0).round() / 10.0,
            "interrotta": interrupted, "byte": BYTES.load(Ordering::SeqCst), "file": FILES.load(Ordering::SeqCst),
            "unita": r.unita, "grandi": r.grandi, "download": downloads, "negate": r.negate});
        write_json(&file("esplora.json"), &out);
    });
    if spawned.is_err() { RUNNING.store(false, Ordering::SeqCst); return json!({"ok": false, "msg": "?"}); }
    json!({"ok": true})
}

pub fn progress() -> Value {
    let (t0, location) = SCAN_STATE.lock().unwrap().clone();
    let alive = RUNNING.load(Ordering::SeqCst);
    let used = free_disk().map(|(l, t)| t - l).unwrap_or(0.0);
    let b = BYTES.load(Ordering::Relaxed) as f64;
    json!({"vivo": alive, "stato": if alive { "in_corso" } else { "finita" }, "inizio": t0, "byte": b, "file": FILES.load(Ordering::Relaxed),
           "dove": location, "pct": if used > 0.0 { ((b / used * 100.0).min(99.0)).round() } else { 0.0 }})
}

pub fn scan_stop() -> Value {
    if !RUNNING.load(Ordering::SeqCst) { return json!({"ok": true, "msg": "nessuna scansione in corso"}); }
    STOP.store(true, Ordering::SeqCst);
    json!({"ok": true, "msg": "fermata: salvo quello che ha già misurato"})
}

/// (free, total) bytes of the home disk.
pub fn free_disk() -> Option<(f64, f64)> {
    #[cfg(unix)]
    {
        let c = std::ffi::CString::new(home().to_string_lossy().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 { return None; }
        let f = st.f_frsize as f64;
        return Some((st.f_bavail as f64 * f, st.f_blocks as f64 * f));
    }
    #[allow(unreachable_code)]
    None
}

fn processes() -> String {
    crate::new_command("/bin/ps").args(["-axo", "command="]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase()).unwrap_or_default()
}

/// A build in progress (cargo, xcodebuild, npm install...): the closed-list folders are not touched.
fn build_running(ps: &str) -> bool {
    #[cfg(test)]
    if std::env::var("JUGGL_SPAZIO_PROVA_SENZA_BUILD").is_ok() { return false; }   // only in tests (cargo test is a build)
    ps.lines().any(|r| {
        let program = r.split_whitespace().next().unwrap_or("").rsplit('/').next().unwrap_or("");
        if ["cargo", "rustc", "xcodebuild", "swift-build", "swift-frontend"].contains(&program) { return true; }
        // npm, pip and brew run inside node, python and ruby: look at the whole command line
        ["npm install", "npm ci", "npm i ", "pnpm install", "pnpm i ", "yarn install", "yarn add", "bun install", "pip install", "pip3 install", "brew install", "brew upgrade"]
            .iter().any(|x| r.contains(x))
    })
}

fn ignored() -> serde_json::Map<String, Value> { load_json(&file("lasciati.json")).and_then(|v| v.as_object().cloned()).unwrap_or_default() }

fn settings() -> Value {
    let i = crate::session_care::settings();
    json!({"obiettivo_disco_gb": i.get("obiettivo_disco_gb").and_then(|x| x.as_f64()).unwrap_or(50.0),
           "avviso_disco_pct": i.get("avviso_disco_pct").and_then(|x| x.as_f64()).unwrap_or(15.0),
           "avviso_disco": i.get("avviso_disco").and_then(|x| x.as_bool()).unwrap_or(false)})
}

/// The chat working in that folder (or in a subfolder), then the project. Only to show it: stays here.
fn owner(p: &str, sessions: &[Value], projects: &[Value]) -> Value {
    let h = home().to_string_lossy().to_string();
    let mut best: Option<&Value> = None;
    for v in sessions {
        let c = str_of(v, "cwd").trim_end_matches('/');
        if c.is_empty() || c == h || c == "/" { continue; }
        if (is_under(c, p) || is_under(p, c)) && best.map(|m| crate::f64_of(v, "da") > crate::f64_of(m, "da")).unwrap_or(true) { best = Some(v); }
    }
    if let Some(v) = best {
        return json!({"tipo": "chat", "nome": str_of(v, "nome"), "sessione": str_of(v, "sessione"), "attiva": str_of(v, "vista") == "lavora"});
    }
    for x in projects { let c = str_of(x, "cartella"); if !c.is_empty() && c != h && is_under(p, c) { return json!({"tipo": "progetto", "nome": str_of(x, "nome")}); } }
    Value::Null
}

#[allow(clippy::too_many_arguments)]
fn item(id: String, group: &str, title: String, what_is: String, bytes: f64, safety: &str, action: &str, alt: &[&str], reason: String,
        owner: Value, age: Option<f64>, kind: &str, elements: Vec<Value>) -> Value {
    json!({"id": id, "gruppo": group, "titolo": title, "cos_e": what_is, "byte": bytes, "sicurezza": safety, "azione": action,
           "alternative": alt, "motivo": reason, "chi": owner, "eta_giorni": age, "tipo": kind, "elementi": elements, "via": "fai",
           "rigenerabile": action == "cestino" && safety == "sicuro", "eseguibile": action == "cestino" || action == "lascia"})
}

fn measure_tree(p: &Path, limit: usize) -> f64 {
    let (mut total, mut n, mut queue) = (0.0, 0usize, vec![p.to_path_buf()]);
    while let Some(q) = queue.pop() {
        n += 1;
        if n > limit { break; }
        let Ok(m) = std::fs::symlink_metadata(&q) else { continue };
        if not_downloaded(&m) || m.file_type().is_symlink() { continue; }
        if m.is_dir() { if let Ok(rd) = std::fs::read_dir(&q) { queue.extend(rd.flatten().map(|x| x.path())); } } else { total += bytes_on_disk(&m) as f64; }
    }
    total
}

fn measure_trash() -> f64 {
    std::fs::read_dir(trash_dir()).map(|rd| rd.flatten().map(|e| measure_tree(&e.path(), 300_000)).sum()).unwrap_or(0.0)
}

fn project_name(p: &str) -> String {
    Path::new(p).ancestors().skip(1).filter_map(|a| a.file_name()).map(|x| x.to_string_lossy().to_string())
        .find(|x| !["src-tauri", "app", "src", "frontend", "web", "packages", "apps"].contains(&x.as_str())).unwrap_or_default()
}

/// All items, from the last full scan (checked again: what no longer exists disappears).
pub fn items() -> Value {
    let ex = load_json(&file("esplora.json")).unwrap_or(Value::Null);
    let ign = ignored();
    let sessions = crate::list_sessions();
    let projects = crate::session_care::projects();
    let ps = processes();
    let building = build_running(&ps);
    let h = home().to_string_lossy().to_string();
    let mut all_items: Vec<Value> = vec![];
    let dismissed = |id: &str, p: &str| ign.contains_key(&format!("voce:{id}")) || ign.contains_key(p);
    // 1) closed list: they come back on their own -> Trash (with confirmation)
    for u in ex.get("unita").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        let p = str_of(&u, "percorso").to_string();
        let b = crate::f64_of(&u, "byte");
        if b < 100e6 || !Path::new(&p).is_dir() || std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(true) { continue; }
        let kind = str_of(&u, "tipo").to_string();
        let (short, how) = describe_kind(&kind);
        let g = age_days(crate::f64_of(&u, "recente"));
        let c = owner(&p, &sessions, &projects);
        let mut reasons = vec![];
        let mut safety = if kind == "node_modules" { "conferma" } else { "sicuro" };
        if c.get("attiva").and_then(|x| x.as_bool()).unwrap_or(false) { safety = "conferma"; reasons.push(t(&format!("la chat \"{}\" sta lavorando qui", str_of(&c, "nome")), &format!("the chat \"{}\" is working here", str_of(&c, "nome")))); }
        if g < 1.0 { safety = "conferma"; reasons.push(t("usata nelle ultime 24 ore", "used in the last 24 hours")); }
        if kind == "node_modules" { reasons.push(t("poi va reinstallata con npm install", "you'll need npm install afterwards")); }
        if [format!("{h}/Desktop"), format!("{h}/Documents")].iter().any(|c| is_under(&p, c)) {
            safety = "conferma";
            reasons.push(t("è in Scrivania o Documenti: se le sincronizzi con iCloud, va nel Cestino anche sugli altri dispositivi", "it's in Desktop or Documents: if they sync with iCloud, it goes to the Trash on your other devices too"));
        }
        let title = if ["rust", "node_modules"].contains(&kind.as_str()) { format!("{short} {} {}", t("di", "of"), project_name(&p)) } else { short };
        let id = format!("u-{}", hash(&p));
        let mut v = item(id.clone(), if ["npm", "pip", "homebrew"].contains(&kind.as_str()) { "cache" } else if kind == "node_modules" { "librerie" } else { "build" },
            title, format!("{} {}{}.", t("Va nel Cestino, si recupera con Annulla.", "Goes to the Trash, restore it with Undo."), how.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), how.chars().skip(1).collect::<String>()),
            b, safety, "cestino", &["lascia"], reasons.join("; "), c, Some(g), &kind, vec![json!({"percorso": p, "byte": b})]);
        v["lasciata"] = json!(dismissed(&id, &p));
        all_items.push(v);
    }
    // 2) large files and old Downloads: suggestion only (Show in Finder), never an action
    let personal_dirs: Vec<String> = ["Desktop", "Downloads", "Documents", "Movies", "Music"].iter().map(|x| format!("{h}/{x}")).collect();
    for g in ex.get("grandi").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        let p = str_of(&g, "percorso").to_string();
        if !Path::new(&p).is_file() || !personal_dirs.iter().any(|c| is_under(&p, c)) || p.contains(".app/") { continue; }
        let b = crate::f64_of(&g, "byte");
        let idle_days = age_days(crate::f64_of(&g, "mtime"));
        let id = format!("f-{}", hash(&p));
        let mut v = item(id.clone(), "file", str_of(&g, "nome").to_string(),
            if idle_days < 1.0 { t("File grande, cambiato di recente. Juggl non lo tocca: aprilo nel Finder e decidi tu.", "Large file, changed recently. Juggl won't touch it: open it in Finder and decide.") }
            else { t(&format!("File grande, fermo da {} giorni. Juggl non lo tocca: aprilo nel Finder e decidi tu.", idle_days as i64),
              &format!("Large file, untouched for {} days. Juggl won't touch it: open it in Finder and decide.", idle_days as i64)) },
            b, "segnala", "apri", &[], String::new(), Value::Null, Some(idle_days), "file", vec![json!({"percorso": p, "byte": b})]);
        v["lasciata"] = json!(dismissed(&id, &p));
        all_items.push(v);
    }
    for d in ex.get("download").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        let p = str_of(&d, "percorso").to_string();
        let idle_days = age_days(crate::f64_of(&d, "mtime"));
        if idle_days < 60.0 || !Path::new(&p).exists() { continue; }
        let b = crate::f64_of(&d, "byte");
        let id = format!("d-{}", hash(&p));
        let name = Path::new(&p).file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        let mut v = item(id.clone(), "file", name, t(&format!("In Download da oltre {} giorni. Juggl non lo tocca: guardalo nel Finder.", idle_days as i64),
            &format!("In Downloads for more than {} days. Juggl won't touch it: have a look in Finder.", idle_days as i64)),
            b, "segnala", "apri", &[], String::new(), Value::Null, Some(idle_days), "download", vec![json!({"percorso": p, "byte": b})]);
        v["lasciata"] = json!(dismissed(&id, &p));
        all_items.push(v);
    }
    // 3) good to know: iOS simulators, swap memory
    let sim = home().join("Library/Developer/CoreSimulator/Devices");
    if sim.is_dir() {
        let b = measure_tree(&sim, 400_000);
        if b > 3e9 {
            all_items.push(item("s-simulatori".into(), "sistema", t("Simulatori iOS", "iOS Simulators"),
                t("Si tolgono da Xcode: Window > Devices and Simulators, oppure Impostazioni > Platforms per i sistemi interi.",
                  "Remove them from Xcode: Window > Devices and Simulators, or Settings > Platforms for whole runtimes."),
                b, "segnala", "manuale", &[], String::new(), Value::Null, None, "manuale", vec![]));
        }
    }
    let swap = crate::new_command("/usr/sbin/sysctl").args(["-n", "vm.swapusage"]).output().ok()
        .and_then(|o| { let x = String::from_utf8_lossy(&o.stdout).to_string(); x.split("used = ").nth(1).and_then(|y| y.split('M').next()).and_then(|y| y.trim().replace(',', ".").parse::<f64>().ok()) })
        .map(|mb| mb * 1048576.0).unwrap_or(0.0);
    if swap > 4e9 {
        all_items.push(item("s-swap".into(), "sistema", t("Memoria di scambio (swap)", "Swap memory"),
            t("macOS sposta sul disco la memoria che non ci sta. Scende solo riavviando il Mac.", "macOS moves memory that doesn't fit onto the disk. It only goes down after a restart."),
            swap, "segnala", "manuale", &[], String::new(), Value::Null, None, "manuale", vec![]));
    }
    let trash_bytes = measure_trash();
    let (free, total) = free_disk().unwrap_or((0.0, 0.0));
    let prefs = settings();
    let target = crate::f64_of(&prefs, "obiettivo_disco_gb") * GB;
    let active = |v: &Value| !v.get("lasciata").and_then(|x| x.as_bool()).unwrap_or(false);
    let safe_bytes: f64 = all_items.iter().filter(|v| active(v) && str_of(v, "sicurezza") == "sicuro").map(|v| crate::f64_of(v, "byte")).sum();
    let chosen: f64 = all_items.iter().filter(|v| active(v) && str_of(v, "sicurezza") == "conferma" && str_of(v, "azione") == "cestino").map(|v| crate::f64_of(v, "byte")).sum();
    all_items.sort_by_key(|v| (!active(v), match str_of(v, "sicurezza") { "sicuro" => 0, "conferma" => 1, _ => 2 }, -(crate::f64_of(v, "byte") as i64)));
    let group_defs = [
        ("build", t("Build", "Builds"), t("Compilazioni che si rifanno alla prossima build.", "Compiled output that comes back on the next build.")),
        ("cache", t("Cache degli sviluppatori", "Developer caches"), t("Pacchetti scaricati che npm, pip e Homebrew riscaricano quando servono.", "Downloaded packages that npm, pip and Homebrew fetch again when needed.")),
        ("librerie", t("Librerie dei progetti", "Project libraries"), t("node_modules: si reinstallano con npm install.", "node_modules: reinstall with npm install.")),
        ("file", t("File grandi e Download vecchi", "Large files and old downloads"), t("Solo da guardare: Juggl non li tocca.", "Just to look at: Juggl doesn't touch them.")),
        ("sistema", t("Da sapere", "Good to know"), t("Cose che non si tolgono da qui.", "Things you can't remove from here."))];
    let mut groups = vec![];
    for (gid, name, explanation) in group_defs {
        let vs: Vec<&Value> = all_items.iter().filter(|v| str_of(v, "gruppo") == gid && active(v)).collect();
        if vs.is_empty() { continue; }
        groups.push(json!({"id": gid, "nome": name, "spiegazione": explanation,
            "byte": vs.iter().filter(|v| str_of(v, "sicurezza") != "segnala").map(|v| crate::f64_of(v, "byte")).sum::<f64>(),
            "sicuro_b": vs.iter().filter(|v| str_of(v, "sicurezza") == "sicuro").map(|v| crate::f64_of(v, "byte")).sum::<f64>(),
            "voci": vs.iter().map(|v| str_of(v, "id").to_string()).collect::<Vec<_>>()}));
    }
    let by_id: serde_json::Map<String, Value> = all_items.iter().map(|v| (str_of(v, "id").to_string(), v.clone())).collect();
    let scan_running = RUNNING.load(Ordering::SeqCst);
    json!({"versione": 2, "motore": "juggl", "lingua": if en() { "en" } else { "it" }, "ts": unix_now(),
        "riepilogo": {"libero_b": free, "totale_b": total, "obiettivo_b": target, "manca_b": (target - free).max(0.0), "sicuro_b": safe_bytes, "scelte_b": chosen,
            "cestino_b": trash_bytes, "swap_b": swap, "soglia_pct": crate::f64_of(&prefs, "avviso_disco_pct"), "avviso_disco": prefs.get("avviso_disco"),
            "libero_pct": if total > 0.0 { (free / total * 1000.0).round() / 10.0 } else { 0.0 },
            "completa_ts": ex.get("ts"), "completa_file": ex.get("file"), "completa_interrotta": ex.get("interrotta"), "archivio_ok": false,
            "conferma_sempre": true, "solo_cestino": true, "cartelle_negate": ex.get("negate").cloned().unwrap_or(json!([]))},
        "completa_in_corso": if scan_running { progress() } else { Value::Null },
        "andamento": trend(), "cresce": [], "gruppi": groups, "voci": by_id,
        "lasciate": all_items.iter().filter(|v| !active(v)).map(|v| str_of(v, "id").to_string()).collect::<Vec<_>>(),
        "protetti": [], "protetti_fissi": [], "storico": history(), "automatico": Value::Null,
        "build_in_corso": if building { json!(["build"]) } else { json!([]) }})
}

fn trend() -> Value {
    let v = load_json(&file("andamento.json")).and_then(|x| x.as_array().cloned()).unwrap_or_default();
    let cutoff = unix_now() - 4.0 * DAY;
    json!(v.into_iter().filter(|x| crate::f64_of(x, "ts") >= cutoff).collect::<Vec<_>>())
}

/// From the keeper: one point of the free space trend every 30 minutes (numbers only, stays on the Mac).
pub fn record_trend() {
    let Some((free, _)) = free_disk() else { return };
    let p = file("andamento.json");
    let mut v = load_json(&p).and_then(|x| x.as_array().cloned()).unwrap_or_default();
    if v.last().map(|x| unix_now() - crate::f64_of(x, "ts") < 1700.0).unwrap_or(false) { return; }
    v.push(json!({"ts": unix_now().round(), "libero_b": free}));
    let n = v.len();
    if n > 400 { v.drain(0..n - 400); }
    write_json(&p, &json!(v));
}

fn log_append(r: &Value) {
    let _ = std::fs::create_dir_all(dir());
    let _ = crate::storage::modify(&file("registro.json"), Some(json!([])), |l| {
        if !l.is_array() { *l = json!([]); }
        let a = l.as_array_mut().unwrap();
        a.push(r.clone());
        let n = a.len();
        if n > 300 { a.drain(0..n - 300); }
        true
    });
}

fn history() -> Value {
    let l = load_json(&file("registro.json")).and_then(|x| x.as_array().cloned()).unwrap_or_default();
    let undone: HashSet<String> = l.iter().filter(|x| str_of(x, "modo") == "annulla").map(|x| str_of(x, "di").to_string()).collect();
    json!(l.iter().rev().filter(|x| str_of(x, "modo") != "annulla").take(40).map(|x| {
        let n = str_of(x, "n").to_string();
        let has_trash = x.get("cestino").and_then(|c| c.as_array()).map(|a| !a.is_empty()).unwrap_or(false);
        let undoable = (has_trash || str_of(x, "modo") == "lascia") && !undone.contains(&n);
        json!({"n": n, "ts": x.get("ts"), "titolo": x.get("titolo"), "modo": x.get("modo"), "esito": x.get("esito"), "byte": x.get("byte"),
               "liberato_b": x.get("liberato_b"), "annullabile": undoable, "annullato": undone.contains(&n), "automatica": false})
    }).collect::<Vec<_>>())
}

/// Final check before moving to the Trash: must still be a closed-list folder, in the home,
/// not a link, not inside a cloud folder, not touched in the last 24 hours if it is a build.
fn can_move(p: &str) -> Result<(), String> {
    let pa = Path::new(p);
    let m = std::fs::symlink_metadata(pa).map_err(|_| t("non c'è più", "it's gone"))?;
    if m.file_type().is_symlink() { return Err(t("è un collegamento", "it's a link")); }
    if not_downloaded(&m) { return Err(t("non è scaricato sul Mac", "it isn't downloaded on this Mac")); }
    safe_path(pa)?;
    let h = home().to_string_lossy().to_string();
    if !is_under(p, &h) || p == h || skipped_dirs().iter().any(|x| is_under(p, x)) { return Err(t("cartella protetta", "protected folder")); }
    let hm = std::fs::symlink_metadata(&h).map(|m| disk_id(&m)).unwrap_or(0);
    if disk_id(&m) != hm { return Err(t("sta su un altro disco", "it's on another disk")); }
    let names: HashSet<String> = std::fs::read_dir(pa).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    let parent_names: HashSet<String> = pa.parent().and_then(|x| std::fs::read_dir(x).ok()).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    let name = pa.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
    match unit_kind(p, &name, &names, &parent_names) {
        Some("app") | None => Err(t("non è nella lista delle cose che si rifanno", "not on the list of things that come back on their own")),
        Some(tp) => {
            if ["rust", "xcode"].contains(&tp) && touched_recently(pa, 24.0) { return Err(t("modificata nelle ultime 24 ore", "changed in the last 24 hours")); }
            if touched_recently(pa, 0.25) { return Err(t("usata negli ultimi 15 minuti", "used in the last 15 minutes")); }
            Ok(())
        }
    }
}

fn touched_recently(p: &Path, hours: f64) -> bool {
    let threshold = unix_now() - hours * 3600.0;
    let t0 = std::time::Instant::now();
    let mut queue = vec![(p.to_path_buf(), 0)];
    while let Some((d, depth)) = queue.pop() {
        if t0.elapsed().as_secs() > 4 { break; }
        let Ok(m) = std::fs::symlink_metadata(&d) else { continue };
        if mtime(&m) > threshold { return true; }
        if depth >= 3 || !m.is_dir() { continue; }
        if let Ok(rd) = std::fs::read_dir(&d) { for e in rd.flatten() { queue.push((e.path(), depth + 1)); } }
    }
    false
}

/// Some program has files open inside these folders (a single lsof, without walking the folders).
fn files_open(roots: &[String]) -> bool {
    // if lsof does not answer, treat them as "open": better not to move than to move them from under a program
    let Ok(o) = crate::new_command("/usr/sbin/lsof").args(["-nP", "-w", "-b", "-F", "n"]).output() else { return true };
    if o.stdout.is_empty() { return true; }
    String::from_utf8_lossy(&o.stdout).lines().filter_map(|r| r.strip_prefix('n')).any(|f| roots.iter().any(|x| is_under(f, x)))
}

/// Into the user's Trash with a free name (like the Finder). Same disk: it is just a rename.
fn move_to_trash(p: &Path) -> Result<PathBuf, String> {
    let c = trash_dir();
    if !c.exists() {
        std::fs::create_dir_all(&c).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&c, std::fs::Permissions::from_mode(0o700)); }
    }
    let name = p.file_name().map(|x| x.to_string_lossy().to_string()).ok_or("nome")?;
    let (b, e) = match name.rfind('.') { Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()), _ => (name.clone(), String::new()) };
    for i in 0..1000 {
        let d = c.join(if i == 0 { name.clone() } else { format!("{b} {i}{e}") });
        if std::fs::symlink_metadata(&d).is_err() {
            std::fs::rename(p, &d).map_err(|e| e.to_string())?;
            return Ok(d);
        }
    }
    Err(t("nessun nome libero nel Cestino", "no free name in the Trash"))
}

fn uuid8() -> String { format!("{:010x}", (unix_now() * 1e6) as u64 & 0xff_ffff_ffff) }

/// One or more items: only "cestino" (closed list) or "lascia". Without confirmation it does nothing.
pub fn act_on(ids: &[String], mode: &str, confirm: bool) -> Value {
    if !confirm && mode != "lascia" { return json!({"ok": false, "msg": t("serve la conferma", "confirmation needed"), "conferma": true}); }
    if !["cestino", "lascia"].contains(&mode) { return json!({"ok": false, "msg": t("qui si può solo spostare nel Cestino", "here you can only move things to the Trash")}); }
    let _g = ACTION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = items();
    let building = build_running(&processes());
    let (mut done, mut reasons, mut trashed_bytes) = (0, vec![], 0.0);
    let mut last_n = String::new();
    for id in ids {
        let Some(v) = d.get("voci").and_then(|x| x.get(id)) else { reasons.push(t("voce non trovata: rifai la scansione", "item not found: scan again")); continue };
        let n = format!("{}{}", uuid8(), done);
        let mut rec = json!({"n": n, "ts": unix_now(), "titolo": str_of(v, "titolo"), "modo": mode, "byte": crate::f64_of(v, "byte"), "esito": "ok"});
        let elements: Vec<String> = v.get("elementi").and_then(|x| x.as_array()).map(|a| a.iter().map(|e| str_of(e, "percorso").to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
        if mode == "lascia" {
            let mut keys = vec![format!("voce:{id}")];
            keys.extend(elements.iter().cloned());
            let _ = crate::storage::modify(&file("lasciati.json"), Some(json!({})), |m| {
                if !m.is_object() { *m = json!({}); }
                for k in &keys { m[k] = json!({"t": unix_now()}); }
                true
            });
            rec["lascia"] = json!(keys);
            done += 1;
        } else {
            if str_of(v, "azione") != "cestino" { reasons.push(t("questa voce è solo da guardare", "this item is view-only")); continue; }
            if building { reasons.push(t("c'è una build in corso: riprova quando finisce", "a build is running: try again when it's done")); continue; }
            if files_open(&elements) { reasons.push(t("un programma ha file aperti lì dentro: chiudilo e riprova", "a program has files open in there: close it and try again")); continue; }
            let mut moved = vec![];
            for p in &elements {
                match can_move(p).and_then(|_| move_to_trash(Path::new(p))) {
                    Ok(a) => {
                        // fingerprint of the moved folder: Undo restores only this one, not a same-named one put in the Trash later
                        #[cfg(unix)]
                        let ino = { use std::os::unix::fs::MetadataExt; std::fs::symlink_metadata(&a).map(|m| m.ino()).unwrap_or(0) };
                        #[cfg(not(unix))]
                        let ino = 0u64;
                        moved.push(json!({"da": p, "a": a.to_string_lossy(), "ino": ino}))
                    }
                    Err(e) => reasons.push(e),
                }
            }
            if moved.is_empty() { continue; }
            done += 1;
            trashed_bytes += crate::f64_of(v, "byte");
            rec["cestino"] = json!(moved);
        }
        last_n = n;
        log_append(&rec);
    }
    reasons.sort();
    reasons.dedup();
    json!({"ok": done > 0, "n": last_n, "fatti": done, "liberato_b": 0, "liberato_gb": 0, "nel_cestino_b": trashed_bytes,
           "msg": reasons.join("; "), "build_in_corso": building})
}

/// The big button: all "sicuro" items to the Trash (with confirmation, summary in the view).
pub fn free_safe(confirm: bool) -> Value {
    let d = items();
    let ids: Vec<String> = d.get("voci").and_then(|x| x.as_object()).map(|m| m.values()
        .filter(|v| str_of(v, "sicurezza") == "sicuro" && str_of(v, "azione") == "cestino" && !v.get("lasciata").and_then(|x| x.as_bool()).unwrap_or(false))
        .map(|v| str_of(v, "id").to_string()).collect()).unwrap_or_default();
    if ids.is_empty() { return json!({"ok": false, "msg": t("niente da spostare", "nothing to move")}); }
    let mut r = act_on(&ids, "cestino", confirm);
    let skipped = ids.len() as i64 - r.get("fatti").and_then(|x| x.as_i64()).unwrap_or(0);
    r["saltati"] = json!((0..skipped.max(0)).map(|_| json!({})).collect::<Vec<_>>());
    r
}

pub fn undo(n: &str) -> Value {
    let _g = ACTION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let l = load_json(&file("registro.json")).and_then(|x| x.as_array().cloned()).unwrap_or_default();
    let Some(rec) = l.iter().find(|x| str_of(x, "n") == n) else { return json!({"ok": false, "msg": t("non trovo questa azione", "can't find this action")}) };
    let (mut restored, mut errors) = (vec![], vec![]);
    if let Some(ch) = rec.get("lascia").and_then(|x| x.as_array()) {
        let _ = crate::storage::modify(&file("lasciati.json"), Some(json!({})), |m| {
            if let Some(o) = m.as_object_mut() { for k in ch { o.remove(k.as_str().unwrap_or("")); } }
            true
        });
        restored.push("lascia".to_string());
    }
    for x in rec.get("cestino").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        let (from, a) = (str_of(&x, "da"), str_of(&x, "a"));
        if std::fs::symlink_metadata(from).is_ok() { errors.push(t("al suo posto c'è già qualcosa", "something is already in its place")); continue; }
        let Ok(trash_meta) = std::fs::symlink_metadata(a) else { errors.push(t("non è più nel Cestino", "it's no longer in the Trash")); continue };
        let trash = std::fs::canonicalize(trash_dir()).map(|c| c.to_string_lossy().to_string()).unwrap_or_default();
        let real_parent = Path::new(a).parent().and_then(|p| std::fs::canonicalize(p).ok()).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        if trash.is_empty() || real_parent != trash { errors.push(t("non viene dal Cestino", "it doesn't come from the Trash")); continue; }
        if let Err(e) = safe_path(Path::new(from)) { errors.push(e); continue; }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let expected = x.get("ino").and_then(|v| v.as_u64()).unwrap_or(0);
            if expected != 0 && trash_meta.ino() != expected { errors.push(t("nel Cestino c'è un'altra cosa con lo stesso nome", "something else with the same name is in the Trash")); continue; }
        }
        #[cfg(not(unix))]
        let _ = &trash_meta;
        match std::fs::rename(a, from) { Ok(()) => restored.push(from.to_string()), Err(e) => errors.push(e.to_string()) }
    }
    log_append(&json!({"n": uuid8(), "ts": unix_now(), "modo": "annulla", "di": n, "titolo": format!("{}: {}", t("Annullato", "Undone"), str_of(rec, "titolo"))}));
    json!({"ok": !restored.is_empty() && errors.is_empty(), "ripristinati": restored, "msg": errors.join("; ")})
}

/// Empties the Trash (or only things put there more than N days ago). The view asks for it in two steps.
pub fn empty_trash(confirm: bool, days: f64) -> Value {
    if !confirm { return json!({"ok": false, "msg": t("serve la conferma", "confirmation needed"), "conferma": true}); }
    let _g = ACTION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let before = free_disk().map(|x| x.0).unwrap_or(0.0);
    let threshold = unix_now() - days * DAY;
    let mut n = 0;
    let rd = match std::fs::read_dir(trash_dir()) {
        Ok(rd) => rd,
        Err(_) => return json!({"ok": false, "msg": t("non riesco a leggere il Cestino (permesso di macOS)", "can't read the Trash (macOS permission)")}),
    };
    {
        for e in rd.flatten() {
            let p = e.path();
            if p.file_name().map(|x| x == ".DS_Store").unwrap_or(false) { continue; }
            let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
            #[cfg(unix)]
            let trashed_at = { use std::os::unix::fs::MetadataExt; m.ctime() as f64 };   // ctime changes when it goes to the Trash
            #[cfg(not(unix))]
            let trashed_at = mtime(&m);
            if days > 0.0 && trashed_at > threshold { continue; }
            let r = if m.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            if r.is_ok() { n += 1; }
        }
    }
    let freed = (free_disk().map(|x| x.0).unwrap_or(0.0) - before).max(0.0);
    log_append(&json!({"n": uuid8(), "ts": unix_now(), "modo": "svuota", "titolo": t("Svuotato il Cestino", "Emptied the Trash"), "liberato_b": freed, "esito": "ok"}));
    json!({"ok": true, "elementi": n, "liberato_b": freed, "liberato_gb": (freed / 1e7).round() / 100.0})
}

fn set_settings(pairs: &[String]) -> Value {
    let mut updates = serde_json::Map::new();
    for c in pairs.chunks(2) {
        if c.len() < 2 { break; }
        let (k, v) = (c[0].as_str(), c[1].as_str());
        if k == "avviso_disco" { updates.insert(k.into(), json!(v == "true")); continue; }
        let limits = match k { "obiettivo_disco_gb" => (10.0, 400.0), "avviso_disco_pct" => (3.0, 40.0), _ => return json!({"ok": false, "msg": "?"}) };
        let Ok(n) = v.replace(',', ".").parse::<f64>() else { return json!({"ok": false, "msg": "?"}) };
        updates.insert(k.into(), json!(n.clamp(limits.0, limits.1)));
    }
    let r = crate::storage::modify(&base_dir().join("impostazioni.json"), Some(json!({})), |m| {
        if !m.is_object() { *m = json!({}); }
        for (k, v) in &updates { m[k] = v.clone(); }
        true
    });
    json!({"ok": r.is_ok()})
}

fn propose_again(id: &str) -> Value {
    let _ = crate::storage::modify(&file("lasciati.json"), Some(json!({})), |m| { if let Some(o) = m.as_object_mut() { o.remove(&format!("voce:{id}")); } true });
    json!({"ok": true})
}

/// Single entry point (arguments already checked by session_care::mac_spazio).
pub fn command(args: &[String]) -> Value {
    if !cfg!(target_os = "macos") { return json!({"ok": false, "msg": t("la gestione dello spazio per ora c'è solo su Mac", "disk space management is Mac-only for now")}); }
    if !home_is_valid() { return json!({"ok": false, "msg": t("cartella utente non valida", "invalid home folder")}); }
    let opt = |n: &str| args.iter().position(|x| x == n).and_then(|i| args.get(i + 1)).cloned();
    let confirm = args.iter().any(|x| x == "--conferma");
    let mut positional = vec![];
    let mut skip_next = false;
    for a in args.iter().skip(1) {
        if skip_next { skip_next = false; continue; }
        if a == "--modo" || a == "--giorni" { skip_next = true; continue; }
        if !a.starts_with("--") { positional.push(a.clone()); }
    }
    match args.first().map(|x| x.as_str()).unwrap_or("") {
        "voci" => items(),
        "avanza" => progress(),
        "esplora-avvia" => scan_start(),
        "esplora-ferma" => scan_stop(),
        "fai" | "fai-molti" => act_on(&positional, &opt("--modo").unwrap_or_default(), confirm),
        "libera-sicuro" => free_safe(confirm),
        "annulla" => positional.first().map(|n| undo(n)).unwrap_or_else(|| json!({"ok": false})),
        "svuota-cestino" => {
            // --giorni, if present, must be a positive number: a wrong value never becomes "empty everything"
            if !args.iter().any(|x| x == "--giorni") { empty_trash(confirm, 0.0) }
            else {
                match opt("--giorni").and_then(|x| x.parse::<f64>().ok()).filter(|g| g.is_finite() && *g > 0.0) {
                    Some(g) => empty_trash(confirm, g),
                    None => json!({"ok": false, "msg": t("numero di giorni non valido", "invalid number of days")}),
                }
            }
        }
        "imposta" => set_settings(&positional),
        "riproponi" => positional.first().map(|n| propose_again(n)).unwrap_or_else(|| json!({"ok": false})),
        _ => json!({"ok": false, "msg": t("comando non disponibile", "command not available")}),
    }
}

/// Test on a fake HOME (fake target folders, node_modules, DerivedData, cloud, large files): scan,
/// items, Trash, Undo, refusals. Runs with HOME and JUGGL_BASE pointed at the test folder.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::fs;

    fn write_file(p: &Path, n: usize) { fs::create_dir_all(p.parent().unwrap()).unwrap(); fs::write(p, vec![7u8; n]).unwrap(); }
    fn make_old(p: &Path) {
        // two days ago, on the whole tree (builds touched in the last 24 hours are not moved)
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400);
        let mut queue = vec![p.to_path_buf()];
        while let Some(q) = queue.pop() {
            if q.is_dir() { queue.extend(fs::read_dir(&q).unwrap().flatten().map(|e| e.path())); }
            let f = fs::File::options().read(true).open(&q).or_else(|_| fs::File::open(&q)).unwrap();
            let _ = f.set_modified(t);
        }
    }

    #[test]
    fn disk_space_fake_home() {
        let _g = crate::test_data_lock();   // JUGGL_BASE and HOME apply to the whole process: one test at a time
        let t0 = std::env::temp_dir().join(format!("juggl-spazio-prova-{}", std::process::id()));
        let h = t0.join("casa");
        let _ = fs::remove_dir_all(&t0);
        // Rust project with an old build, web project with node_modules, DerivedData, a "cloud" folder, a large file
        write_file(&h.join("prog/Cargo.toml"), 10);
        write_file(&h.join("prog/target/CACHEDIR.TAG"), 10);
        write_file(&h.join("prog/target/debug/x.bin"), 120 << 20);
        write_file(&h.join("sito/package.json"), 10);
        write_file(&h.join("sito/node_modules/a/index.js"), 110 << 20);
        write_file(&h.join("Library/Developer/Xcode/DerivedData/App-abc/b.o"), 105 << 20);
        write_file(&h.join("Library/CloudStorage/Dropbox/prog2/Cargo.toml"), 10);
        write_file(&h.join("Library/CloudStorage/Dropbox/prog2/target/CACHEDIR.TAG"), 10);
        write_file(&h.join("Library/CloudStorage/Dropbox/prog2/target/y.bin"), 101 << 20);
        make_old(&h.join("Library/CloudStorage/Dropbox/prog2/target"));
        write_file(&h.join("Movies/grande.mov"), 520 << 20);
        write_file(&h.join("Documents/cartella-normale/x.txt"), 10);
        // escape routes: a real folder OUTSIDE the fake HOME, reached through a symbolic link and a hard link
        let outside = t0.join("obiettivo-finto");
        write_file(&outside.join("prog3/Cargo.toml"), 10);
        write_file(&outside.join("prog3/target/CACHEDIR.TAG"), 10);
        write_file(&outside.join("prog3/target/z.bin"), 101 << 20);
        make_old(&outside.join("prog3/target"));
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, h.join("collegamento")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("prog3/target"), h.join("prog/target-link")).unwrap();
        write_file(&outside.join("dato.bin"), 1 << 20);
        fs::hard_link(outside.join("dato.bin"), h.join("prog/target/debug/dato-fisso.bin")).unwrap();
        // other apps' containers: never read
        write_file(&h.join("Library/Containers/com.esempio.app/Data/prog4/Cargo.toml"), 10);
        write_file(&h.join("Library/Containers/com.esempio.app/Data/prog4/target/CACHEDIR.TAG"), 10);
        write_file(&h.join("Library/Containers/com.esempio.app/Data/prog4/target/w.bin"), 101 << 20);
        make_old(&h.join("prog/target"));
        make_old(&h.join("Library/Developer/Xcode/DerivedData"));
        // a folder macOS does not let us read (like Documents without permission): skipped and reported
        fs::create_dir_all(h.join("Riservata/dentro")).unwrap();
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; fs::set_permissions(h.join("Riservata"), fs::Permissions::from_mode(0o000)).unwrap(); }
        let _env = crate::EnvGuard::new(&["HOME", "JUGGL_BASE", "JUGGL_LINGUA"]);
        std::env::set_var("HOME", &h);
        std::env::set_var("JUGGL_BASE", t0.join("dati"));
        std::env::set_var("JUGGL_LINGUA", "it");
        // synchronous scan (like the view's thread)
        scan_start();
        for _ in 0..600 { if !RUNNING.load(Ordering::SeqCst) { break; } std::thread::sleep(std::time::Duration::from_millis(50)); }
        let v = items();
        // for the view screenshots: only if the tests folder sits next to the clone (no files elsewhere)
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prove/dati");
        if folder.is_dir() { let _ = fs::write(folder.join("voci-integrato.json"), v.to_string()); }
        let items_map = v["voci"].as_object().unwrap();
        let by_kind = |tp: &str| items_map.values().find(|x| str_of(x, "tipo") == tp).cloned();
        let rust = by_kind("rust").expect("build Rust trovata");
        assert_eq!(str_of(&rust, "azione"), "cestino");
        assert_eq!(str_of(&rust, "sicurezza"), "sicuro");
        assert_eq!(str_of(&by_kind("node_modules").expect("node_modules"), "sicurezza"), "conferma");
        assert!(by_kind("xcode").is_some(), "DerivedData");
        let large = by_kind("file").expect("file grande");
        assert_eq!(str_of(&large, "azione"), "apri", "i file grandi sono solo da guardare");
        // never anything from cloud folders, other apps' containers, or outside the fake HOME
        assert!(v["riepilogo"]["cartelle_negate"].as_array().unwrap().iter().any(|x| x.as_str().unwrap_or("").ends_with("Riservata")), "{}", v["riepilogo"]);
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; fs::set_permissions(h.join("Riservata"), fs::Permissions::from_mode(0o755)).unwrap(); }
        let all_text = serde_json::to_string(&v).unwrap();
        assert!(!all_text.contains("CloudStorage") && !all_text.contains("Containers") && !all_text.contains("obiettivo-finto") && !all_text.contains("collegamento"), "{all_text}");
        assert_eq!(items_map.values().filter(|x| str_of(x, "tipo") == "rust").count(), 1, "solo la build vera, non quella raggiunta dal collegamento");
        assert!(can_move(&h.join("prog/target-link").to_string_lossy()).is_err(), "un collegamento non si sposta");
        assert!(can_move(&outside.join("prog3/target").to_string_lossy()).is_err(), "fuori dalla HOME non si sposta");
        // review Nov 15: ".." and a parent that goes through a link (the real build outside, reached from inside home)
        assert!(can_move(&format!("{}/prog/../prog/target", h.display())).is_err(), "niente ..");
        assert!(can_move(&h.join("collegamento/prog3/target").to_string_lossy()).is_err(), "genitore che e' un collegamento");
        // a wrong --giorni never empties everything
        let g = command(&["svuota-cestino".into(), "--conferma".into(), "--giorni".into(), "abc".into()]);
        assert!(!g["ok"].as_bool().unwrap(), "{g}");
        // without confirmation it does nothing; the large file is never moved
        let rust_id = str_of(&rust, "id").to_string();
        assert!(!act_on(&[rust_id.clone()], "cestino", false)["ok"].as_bool().unwrap());
        assert!(!act_on(&[str_of(&large, "id").to_string()], "cestino", true)["ok"].as_bool().unwrap());
        assert!(!act_on(&[rust_id.clone()], "cancella", true)["ok"].as_bool().unwrap(), "niente cancellazione diretta");
        assert!(can_move(&h.join("Documents/cartella-normale").to_string_lossy()).is_err());
        // with a build running (here: cargo test itself) nothing is moved
        let b = act_on(&[rust_id.clone()], "cestino", true);
        assert!(!b["ok"].as_bool().unwrap() && b["build_in_corso"].as_bool().unwrap(), "{b}");
        std::env::set_var("JUGGL_SPAZIO_PROVA_SENZA_BUILD", "1");
        // to the Trash and Undo
        let r = act_on(&[rust_id.clone()], "cestino", true);
        assert!(r["ok"].as_bool().unwrap(), "{r}");
        assert!(!h.join("prog/target").exists() && h.join(".Trash/target").exists());
        // the Trash is the fake HOME's one, not the real one; the hard link does not touch the outside file
        let reg = load_json(&file("registro.json")).unwrap();
        let dest = reg.as_array().unwrap().iter().rev().find_map(|x| x["cestino"][0]["a"].as_str().map(String::from)).unwrap();
        assert!(dest.starts_with(&h.to_string_lossy().to_string()), "Cestino fuori dalla HOME finta: {dest}");
        assert_eq!(fs::metadata(outside.join("dato.bin")).unwrap().len(), 1 << 20);
        assert!(outside.join("prog3/target/z.bin").exists() && h.join("collegamento").symlink_metadata().unwrap().file_type().is_symlink());
        let a = undo(str_of(&r, "n"));
        assert!(a["ok"].as_bool().unwrap(), "{a}");
        assert!(h.join("prog/target/debug/x.bin").exists());
        // dismiss and propose again
        let l = act_on(&[rust_id.clone()], "lascia", false);
        assert!(items()["lasciate"].as_array().unwrap().iter().any(|x| x == &json!(rust_id)));
        undo(str_of(&l, "n"));
        assert!(!items()["lasciate"].as_array().unwrap().iter().any(|x| x == &json!(rust_id)));
        let _ = fs::remove_dir_all(&t0);
    }
}
