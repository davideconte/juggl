// Documents in replies: the files a session creates, edits or cites (texts, drafts, PDF, CSV, code...)
// become cards under the reply, on the Mac and on the iPhone, and open in the viewer without going through iCloud or Finder.
//
// - `enrich`: after the transcript is read, each agent reply gets "documenti" = the files cited in its
//   text and (on the last reply of the turn) those written or edited with Write / Edit / NotebookEdit in that turn.
// - Every file shown in a card enters the allowed list: the iPhone can ask for ONLY those (/api/documento),
//   never an arbitrary path. Secret folders (.ssh, .config, .aws, Keychains, .env, files with tokens) never leave.
// - `leggi_documento`: the content ready to show (text, CSV already in rows, Word/RTF converted to HTML with textutil).
use crate::{is_media, home, str_of};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

const CODE_EXTENSIONS: [&str; 52] = ["rs", "swift", "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "sh", "zsh", "bash", "rb", "go", "java", "kt",
    "c", "h", "cc", "cpp", "hpp", "m", "mm", "css", "scss", "less", "sql", "yml", "yaml", "toml", "xml", "plist", "ini", "conf", "cfg",
    "php", "vue", "svelte", "lua", "gradle", "liquid", "graphql", "proto", "dart", "r", "jsonl", "ndjson", "lock", "tf", "ex", "erl", "scala"];
const PER_REPLY: usize = 30;
/// Maximum text sent to the iPhone (beyond: reduced preview, "troncato").
pub const IPHONE_LIMIT: usize = 600_000;
/// On the Mac we read more (same machine, no network).
pub const MAC_LIMIT: usize = 4_000_000;
/// Raw files (PDF, images, video) served to the iPhone: beyond this, preview only.
pub const MAX_RAW: u64 = 60 * 1024 * 1024;

/// Document type from the extension: decides icon, viewer and conversion.
pub fn doc_type(p: &str) -> &'static str {
    let name = p.rsplit('/').next().unwrap_or("");
    if !name.contains('.') || name.starts_with('.') && name.matches('.').count() == 1 { return ""; }
    let e = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match e.as_str() {
        "md" | "markdown" | "mdx" => "markdown",
        "txt" | "text" | "log" | "srt" | "vtt" => "testo",
        "json" | "geojson" | "webmanifest" => "json",
        "csv" | "tsv" => "csv",
        "pdf" => "pdf",
        "html" | "htm" => "html",
        "docx" | "doc" | "rtf" | "odt" | "wordml" | "webarchive" => "documento",
        "pages" | "key" | "numbers" | "xlsx" | "xls" | "pptx" | "ppt" => "ufficio",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "svg" => "immagine",
        "mp4" | "mov" | "m4v" | "webm" => "video",
        x if CODE_EXTENSIONS.contains(&x) => "codice",
        _ => "",
    }
}

/// Never off the Mac: keys, passwords, configs with tokens.
pub fn is_secret(p: &str) -> bool {
    let l = p.to_lowercase();
    let name = l.rsplit('/').next().unwrap_or("");
    ["/.ssh/", "/.config/", "/.aws/", "/.gnupg/", "/.kube/", "/.docker/", "/library/keychains/", "/.password-store/", "/.secrets/",
        "/secrets/", "/.azure/", "/.gcloud/", "/gcloud-sa/", "/.claude/state/", "/.local/share/keyrings/"].iter().any(|x| l.contains(x))
        || name.starts_with(".env") || name.ends_with(".env") || name.contains(".env.")
        || ["token", "secret", "segret", "password", "passwd", "credential", "apikey", "api-key", "api_key", "id_rsa", "id_ed25519", "id_ecdsa", "private-key", "chiave"]
            .iter().any(|x| name.contains(x))
        || [".key", ".pem", ".p8", ".p12", ".pfx", ".keychain", ".keychain-db", ".kdbx", ".crt", ".cer", ".mobileprovision"].iter().any(|x| name.ends_with(x))
        || [".netrc", ".npmrc", ".pypirc", ".git-credentials", "ponte.json", "iphone-push.json", "personale.json", "known_hosts", "authorized_keys"].contains(&name)
}

/// Temporary or system files of no interest to whoever reads the chat. `auto`: collected from the tools (stricter
/// than a path cited on purpose in the text).
fn is_system(p: &str, auto: bool) -> bool {
    let l = p.to_lowercase();
    let name = l.rsplit('/').next().unwrap_or("");
    if l.starts_with("/tmp/") || l.starts_with("/private/") || l.starts_with("/var/") || l.starts_with("/system/") || l.starts_with("/usr/")
        || l.starts_with("/opt/") || l.starts_with("/applications/") || l.starts_with("/library/") { return true; }
    if cfg!(windows) && (l.get(1..).map(|r| r.starts_with(":/windows/") || r.starts_with(":/program files") || r.starts_with(":/programdata/")).unwrap_or(false)
        || l.contains("/appdata/local/temp/") || l.contains("/appdata/local/packages/")) { return true; }
    if ["/library/caches/", "/node_modules/", "/.git/", "/target/", "/deriveddata/", "/.next/", "/.build/", "/dd/", "/.cache/", "/__pycache__/",
        "/.venv/", "/venv/", "/site-packages/", "/tavolo/stato/", "/tavolo/coda/", "/tavolo/sock/", "/.trash/", "/.claude/projects/"].iter().any(|x| l.contains(x)) { return true; }
    // rename (14/11): the same internal folders also in the data folder of new users (~/.juggl)
    { let b = crate::base_dir().to_string_lossy().replace('\\', "/").to_lowercase(); if ["stato/", "coda/", "sock/"].iter().any(|x| l.contains(&format!("{b}/{x}"))) { return true; } }
    if name.contains(".bak") || name.ends_with('~') || [".orig", ".swp", ".tmp", ".ds_store", ".pyc", ".lock", ".pid", ".parte"].iter().any(|x| name.ends_with(x)) { return true; }
    if auto && ["/build/", "/dist/", "/.claude/", "/ui-bak/", "/uscite/"].iter().any(|x| l.contains(x)) { return true; }
    false
}

