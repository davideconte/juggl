// Imports projects and chats (welcome and Settings, Nov 9).
//
// Three steps: find, propose, create.
// - Finds the chats already done with Claude Code (~/.claude/projects/*/*.jsonl, grouped by working folder) and the
//   projects on disk in a few known folders (never the whole home or the root), with limited depth and time.
//   iCloud and Dropbox: only folders and files already downloaded are looked at ("dataless" attribute read with lstat, which
//   downloads nothing); a folder that is not downloaded is never opened.
// - Proposes the merged list: a project with existing chats appears only once, with "N chat".
// - Creates, for each chosen project, a "ready" session: a hibernated state without a conversation, with the context
//   of the project in a file (~/tavolo/contesti/<id>.md). No conversation with Claude starts (zero tokens):
//   it starts when you open it and write, and the context reaches it as an added system instruction (hosted.rs).
//   Existing chats become resumable archived chats (same path as Archive: no duplicates).
// Everything stays on this Mac: nothing is sent. Secret files (.env, keys, .ssh) are not read.
#[cfg(windows)] use crate::sys as libc;
use crate::{storage, base_dir, home, load_json, unix_now, str_of};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::Emitter;

/// Max time for the disk search (chats aside, which are always all read).
const DISK_TIME: Duration = Duration::from_millis(4500);
/// Levels below each starting folder.
const DEPTH: usize = 4;
/// Checked by default: active in the last 60 days.
const ACTIVE_DAYS: f64 = 60.0;
/// Chats per project that become archived (the most recent). The others stay in search (⌘K) and can be resumed from there.
const CHATS_PER_PROJECT: usize = 30;
/// Max length of the context (about 2,000 tokens).
const MAX_CONTEXT: usize = 8000;

/// Folders that are never opened: dependencies, build outputs, caches, system libraries.
const SKIP: [&str; 34] = ["node_modules", "build", "dist", "out", "target", "vendor", "Pods", "DerivedData", "Library", "Caches", "cache",
    "Cache", "__pycache__", "venv", "env", "coverage", "tmp", "temp", "bower_components", "Carthage", "Applications", "Movies", "Music",
    "Pictures", "Public", "Downloads", "site-packages", "logs", "Trash", "Archivio", "Backup", "Backups", "iCloud Photos", "Photos"];
/// macOS packages (they look like folders): not opened.
const PACKAGES: [&str; 20] = ["app", "bundle", "framework", "photoslibrary", "musiclibrary", "tvlibrary", "fcpbundle", "logicx", "band",
    "dSYM", "xcassets", "lproj", "xcarchive", "imovielibrary", "rtfd", "pages", "numbers", "key", "sparsebundle", "plugin"];
/// The file that makes a folder a project.
const MARKERS: [&str; 13] = [".git", "package.json", "Cargo.toml", "pyproject.toml", "composer.json", "CLAUDE.md", "AGENTS.md", "README.md",
    "README", "readme.md", "README.txt", "Package.swift", "go.mod"];

const SF_DATALESS: u32 = 0x4000_0000;

/// The file or folder is only in the cloud (iCloud, Dropbox online): opening it would download it. lstat does not download.
fn is_dataless(m: &fs::Metadata) -> bool {
    #[cfg(target_os = "macos")]
    { use std::os::macos::fs::MetadataExt; m.st_flags() & SF_DATALESS != 0 }
    #[cfg(not(target_os = "macos"))]
    { let _ = m; false }
}
fn is_local(p: &Path) -> bool { fs::symlink_metadata(p).map(|m| !is_dataless(&m)).unwrap_or(false) }

fn mtime(m: &fs::Metadata) -> f64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn hidden_or_skipped(name: &str) -> bool {
    if name.starts_with('.') || SKIP.contains(&name) { return true; }
    name.rsplit_once('.').map(|(_, e)| PACKAGES.iter().any(|x| x.eq_ignore_ascii_case(e))).unwrap_or(false)
}

/// "~/Developer/foo" instead of "<home>/Developer/foo"; iCloud with its own name.
fn short_path(p: &str, h: &Path) -> String {
    let h = h.to_string_lossy();
    let icloud = format!("{h}/Library/Mobile Documents/com~apple~CloudDocs");
    if let Some(r) = p.strip_prefix(&icloud) { return format!("iCloud Drive{r}"); }
    if let Some(r) = p.strip_prefix(&format!("{h}/Library/CloudStorage/")) { return r.to_string(); }
    if p == h { return "~".into(); }
    p.strip_prefix(&*h).map(|r| format!("~{r}")).unwrap_or_else(|| p.to_string())
}

fn in_cloud(p: &str) -> bool { p.contains("/Library/Mobile Documents/") || p.contains("/Library/CloudStorage/") || p.contains("/Dropbox") }

// ---------- find: projects on disk ----------

#[derive(Clone, Debug, Default)]
struct Project { cartella: String, tipi: Vec<String>, modificato: f64 }

/// What the content of a folder says: the markers found (from the listing, without opening the files).
fn markers(names: &[String]) -> Vec<String> {
    let mut t = vec![];
    let has = |n: &str| names.iter().any(|x| x == n);
    if names.iter().any(|x| x.ends_with(".xcodeproj") || x.ends_with(".xcworkspace")) || has("Package.swift") { t.push("xcode"); }
    if has("Cargo.toml") { t.push("rust"); }
    if has("package.json") { t.push("node"); }
    if has("pyproject.toml") { t.push("python"); }
    if has("composer.json") { t.push("php"); }
    if has("go.mod") { t.push("go"); }
    if has(".git") { t.push("git"); }
    if has("CLAUDE.md") || has("AGENTS.md") { t.push("claude"); }
    if t.is_empty() && MARKERS.iter().any(|m| has(m)) { t.push("docs"); }
    t.into_iter().map(String::from).collect()
}

/// Last change of a project: the folder, its top-level entries (lstat, no download) and the git history.
fn last_modified(dir: &Path) -> f64 {
    let mut t = fs::symlink_metadata(dir).map(|m| mtime(&m)).unwrap_or(0.0);
    for p in [".git/logs/HEAD", ".git/index", ".git/HEAD"] {
        if let Ok(m) = fs::symlink_metadata(dir.join(p)) { t = t.max(mtime(&m)); }
    }
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten().take(200) {
            let n = e.file_name().to_string_lossy().to_string();
            if n == ".DS_Store" || n == "node_modules" || n == ".git" { continue; }
            if let Ok(m) = e.metadata() { t = t.max(mtime(&m)); }
        }
    }
    t
}

struct Walk<'a> { scadenza: Instant, viste: usize, nuvola_saltate: usize, trovati: Vec<Project>, gia: HashSet<String>, avviso: &'a mut dyn FnMut(Value), ultimo: Instant, h: PathBuf, escludi: Vec<String> }

impl Walk<'_> {
    fn notify(&mut self, dir: &Path, force: bool) {
        if !force && self.ultimo.elapsed() < Duration::from_millis(90) { return; }
        self.ultimo = Instant::now();
        let v = json!({"fase": "disco", "cartella": short_path(&dir.to_string_lossy(), &self.h), "viste": self.viste, "trovati": self.trovati.len()});
        (self.avviso)(v);
    }
}

/// Breadth-first visit (upper levels first): when time runs out the most visible projects remain.
/// `max`: max depth below the root. The root is never a project.
fn visit(root: &Path, max: usize, g: &mut Walk, deadline: Instant) {
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = [(root.to_path_buf(), 0)].into();
    while let Some((dir, depth)) = queue.pop_front() {
        if Instant::now() > deadline || Instant::now() > g.scadenza { return; }
        let ds = dir.to_string_lossy().to_string();
        if g.escludi.iter().any(|x| ds == *x || ds.starts_with(&format!("{x}/"))) { continue; }
        // the folder is only in the cloud: listing it would download it
        match fs::symlink_metadata(&dir) { Ok(m) if is_dataless(&m) => { g.nuvola_saltate += 1; continue; } Ok(m) if !m.is_dir() => continue, Err(_) => continue, _ => {} }
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        g.viste += 1;
        g.notify(&dir, false);
        let mut names = vec![]; let mut subdirs = vec![];
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let ftype = e.file_type().ok();
            names.push(n.clone());
            if ftype.map(|t| t.is_dir()).unwrap_or(false) && !hidden_or_skipped(&n) { subdirs.push(e.path()); }
        }
        if depth > 0 {
            let t = markers(&names);
            if !t.is_empty() {
                let docs_only = t.len() == 1 && t[0] == "docs";
                if g.gia.insert(ds.clone()) { g.trovati.push(Project { cartella: ds, tipi: t, modificato: last_modified(&dir) }); }
                // do not go down inside a project (sub-packages are part of the project); a folder with only a
                // README can instead contain other projects
                if !docs_only { continue; }
            }
        }
        if depth >= max { continue; }
        subdirs.sort();
        for p in subdirs { queue.push_back((p, depth + 1)); }
    }
}

/// The starting folders that exist: first the project ones, then Desktop, Documents, iCloud, Dropbox.
/// Folders added by hand by the user (impostazioni.json "importa_radici"): the ones that still exist.
/// Test: on the Mac, with importa_radici empty, the roots are exactly the same as before (no extra folder).
#[cfg(all(test, target_os = "macos"))]
mod roots_tests {
    #[test]
    fn mac_without_added_roots_unchanged() {
        let h = std::path::Path::new("/home/prova-inesistente");
        let r = super::roots(h);
        assert!(r.is_empty(), "nessuna cartella esiste: {r:?}");
        assert!(super::added_roots().is_empty() || std::env::var("JUGGL_BASE").is_ok());
    }
}

fn added_roots() -> Vec<PathBuf> {
    crate::load_json(&base_dir().join("impostazioni.json")).and_then(|v| v.get("importa_radici").cloned())
        .and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().filter_map(|x| x.as_str()).map(PathBuf::from).filter(|p| p.is_dir()).collect()
}

