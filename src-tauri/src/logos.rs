// Project icons and colors (list Nov 15, the user's request: "keep the icons; Juggl should set them as soon as possible for
// every project, automatically; a customizable color").
// - Automatic icon: for every project without a logo (or whose file disappeared) it tries, in order:
//   1) the project's files (import::trova_logo: apple-touch-icon, icon.png, favicon.png...);
//   2) ONLY if the "Search project logos online" setting is on (loghi_online, OFF by default, coordinator Nov 15) and
//      only on macOS (system curl; elsewhere the initials stay): the project's website ("sito" field, or CNAME,
//      package.json "homepage", SITE_URL / NEXT_PUBLIC_SITE_URL in .env*). From the home page apple-touch-icon or the
//      biggest icon, then /apple-touch-icon.png and /favicon.ico. Careful networking: https only, 5 s timeout, -q (no ~/.curlrc),
//      no cookies or .netrc, generic user agent, redirects followed by hand only on the same domain (or www.), at most 3.
//      Real images only (png, jpg, gif, webp, ico; no svg, which can contain code), at most 512 KB, saved in
//      base/loghi. Nothing is executed or interpreted: only the <link> tags of the page are read.
//   If nothing is found the list draws the initials in the project color. At most one attempt every 24 hours per project
//   ("logo_provato"); logos set by hand ("logo_auto" missing) are never replaced.
// - Color: "colore" in progetti.json (e.g. "#3b5b7a"); if missing the list picks it from the project name (same rule on
//   Mac and iPhone, see NOTE-MERGE-sidebar.md). Only sober palette colors: never gold, never orange (it belongs to "ti aspetta").
// Commands: progetto_colore, progetto_logo_sito, progetto_logo_iniziali. Startup: logos::avvia() from setup (main.rs).
use crate::{storage, base_dir, f64_of, session_care, load_json, unix_now, str_of};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Sober palette (white text on top at least 4.5:1). No gold, no orange.
pub const PALETTE: [&str; 8] = ["#475569", "#3b5b7a", "#2f6b66", "#55663a", "#6a4c7a", "#8a4545", "#3e4a8c", "#5e5148"];
const RETRY_S: f64 = 86400.0;
const MAX_BYTES: u64 = 512 * 1024;
const MAX_PAGE: &str = "1048576";

pub fn start() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(20));
        loop {
            let _ = std::panic::catch_unwind(fill_missing);
            std::thread::sleep(Duration::from_secs(1800));
        }
    });
}

fn projects_file() -> PathBuf { base_dir().join("progetti.json") }

/// "Search project logos online": off if missing (new users), the user turns it on from the icon panel.
fn online() -> bool { session_care::settings().get("loghi_online").and_then(|x| x.as_bool()).unwrap_or(false) }

#[tauri::command]
pub fn loghi_online_imposta(acceso: bool) -> Value {
    let r = storage::modify(&base_dir().join("impostazioni.json"), Some(json!({})), |d| { d["loghi_online"] = json!(acceso); true });
    match r { Ok(_) => json!({"ok": true, "acceso": acceso}), Err(e) => json!({"ok": false, "msg": e}) }
}

/// One pass: looks for an icon for the projects that have none. Web lookups stay outside the file lock.
pub fn fill_missing() {
    let Some(registry) = load_json(&projects_file()) else { return };
    let projects = registry.get("progetti").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    for p in projects {
        let id = str_of(&p, "id").to_string();
        if id.is_empty() || id == "locale" { continue; }
        let logo = session_care::expand_path(str_of(&p, "logo"));
        if !logo.is_empty() && Path::new(&logo).is_file() { continue; }
        if unix_now() - f64_of(&p, "logo_provato") < RETRY_S { continue; }
        let folder = PathBuf::from(session_care::expand_path(str_of(&p, "cartella")));
        let mut found: Option<(String, &str)> = None;
        if folder.is_dir() { if let Some(l) = crate::import::find_logo(&folder) { found = Some((l, "file del progetto")); } }
        // "do not search online" for this project (field cerca_online: false, from the panel)
        if found.is_none() && online() && p.get("cerca_online").and_then(|x| x.as_bool()) != Some(false) {
            let site = if str_of(&p, "sito").is_empty() { find_site(&folder) } else { Some(str_of(&p, "sito").to_string()) };
            if let Some(u) = site { if let Some(l) = from_site(&id, &u) { found = Some((l, "sito")); } }
        }
        let _ = storage::modify(&projects_file(), Some(json!({"progetti": []})), |registry| {
            let Some(l) = registry["progetti"].as_array_mut() else { return false };
            let Some(x) = l.iter_mut().find(|x| str_of(x, "id") == id) else { return false };
            // meanwhile the user set it by hand: touch nothing
            let current_logo = session_care::expand_path(str_of(x, "logo"));
            if !current_logo.is_empty() && Path::new(&current_logo).is_file() { return false; }
            x["logo_provato"] = json!(unix_now());
            if let Some((l, source)) = &found { x["logo"] = json!(l); x["logo_auto"] = json!(source); }
            true
        });
        if let Some((_, source)) = &found { crate::log_juggl(&format!("loghi: {id} ha un'icona (dal {source})")); }
    }
}