/// Windows: paths are kept with "/" (C:/Users/...): Windows opens them anyway and the rest of the module stays the same.
pub fn norm(p: &str) -> String { if cfg!(windows) { p.replace('\\', "/") } else { p.to_string() } }

/// Absolute path: "/..." on the Mac, "C:/..." on Windows (after norm).
pub fn is_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with('/') || (cfg!(windows) && b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/')
}

/// A candidate path (after "~/", "/Users/"...) becomes a real file: I try the longest piece and then, shortening
/// at spaces, commas and parentheses, the shorter ones (iCloud paths have spaces: "Mobile Documents").
fn resolve(cand: &str, start: &str) -> Option<String> {
    let mut cuts: Vec<usize> = vec![cand.len()];
    for (i, c) in cand.char_indices().rev() { if matches!(c, ' ' | ')' | ',' | ';' | '\'') && i > start.len() { cuts.push(i); } }
    for end_idx in cuts.into_iter().take(40) {
        let piece = cand[..end_idx].trim_end_matches(|c: char| ".:;,!?)'’".contains(c) || c.is_whitespace());
        if piece.len() <= start.len() { continue; }
        let p = if let Some(r) = piece.strip_prefix("~/") { home().join(r).to_string_lossy().to_string() }
            else if let Some(r) = piece.strip_prefix("iCloud Drive/") { home().join("Library/Mobile Documents/com~apple~CloudDocs").join(r).to_string_lossy().to_string() }
            else { piece.to_string() };
        let p = norm(&p);
        if p.contains("/../") { continue; }
        if !crate::fast_open::is_network(&p) && Path::new(&p).is_file() { return Some(p); }   // fast chats (15/11): never checks on network disks
    }
    None
}

/// Paths of existing files cited in a text: absolute, with "~/", iCloud ("iCloud Drive/..."), also with spaces
/// or written with "\ ", inside `code`, Markdown links or "file://".
pub fn cited_paths(t: &str) -> Vec<String> {
    let t = if cfg!(windows) { t.replace("file:///", "").replace('\\', "/") } else { t.to_string() };
    if !t.contains('/') { return vec![]; }
    let t = t.replace("file://", "").replace("\\ ", " ");
    #[cfg(windows)]
    let drives: Vec<String> = (b'A'..=b'Z').map(|c| format!("{}:/", c as char)).collect();
    #[cfg(windows)]
    let starts: Vec<&str> = ["~/"].into_iter().chain(drives.iter().map(|x| x.as_str())).collect();
    #[cfg(unix)]
    let starts = ["/Users/", "~/", "/Volumes/", "iCloud Drive/"];
    let mut out: Vec<String> = vec![];
    let mut skip_until = 0usize;
    for (i, _) in t.char_indices() {
        if i < skip_until { continue; }
        let rest = &t[i..];
        let Some(start) = starts.iter().find(|x| rest.starts_with(**x)) else { continue };
        if i > 0 {
            let c = t[..i].chars().last().unwrap_or(' ');
            if !(c.is_whitespace() || "([`'\"<:*>«“".contains(c)) { continue; }
        }
        let end_idx = rest.find(|c: char| "\n\r\t`\"<>|*[]{}»”".contains(c)).unwrap_or(rest.len());
        if let Some(p) = resolve(&rest[..end_idx], start) {
            skip_until = i + 1;
            if !out.contains(&p) { out.push(p); }
            if out.len() >= 16 { break; }
        }
    }
    out
}

/// Short folder for the card: "iCloud · Clienti", "Scrivania", "Documenti · CLIENTI", "tavolo · ui".
pub fn short_folder(p: &str) -> String {
    let h = home().to_string_lossy().to_string();
    let icloud = format!("{h}/Library/Mobile Documents/com~apple~CloudDocs/");
    let folder_parts = |r: &str| -> Vec<String> { let v: Vec<&str> = r.split('/').filter(|x| !x.is_empty()).collect(); v[..v.len().saturating_sub(1)].iter().map(|x| x.to_string()).collect() };
    let rename_dir = |d: &str| match d { "Desktop" => "Scrivania".to_string(), "Documents" => "Documenti".to_string(), "Downloads" => "Download".to_string(), x => x.to_string() };
    if let Some(r) = p.strip_prefix(&icloud) {
        let d = folder_parts(r);
        return match d.len() {
            0 => "iCloud".into(),
            1 if d[0] == "Desktop" || d[0] == "Documents" => rename_dir(&d[0]),
            _ if d[0] == "Desktop" || d[0] == "Documents" => format!("{} · {}", rename_dir(&d[0]), d.last().unwrap()),
            _ => format!("iCloud · {}", d.last().unwrap()),
        };
    }
    // Dropbox, Google Drive, OneDrive (~/Library/CloudStorage/<service>/...): "Dropbox · CLIENTI"
    if let Some(r) = p.strip_prefix(&format!("{h}/Library/CloudStorage/")) {
        let d = folder_parts(r);
        let service = d.first().map(|x| x.split('-').next().unwrap_or(x).to_string()).unwrap_or_default();
        return match d.len() { 0 | 1 => service, _ => format!("{service} · {}", d.last().unwrap()) };
    }
    if let Some(r) = p.strip_prefix(&format!("{h}/")) {
        let d = folder_parts(r);
        return match d.len() { 0 => "Inizio".into(), 1 => rename_dir(&d[0]), _ => format!("{} · {}", rename_dir(&d[0]), d.last().unwrap()) };
    }
    if let Some(r) = p.strip_prefix("/Volumes/") {
        let d = folder_parts(r);
        return match d.len() { 0 => "Disco".into(), 1 => d[0].clone(), _ => format!("{} · {}", d[0], d.last().unwrap()) };
    }
    Path::new(p).parent().and_then(|x| x.file_name()).map(|x| x.to_string_lossy().to_string()).unwrap_or_default()
}