/// "Add a folder" in the import: the app has no folder picker, the search runs in the usual folders;
/// this adds a folder chosen by the user (by typing the path, also "~/lavoro"), then the search is redone.
#[tauri::command(async)]
pub fn importa_aggiungi_radice(percorso: String) -> Value {
    let t = percorso.trim();
    let p = if let Some(r) = t.strip_prefix("~/") { home().join(r) } else if t == "~" { home() } else { PathBuf::from(t) };
    let Ok(c) = fs::canonicalize(&p) else { return json!({"ok": false, "msg": "Non trovo questa cartella"}) };
    if !c.is_dir() { return json!({"ok": false, "msg": "Non e' una cartella"}) }
    let text = c.to_string_lossy().to_string();
    if crate::documents::is_secret(&text) { return json!({"ok": false, "msg": "Questa cartella non si puo' aggiungere"}) }
    let r = crate::storage::modify(&base_dir().join("impostazioni.json"), Some(json!({})), |v| {
        let mut l: Vec<Value> = v.get("importa_radici").and_then(|x| x.as_array().cloned()).unwrap_or_default();
        if !l.iter().any(|x| x.as_str() == Some(text.as_str())) { l.push(json!(text)); }
        if l.len() > 10 { l.remove(0); }
        v["importa_radici"] = json!(l); true
    });
    match r { Ok(_) => json!({"ok": true, "cartella": text}), Err(e) => json!({"ok": false, "msg": e.to_string()}) }
}

fn roots(h: &Path) -> Vec<(PathBuf, usize, f64)> {
    let mut r: Vec<(PathBuf, usize, f64)> = vec![];
    for n in ["Developer", "Projects", "Progetti", "Code", "code", "dev", "Dev", "src", "repos", "GitHub", "Sites", "Lavori", "Work", "workspace"] {
        r.push((h.join(n), DEPTH, 1.0));
    }
    for n in ["Desktop", "Documents"] { r.push((h.join(n), DEPTH, 1.2)); }
    // Ubuntu/Windows in Italian (Nov 13, reported by a tester): Documenti and Scrivania; then the folders added by hand by the user
    #[cfg(not(target_os = "macos"))]   // on the Mac the search stays as before
    for n in ["Documenti", "Scrivania", "Progetti", "Sviluppo", "sviluppo", "lavoro"] { r.push((h.join(n), DEPTH, 1.2)); }
    for p in added_roots() { r.push((p, DEPTH + 1, 1.2)); }
    r.push((h.join("Library/Mobile Documents/com~apple~CloudDocs"), DEPTH, 1.0));
    let dropbox = h.join("Dropbox");
    if dropbox.exists() { r.push((dropbox, DEPTH, 0.8)); }
    else if let Ok(rd) = fs::read_dir(h.join("Library/CloudStorage")) {
        for e in rd.flatten() { if e.file_name().to_string_lossy().starts_with("Dropbox") { r.push((e.path(), DEPTH, 0.8)); } }
    }
    // the home: only the first-level folders (~/my-project), never deeper (on Linux and Windows two levels: ~/lavoro/my-project)
    r.push((h.to_path_buf(), if cfg!(target_os = "macos") { 1 } else { 2 }, 0.4));
    // same folder seen twice (alias, case on a case-insensitive disk): only one
    // ~/Dropbox is often a link: the real folder is visited
    let mut seen = HashSet::new();
    r.into_iter().filter(|(p, _, _)| p.is_dir()).filter_map(|(p, m, w)| { let c = fs::canonicalize(&p).unwrap_or(p); seen.insert(c.clone()).then_some((c, m, w)) }).collect()
}

// ---------- find: Claude Code chats ----------

#[derive(Clone, Debug, Default)]
struct Chat { id: String, percorso: String, cwd: String, t: f64, titolo: String, primo: String, ultima: String, in_juggl: bool }

fn text_of(c: &Value) -> String {
    match c {
        Value::String(t) => t.clone(),
        Value::Array(a) => a.iter().filter(|x| str_of(x, "type") == "text").map(|x| str_of(x, "text")).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}
fn short(t: &str, n: usize) -> String {
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= n { t } else { format!("{}…", t.chars().take(n).collect::<String>().trim_end()) }
}
/// A message really written by a person (not a system block, a command or the summary of a compaction).
fn by_person(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && !t.starts_with('<') && !t.starts_with("This session is being continued") && !t.starts_with("Caveat:") && !t.starts_with("[Request interrupted")
}

/// Reads a chat without opening all of it: the start (folder, first message) and the end (title, last reply).
fn read_chat(p: &Path, state_dir: &Path) -> Option<Chat> {
    let id = p.file_stem()?.to_string_lossy().to_string();
    if !storage::is_valid_id(&id) { return None; }
    let m = fs::metadata(p).ok()?;
    let mut c = Chat { id: id.clone(), percorso: p.to_string_lossy().into(), t: mtime(&m), ..Default::default() };
    c.in_juggl = state_dir.join(format!("{id}.json")).exists();
    let (mut human_msgs, mut sdk, mut summary) = (0usize, false, String::new());
    let f = fs::File::open(p).ok()?;
    let mut r = BufReader::with_capacity(64 * 1024, f);
    let (mut bytes_read, mut line) = (0usize, String::new());
    while bytes_read < 1_000_000 {
        line.clear();
        let n = r.read_line(&mut line).unwrap_or(0);
        if n == 0 { break; }
        bytes_read += n;
        // huge lines (pasted images): only if they are messages, up to 3 MB
        let is_user = line.contains("\"type\":\"user\"") || line.contains("\"type\": \"user\"");
        if line.len() > 3_000_000 || (line.len() > 400_000 && !is_user) { continue; }
        if line.contains("\"entrypoint\":\"sdk") || line.contains("\"entrypoint\": \"sdk") { sdk = true; }
        // the folder without reading the whole line (if it has special characters it is parsed properly)
        // the real field (at the top of the line): small lines are parsed whole, because a nested "cwd" (output of a
        // hook, input of a tool) is not the chat folder
        if c.cwd.is_empty() && line.len() < 60_000 && line.contains("\"cwd\"") {
            if let Ok(d) = serde_json::from_str::<Value>(&line) { c.cwd = str_of(&d, "cwd").to_string(); }
        }
        if c.cwd.is_empty() && line.len() >= 60_000 {
            for k in ["\"cwd\":\"", "\"cwd\": \""] {
                if let Some(i) = line.find(k) {
                    let v = line[i + k.len()..].split('"').next().unwrap_or("");
                    if !v.contains('\\') { c.cwd = v.to_string(); }
                    else if let Ok(d) = serde_json::from_str::<Value>(&line) { c.cwd = str_of(&d, "cwd").to_string(); }
                    break;
                }
            }
        }
        if !is_user && !line.contains("\"summary\"") { continue; }
        let Ok(d) = serde_json::from_str::<Value>(&line) else { continue };
        if str_of(&d, "type") == "summary" && summary.is_empty() { summary = str_of(&d, "summary").to_string(); }
        if str_of(&d, "type") == "user" && !d.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false) && !d.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) {
            let t = d.get("message").map(|m| text_of(m.get("content").unwrap_or(&Value::Null))).unwrap_or_default();
            if by_person(&t) { human_msgs += 1; if c.primo.is_empty() { c.primo = t; } }
        }
        if !c.cwd.is_empty() && human_msgs >= 2 { break; }
    }
    // title and last reply from the end of the file
    let (lines, _) = crate::tail_file(p, 160_000);
    let (mut custom, mut ai) = (String::new(), String::new());
    // the end of the file counts for messages only if the start that was read did not reach that far
    let tail_is_new = (bytes_read as u64) < m.len().saturating_sub(160_000);
    let mut last_user = String::new();
    for r in lines.iter() {
        if r.len() > 400_000 { continue; }
        if tail_is_new && (r.contains("\"type\":\"user\"") || r.contains("\"type\": \"user\"")) {
            if let Ok(d) = serde_json::from_str::<Value>(r) {
                if !d.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false) && !d.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) {
                    let t = d.get("message").map(|m| text_of(m.get("content").unwrap_or(&Value::Null))).unwrap_or_default();
                    if by_person(&t) { human_msgs += 1; last_user = t; }
                    if c.cwd.is_empty() { c.cwd = str_of(&d, "cwd").to_string(); }
                }
            }
            continue;
        }
        if r.contains("\"customTitle\"") || r.contains("\"aiTitle\"") || r.contains("\"type\":\"summary\"") {
            if let Ok(d) = serde_json::from_str::<Value>(r) {
                if !str_of(&d, "customTitle").is_empty() { custom = str_of(&d, "customTitle").to_string(); }
                if !str_of(&d, "aiTitle").is_empty() { ai = str_of(&d, "aiTitle").to_string(); }
                if !str_of(&d, "summary").is_empty() { summary = str_of(&d, "summary").to_string(); }
            }
        } else if r.contains("assistant") && r.contains("\"text\"") && !r.contains("\"isSidechain\":true") && !r.contains("\"isSidechain\": true") {
            if let Ok(d) = serde_json::from_str::<Value>(r) {
                if str_of(&d, "type") != "assistant" { continue; }
                let t = d.get("message").map(|m| text_of(m.get("content").unwrap_or(&Value::Null))).unwrap_or_default();
                if !t.trim().is_empty() { c.ultima = t; }
            }
        }
    }
    if c.primo.is_empty() { c.primo = last_user; }
    // nothing written by a person, or a one-line automatic call (scripts, other apps): not a chat.
    // Sessions hosted by Juggl are "sdk" too: those are always kept, and long ones as well.
    if c.primo.is_empty() || (sdk && human_msgs < 2 && !c.in_juggl && m.len() < 300_000) { return None; }
    c.titolo = [custom, ai, summary].into_iter().find(|x| !x.trim().is_empty()).unwrap_or_else(|| short(&c.primo, 70));
    c.titolo = short(&c.titolo, 80);
    Some(c)
}