/// Website address, if the project folder says it.
fn find_site(dir: &Path) -> Option<String> {
    if !dir.is_dir() { return None; }
    for c in ["CNAME", "public/CNAME", "static/CNAME"] {
        if let Ok(t) = fs::read_to_string(dir.join(c)) { let d = t.trim(); if d.contains('.') && !d.contains(' ') { return Some(format!("https://{d}")); } }
    }
    if let Some(h) = load_json(&dir.join("package.json")).and_then(|v| v.get("homepage").and_then(|x| x.as_str()).map(String::from)) {
        if h.starts_with("http") && !h.contains("github.com") { return Some(h); }
    }
    for e in [".env.production", ".env", ".env.local", ".env.example"] {
        let Ok(t) = fs::read_to_string(dir.join(e)) else { continue };
        for r in t.lines() {
            let r = r.trim();
            for k in ["NEXT_PUBLIC_SITE_URL=", "SITE_URL=", "PUBLIC_SITE_URL=", "NEXT_PUBLIC_BASE_URL=", "APP_URL="] {
                if let Some(v) = r.strip_prefix(k) {
                    let v = v.trim().trim_matches(['"', '\'']);
                    if v.starts_with("https://") && !v.contains("localhost") && !v.contains("127.0.0.1") { return Some(v.to_string()); }
                }
            }
        }
    }
    None
}

/// Host of an https address ("https://www.x.it/a" -> "www.x.it"); None if it is not https.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn host(u: &str) -> Option<String> {
    let r = u.strip_prefix("https://")?;
    let h = r.split(['/', '?', '#']).next()?.split('@').last()?.split(':').next()?.to_lowercase();
    (!h.is_empty() && h.contains('.')).then_some(h)
}
/// Same site: equal, or with/without "www.".
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn same_site(a: &str, b: &str) -> bool { a.trim_start_matches("www.") == b.trim_start_matches("www.") }

/// A careful GET request with the system curl (macOS only). Redirects are followed by hand, only on the same site.
#[cfg(target_os = "macos")]
fn download(url: &str, max: &str, out_path: Option<&Path>) -> Option<Vec<u8>> {
    let origin = host(url)?;
    let mut u = url.to_string();
    for _ in 0..4 {
        if !same_site(&host(&u)?, &origin) { return None; }
        let mut c = crate::new_command("/usr/bin/curl");
        // -q first: ignores ~/.curlrc. No -L, -b, -n: no automatic redirects, cookies or .netrc.
        c.args(["-q", "-sS", "--proto", "=https", "--max-time", "5", "--max-filesize", max, "-A", "Mozilla/5.0 (Macintosh)",
            "-w", "\n%{http_code} %{redirect_url}"]);
        let tmp = out_path.map(|p| p.with_extension("parziale"));
        if let Some(t) = &tmp { c.arg("-o").arg(t); }
        c.arg(&u);
        let o = c.output().ok()?;
        if !o.status.success() { if let Some(t) = &tmp { let _ = fs::remove_file(t); } return None; }
        let out = o.stdout;
        let end = out.iter().rposition(|&b| b == b'\n').unwrap_or(0);
        let tail = String::from_utf8_lossy(&out[end..]).trim().to_string();
        let mut parts = tail.splitn(2, ' ');
        let code: u32 = parts.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        let location = parts.next().unwrap_or("").trim().to_string();
        if (300..400).contains(&code) && !location.is_empty() { if let Some(t) = &tmp { let _ = fs::remove_file(t); } u = location; continue; }
        if code != 200 { if let Some(t) = &tmp { let _ = fs::remove_file(t); } return None; }
        if let (Some(t), Some(p)) = (&tmp, out_path) { fs::rename(t, p).ok()?; return Some(vec![]); }
        return Some(out[..end].to_vec());
    }
    None
}
#[cfg(not(target_os = "macos"))]
fn download(_url: &str, _max: &str, _out_path: Option<&Path>) -> Option<Vec<u8>> { None }   // Windows/Linux: the initials stay