fn mtime(md: &fs::Metadata) -> u64 { md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0) }

/// First 2-3 readable lines of a short text (without Markdown marks), for the preview in the card.
fn preview(p: &str, md: &fs::Metadata) -> String {
    static C: OnceLock<Mutex<HashMap<String, (u64, u64, String)>>> = OnceLock::new();
    let c = C.get_or_init(|| Mutex::new(HashMap::new()));
    let (n, mt) = (md.len(), mtime(md));
    if let Some((a, b, t)) = c.lock().unwrap_or_else(|e| e.into_inner()).get(p) { if *a == n && *b == mt { return t.clone(); } }
    let mut buf = vec![0u8; 6000];
    let k = fs::File::open(p).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    buf.truncate(k);
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut lines: Vec<String> = vec![];
    let mut in_header = false;
    for (i, r) in text.lines().enumerate() {
        let r = r.trim();
        if i == 0 && r == "---" { in_header = true; continue; }     // YAML header of the Markdown
        if in_header { if r == "---" { in_header = false; } continue; }
        if r.is_empty() || r.starts_with("```") || r.chars().all(|c| "-=*_|: ".contains(c)) || r.starts_with("<!--") { continue; }
        let cleaned = r.trim_start_matches(|c: char| "#>-*+ ".contains(c)).replace("**", "").replace("__", "").replace('`', "");
        let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        if cleaned.is_empty() { continue; }
        lines.push(cleaned.chars().take(140).collect());
        if lines.len() >= 3 { break; }
    }
    let t = lines.join("\n");
    let mut g = c.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 3000 { g.clear(); }
    g.insert(p.to_string(), (n, mt, t.clone()));
    t
}

fn allowed_set() -> &'static Mutex<HashSet<String>> {
    static L: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashSet::new()))
}

/// The iPhone can only ask for files that appeared in a card (that is, in session replies).
pub fn is_allowed(p: &str) -> bool { !is_secret(p) && allowed_set().lock().unwrap_or_else(|e| e.into_inner()).contains(p) }

fn allow(p: &str) {
    let mut g = allowed_set().lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 20_000 { g.clear(); }
    g.insert(p.to_string());
}

/// Card of a file: name, short folder, size, type, preview for short texts. None if it should not be shown.
fn card(p: &str, action: &str, auto: bool) -> Option<Value> {
    let t = doc_type(p);
    if t.is_empty() || is_media(p) || is_secret(p) || is_system(p, auto) { return None; }
    if crate::fast_open::is_network(p) { return None; }   // fast chats (15/11): a slow network disk blocked opening the chat
    let md = fs::metadata(p).ok()?;
    if !md.is_file() || md.len() == 0 { return None; }
    allow(p);
    let mut v = json!({"path": p, "nome": p.rsplit('/').next().unwrap_or(p), "cartella": short_folder(p), "byte": md.len(), "tipo": t, "azione": action});
    if matches!(t, "testo" | "markdown") && md.len() <= 512 * 1024 {
        let a = preview(p, &md);
        if !a.is_empty() { v["anteprima"] = json!(a); }
    }
    Some(v)
}

/// Files attached by you ("Allegato: /path", from the iPhone or the Mac) that are not images or video: PDF, documents,
/// audio, zip. They become cards under your bubble (icon, name, size) and open in the phone viewer (09/11).
fn your_attachments(e: &mut Value) {
    let t = str_of(e, "testo").to_string();
    if !t.contains("Allegato: /") && !(cfg!(windows) && t.contains("Allegato: ")) { return; }
    let mut list: Vec<Value> = vec![];
    for part in t.split("Allegato: ").skip(1) {
        // to end of line (names with spaces), otherwise to the first space (the terminal joins the lines)
        let row = part.lines().next().unwrap_or("").trim();
        if crate::fast_open::is_network(row) { continue; }   // fast chats (15/11)
        let p = if Path::new(row).is_file() { row } else { row.split_whitespace().next().unwrap_or("") };
        let p = &norm(p);
        if !is_absolute(p) || p.contains("/../") || is_media(p) || is_secret(p) || crate::fast_open::is_network(p) || list.iter().any(|x| str_of(x, "path") == p) { continue; }
        let Ok(md) = fs::metadata(p) else { continue };
        if !md.is_file() { continue; }
        allow(p);
        let t = match doc_type(p) { "" => "altro", x => x };
        // the Mac saves attachments as "<time>-name": in the card the real name
        let full = p.rsplit('/').next().unwrap_or(p);
        let digits = full.chars().take_while(|c| c.is_ascii_digit()).count();
        let name = if digits >= 9 && full[digits..].starts_with('-') && full.len() > digits + 1 { &full[digits + 1..] } else { full };
        list.push(json!({"path": p, "nome": name, "cartella": short_folder(p), "byte": md.len(), "tipo": t, "azione": "allegato"}));
        if list.len() >= PER_REPLY { break; }
    }
    if !list.is_empty() { e["documenti"] = json!(list); }
}