/// The chats that count: excluding Juggl's own (~/tavolo), those of temporary folders and of ~/.claude.
fn all_chats(h: &Path, b: &Path, notify: &mut dyn FnMut(Value)) -> Vec<Chat> {
    let state_dir = b.join("stato");
    let bs = b.to_string_lossy().to_string();
    let cl = h.join(".claude").to_string_lossy().to_string();
    let mut files = vec![];
    // Claude folders to skip without opening the files: Juggl's (internal calls, thousands) and the temporary ones
    let juggl_dir = crate::hosted::claude_project(&bs);
    let home_dir = crate::hosted::claude_project(&h.to_string_lossy());
    for d in fs::read_dir(h.join(".claude/projects")).into_iter().flatten().flatten() {
        if !d.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
        let n = d.file_name().to_string_lossy().to_string();
        if n == juggl_dir { continue; }
        if ["-private-tmp-", "-private-var-folders-", "-tmp-", "-var-folders-"].iter().any(|x| n.starts_with(x)) && !n.starts_with(&home_dir) { continue; }
        for e in fs::read_dir(d.path()).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("jsonl") { files.push(p); }
        }
    }
    let total = files.len();
    // in parallel (6 threads): each file is read only at the start and at the end
    let done = std::sync::atomic::AtomicUsize::new(0);
    let next_idx = std::sync::atomic::AtomicUsize::new(0);
    let read_chats: Mutex<Vec<Chat>> = Mutex::new(vec![]);
    std::thread::scope(|sc| {
        for _ in 0..6 {
            sc.spawn(|| loop {
                let i = next_idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(p) = files.get(i) else { break };
                if let Some(c) = read_chat(p, &state_dir) { read_chats.lock().unwrap_or_else(|e| e.into_inner()).push(c); }
                done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            });
        }
        let mut last = Instant::now();
        while done.load(std::sync::atomic::Ordering::Relaxed) < total {
            std::thread::sleep(Duration::from_millis(30));
            if last.elapsed() > Duration::from_millis(90) { last = Instant::now(); notify(json!({"fase": "chat", "lette": done.load(std::sync::atomic::Ordering::Relaxed), "totale": total})); }
        }
    });
    notify(json!({"fase": "chat", "lette": total, "totale": total}));
    let hs = h.to_string_lossy().to_string();
    read_chats.into_inner().unwrap_or_else(|e| e.into_inner()).into_iter().filter(|c| {
        // temporary ones out; the examined home always counts (tests run in a fake home inside a temporary folder)
        let temp = crate::hosted::is_temp_dir(&c.cwd) && c.cwd != hs && !c.cwd.starts_with(&format!("{hs}/"));
        let hidden = c.cwd.strip_prefix(&format!("{hs}/")).map(|r| r.split('/').any(|x| x.starts_with('.'))).unwrap_or(false);
        !(c.cwd.is_empty() || temp || hidden || c.cwd == bs || c.cwd.starts_with(&format!("{bs}/")) || c.cwd.starts_with(&format!("{cl}/")) || c.cwd == cl)
    }).collect()
}

// ---------- merge and propose ----------

#[derive(Clone, Debug, Default)]
struct Entry { cartella: String, nome: String, tipi: Vec<String>, modificato: f64, chat: Vec<Chat>, esiste: bool, senza_progetto: bool }

/// Last search: "crea" works on this one (the interface only sends the chosen folders).
static LAST_SCAN: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// Resolved path (links, /var -> /private/var) even if the folder no longer exists: the first existing ancestor
/// is resolved and the rest is reattached.
fn resolve(p: &str) -> String {
    let mut base = PathBuf::from(p);
    let mut rest: Vec<std::ffi::OsString> = vec![];
    loop {
        if let Ok(c) = fs::canonicalize(&base) {
            let mut c = c;
            for r in rest.iter().rev() { c.push(r); }
            return c.to_string_lossy().to_string();
        }
        match (base.file_name().map(|x| x.to_os_string()), base.parent().map(|x| x.to_path_buf())) {
            (Some(n), Some(par)) => { rest.push(n); base = par; }
            _ => return p.to_string(),
        }
    }
}

/// Name to show: the folder; if it has a generic name ("sorgente", "app", "remotion") also the one above.
fn folder_name(p: &str) -> String {
    const GENERIC: [&str; 22] = ["sorgente", "sorgenti", "src", "source", "app", "proj", "project", "progetto", "remotion", "code", "codice", "web", "www",
        "public_html", "frontend", "backend", "sito", "site", "repo", "main", "v2", "nuovo"];
    let parts: Vec<&str> = p.trim_end_matches('/').rsplit('/').filter(|x| !x.is_empty()).collect();
    let Some(last) = parts.first() else { return p.to_string() };
    if !GENERIC.contains(&last.to_lowercase().as_str()) { return last.to_string(); }
    // the folder above, if it is not generic too; never a starting folder or the home (~/Developer/app stays "app")
    const ROOT_NAMES: [&str; 16] = ["Developer", "Projects", "Progetti", "Code", "code", "dev", "Sites", "Documents", "Desktop", "com~apple~CloudDocs",
        "Dropbox", "repos", "GitHub", "workspace", "Lavori", "Work"];
    match parts.iter().enumerate().skip(1).take(3).find(|(_, x)| !GENERIC.contains(&x.to_lowercase().as_str())) {
        Some((i, above)) if !ROOT_NAMES.contains(above) && parts.get(i + 1) != Some(&"Users") => format!("{above} {last}"),
        _ => last.to_string(),
    }
}

/// The folders that are never a project: the home, the root, the starting folders.
fn is_generic(p: &str, h: &Path) -> bool {
    let hs = h.to_string_lossy();
    p == "/" || p == hs || ["Desktop", "Documents", "Downloads", "Developer", "Projects", "Progetti", "Code", "Sites", "Dropbox", "Library/Mobile Documents/com~apple~CloudDocs"]
        .iter().any(|x| p == format!("{hs}/{x}"))
}

fn merge(h: &Path, projects: Vec<Project>, chats: Vec<Chat>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = projects.into_iter().map(|p| Entry { nome: folder_name(&p.cartella), cartella: p.cartella, tipi: p.tipi, modificato: p.modificato, esiste: true, ..Default::default() }).collect();
    let hs = h.to_string_lossy().to_string();
    // chat folders that are not among the found projects: look at them one by one (if they exist and are already on the Mac)
    // same folder written in two ways (links like ~/Dropbox, /var and /private/var): the resolved path
    let mut resolved: HashMap<String, String> = HashMap::new();
    let mut per_cwd: HashMap<String, Vec<Chat>> = HashMap::new();
    for c in chats {
        let k = resolved.entry(c.cwd.clone()).or_insert_with(|| resolve(&c.cwd)).clone();
        per_cwd.entry(k).or_default().push(c);
    }
    let mut cwds: Vec<String> = per_cwd.keys().cloned().collect();
    cwds.sort_by_key(|c| c.len());
    for cwd in cwds {
        let list = per_cwd.remove(&cwd).unwrap_or_default();
        // inside a project already in the list (also one of its subfolders): they go there
        if let Some(v) = entries.iter_mut().filter(|v| !v.senza_progetto && (v.cartella == cwd || cwd.starts_with(&format!("{}/", v.cartella)))).max_by_key(|v| v.cartella.len()) {
            v.chat.extend(list); continue;
        }
        if cwd == "/" || cwd == hs || is_generic(&cwd, h) {
            // chats opened in the home or in a generic folder: a separate group, chats only (no project)
            match entries.iter_mut().find(|v| v.senza_progetto) {
                Some(v) => v.chat.extend(list),
                None => entries.push(Entry { cartella: hs.clone(), nome: "Chat senza progetto".into(), senza_progetto: true, esiste: true, chat: list, ..Default::default() }),
            }
            continue;
        }
        let p = Path::new(&cwd);
        let exists = p.is_dir();
        let mut kinds = vec![];
        let mut modified = 0.0;
        if exists && is_local(p) {
            if let Ok(rd) = fs::read_dir(p) { kinds = markers(&rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect::<Vec<_>>()); }
            modified = last_modified(p);
        }
        entries.push(Entry { cartella: cwd.clone(), nome: folder_name(&cwd), tipi: kinds, modificato: modified, chat: list, esiste: exists, ..Default::default() });
    }
    for v in entries.iter_mut() {
        v.chat.sort_by(|a, b| b.t.partial_cmp(&a.t).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(c) = v.chat.first() { v.modificato = v.modificato.max(c.t); }
    }
    entries.sort_by(|a, b| b.modificato.partial_cmp(&a.modificato).unwrap_or(std::cmp::Ordering::Equal));
    entries
}

/// The folders Juggl already knows: registered projects and sessions (also hibernated and archived).
fn known(b: &Path) -> (HashSet<String>, HashSet<String>) {
    let reg: HashSet<String> = load_json(&b.join("progetti.json")).and_then(|v| v.get("progetti").cloned()).and_then(|v| v.as_array().cloned()).unwrap_or_default()
        .iter().map(|p| crate::session_care::expand_path(str_of(p, "cartella")).trim_end_matches('/').to_string()).filter(|x| !x.is_empty()).collect();
    let mut ready = HashSet::new();
    for e in fs::read_dir(b.join("stato")).into_iter().flatten().flatten() {
        let Some(v) = load_json(&e.path()) else { continue };
        if v.get("pronta").and_then(|x| x.as_bool()).unwrap_or(false) { ready.insert(str_of(&v, "cwd").to_string()); }
    }
    (reg, ready)
}

fn main_kind(t: &[String]) -> &'static str {
    for k in ["xcode", "rust", "node", "python", "php", "go", "git", "claude", "docs"] { if t.iter().any(|x| x == k) { return k; } }
    "cartella"
}