/// Real image (png, jpg, gif, webp, ico, svg), not an error page.
fn image_kind(p: &Path) -> Option<&'static str> {
    let m = fs::metadata(p).ok()?;
    if m.len() < 200 || m.len() > MAX_BYTES { return None; }
    let b = fs::read(p).ok()?;
    let head = &b[..b.len().min(512)];
    if head.starts_with(b"\x89PNG") { return Some("png"); }
    if head.starts_with(b"\xff\xd8") { return Some("jpg"); }
    if head.starts_with(b"GIF8") { return Some("gif"); }
    if head.len() > 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" { return Some("webp"); }
    if head.starts_with(&[0, 0, 1, 0]) { return Some("ico"); }
    None   // no svg: it can contain code
}

/// From the site's home page: apple-touch-icon, then the biggest icon, then the usual paths.
pub fn from_site(id: &str, site: &str) -> Option<String> {
    let site = if site.starts_with("https://") { site.to_string() } else { format!("https://{}", site.trim_start_matches("http://")) };
    host(&site)?;
    let root = {
        let parts = site.splitn(4, '/').collect::<Vec<_>>();
        if parts.len() >= 3 { format!("{}//{}", parts[0], parts[2]) } else { site.clone() }
    };
    let mut candidates: Vec<(i64, String)> = vec![];
    if let Some(html) = download(&site, MAX_PAGE, None) {
        let h = String::from_utf8_lossy(&html[..html.len().min(400_000)]).to_string();
        for tag in h.split("<link").skip(1) {
            let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
            let low = tag.to_lowercase();
            let rel = attr(&low, "rel").unwrap_or_default();
            if !rel.contains("icon") { continue; }
            let Some(href) = attr(tag, "href") else { continue };
            let side: i64 = attr(&low, "sizes").and_then(|z| z.split(['x', ' ']).next().and_then(|n| n.parse().ok())).unwrap_or(0);
            if href.to_lowercase().ends_with(".svg") || href.starts_with("data:") { continue; }
            let score = if rel.contains("apple-touch-icon") { 10_000 } else { 0 } + side;
            candidates.push((score, absolute_url(&root, &site, &href)));
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.push((0, format!("{root}/apple-touch-icon.png")));
    candidates.push((0, format!("{root}/favicon.ico")));
    let dir = base_dir().join("loghi");
    let _ = fs::create_dir_all(&dir);
    let tmp = dir.join(format!(".{id}.scarico"));
    for (_, u) in candidates.into_iter().take(6) {
        let _ = fs::remove_file(&tmp);
        if download(&u, &MAX_BYTES.to_string(), Some(&tmp)).is_none() { continue; }
        if let Some(ext) = image_kind(&tmp) {
            let dest = dir.join(format!("{id}.{ext}"));
            if fs::rename(&tmp, &dest).is_ok() { return Some(dest.to_string_lossy().to_string()); }
        }
    }
    let _ = fs::remove_file(&tmp);
    None
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let low = tag.to_lowercase();
    let i = low.find(&format!("{name}="))? + name.len() + 1;
    let r = &tag[i..];
    let (q, r) = match r.chars().next()? { '"' => ('"', &r[1..]), '\'' => ('\'', &r[1..]), _ => (' ', r) };
    let end = r.find(q).unwrap_or(r.len());
    Some(r[..end].trim().to_string())
}

fn absolute_url(root: &str, pagina: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") { return href.to_string(); }
    if let Some(r) = href.strip_prefix("//") { return format!("https://{r}"); }
    if href.starts_with('/') { return format!("{root}{href}"); }
    let dir = pagina.rsplit_once('/').map(|x| x.0).filter(|c| c.len() > root.len()).unwrap_or(root);
    format!("{dir}/{href}")
}

fn edit_project(pid: &str, change: impl FnOnce(&mut Value)) -> Value {
    let mut done = false;
    let r = storage::modify(&projects_file(), Some(json!({"progetti": []})), |registry| {
        let Some(l) = registry["progetti"].as_array_mut() else { return false };
        let Some(x) = l.iter_mut().find(|x| str_of(x, "id") == pid) else { return false };
        change(x); done = true; true
    });
    match r { Ok(_) if done => json!({"ok": true}), Ok(_) => json!({"ok": false, "msg": "progetto non trovato"}), Err(e) => json!({"ok": false, "msg": e}) }
}

/// Project color: one from the palette, or "" to go back to the automatic one.
#[tauri::command]
pub fn progetto_colore(pid: String, colore: String) -> Value {
    if !colore.is_empty() && !PALETTE.contains(&colore.as_str()) { return json!({"ok": false, "msg": "colore non previsto"}); }
    edit_project(&pid, |x| { if colore.is_empty() { if let Some(o) = x.as_object_mut() { o.remove("colore"); } } else { x["colore"] = json!(colore); } })
}

/// Icon taken from the site the user gives (remembered in "sito"). Downloads right away.
#[tauri::command(async)]
pub fn progetto_logo_sito(pid: String, sito: String) -> Value {
    let sito = sito.trim().to_string();
    if sito.is_empty() || sito.contains(' ') || !sito.contains('.') { return json!({"ok": false, "msg": "indirizzo non valido"}); }
    match from_site(&pid, &sito) {
        Some(l) => edit_project(&pid, |x| { x["logo"] = json!(l); x["logo_auto"] = json!("sito"); x["sito"] = json!(sito); x["logo_provato"] = json!(unix_now()); }),
        None => json!({"ok": false, "msg": "nessuna icona trovata su quel sito"}),
    }
}

/// Excludes (or re-admits) a project from the online logo search, for those who keep the general switch on.
#[tauri::command]
pub fn progetto_cerca_online(pid: String, si: bool) -> Value {
    edit_project(&pid, |x| { if si { if let Some(o) = x.as_object_mut() { o.remove("cerca_online"); } } else { x["cerca_online"] = json!(false); } })
}

/// No logo: the list draws the initials in the project color. Does not retry by itself for 30 days.
#[tauri::command]
pub fn progetto_logo_iniziali(pid: String) -> Value {
    edit_project(&pid, |x| { x["logo"] = json!(""); if let Some(o) = x.as_object_mut() { o.remove("logo_auto"); } x["logo_provato"] = json!(unix_now() + 29.0 * 86400.0); })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attributes_and_urls() {
        assert_eq!(attr(r#" rel="apple-touch-icon" href="/a.png" sizes="180x180""#, "href").as_deref(), Some("/a.png"));
        assert_eq!(attr(" rel=icon href=/f.svg", "href").as_deref(), Some("/f.svg"));
        assert_eq!(absolute_url("https://x.it", "https://x.it/", "/a.png"), "https://x.it/a.png");
        assert_eq!(absolute_url("https://x.it", "https://x.it/", "//cdn.y.it/a.png"), "https://cdn.y.it/a.png");
        assert_eq!(absolute_url("https://x.it", "https://x.it/blog/pagina", "img/a.png"), "https://x.it/blog/img/a.png");
    }
    #[test]
    fn same_site_and_host() {
        assert_eq!(host("https://www.x.it/a?b").as_deref(), Some("www.x.it"));
        assert_eq!(host("http://x.it"), None);
        assert!(same_site("www.x.it", "x.it"));
        assert!(!same_site("x.it.evil.com", "x.it"));
    }
    #[test]
    fn palette_has_no_gold_or_orange() {
        for c in PALETTE {
            let n = u32::from_str_radix(&c[1..], 16).unwrap();
            let (r, g, b) = ((n >> 16) & 255, (n >> 8) & 255, n & 255);
            assert!(!(r > g && g > b + 30 && r > 150), "{c} tende all'oro o all'arancio");
            assert!(r.max(g).max(b) < 160, "{c} troppo chiaro per il testo bianco");
        }
    }
}