// ---------- temporary pages (`juggl pagina`): link https://share.juggl.dev/<slug>-<hash>/ ----------
// Since 17/11 pages live on a separate subdomain. The earlier addresses (juggl.dev/p/ and the previous domain) redirect
// there with 301 and stay recognized (links already written in chats), always brought back to the new address.

pub const PAGES_URL: &str = "https://share.juggl.dev/";
#[cfg(not(feature = "personale"))]
pub const OLD_PAGES_URLS: [&str; 1] = ["https://juggl.dev/p/"];

/// Link of a page with the new address (old ones are converted, others stay the same).
pub fn page_link_new(l: &str) -> String {
    for v in OLD_PAGES_URLS {
        if let Some(r) = l.strip_prefix(v) { return format!("{PAGES_URL}{r}"); }
    }
    l.to_string()
}

/// Links to pages published with `juggl pagina` inside a text (new or old address), in order, already with the new one.
pub fn page_links(t: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut rest = t;
    loop {
        let first_hit = std::iter::once(PAGES_URL).chain(OLD_PAGES_URLS).filter_map(|p| rest.find(p).map(|i| (i, p))).min_by_key(|x| x.0);
        let Some((i, pref)) = first_hit else { break };
        let da = &rest[i..];
        let end_idx = da.find(|c: char| c.is_whitespace() || "<>\"'`)]|*".contains(c)).unwrap_or(da.len());
        let l = da[..end_idx].trim_end_matches(|c: char| ".,;:!?".contains(c)).to_string();
        if l.len() > pref.len() {
            let l = page_link_new(&l);
            if !out.contains(&l) { out.push(l); }
        }
        rest = &da[end_idx.max(1)..];
    }
    out
}

/// Registry of pages (~/tavolo/pagine.json), reread only when it changes.
fn pages_registry() -> Vec<Value> {
    static C: OnceLock<Mutex<(u64, Vec<Value>)>> = OnceLock::new();
    let c = C.get_or_init(|| Mutex::new((0, vec![])));
    let p = crate::base_dir().join("pagine.json");
    let mt = fs::metadata(&p).ok().map(|m| mtime(&m) * 1000 + m.len() % 1000).unwrap_or(0);
    let mut g = c.lock().unwrap_or_else(|e| e.into_inner());
    if g.0 != mt {
        let v = crate::load_json(&p).and_then(|v| v.get("pagine").and_then(|x| x.as_array()).cloned()).unwrap_or_default();
        *g = (mt, v);
    }
    g.1.clone()
}

/// "Pagina" card: title from the registry (or the last part of the path), expired if `scade` has passed.
pub fn page_card(link: &str) -> Value {
    let link = &page_link_new(link); // [juggl.dev pages 13/11] the old registry too has links with the old prefix
    let norm = |x: &str| page_link_new(x).trim_end_matches('/').to_string();
    let entry = pages_registry().into_iter().find(|v| norm(str_of(v, "link")) == norm(link));
    let last_part = link.trim_end_matches('/').rsplit('/').next().unwrap_or("pagina").to_string();
    let title = entry.as_ref().map(|v| str_of(v, "titolo").to_string()).filter(|x| !x.is_empty()).unwrap_or(last_part);
    let expires = entry.as_ref().and_then(|v| v.get("scade")).and_then(|x| x.as_f64());
    let expired = expires.map(|t| t < crate::unix_now()).unwrap_or(false);
    let project = entry.as_ref().map(|v| str_of(v, "progetto").to_string()).unwrap_or_default();
    json!({"path": link, "link": link, "nome": title, "cartella": if project.is_empty() { "Pagina".to_string() } else { format!("Pagina · {project}") },
        "byte": 0, "tipo": "pagina", "azione": "citato", "scade": expires, "scaduta": expired})
}