fn to_json(v: &Entry, h: &Path, b: &Path, reg: &HashSet<String>, ready: &HashSet<String>, now: f64) -> Value {
    let new_count = v.chat.iter().filter(|c| !c.in_juggl).count();
    let already = (reg.contains(&v.cartella) || ready.contains(&v.cartella)) && new_count == 0;
    let active = now - v.modificato < ACTIVE_DAYS * 86400.0;
    let kind = if v.senza_progetto { "chat" } else { main_kind(&v.tipi) };
    let logo = if v.esiste && !v.senza_progetto { find_logo(Path::new(&v.cartella)).unwrap_or_default() } else { String::new() };
    let _ = b;
    json!({
        "cartella": v.cartella, "nome": v.nome, "breve": short_path(&v.cartella, h), "tipo": kind, "tipi": v.tipi, "modificato": v.modificato,
        "chat_n": v.chat.len(), "chat_nuove": new_count, "chat_ultima": v.chat.first().map(|c| c.titolo.clone()).unwrap_or_default(),
        "esiste": v.esiste, "nuvola": in_cloud(&v.cartella), "gia": already, "senza_progetto": v.senza_progetto, "logo": logo,
        "scelta": active && !already && v.esiste && (!v.senza_progetto || new_count > 0),
    })
}

/// Result of a scan: the entries (folders with project and/or chats) and the measures.
struct ScanResult { voci: Vec<Entry>, h: PathBuf, n_chat: usize, viste: usize, saltate: usize, scaduto: bool, t_chat: u64, t_disco: u64, t0: Instant }

/// Reads Claude Code chats and folders (reads only) and merges them into entries. Base of `search_in` and `derive_in`.
fn scan(h: &Path, b: &Path, notify: &mut dyn FnMut(Value)) -> ScanResult {
    let t0 = Instant::now();
    let chats = all_chats(h, b, notify);
    // from here on the resolved home (like the starting folders and the chat folders)
    let hr = fs::canonicalize(h).unwrap_or_else(|_| h.to_path_buf());
    let h = hr.as_path();
    let t_chat = t0.elapsed().as_millis() as u64;
    let t1 = Instant::now();
    let exclude = vec![b.to_string_lossy().to_string(), h.join(".claude").to_string_lossy().to_string()];
    let mut g = Walk { scadenza: t1 + DISK_TIME, viste: 0, nuvola_saltate: 0, trovati: vec![], gia: HashSet::new(), avviso: notify, ultimo: Instant::now(), h: h.to_path_buf(), escludi: exclude };
    let rr = roots(h);
    let weights: f64 = rr.iter().map(|x| x.2).sum::<f64>().max(1.0);
    for (r, max, weight) in rr {
        // each starting folder has its share of time, plus what is left over from the previous ones
        let left = g.scadenza.saturating_duration_since(Instant::now());
        let share = DISK_TIME.mul_f64(weight / weights).max(Duration::from_millis(250));
        visit(&r, max, &mut g, Instant::now() + share.max(left / 3).min(left));
    }
    let timed_out = Instant::now() > g.scadenza;
    g.notify(h, true);
    let (seen, skipped, found) = (g.viste, g.nuvola_saltate, g.trovati);
    let t_disk = t1.elapsed().as_millis() as u64;
    let n_chat = chats.len();
    let entries = merge(h, found, chats);
    ScanResult { voci: entries, h: h.to_path_buf(), n_chat, viste: seen, saltate: skipped, scaduto: timed_out, t_chat, t_disco: t_disk, t0 }
}

/// Find: Claude Code chats + projects on disk. `notify` receives the progress. Reads only.
pub fn search_in(h: &Path, b: &Path, notify: &mut dyn FnMut(Value)) -> Value {
    let ScanResult { voci: entries, h, n_chat, viste: seen, saltate: skipped, scaduto: timed_out, t_chat, t_disco: t_disk, t0 } = scan(h, b, notify);
    let h = h.as_path();
    let (reg, ready) = known(b);
    let now = unix_now();
    // guided proposal: few visible entries (hard limit), the rest under "Mostra altre" (also capped)
    let (entries, main_flags, omitted) = proposal(entries, h, now);
    let out: Vec<Value> = entries.iter().zip(main_flags.iter()).map(|(v, p)| {
        let mut j = to_json(v, h, b, &reg, &ready, now);
        j["principale"] = json!(*p);
        j["gruppo"] = json!(group_of(v, now));
        if !*p { j["scelta"] = json!(false); }
        j
    }).collect();
    *LAST_SCAN.lock().unwrap_or_else(|e| e.into_inner()) = entries;
    json!({"voci": out, "chat": n_chat, "cartelle_viste": seen, "nuvola_saltate": skipped, "tempo_scaduto": timed_out, "omesse": omitted,
        "ms_chat": t_chat, "ms_disco": t_disk, "ms": t0.elapsed().as_millis() as u64})
}

#[tauri::command(async)]
pub fn importa_cerca(app: tauri::AppHandle) -> Value {
    let mut notify = |v: Value| { let _ = app.emit("importa_avanzamento", v); };
    search_in(&home(), &base_dir(), &mut notify)
}

// ---------- project context ----------

fn read_small(p: &Path, max: usize) -> Option<String> {
    let ps = p.to_string_lossy();
    if crate::documents::is_secret(&ps) { return None; }
    let m = fs::symlink_metadata(p).ok()?;
    if !m.is_file() || is_dataless(&m) || m.len() > 2_000_000 { return None; }
    let mut buf = vec![];
    fs::File::open(p).ok()?.take(max as u64).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).to_string())
}

/// Summary of a markdown text without models: title, first paragraphs and section titles.
fn md_summary(t: &str, max: usize) -> String {
    let mut out = String::new();
    let mut sections = vec![];
    let mut in_code = false;
    for r in t.lines() {
        let r = r.trim_end();
        if r.trim_start().starts_with("```") { in_code = !in_code; continue; }
        if in_code || r.trim().is_empty() || r.trim_start().starts_with("<") || r.trim_start().starts_with("![") || r.trim_start().starts_with("[![") { continue; }
        if let Some(h) = r.strip_prefix("## ").or_else(|| r.strip_prefix("### ")) { if sections.len() < 12 { sections.push(h.trim().to_string()); } continue; }
        if r.starts_with("# ") { if out.is_empty() { out.push_str(r.trim_start_matches("# ").trim()); out.push_str(". "); } continue; }
        if out.chars().count() < max { out.push_str(r.trim()); out.push(' '); }
    }
    let mut o = short(&out, max);
    if !sections.is_empty() { o.push_str(&format!("\nSezioni: {}.", sections.join(", "))); }
    o
}

/// Libraries and tools recognizable from the project files (only the names, never the configuration values).
fn stack(dir: &Path, kinds: &[String]) -> Vec<String> {
    let mut st: Vec<String> = vec![];
    fn add_unique(st: &mut Vec<String>, x: &str) { if !st.iter().any(|y| y == x) { st.push(x.to_string()); } }
    macro_rules! push_unique { ($x:expr) => { add_unique(&mut st, $x) }; }
    if let Some(t) = read_small(&dir.join("package.json"), 200_000) {
        if let Ok(v) = serde_json::from_str::<Value>(&t) {
            let mut deps: Vec<String> = vec![];
            for k in ["dependencies", "devDependencies"] { if let Some(o) = v.get(k).and_then(|x| x.as_object()) { deps.extend(o.keys().cloned()); } }
            for (k, n) in [("next", "Next.js"), ("react", "React"), ("vue", "Vue"), ("nuxt", "Nuxt"), ("svelte", "Svelte"), ("@sveltejs/kit", "SvelteKit"), ("astro", "Astro"),
                ("@angular/core", "Angular"), ("expo", "Expo"), ("react-native", "React Native"), ("electron", "Electron"), ("@tauri-apps/api", "Tauri"),
                ("express", "Express"), ("@nestjs/core", "NestJS"), ("fastify", "Fastify"), ("hono", "Hono"), ("prisma", "Prisma"), ("@prisma/client", "Prisma"),
                ("drizzle-orm", "Drizzle"), ("tailwindcss", "Tailwind"), ("typescript", "TypeScript"), ("vite", "Vite"), ("firebase", "Firebase"),
                ("@supabase/supabase-js", "Supabase"), ("stripe", "Stripe"), ("three", "Three.js"), ("remotion", "Remotion"), ("@shopify/hydrogen", "Shopify Hydrogen")] {
                if deps.iter().any(|d| d == k) { push_unique!(n); }
            }
            if st.is_empty() { push_unique!("Node.js"); }
        }
    }
    if let Some(t) = read_small(&dir.join("Cargo.toml"), 100_000) {
        push_unique!("Rust");
        for (k, n) in [("tauri", "Tauri"), ("tokio", "Tokio"), ("axum", "Axum"), ("actix-web", "Actix"), ("bevy", "Bevy")] { if t.contains(k) { push_unique!(n); } }
    }
    let py = [read_small(&dir.join("pyproject.toml"), 100_000), read_small(&dir.join("requirements.txt"), 100_000)].into_iter().flatten().collect::<Vec<_>>().join("\n").to_lowercase();
    if !py.is_empty() {
        push_unique!("Python");
        for (k, n) in [("django", "Django"), ("flask", "Flask"), ("fastapi", "FastAPI"), ("torch", "PyTorch"), ("pandas", "pandas"), ("streamlit", "Streamlit")] { if py.contains(k) { push_unique!(n); } }
    }
    if let Some(t) = read_small(&dir.join("composer.json"), 100_000) {
        push_unique!("PHP");
        if t.contains("laravel/framework") { push_unique!("Laravel"); }
        if t.contains("symfony/") { push_unique!("Symfony"); }
    }
    if kinds.iter().any(|x| x == "xcode") { push_unique!("Swift / Xcode"); }
    if kinds.iter().any(|x| x == "go") { push_unique!("Go"); }
    if dir.join("wp-config.php").exists() || dir.join("wp-content").is_dir() { push_unique!("WordPress"); }
    if dir.join("Dockerfile").exists() || dir.join("docker-compose.yml").exists() || dir.join("compose.yaml").exists() { push_unique!("Docker"); }
    st
}