fn close_turn(events: &mut [Value], written: &[(String, &'static str)], cited: &HashSet<String>, last: Option<usize>) {
    let Some(j) = last else { return };
    let mut list: Vec<Value> = events[j].get("documenti").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    // created first, then modified: the new file is almost always the one to read
    let mut order: Vec<&(String, &'static str)> = written.iter().collect();
    order.sort_by_key(|x| if x.1 == "creato" { 0 } else { 1 });
    for (p, act) in order {
        if list.len() >= PER_REPLY { break; }
        if cited.contains(p) || list.iter().any(|x| str_of(x, "path") == p) { continue; }
        if let Some(v) = card(p, act, true) { list.push(v); }
    }
    if !list.is_empty() { events[j]["documenti"] = json!(list); }
}

/// Adds "documenti" to agent replies (events already read from the transcript, in order).
pub fn enrich(events: &mut [Value]) {
    let mut written: Vec<(String, &'static str)> = vec![];
    let mut cited: HashSet<String> = HashSet::new();
    let mut last: Option<usize> = None;
    for i in 0..events.len() {
        match str_of(&events[i], "tipo") {
            "tu" => {
                your_attachments(&mut events[i]);
                if events[i].get("durante").is_none() {
                    close_turn(events, &written, &cited, last);
                    written.clear(); cited.clear(); last = None;
                }
            }
            "strumento" => {
                let act: &'static str = match str_of(&events[i], "nome") { "Write" => "creato", "Edit" | "MultiEdit" | "NotebookEdit" => "modificato", _ => continue };
                let p = norm(str_of(&events[i], "extra"));
                if !is_absolute(&p) { continue; }
                if let Some(x) = written.iter_mut().find(|x| x.0 == p) { if act == "creato" { x.1 = act; } } else { written.push((p, act)); }
            }
            "agente" => {
                let mut list: Vec<Value> = vec![];
                for p in cited_paths(str_of(&events[i], "testo")) {
                    if list.len() >= PER_REPLY { break; }
                    let act = written.iter().find(|x| x.0 == p).map(|x| x.1).unwrap_or("citato");
                    if let Some(v) = card(&p, act, false) { cited.insert(p); list.push(v); }
                }
                for l in page_links(str_of(&events[i], "testo")) {
                    if list.len() >= PER_REPLY { break; }
                    list.push(page_card(&l));
                }
                if !list.is_empty() { events[i]["documenti"] = json!(list); }
                last = Some(i);
            }
            _ => {}
        }
    }
    close_turn(events, &written, &cited, last);
}

// ---------- reading for the viewer ----------

/// Text of a file up to `limit` bytes: UTF-8, or Latin-1 (CSV exported from Excel). None if binary.
fn read_text(p: &str, limit: usize) -> Option<(String, bool)> {
    let mut f = fs::File::open(p).ok()?;
    let mut b = Vec::new();
    f.by_ref().take(limit as u64 + 1).read_to_end(&mut b).ok()?;
    let truncated = b.len() > limit;
    if truncated { b.truncate(limit); if let Some(k) = b.iter().rposition(|x| *x == b'\n') { b.truncate(k + 1); } }
    if b.iter().take(8000).any(|x| *x == 0) { return None; }
    let t = match std::str::from_utf8(&b) {
        Ok(t) => t.to_string(),
        Err(e) if truncated && e.valid_up_to() + 4 >= b.len() => String::from_utf8_lossy(&b[..e.valid_up_to()]).to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    };
    Some((t.trim_start_matches('\u{feff}').to_string(), truncated))
}

/// CSV / TSV into rows (separator chosen from the first line, quotes respected), with a cap on rows and columns.
fn csv_rows(t: &str, tsv: bool) -> (Vec<Vec<String>>, bool) {
    let first = t.lines().next().unwrap_or("");
    let sep = if tsv { '\t' } else { [',', ';', '\t'].into_iter().max_by_key(|c| first.matches(*c).count()).unwrap_or(',') };
    let (mut out, mut row, mut cell) = (vec![], vec![], String::new());
    let mut quoted = false;
    let mut it = t.chars().peekable();
    let mut cut = false;
    while let Some(c) = it.next() {
        if quoted {
            if c == '"' { if it.peek() == Some(&'"') { cell.push('"'); it.next(); } else { quoted = false; } } else { cell.push(c); }
            continue;
        }
        match c {
            '"' if cell.is_empty() => quoted = true,
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                out.push(std::mem::take(&mut row));
                if out.len() >= 3000 { cut = true; break; }
            }
            x if x == sep => row.push(std::mem::take(&mut cell)),
            x => cell.push(x),
        }
    }
    if !cell.is_empty() || !row.is_empty() { row.push(cell); out.push(row); }
    let out: Vec<Vec<String>> = out.into_iter().filter(|r: &Vec<String>| !(r.len() == 1 && r[0].trim().is_empty()))
        .map(|r| r.into_iter().take(60).map(|c| if c.chars().count() > 400 { c.chars().take(400).collect::<String>() + "…" } else { c }).collect()).collect();
    (out, cut)
}

/// Command with a time limit (textutil and qlmanage sometimes hang on odd files).
fn run_with_timeout(c: &mut Command, max: Duration) -> Option<Vec<u8>> {
    let mut child = c.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut output = child.stdout.take()?;
    let reader = std::thread::spawn(move || { let mut v = vec![]; let _ = output.read_to_end(&mut v); v });
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => { let v = reader.join().unwrap_or_default(); return if st.success() { Some(v) } else { None }; }
            Ok(None) if start.elapsed() < max => std::thread::sleep(Duration::from_millis(30)),
            _ => { let _ = child.kill(); let _ = child.wait(); return None; }
        }
    }
}

/// Word, RTF, ODT: HTML (layout) and plain text (for "Copia tutto"), converted by textutil on the Mac.
fn convert(p: &str, md: &fs::Metadata) -> Option<(String, String)> {
    static C: OnceLock<Mutex<HashMap<String, (u64, u64, String, String)>>> = OnceLock::new();
    let c = C.get_or_init(|| Mutex::new(HashMap::new()));
    let (n, mt) = (md.len(), mtime(md));
    if let Some((a, b, h, t)) = c.lock().unwrap_or_else(|e| e.into_inner()).get(p) { if *a == n && *b == mt { return Some((h.clone(), t.clone())); } }
    let html = run_with_timeout(crate::new_command("/usr/bin/textutil").args(["-convert", "html", "-stdout", "--"]).arg(p), Duration::from_secs(20))?;
    let txt = run_with_timeout(crate::new_command("/usr/bin/textutil").args(["-convert", "txt", "-stdout", "--"]).arg(p), Duration::from_secs(20)).unwrap_or_default();
    let (h, t) = (String::from_utf8_lossy(&html).to_string(), String::from_utf8_lossy(&txt).to_string());
    let mut g = c.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 24 { g.clear(); }
    g.insert(p.to_string(), (n, mt, h.clone(), t.clone()));
    Some((h, t))
}

/// Image preview (Quick Look) for Pages, Keynote, Numbers, Excel, PowerPoint and for PDFs that are too large.
pub fn image_preview(p: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let md = fs::metadata(p).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (p, md.len(), mtime(&md), 1600).hash(&mut h);
    let dir = home().join("Library/Caches/juggl-documenti");
    let png = dir.join(format!("{:016x}.png", h.finish()));
    if png.exists() { return Some(png); }
    let tmp = dir.join(format!("ql-{:016x}", h.finish()));
    let _ = fs::create_dir_all(&tmp);
    let _ = run_with_timeout(crate::new_command("/usr/bin/qlmanage").args(["-t", "-s", "1600", "-o"]).arg(&tmp).arg(p), Duration::from_secs(20));
    let name = format!("{}.png", Path::new(p).file_name()?.to_string_lossy());
    let ok = fs::rename(tmp.join(name), &png).is_ok();
    let _ = fs::remove_dir_all(&tmp);
    if ok { Some(png) } else { None }
}

/// Text of a PDF (PDFKit via JavaScript for Automation): used by "Copia tutto".
pub fn pdf_text(p: &str) -> Option<String> {
    let js = r#"function run(a){ObjC.import("PDFKit");var d=$.PDFDocument.alloc.initWithURL($.NSURL.fileURLWithPath(a[0]));if(!d||d.isNil())return "";return ObjC.unwrap(d.string)||""}"#;
    let o = run_with_timeout(crate::new_command("/usr/bin/osascript").args(["-l", "JavaScript", "-e", js, "--"]).arg(p), Duration::from_secs(25))?;
    let t = String::from_utf8_lossy(&o).trim_end().to_string();
    if t.is_empty() { None } else { Some(t) }
}

/// All the text of a document, for "Copia tutto" on the Mac (also of PDFs and Word files).
pub fn full_text(p: &str) -> Value {
    let p = crate::session_care::expand_path(p);
    if is_secret(&p) { return json!({"ok": false}); }
    let t = match doc_type(&p) {
        "pdf" => pdf_text(&p),
        "documento" => fs::metadata(&p).ok().and_then(|md| convert(&p, &md)).map(|x| x.1),
        _ => read_text(&p, 20_000_000).map(|x| x.0),
    };
    match t { Some(t) => json!({"ok": true, "testo": t}), None => json!({"ok": false}) }
}

/// Content ready for the viewer. `limit`: at most that many bytes of text (beyond: "troncato", reduced preview).
pub fn read_document(p: &str, limit: usize) -> Value {
    let p = crate::session_care::expand_path(p);
    if is_secret(&p) { return json!({"ok": false, "msg": "Questo file resta sul Mac: può contenere chiavi o password."}); }
    let Ok(md) = fs::metadata(&p) else { return json!({"ok": false, "msg": "Il file non c'è più."}) };
    if !md.is_file() { return json!({"ok": false, "msg": "Non è un file."}); }
    let mut t = doc_type(&p);
    let mut v = json!({"ok": true, "path": p, "nome": p.rsplit('/').next().unwrap_or(""), "cartella": short_folder(&p), "byte": md.len(),
        "modificato": mtime(&md), "estensione": p.rsplit('.').next().unwrap_or("").to_lowercase()});
    match t {
        "markdown" | "testo" | "codice" | "json" | "csv" | "html" | "" => match read_text(&p, limit) {
            Some((text, truncated)) => {
                if t.is_empty() { t = "testo"; }
                let text = if t == "json" && !truncated {
                    serde_json::from_str::<Value>(&text).ok().and_then(|x| serde_json::to_string_pretty(&x).ok()).unwrap_or(text)
                } else { text };
                if t == "csv" {
                    let (r, cut) = csv_rows(&text, p.to_lowercase().ends_with(".tsv"));
                    v["righe"] = json!(r);
                    if cut { v["righe_tagliate"] = json!(true); }
                }
                v["testo"] = json!(text);
                v["troncato"] = json!(truncated);
            }
            None => t = "altro",
        },
        "documento" => match convert(&p, &md) {
            Some((h, text)) => { v["html"] = json!(h); v["testo"] = json!(text); }
            None => { t = "altro"; v["msg"] = json!("Non sono riuscito a convertirlo."); }
        },
        "ufficio" => { v["anteprima"] = json!(image_preview(&p).is_some()); }
        "pdf" | "immagine" | "video" => { if md.len() > MAX_RAW { v["grande"] = json!(true); } }
        _ => {}
    }
    v["tipo"] = json!(t);
    v
}

/// MIME type to serve the raw file.
pub fn mime(p: &str) -> &'static str {
    match p.rsplit('.').next().unwrap_or("").to_lowercase().as_str() {
        "pdf" => "application/pdf", "png" => "image/png", "jpg" | "jpeg" => "image/jpeg", "gif" => "image/gif", "webp" => "image/webp",
        "heic" => "image/heic", "svg" => "image/svg+xml", "mp4" | "m4v" => "video/mp4", "mov" => "video/quicktime", "webm" => "video/webm",
        "html" | "htm" => "text/html; charset=utf-8", "json" => "application/json", "csv" => "text/csv; charset=utf-8",
        "md" | "txt" | "log" => "text/plain; charset=utf-8",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "doc" => "application/msword", "rtf" => "application/rtf",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let d = home().join(format!(".juggl-prove-documenti-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn secrets_never() {
        for p in ["/home/x/.ssh/id_rsa", "/home/x/.config/servizio/asc.env", "/home/x/progetto/.env", "/home/x/progetto/.env.local",
            "/home/x/a/token.txt", "/home/x/Library/Keychains/login.keychain-db", "/home/x/a/AuthKey.p8", "/home/x/tavolo/ponte.json",
            "/home/x/.aws/credentials", "/home/x/a/my-secret-notes.md"] { assert!(is_secret(p), "{p}"); }
        for p in ["/home/x/Desktop/bozza.md", "/home/x/Documents/offerta.pdf", "/home/x/tavolo/app/ui/index.html"] { assert!(!is_secret(p), "{p}"); }
    }

    #[test]
    fn types() {
        assert_eq!(doc_type("/a/b.md"), "markdown"); assert_eq!(doc_type("/a/B.PDF"), "pdf"); assert_eq!(doc_type("/a/x.csv"), "csv");
        assert_eq!(doc_type("/a/x.docx"), "documento"); assert_eq!(doc_type("/a/main.rs"), "codice"); assert_eq!(doc_type("/a/Makefile"), "");
        assert_eq!(doc_type("/a/.gitignore"), "");
    }

    #[test]
    fn short_folders() {
        let h = home().to_string_lossy().to_string();
        assert_eq!(short_folder(&format!("{h}/Library/Mobile Documents/com~apple~CloudDocs/Clienti/bozza.md")), "iCloud · Clienti");
        assert_eq!(short_folder(&format!("{h}/Library/Mobile Documents/com~apple~CloudDocs/x.md")), "iCloud");
        assert_eq!(short_folder(&format!("{h}/Desktop/x.md")), "Scrivania");
        assert_eq!(short_folder(&format!("{h}/Documents/WORKSPACE/CLIENTI/X/a.md")), "Documenti · X");
        assert_eq!(short_folder(&format!("{h}/tavolo/app/ui/index.html")), "tavolo · ui");
        assert_eq!(short_folder(&format!("{h}/Library/CloudStorage/Dropbox/CLIENTI/MOTION/a.md")), "Dropbox · MOTION");
        assert_eq!(short_folder(&format!("{h}/Library/CloudStorage/GoogleDrive-x@y.com/Il mio Drive/a.pdf")), "GoogleDrive · Il mio Drive");
    }

    #[test]
    fn cited_also_with_spaces() {
        let d = test_dir("citati");
        let sp = d.join("Mobile Documents finto");
        fs::create_dir_all(&sp).unwrap();
        let a = sp.join("bozza messaggio.md");
        fs::write(&a, "# Ciao\n\nprima riga\nseconda").unwrap();
        let b = d.join("dati.csv");
        fs::write(&b, "a,b\n1,2\n").unwrap();
        let (a, b) = (a.to_string_lossy().to_string(), b.to_string_lossy().to_string());
        let short = b.replacen(&home().to_string_lossy().to_string(), "~", 1);
        let t = format!("La bozza resta in `{a}`. Dati in {short}, poi [link]({b}) e {a}: fine.\nNon esiste: /home/nessuno/x.md");
        let v = cited_paths(&t);
        assert_eq!(v, vec![a.clone(), b.clone()]);
        let esc = format!("vedi {}", a.replace(' ', "\\ "));
        assert_eq!(cited_paths(&esc), vec![a.clone()]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn cards_in_the_turn() {
        let d = test_dir("turno");
        let written_file = d.join("offerta.md");
        fs::write(&written_file, "---\ntitolo: x\n---\n# Offerta\n\n**Prezzo**: 100\n- voce uno\n- voce due\n").unwrap();
        let modified_file = d.join("main.rs");
        fs::write(&modified_file, "fn main() {}\n").unwrap();
        let secret_file = d.join(".env");
        fs::write(&secret_file, "TOKEN=1").unwrap();
        let (sc, mo, se) = (written_file.to_string_lossy().to_string(), modified_file.to_string_lossy().to_string(), secret_file.to_string_lossy().to_string());
        let mut ev = vec![
            json!({"tipo": "tu", "testo": "fai"}),
            json!({"tipo": "strumento", "nome": "Write", "testo": sc, "extra": sc}),
            json!({"tipo": "agente", "testo": "sto lavorando"}),
            json!({"tipo": "strumento", "nome": "Edit", "testo": mo, "extra": mo}),
            json!({"tipo": "strumento", "nome": "Write", "testo": se, "extra": se}),
            json!({"tipo": "agente", "testo": format!("Fatto, l'offerta e' in {sc}.")}),
            json!({"tipo": "tu", "testo": "grazie"}),
            json!({"tipo": "agente", "testo": "prego"}),
        ];
        enrich(&mut ev);
        assert!(ev[2].get("documenti").is_none());
        let docs = ev[5]["documenti"].as_array().unwrap();
        let p: Vec<&str> = docs.iter().map(|x| x["path"].as_str().unwrap()).collect();
        assert_eq!(p, vec![sc.as_str(), mo.as_str()]);
        assert_eq!(docs[0]["azione"], "creato");
        assert_eq!(docs[0]["anteprima"], "Offerta\nPrezzo: 100\nvoce uno");
        assert_eq!(docs[1]["azione"], "modificato");
        assert!(ev[7].get("documenti").is_none());
        assert!(is_allowed(&sc) && !is_allowed(&se));
        let _ = fs::remove_dir_all(&d);
    }


    #[test]
    fn pages() {
        // [juggl.dev pages 13/11] new and old prefix in the same text: order of appearance, all with the new prefix, no duplicates
        let t = "Ecco la pagina: https://share.juggl.dev/bozza-a1b2c3/. E il link (https://juggl.dev/p/x-9f/), poi https://juggl.dev/p/y-7e/ e https://altro.it/p/y, di nuovo https://juggl.dev/p/bozza-a1b2c3/ e https://juggl.dev/ e https://share.juggl.dev/";
        assert_eq!(page_links(t), vec!["https://share.juggl.dev/bozza-a1b2c3/", "https://share.juggl.dev/x-9f/", "https://share.juggl.dev/y-7e/"]);
        assert_eq!(page_link_new("https://altro.it/p/y"), "https://altro.it/p/y");
        for l in ["https://juggl.dev/p/nessuna-registrata-ff00/", "https://share.juggl.dev/nessuna-registrata-ff00/"] {
            let v = page_card(l);
            assert_eq!(v["tipo"], "pagina"); assert_eq!(v["scaduta"], false);
            assert_eq!(v["link"], "https://share.juggl.dev/nessuna-registrata-ff00/");
            assert!(v["nome"].as_str().unwrap().starts_with("nessuna-registrata"));
        }
    }

    #[test]
    fn csv_and_reading() {
        let (r, _) = csv_rows("nome;prezzo\n\"Rossi; Mario\";10\n\"Detto \"\"ok\"\"\";2\n", false);
        assert_eq!(r, vec![vec!["nome", "prezzo"], vec!["Rossi; Mario", "10"], vec!["Detto \"ok\"", "2"]]);
        let d = test_dir("leggi");
        let f = d.join("x.json");
        fs::write(&f, "{\"a\":1}").unwrap();
        let v = read_document(&f.to_string_lossy(), 1000);
        assert_eq!(v["tipo"], "json");
        assert!(v["testo"].as_str().unwrap().contains("\n"));
        let latin = d.join("l.csv");
        fs::write(&latin, [b'c', b'a', b'f', 0xe8, b'\n']).unwrap();
        assert_eq!(read_document(&latin.to_string_lossy(), 1000)["testo"], "cafè\n");
        let long_file = d.join("lungo.txt");
        fs::write(&long_file, "riga\n".repeat(1000)).unwrap();
        let v = read_document(&long_file.to_string_lossy(), 1000);
        assert_eq!(v["troncato"], true);
        assert!(v["testo"].as_str().unwrap().len() <= 1000);
        let _ = fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod real_test {
    /// Content for the viewer of the files in a folder: DOC_DIR=... cargo test --release save_readings -- --ignored
    #[test]
    #[ignore]
    fn save_readings() {
        let d = std::env::var("DOC_DIR").expect("DOC_DIR");
        let mut all_files = serde_json::Map::new();
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path().to_string_lossy().to_string();
            if p.ends_with(".json.out") { continue; }
            all_files.insert(p.clone(), super::read_document(&p, super::MAC_LIMIT));
        }
        std::fs::write(format!("{d}/letture.json.out"), serde_json::to_string(&all_files).unwrap()).unwrap();
    }

    /// Cards found in real sessions: cargo test --release real_test -- --ignored --nocapture
    #[test]
    #[ignore]
    fn print_cards() {
        for v in crate::list_sessions().into_iter().filter(|v| crate::str_of(v, "vista") != "ibernata") {
            let t = std::time::Instant::now();
            let d = crate::session_detail_pub(&v);
            let ms = t.elapsed().as_millis();
            let mut n = 0;
            for e in d["eventi"].as_array().into_iter().flatten() {
                for x in e["documenti"].as_array().into_iter().flatten() {
                    n += 1;
                    if n <= 6 { println!("  {:<11} {:<9} {:<34} {}", crate::str_of(x, "azione"), crate::str_of(x, "tipo"), crate::str_of(x, "cartella"), crate::str_of(x, "nome")); }
                }
            }
            println!("{} · {} schede · {} ms", crate::str_of(&v, "nome"), n, ms);
        }
    }
}

#[cfg(test)]
mod your_attachments_tests {
    use serde_json::json;
    #[test]
    fn card_under_your_bubble() {
        let dir = std::env::temp_dir().join("prova-allegati-tuoi");
        let _ = std::fs::create_dir_all(&dir);
        let pdf = dir.join("1794254161-preventivo.pdf");
        let zip = dir.join("cartella.zip");
        std::fs::write(&pdf, b"%PDF-1.4 prova").unwrap();
        std::fs::write(&zip, b"PK prova").unwrap();
        let t = format!("guarda questi\n\nAllegato: {}\nAllegato: {}\nAllegato: /x/foto.jpg", pdf.display(), zip.display());
        let mut e = json!({"tipo": "tu", "testo": t});
        super::your_attachments(&mut e);
        let d = e["documenti"].as_array().unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0]["nome"], json!("preventivo.pdf"));
        assert_eq!(d[0]["tipo"], json!("pdf"));
        assert_eq!(d[1]["tipo"], json!("altro"));
        assert!(super::is_allowed(&pdf.to_string_lossy()));
    }
}