/// Logo or icon of the project, if there is an image in the usual places (already on the Mac, not too large).
pub(crate) fn find_logo(dir: &Path) -> Option<String> {   // list (Nov 15): also used by logos.rs
    for r in ["public/apple-touch-icon.png", "app/apple-icon.png", "src/app/apple-icon.png", "public/logo.png", "public/icon.png", "app/icon.png", "src/app/icon.png",
        "public/favicon.png", "public/android-chrome-512x512.png", "public/android-chrome-192x192.png", "public/icon-512.png", "public/icons/icon-512x512.png",
        "assets/icon.png", "assets/logo.png", "assets/images/icon.png", "src-tauri/icons/128x128@2x.png", "src-tauri/icons/icon.png", "static/logo.png",
        "static/favicon.png", "resources/icon.png", "logo.png", "icon.png", "docs/logo.png"] {
        let p = dir.join(r);
        if let Ok(m) = fs::symlink_metadata(&p) { if m.is_file() && !is_dataless(&m) && m.len() > 200 && m.len() < 3_000_000 { return Some(p.to_string_lossy().into()); } }
    }
    None
}

/// System git only if the Apple tools are there: otherwise /usr/bin/git opens the install window.
fn git_bin() -> Option<PathBuf> {
    for p in ["/opt/homebrew/bin/git", "/usr/local/bin/git"] { if Path::new(p).is_file() { return Some(p.into()); } }
    let ok = crate::new_command("/usr/bin/xcode-select").arg("-p").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    ok.then(|| PathBuf::from("/usr/bin/git"))
}

fn run_short(cmd: &mut Command, max: Duration) -> Option<String> {
    let mut f = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout_pipe = f.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let mut t = String::new(); let _ = stdout_pipe.read_to_string(&mut t); let _ = tx.send(t); });
    let start = Instant::now();
    loop {
        match f.try_wait() {
            Ok(Some(st)) => { let t = rx.recv_timeout(Duration::from_millis(500)).unwrap_or_default(); return st.success().then_some(t); }
            Ok(None) if start.elapsed() < max => std::thread::sleep(Duration::from_millis(20)),
            _ => { let _ = f.kill(); let _ = f.wait(); return None; }
        }
    }
}

fn last_commits(dir: &Path) -> Vec<String> {
    if in_cloud(&dir.to_string_lossy()) || !is_local(&dir.join(".git")) { return vec![]; }
    let Some(git) = git_bin() else { return vec![] };
    let mut c = crate::new_command(git);
    crate::NoConsoleWindow::no_console_window(&mut c);
    c.args(["-c", "core.fsmonitor=false", "-C"]).arg(dir).args(["log", "-10", "--no-color", "--format=%h %ad %s", "--date=format:%d/%m/%y"]).env("GIT_OPTIONAL_LOCKS", "0");
    run_short(&mut c, Duration::from_secs(3)).map(|t| t.lines().map(|r| short(r, 110)).collect()).unwrap_or_default()
}

/// Recently changed files (only dates and names, with lstat: no download, no secrets, no dependencies).
fn recent_files(dir: &Path, count: usize) -> Vec<(f64, String)> {
    let mut all: Vec<(f64, String)> = vec![];
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = [(dir.to_path_buf(), 0)].into();
    let mut seen = 0;
    while let Some((d, depth)) = queue.pop_front() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 5000 { break; }
            let n = e.file_name().to_string_lossy().to_string();
            if hidden_or_skipped(&n) || n == ".DS_Store" { continue; }
            let Ok(m) = e.metadata() else { continue };
            if is_dataless(&m) { continue; }
            let p = e.path();
            if m.is_dir() { if depth < 3 { queue.push_back((p, depth + 1)); } continue; }
            let ps = p.to_string_lossy().to_string();
            if crate::documents::is_secret(&ps) || n.ends_with(".lock") || n == "package-lock.json" || n == "pnpm-lock.yaml" || n == "yarn.lock" || n.ends_with(".log") { continue; }
            all.push((mtime(&m), ps.strip_prefix(&format!("{}/", dir.to_string_lossy())).unwrap_or(&ps).to_string()));
        }
    }
    all.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    all.truncate(count);
    all
}

/// Day/month/year in local time.
fn date_str(t: f64) -> String {
    if t <= 0.0 { return String::new(); }
    let secs = t as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() { return String::new(); }
    format!("{:02}/{:02}/{:02}", tm.tm_mday, tm.tm_mon + 1, (tm.tm_year + 1900) % 100)
}

/// The starting context of a ready session: what the project is, what is inside, what was done recently.
fn context(v: &Entry) -> String {
    let dir = Path::new(&v.cartella);
    let mut t = format!("# Contesto del progetto {}\n\nQuesto contesto l'ha preparato Juggl il {} quando il progetto e' stato importato. E' una fotografia di quel momento: per lo stato attuale guarda i file. Usalo per orientarti senza rileggere tutto da capo, e non citarlo se non serve.\n\n",
        v.nome, date_str(unix_now()));
    t.push_str(&format!("- Cartella: {}\n", v.cartella));
    let st = if v.esiste { stack(dir, &v.tipi) } else { vec![] };
    if !st.is_empty() { t.push_str(&format!("- Stack: {}\n", st.join(", "))); }
    if v.tipi.iter().any(|x| x == "git") { t.push_str("- Repository git\n"); }
    if in_cloud(&v.cartella) { t.push_str("- La cartella sta in iCloud o Dropbox: alcuni file potrebbero non essere scaricati, non forzarne il download senza chiedere.\n"); }
    if v.esiste {
        for n in ["README.md", "README", "readme.md", "README.txt"] {
            if let Some(r) = read_small(&dir.join(n), 60_000) { let r = md_summary(&r, 700); if !r.trim().is_empty() { t.push_str(&format!("\n## README in breve\n{r}\n")); } break; }
        }
        if let Some(r) = read_small(&dir.join("CLAUDE.md"), 60_000) {
            t.push_str(&format!("\n## CLAUDE.md\nC'e' un CLAUDE.md: Claude Code lo legge da solo. In breve: {}\n", md_summary(&r, 350)));
        }
        if let Some(r) = read_small(&dir.join("AGENTS.md"), 60_000) { t.push_str(&format!("\n## AGENTS.md in breve\n{}\n", md_summary(&r, 600))); }
        let c = last_commits(dir);
        if !c.is_empty() { t.push_str(&format!("\n## Ultimi commit\n{}\n", c.iter().map(|x| format!("- {x}")).collect::<Vec<_>>().join("\n"))); }
        let f = recent_files(dir, 12);
        if !f.is_empty() { t.push_str(&format!("\n## File cambiati di recente\n{}\n", f.iter().map(|(m, p)| format!("- {} {p}", date_str(*m))).collect::<Vec<_>>().join("\n"))); }
    }
    if !v.chat.is_empty() {
        t.push_str(&format!("\n## Chat precedenti su questo progetto ({} in tutto, le ultime qui sotto)\nSono conversazioni gia' fatte con Claude Code in questa cartella. Si riprendono da Archiviate in Juggl.\n", v.chat.len()));
        for c in v.chat.iter().take(5) {
            t.push_str(&format!("- {} \"{}\". Chiesto: {}", date_str(c.t), c.titolo, short(&c.primo, 160)));
            if !c.ultima.is_empty() { t.push_str(&format!(" Ultima risposta: {}", short(&c.ultima, 260))); }
            t.push('\n');
        }
    }
    if t.chars().count() > MAX_CONTEXT { t = format!("{}…\n", t.chars().take(MAX_CONTEXT).collect::<String>()); }
    t
}

// ---------- create ----------

fn slug(n: &str) -> String {
    let mut s: String = n.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while s.contains("--") { s = s.replace("--", "-"); }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "progetto".into() } else { s }
}

fn with_tilde(p: &str, h: &Path) -> String {
    let hs = h.to_string_lossy();
    p.strip_prefix(&format!("{hs}/")).map(|r| format!("~/{r}")).unwrap_or_else(|| p.to_string())
}

/// Registers the project in progetti.json (name, folder, logo), if not there yet. Never a generic folder or one that
/// contains other registered ones: it would take the sessions of the other projects.
fn register_project(b: &Path, h: &Path, v: &Entry, logo: &str, auto: &str) -> bool {
    if v.senza_progetto || !v.esiste || is_generic(&v.cartella, h) { return false; }
    let mut done = false;
    let _ = storage::modify(&b.join("progetti.json"), Some(json!({"progetti": []})), |reg| {
        if !reg["progetti"].is_array() { reg["progetti"] = json!([]); }
        let list = reg["progetti"].as_array_mut().unwrap();
        // "~" expands with the examined home (not the process one): same thing in production, and tests with a fake home
        let hs = h.to_string_lossy().to_string();
        let folders: Vec<String> = list.iter().map(|p| { let c = str_of(p, "cartella"); if let Some(r) = c.strip_prefix("~/") { format!("{hs}/{r}") } else { crate::session_care::expand_path(c) } }.trim_end_matches('/').to_string()).collect();
        if folders.iter().any(|c| *c == v.cartella || c.starts_with(&format!("{}/", v.cartella))) { return false; }
        // automatic ones: never a folder inside an existing project
        if !auto.is_empty() && folders.iter().any(|c| v.cartella.starts_with(&format!("{c}/"))) { return false; }
        let base_id = slug(&v.nome);
        let mut id = base_id.clone();
        let mut k = 2;
        while list.iter().any(|p| str_of(p, "id") == id) { id = format!("{base_id}-{k}"); k += 1; }
        // words to recognize the project from a session name: only the distinctive ones
        const COMMON: [&str; 24] = ["sito", "site", "website", "progetto", "project", "prova", "test", "nuovo", "nuova", "vecchio", "script", "scripts",
            "frontend", "backend", "server", "client", "mobile", "desktop", "landing", "main", "master", "demo", "template", "lavori"];
        let words: Vec<String> = v.nome.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 5 && !COMMON.contains(w)).map(String::from).collect();
        let mut entry = json!({"id": id, "nome": v.nome, "cartella": with_tilde(&v.cartella, h), "parole": words, "importato_il": unix_now()});
        // "auto": how Juggl found it by itself ("git" or "claude"); by hand or from the guided import the field is absent
        if !auto.is_empty() { entry["auto"] = json!(auto); }
        if !logo.is_empty() { entry["logo"] = json!(with_tilde(logo, h)); }
        list.push(entry);
        done = true;
        true
    });
    done
}

/// Existing chats become archived: they show up in Archived and Resume reopens them with the whole conversation.
/// The ones Juggl already knows (state present) are not touched.
fn import_chats(b: &Path, v: &Entry) -> usize {
    let mut n = 0;
    for c in v.chat.iter().filter(|c| !c.in_juggl).take(CHATS_PER_PROJECT) {
        let p = b.join("stato").join(format!("{}.json", c.id));
        if p.exists() { continue; }
        let st = json!({"sessione": c.id, "stato": "archiviata", "archiviata_il": c.t, "motivo_archivio": "importata", "archiviata_da": "importata",
            "stato_prima": "ferma", "nome_archivio": c.titolo, "cwd": c.cwd, "cwd_origine": c.cwd, "transcript": c.percorso, "ospite": "tavolo",
            "agente": "claude", "da": c.t, "agg": unix_now(), "chiesto": short(&c.primo, 200), "risposta": short(&c.ultima, 400), "importata_il": unix_now()});
        if storage::write_json(&p, &st).is_ok() { n += 1; }
    }
    n
}

/// Creates the ready sessions for the chosen folders (from the last search). Starts nothing.
pub fn create_in(h: &Path, b: &Path, folders: &[String]) -> Value {
    let entries = LAST_SCAN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (_, ready) = known(b);
    let _ = fs::create_dir_all(b.join("stato"));
    let _ = fs::create_dir_all(b.join("contesti"));
    let (mut sessions, mut chats, mut projects, mut skipped) = (vec![], 0usize, 0usize, 0usize);
    for c in folders {
        let Some(v) = entries.iter().find(|v| v.cartella == *c) else { skipped += 1; continue };
        chats += import_chats(b, v);
        // chats only (home, vanished folder) or already ready and never opened: no new session
        if v.senza_progetto || !v.esiste || ready.contains(&v.cartella) { continue; }
        let logo = find_logo(Path::new(&v.cartella)).unwrap_or_default();
        if register_project(b, h, v, &logo, "") { projects += 1; }
        let id = crate::hosted::uuid();
        let fc = b.join("contesti").join(format!("{id}.md"));
        if storage::write(&fc, context(v).as_bytes()).is_err() { skipped += 1; continue; }
        let recent: Vec<&str> = v.chat.iter().take(5).map(|c| c.id.as_str()).collect();
        let st = json!({"sessione": id, "stato": "ibernata", "pronta": true, "motivo_ibernazione": "pronta", "ibernata_il": unix_now(),
            "da": if v.modificato > 0.0 { v.modificato } else { unix_now() }, "agg": unix_now(), "nome_manuale": v.nome, "cwd": v.cartella, "cwd_origine": v.cartella,
            "ospite": "tavolo", "agente": "claude", "contesto": fc.to_string_lossy(), "chat_precedenti": recent, "chat_n": v.chat.len(),
            "stack": stack(Path::new(&v.cartella), &v.tipi), "importata_il": unix_now()});
        if storage::write_json(&b.join("stato").join(format!("{id}.json")), &st).is_ok() { sessions.push(id); } else { skipped += 1; }
    }
    json!({"ok": true, "sessioni": sessions.len(), "ids": sessions, "chat": chats, "progetti": projects, "saltate": skipped})
}

// ---------- guided import proposal: few sensible folders ----------

/// Visible entries (checked by default). The rest goes under "Mostra altre".
const MAX_MAIN: usize = 12;
/// Entries under "Mostra altre": never a list of hundreds. The others are not proposed (but the ⌘K search finds the chats).
const MAX_OTHERS: usize = 60;

fn has_recent_chat(v: &Entry, now: f64) -> bool { v.chat.iter().any(|x| now - x.t < CHAT_AUTO_DAYS * 86400.0) }

/// Grouping: "chat" (recent chats with Claude), "repo" (active git repos), "progetto" (other active projects with a
/// project file), "altro" (folders with only a README, or idle for a long time).
fn group_of(v: &Entry, now: f64) -> &'static str {
    if v.senza_progetto || has_recent_chat(v, now) { return "chat"; }
    if v.tipi.iter().any(|t| t == "git") && now - v.modificato < REPO_AUTO_DAYS * 86400.0 { return "repo"; }
    // project with its own file (package.json, Cargo.toml...) touched in the last two months: visible, checked by default
    if v.tipi.iter().any(|t| t != "docs") && now - v.modificato < ACTIVE_DAYS * 86400.0 { return "progetto"; }
    "altro"
}

/// Picks and sorts the entries to propose. Returns (entries, whether they are "visible", how many were dropped).
/// Main: recent chats and active repos (at most MAX_MAIN, the most recent). The rest, without hidden
/// folders/dependencies/caches, under "Mostra altre" (at most MAX_OTHERS). The home chats stay a single entry.
fn proposal(entries: Vec<Entry>, h: &Path, now: f64) -> (Vec<Entry>, Vec<bool>, usize) {
    let total = entries.len();
    let (mut primary, mut others): (Vec<Entry>, Vec<Entry>) = (vec![], vec![]);
    for v in entries {
        if !v.senza_progetto && (path_to_skip(&v.cartella) || is_generic(&v.cartella, h)) { continue; }
        let visible = if v.senza_progetto { v.chat.iter().any(|c| !c.in_juggl) } else { v.esiste && matches!(group_of(&v, now), "chat" | "repo" | "progetto") };
        if visible { primary.push(v) } else { others.push(v) }
    }
    let by_date = |a: &Entry, b: &Entry| b.modificato.partial_cmp(&a.modificato).unwrap_or(std::cmp::Ordering::Equal);
    primary.sort_by(by_date); others.sort_by(by_date);
    // whoever does not fit among the main ones goes down to the others
    if primary.len() > MAX_MAIN { let rest = primary.split_off(MAX_MAIN); others.extend(rest); others.sort_by(by_date); }
    others.truncate(MAX_OTHERS);
    let shown = primary.len() + others.len();
    let flag: Vec<bool> = primary.iter().map(|_| true).chain(others.iter().map(|_| false)).collect();
    let mut all = primary; all.extend(others);
    (all, flag, total.saturating_sub(shown))
}

// ---------- projects derived automatically (first impression of a new user) ----------

/// Most recent chat within these days: the folder is a project in use.
const CHAT_AUTO_DAYS: f64 = 30.0;
/// Git repos: active in the last months (the ones idle for years are not "my projects").
const REPO_AUTO_DAYS: f64 = 180.0;
/// Max number of derived projects: never a list of hundreds.
const MAX_AUTO: usize = 40;

/// A folder that is never a project: inside hidden folders, dependencies, caches, system libraries, packages.
fn path_to_skip(p: &str) -> bool {
    p.split('/').filter(|x| !x.is_empty()).any(hidden_or_skipped)
}

/// Which entries become a project automatically and why ("git" or "claude"). Only real folders, with a name, never the home or the
/// starting folders, never cloud-only, never one inside another (the outermost stays). The most recent, at most MAX_AUTO.
fn pick_automatic<'a>(entries: &'a [Entry], h: &Path, now: f64) -> Vec<(&'a Entry, &'static str)> {
    let mut c: Vec<(&Entry, &'static str)> = entries.iter().filter(|v| v.esiste && !v.senza_progetto && !is_generic(&v.cartella, h)
        && !path_to_skip(&v.cartella) && !in_cloud(&v.cartella)).filter_map(|v| {
        let git = v.tipi.iter().any(|t| t == "git") && v.modificato > 0.0 && now - v.modificato < REPO_AUTO_DAYS * 86400.0;
        let chat = v.chat.iter().any(|x| now - x.t < CHAT_AUTO_DAYS * 86400.0);
        if git { Some((v, "git")) } else if chat { Some((v, "claude")) } else { None }
    }).collect();
    c.sort_by_key(|(v, _)| v.cartella.len());
    let mut accepted: Vec<(&Entry, &'static str)> = vec![];
    for (v, how) in c {
        if accepted.iter().any(|(a, _)| v.cartella.starts_with(&format!("{}/", a.cartella))) { continue; }
        accepted.push((v, how));
    }
    accepted.sort_by(|a, b| b.0.modificato.partial_cmp(&a.0.modificato).unwrap_or(std::cmp::Ordering::Equal));
    accepted.truncate(MAX_AUTO);
    accepted
}

/// Derives the projects from the user's folders and registers them in progetti.json with "auto". It adds, does not remove and does not
/// touch the existing ones. No sessions, no imported chats: only the project list. Returns how many were added.
pub fn derive_in(h: &Path, b: &Path) -> usize {
    let sc = scan(h, b, &mut |_| {});
    let now = unix_now();
    let mut n = 0;
    for (v, how) in pick_automatic(&sc.voci, &sc.h, now) {
        let logo = find_logo(Path::new(&v.cartella)).unwrap_or_default();
        if register_project(b, &sc.h, v, &logo, how) { n += 1; }
    }
    n
}

/// At startup: only for those with no project yet (new user) and only once. Those who already have some never change.
pub fn derive_at_startup() { derive_at_startup_in(&home(), &base_dir()); }

fn derive_at_startup_in(h: &Path, b: &Path) -> Option<usize> {
    let has_projects = load_json(&b.join("progetti.json")).and_then(|v| v.get("progetti").and_then(|p| p.as_array().map(|a| !a.is_empty()))).unwrap_or(false);
    let done = load_json(&b.join("impostazioni.json")).map(|v| v.get("progetti_auto_il").is_some()).unwrap_or(false);
    if has_projects || done { return None; }
    let n = derive_in(h, b);
    let _ = storage::modify(&b.join("impostazioni.json"), Some(json!({})), |v| { v["progetti_auto_il"] = json!(unix_now()); true });
    crate::log_juggl(&format!("progetti: ricavati {n} progetti dalle cartelle (utente nuovo)"));
    Some(n)
}

/// From the import button: derives the projects (adds only the missing ones).
#[tauri::command(async)]
pub fn importa_ricava(app: tauri::AppHandle) -> Value {
    let n = derive_in(&home(), &base_dir());
    let _ = app.emit("cambiato", ());
    json!({"ok": true, "progetti": n})
}

#[tauri::command(async)]
pub fn importa_crea(app: tauri::AppHandle, cartelle: Vec<String>) -> Value {
    let r = create_in(&home(), &base_dir(), &cartelle);
    crate::log_juggl(&format!("importa: {} sessioni pronte, {} chat archiviate, {} progetti", r["sessioni"], r["chat"], r["progetti"]));
    let _ = app.emit("cambiato", ());
    r
}

/// The prepared context of a ready session (to show it in the chat before the first message).
pub fn context_of(st: &Value) -> String {
    let p = str_of(st, "contesto");
    if p.is_empty() || !p.starts_with(&base_dir().join("contesti").to_string_lossy().to_string()) { return String::new(); }
    fs::read_to_string(p).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("juggl-importa-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }
    fn write_file(p: &Path, t: &str) { fs::create_dir_all(p.parent().unwrap()).unwrap(); fs::write(p, t).unwrap(); }

    #[test]
    fn finds_projects_and_skips_dependencies() {
        let h = test_dir("disco");
        write_file(&h.join("Developer/sito/package.json"), r#"{"dependencies":{"next":"15","react":"19"}}"#);
        write_file(&h.join("Developer/sito/node_modules/x/package.json"), "{}");
        write_file(&h.join("Developer/app/Cargo.toml"), "[dependencies]\ntauri = \"2\"");
        fs::create_dir_all(h.join("Developer/repo/.git")).unwrap();
        write_file(&h.join("Documents/a/b/c/d/e/package.json"), "{}");   // too deep
        write_file(&h.join("Documents/note/.nascosta/package.json"), "{}");
        let b = h.join("tavolo");
        let mut n = 0;
        let r = search_in(&h, &b, &mut |_| n += 1);
        let names: Vec<String> = r["voci"].as_array().unwrap().iter().map(|v| str_of(v, "nome").to_string()).collect();
        assert!(names.contains(&"sito".into()) && names.contains(&"app".into()) && names.contains(&"repo".into()), "{names:?}");
        assert!(!names.contains(&"x".into()) && !names.contains(&"e".into()) && !names.contains(&".nascosta".into()), "{names:?}");
        let site = r["voci"].as_array().unwrap().iter().find(|v| str_of(v, "nome") == "sito").unwrap();
        assert_eq!(str_of(site, "tipo"), "node");
        assert!(site["scelta"].as_bool().unwrap(), "attivo adesso = spuntato");
        let _ = fs::remove_dir_all(&h);
    }

    fn user_line(cwd: &str, t: &str, ep: &str) -> String {
        json!({"type": "user", "cwd": cwd, "entrypoint": ep, "message": {"role": "user", "content": t}}).to_string()
    }

    /// New user (fake home): git repos and folders with recent chats become projects automatically; the noise (a thousand
    /// folders, node_modules, Library, caches, hidden, home) does not; a folder inside a project is not another project.
    #[test]
    fn projects_derived_without_noise() {
        let h = test_dir("auto");
        for i in 0..8 { fs::create_dir_all(h.join(format!("Developer/repo{i}/.git"))).unwrap(); write_file(&h.join(format!("Developer/repo{i}/README.md")), "x"); }
        // sub-repo inside a repo: the outermost stays
        fs::create_dir_all(h.join("Developer/repo0/vendor-interno/.git")).unwrap();
        // folder without markers but with a recent chat
        let with_chat = h.join("Documents/clienti/lavoro-con-chat");
        fs::create_dir_all(&with_chat).unwrap();
        // folder without markers with an old chat (90 days): no longer a project in use
        let old_dir = h.join("Documents/clienti/lavoro-vecchio");
        fs::create_dir_all(&old_dir).unwrap();
        let pj = h.join(".claude/projects/-x-chat");
        let (c1, c2) = (with_chat.to_string_lossy().to_string(), old_dir.to_string_lossy().to_string());
        write_file(&pj.join("aaaaaaaa-1111-4111-8111-111111111111.jsonl"), &user_line(&c1, "organizza i file del cliente", "cli"));
        let old_file = pj.join("bbbbbbbb-2222-4222-8222-222222222222.jsonl");
        write_file(&old_file, &user_line(&c2, "una cosa di mesi fa", "cli"));
        let three_months = std::time::SystemTime::now() - Duration::from_secs(90 * 86400);
        fs::File::options().write(true).open(&old_file).unwrap().set_modified(three_months).unwrap();
        // noise: a thousand folders in places that are not looked at, dependencies, caches, libraries, hidden
        for i in 0..1000 { fs::create_dir_all(h.join(format!("Library/Caches/app{i}/.git"))).unwrap(); }
        for i in 0..50 { fs::create_dir_all(h.join(format!("Developer/repo1/node_modules/dip{i}/.git"))).unwrap(); }
        for i in 0..50 { fs::create_dir_all(h.join(format!(".cache/x{i}/.git"))).unwrap(); fs::create_dir_all(h.join(format!("Documents/.nascosta/y{i}/.git"))).unwrap(); }
        fs::create_dir_all(h.join(".git")).unwrap();   // the home itself is a repo (it happens)
        let b = h.join("tavolo");
        let n = derive_in(&h, &b);
        let reg = load_json(&b.join("progetti.json")).unwrap();
        let list = reg["progetti"].as_array().unwrap();
        let names: Vec<String> = list.iter().map(|p| str_of(p, "nome").to_string()).collect();
        assert_eq!(n, list.len());
        assert_eq!(names.iter().filter(|x| x.starts_with("repo")).count(), 8, "{names:?}");
        assert!(names.contains(&"lavoro-con-chat".to_string()), "{names:?}");
        assert!(!names.contains(&"lavoro-vecchio".to_string()), "chat vecchia: {names:?}");
        assert!(!names.contains(&"vendor-interno".to_string()), "dentro un repo: {names:?}");
        assert!(names.iter().all(|x| !x.starts_with("dip") && !x.starts_with("app") && !x.starts_with('x') && !x.starts_with('y')), "rumore: {names:?}");
        assert!(list.len() <= MAX_AUTO && list.len() == 9, "{names:?}");
        let r0 = list.iter().find(|p| str_of(p, "nome") == "repo0").unwrap();
        assert_eq!(str_of(r0, "auto"), "git");
        assert_eq!(str_of(list.iter().find(|p| str_of(p, "nome") == "lavoro-con-chat").unwrap(), "auto"), "claude");
        assert!(!str_of(r0, "id").is_empty() && str_of(r0, "cartella").starts_with("~/Developer"), "{r0}");
        // again: no duplicates, removes nothing
        assert_eq!(derive_in(&h, &b), 0);
        assert_eq!(load_json(&b.join("progetti.json")).unwrap()["progetti"].as_array().unwrap().len(), 9);
        let _ = fs::remove_dir_all(&h);
    }

    /// Those who already have projects (or a setting saying it was done) are not touched by the startup run.
    #[test]
    fn startup_skips_users_with_projects() {
        let h = test_dir("auto-avvio");
        fs::create_dir_all(h.join("Developer/nuovo/.git")).unwrap();
        let b = h.join("tavolo");
        write_file(&b.join("progetti.json"), r#"{"progetti":[{"id":"mio","nome":"mio","cartella":"~/Developer/mio","parole":[]}]}"#);
        assert_eq!(derive_at_startup_in(&h, &b), None, "ha gia' un progetto: non si tocca");
        assert_eq!(load_json(&b.join("progetti.json")).unwrap()["progetti"].as_array().unwrap().len(), 1);
        // new user: derives only once, then never again (even if the projects are deleted)
        let h2 = test_dir("auto-avvio2");
        fs::create_dir_all(h2.join("Developer/uno/.git")).unwrap();
        let b2 = h2.join("tavolo");
        assert_eq!(derive_at_startup_in(&h2, &b2), Some(1));
        let _ = fs::write(b2.join("progetti.json"), r#"{"progetti":[]}"#);
        assert_eq!(derive_at_startup_in(&h2, &b2), None, "una volta sola");
        let _ = fs::remove_dir_all(&h2);
        let _ = fs::remove_dir_all(&h);
    }

    /// Guided import: with 200 repos, 80 folders with chats and a thousand of noise, at most MAX_MAIN visible, under
    /// "Mostra altre" at most MAX_OTHERS, never hidden folders/dependencies; each entry has name, group and date.
    #[test]
    fn guided_import_few_entries() {
        let h = test_dir("guidato");
        for i in 0..200 { fs::create_dir_all(h.join(format!("Developer/repo{i:03}/.git"))).unwrap(); }
        let pj = h.join(".claude/projects/-x-chat");
        for i in 0..80 {
            let d = h.join(format!("Documents/lavori/lavoro{i:02}"));
            fs::create_dir_all(&d).unwrap();
            write_file(&pj.join(format!("{i:08x}-1111-4111-8111-111111111111.jsonl")), &user_line(&d.to_string_lossy(), "fai questo lavoro", "cli"));
        }
        for i in 0..1000 { fs::create_dir_all(h.join(format!("Library/Caches/app{i}/.git"))).unwrap(); }
        for i in 0..100 { fs::create_dir_all(h.join(format!("Developer/repo000/node_modules/d{i}/.git"))).unwrap(); }
        let b = h.join("tavolo");
        let r = search_in(&h, &b, &mut |_| {});
        let entries = r["voci"].as_array().unwrap();
        let visible = entries.iter().filter(|v| v["principale"].as_bool().unwrap()).count();
        let others = entries.len() - visible;
        assert!(visible <= MAX_MAIN && visible > 0, "in vista {visible}");
        assert!(others <= MAX_OTHERS, "altre {others}");
        assert!(entries.len() <= MAX_MAIN + MAX_OTHERS);
        assert!(r["omesse"].as_u64().unwrap() > 0, "il resto e' omesso: {}", r["omesse"]);
        assert!(entries.iter().all(|v| !str_of(v, "cartella").contains("Library") && !str_of(v, "cartella").contains("node_modules")));
        // the visible entries come first, each with name, group and date
        let first: Vec<&Value> = entries.iter().take(visible).collect();
        assert!(first.iter().all(|v| v["principale"] == true && !str_of(v, "nome").is_empty() && ["chat", "repo", "progetto"].contains(&str_of(v, "gruppo")) && v["modificato"].as_f64().unwrap() > 0.0));
        // default check only on the visible entries
        assert!(entries.iter().filter(|v| v["scelta"] == true).all(|v| v["principale"] == true));
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn chats_merged_into_project_and_filtered() {
        let h = test_dir("chat");
        let site = h.join("Developer/sito");
        write_file(&site.join("package.json"), "{}");
        let ss = site.to_string_lossy().to_string();
        let pj = h.join(".claude/projects/-x-sito");
        let id1 = "11111111-1111-4111-8111-111111111111";
        let id2 = "22222222-2222-4222-8222-222222222222";
        let id3 = "33333333-3333-4333-8333-333333333333";
        let id4 = "44444444-4444-4444-8444-444444444444";
        write_file(&pj.join(format!("{id1}.jsonl")), &[user_line(&ss, "sistema il login", "cli"),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Fatto, login sistemato."}]}}).to_string(),
            json!({"type": "ai-title", "aiTitle": "Login sistemato"}).to_string()].join("\n"));
        // subfolder of the project: same entry
        write_file(&pj.join(format!("{id2}.jsonl")), &user_line(&format!("{ss}/src"), "rifai il menu", "cli"));
        // one-line automatic call: not a chat
        write_file(&pj.join(format!("{id3}.jsonl")), &user_line(&ss, "classifica questo", "sdk-cli"));
        // temporary folder: out
        write_file(&pj.join(format!("{id4}.jsonl")), &user_line("/private/tmp/x", "prova", "cli"));
        let b = h.join("tavolo");
        let r = search_in(&h, &b, &mut |_| {});
        let entries = r["voci"].as_array().unwrap();
        let v = entries.iter().find(|v| str_of(v, "nome") == "sito").unwrap();
        assert_eq!(v["chat_n"], 2, "{entries:?}");
        assert_eq!(str_of(v, "chat_ultima").is_empty(), false);
        assert_eq!(entries.iter().filter(|v| str_of(v, "nome") == "sito").count(), 1, "una volta sola");
        // create (with the folder from the list, as the interface does): one ready session, two archived chats,
        // the project registered, no process started
        let ss = str_of(v, "cartella").to_string();
        let c = create_in(&h, &b, &[ss.clone()]);
        assert_eq!(c["sessioni"], 1); assert_eq!(c["chat"], 2); assert_eq!(c["progetti"], 1);
        let id = c["ids"][0].as_str().unwrap();
        let st = load_json(&b.join("stato").join(format!("{id}.json"))).unwrap();
        assert_eq!(str_of(&st, "stato"), "ibernata"); assert_eq!(st["pronta"], true);
        let ctx = fs::read_to_string(str_of(&st, "contesto")).unwrap();
        assert!(ctx.contains("Login sistemato") && ctx.contains(&ss), "{ctx}");
        let a = load_json(&b.join("stato").join(format!("{id1}.json"))).unwrap();
        assert_eq!(str_of(&a, "stato"), "archiviata"); assert_eq!(str_of(&a, "nome_archivio"), "Login sistemato");
        // again: no duplicates (chats already in Juggl, ready session already done)
        let r2 = search_in(&h, &b, &mut |_| {});
        let v2 = r2["voci"].as_array().unwrap().iter().find(|v| str_of(v, "nome") == "sito").unwrap().clone();
        assert_eq!(v2["chat_nuove"], 0); assert_eq!(v2["gia"], true); assert_eq!(v2["scelta"], false);
        let c2 = create_in(&h, &b, &[ss]);
        assert_eq!(c2["sessioni"], 0); assert_eq!(c2["chat"], 0);
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn context_without_secrets() {
        let h = test_dir("segreti");
        let p = h.join("Developer/api");
        write_file(&p.join("package.json"), r#"{"dependencies":{"express":"4"}}"#);
        write_file(&p.join(".env"), "STRIPE_KEY=sk_live_segretissimo");
        write_file(&p.join("config/token.json"), "{\"t\":\"abc\"}");
        write_file(&p.join("README.md"), "# Api\n\nServer delle prenotazioni.\n\n## Avvio\nnpm start");
        let v = Entry { cartella: p.to_string_lossy().into(), nome: "api".into(), tipi: vec!["node".into()], esiste: true, ..Default::default() };
        let c = context(&v);
        assert!(c.contains("Express") && c.contains("Server delle prenotazioni") && c.contains("Sezioni: Avvio"), "{c}");
        assert!(!c.contains("sk_live") && !c.contains(".env") && !c.contains("token.json"), "{c}");
        let _ = fs::remove_dir_all(&h);
    }

    /// Manual test: IMPORTA_HOME and IMPORTA_BASE. Without IMPORTA_CREA read only (no writes).
    /// cargo test --release import::tests::on_folder -- --ignored --nocapture
    #[test]
    #[ignore]
    fn on_folder() {
        let h = PathBuf::from(std::env::var("IMPORTA_HOME").expect("IMPORTA_HOME"));
        let b = PathBuf::from(std::env::var("IMPORTA_BASE").unwrap_or_else(|_| h.join("tavolo").to_string_lossy().into()));
        let mut notices = 0;
        let r = search_in(&h, &b, &mut |_| notices += 1);
        println!("{}", serde_json::to_string_pretty(&json!({"ms": r["ms"], "ms_chat": r["ms_chat"], "ms_disco": r["ms_disco"], "chat": r["chat"], "cartelle_viste": r["cartelle_viste"],
            "nuvola_saltate": r["nuvola_saltate"], "tempo_scaduto": r["tempo_scaduto"], "avvisi": notices, "voci": r["voci"].as_array().map(|a| a.len())})).unwrap());
        for v in r["voci"].as_array().unwrap() {
            println!("{} {:<34} {:<9} chat {:>3} (nuove {:>3}) {:>6.0} g  {}{}{}", if v["scelta"].as_bool().unwrap() { "[x]" } else { "[ ]" }, str_of(v, "nome"), str_of(v, "tipo"), v["chat_n"], v["chat_nuove"],
                (unix_now() - v["modificato"].as_f64().unwrap_or(0.0)) / 86400.0, str_of(v, "breve"), if v["gia"].as_bool().unwrap() { "  GIA'" } else { "" }, if str_of(v, "logo").is_empty() { "" } else { "  logo" });
        }
        if let Ok(f) = std::env::var("IMPORTA_FILE") {
            let c = read_chat(Path::new(&f), &b.join("stato"));
            println!("FILE {:?}", c.map(|c| (c.cwd, c.titolo, c.in_juggl, short(&c.primo, 60))));
        }
        if std::env::var("IMPORTA_CREA").is_ok() {
            let chosen: Vec<String> = r["voci"].as_array().unwrap().iter().filter(|v| v["scelta"].as_bool().unwrap()).map(|v| str_of(v, "cartella").to_string()).collect();
            println!("CREA {}", create_in(&h, &b, &chosen));
        }
    }

    #[test]
    fn generic_folder_names() {
        // paths under the Mac users folder ("/Users" + rest)
        let u = |p: &str| format!("/Users{p}");
        assert_eq!(folder_name(&u("/x/Library/Mobile Documents/com~apple~CloudDocs/Social/reel-0038/sorgente")), "reel-0038 sorgente");
        assert_eq!(folder_name(&u("/x/Developer/app")), "app");
        assert_eq!(folder_name(&u("/x/app")), "app");
        assert_eq!(folder_name(&u("/x/Desktop/video/remotion")), "video remotion");
        assert_eq!(folder_name(&u("/x/Developer/sito-negozio")), "sito-negozio");
    }

    #[test]
    fn never_generic_projects() {
        let h = PathBuf::from("/home/prova");
        assert!(is_generic("/home/prova", &h) && is_generic("/", &h) && is_generic("/home/prova/Documents", &h));
        assert!(!is_generic("/home/prova/Documents/sito", &h));
        assert_eq!(short_path("/home/prova/Library/Mobile Documents/com~apple~CloudDocs/Lavori/x", &h), "iCloud Drive/Lavori/x");
        assert!(hidden_or_skipped("node_modules") && hidden_or_skipped("Foto.photoslibrary") && hidden_or_skipped(".git") && !hidden_or_skipped("sito"));
    }
}
