// Media ribbon: the images and videos all sessions are working on, for the strip at the top of the app
// (and for the iPhone, /api/media-recenti). It reads the tail of the transcripts (and of recent sub-agents) only once:
// then, on each round, only the added bytes. Thumbnails are made by sips (images) or qlmanage (videos), in a cache.
use crate::{home, tool_media, unix_now, media_paths, str_of, list_sessions, find_transcript};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

const TYPES: [&str; 8] = ["png", "jpg", "jpeg", "webp", "gif", "heic", "mp4", "mov"];
const TAIL: u64 = 1_500_000;          // how much I read, at most, of a transcript's tail
const TAIL_SUB: u64 = 400_000;      // and of a sub-agent
const MIN_BYTES: u64 = 8 * 1024;       // below this: icons, not media
const PER_SESSION: usize = 12;       // a session that generates a hundred images does not take the whole ribbon
const TOTAL: usize = 40;
const SIDE: &str = "480";             // long side of the thumbnails (the ribbon shows them up to 80 px high, Retina screen)

#[derive(Clone)]
struct Item { path: String, t: String }

/// For each transcript: how far I have read it and the media found.
struct ReadState { fine: u64, voci: Vec<Item> }

fn read_states() -> &'static Mutex<HashMap<PathBuf, ReadState>> {
    static L: OnceLock<Mutex<HashMap<PathBuf, ReadState>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_dir() -> PathBuf { home().join("Library/Caches/juggl-miniature") }

fn kind_of(p: &str) -> Option<&'static str> {
    let e = p.rsplit('.').next()?.to_lowercase();
    if !p.contains('.') || !TYPES.contains(&e.as_str()) { return None; }
    Some(if matches!(e.as_str(), "mp4" | "mov") { "video" } else { "immagine" })
}

/// Quick filter before reading the JSON: the line names at least one media file (base64 contains no dots).
fn maybe_media(r: &str) -> bool {
    TYPES.iter().any(|e| r.contains(&format!(".{e}")) || r.contains(&format!(".{}", e.to_uppercase())))
}

fn result_texts(x: &Value, into: &mut Vec<String>) {
    match x.get("content") {
        Some(Value::String(t)) => into.extend(media_paths(t)),
        Some(Value::Array(a)) => for y in a { if str_of(y, "type") == "text" { into.extend(media_paths(str_of(y, "text"))); } },
        _ => {}
    }
}

/// Media named in a transcript line: files read or written by the tools (same rule as the
/// conversation, `tool_media`), paths in commands, in results, in answers and in "Allegato:" lines.
fn media_of_line(r: &str, out: &mut Vec<Item>) {
    if !maybe_media(r) { return; }
    let Ok(d) = serde_json::from_str::<Value>(r) else { return };
    let t = str_of(&d, "timestamp").to_string();
    let mut found: Vec<String> = vec![];
    match d.get("message").and_then(|m| m.get("content")) {
        Some(Value::String(x)) => found.extend(media_paths(x)),
        Some(Value::Array(a)) => for x in a {
            match str_of(x, "type") {
                "text" => found.extend(media_paths(str_of(x, "text"))),
                "tool_use" => {
                    let name = str_of(x, "name");
                    let input = x.get("input").cloned().unwrap_or(json!({}));
                    found.extend(tool_media(name, &input));
                    // other tools (MCP, generators, mostra...): paths in their parameters
                    if !matches!(name, "Read" | "Write" | "Bash" | "bash" | "Edit") { found.extend(media_paths(&input.to_string())); }
                }
                "tool_result" => result_texts(x, &mut found),
                _ => {}
            }
        },
        // transcripts of other agents (Codex and similar): search the whole line
        None => found.extend(media_paths(r)),
        _ => {}
    }
    for p in found { if kind_of(&p).is_some() { out.push(Item { path: p, t: t.clone() }); } }
}

/// Media of a transcript: the first time the tail (at most `how_much` bytes), then only the new lines.
fn media_of_file(p: &Path, how_much: u64) -> Vec<Item> {
    let Ok(md) = fs::metadata(p) else { return vec![] };
    let n = md.len();
    let mut l = read_states().lock().unwrap_or_else(|e| e.into_inner());
    let (start_at, mut voci, first) = match l.remove(p) {
        Some(x) if x.fine <= n => (x.fine, x.voci, false),
        _ => (n.saturating_sub(how_much), vec![], true),
    };
    let mut fine = start_at;
    if n > start_at {
        // trial (Nov 16): in chunks, huge lines (embedded images) skipped; complete lines only
        fine = lines_in_chunks(p, start_at, n, first && start_at > 0, |r| media_of_line(r, &mut voci));
    }
    let k = voci.len();
    if k > 300 { voci.drain(..k - 300); }
    l.insert(p.to_path_buf(), ReadState { fine, voci: voci.clone() });
    voci
}

/// Deep read for the ribbon's "load more": the whole transcript (at most DEEP bytes from the end),
/// in its own cache (redone only if the file grew), so the quick read of each round does not change.
const DEEP: u64 = 40_000_000;
/// trial (Nov 16): complete lines of the file between `start_at` and `n`, read in 4 MB chunks (never all in memory). Lines
/// longer than 512 KB (images embedded in base64) are skipped without reading them as text. `skip_first`: we start
/// in the middle of a line. Returns where to resume (after the last complete line).
const CHUNK: usize = 4 * 1024 * 1024;
const LINE_MAX: usize = 512 * 1024;
fn lines_in_chunks(p: &Path, start_at: u64, n: u64, skip_first: bool, mut f: impl FnMut(&str)) -> u64 {
    let Ok(mut fh) = fs::File::open(p) else { return start_at };
    if fh.seek(SeekFrom::Start(start_at)).is_err() { return start_at; }
    let mut fine = start_at;            // after the last complete line seen
    let mut read_upto = start_at;           // how far I have read the file
    let mut rest: Vec<u8> = vec![];
    let mut skipping = skip_first;  // inside a line not to be read (cut or too long)
    let mut buf = vec![0u8; CHUNK];
    while read_upto < n {
        let how_much = CHUNK.min((n - read_upto) as usize);
        let Ok(k) = fh.read(&mut buf[..how_much]) else { break };
        if k == 0 { break; }
        read_upto += k as u64;
        let mut i = 0;
        while i < k {
            match buf[i..k].iter().position(|&c| c == b'\n') {
                Some(j) => {
                    if !skipping && rest.len() + j <= LINE_MAX {
                        rest.extend_from_slice(&buf[i..i + j]);
                        f(&String::from_utf8_lossy(&rest));
                    }
                    rest.clear(); skipping = false;
                    i += j + 1;
                    fine = read_upto - (k - i) as u64;
                }
                None => {
                    if !skipping { rest.extend_from_slice(&buf[i..k]); if rest.len() > LINE_MAX { rest.clear(); skipping = true; } }
                    i = k;
                }
            }
        }
    }
    fine
}

fn deep_media(p: &Path) -> Vec<Item> {
    static C: OnceLock<Mutex<HashMap<PathBuf, ReadState>>> = OnceLock::new();
    let c = C.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(md) = fs::metadata(p) else { return vec![] };
    let n = md.len();
    let mut l = c.lock().unwrap_or_else(|e| e.into_inner());
    // the first time everything (up to DEEP from the end), then only the added lines
    let (start_at, mut voci, first) = match l.remove(p) {
        Some(x) if x.fine <= n => (x.fine, x.voci, false),
        _ => (n.saturating_sub(DEEP), vec![], true),
    };
    let mut fine = start_at;
    if n > start_at {
        // trial (Nov 16): it used to read up to 40 MB in one go and copy them (a chat with 396 embedded images:
        // ~80 MB of peak for nothing, those lines have no paths). Now in 4 MB chunks, huge lines skipped.
        fine = lines_in_chunks(p, start_at, n, first && start_at > 0, |r| media_of_line(r, &mut voci));
    }
    let k = voci.len();
    if k > 3000 { voci.drain(..k - 3000); }
    l.insert(p.to_path_buf(), ReadState { fine, voci: voci.clone() });
    voci
}

/// Session transcript plus those of the sub-agents touched in the last 6 hours (at most 4).
fn session_items(v: &Value) -> Vec<Item> {
    let Some(p) = find_transcript(v) else { return vec![] };
    let mut all_items = media_of_file(&p, TAIL);
    let dir = p.with_extension("").join("subagents");
    let mut subs: Vec<(f64, PathBuf)> = fs::read_dir(&dir).into_iter().flatten().flatten().filter_map(|e| {
        let q = e.path();
        if q.extension().and_then(|x| x.to_str()) != Some("jsonl") { return None; }
        let t = e.metadata().ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs_f64();
        if unix_now() - t > 6.0 * 3600.0 { return None; }
        Some((t, q))
    }).collect();
    subs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (_, q) in subs.into_iter().take(4) { all_items.extend(media_of_file(&q, TAIL_SUB)); }
    all_items
}

// ---------- thumbnails ----------

fn cache_key(p: &str, md: &fs::Metadata) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    SIDE.hash(&mut h);   // changing the side, they are redone
    md.len().hash(&mut h);
    md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0).hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Launches a command and waits for it at most `max` (qlmanage sometimes hangs).
fn run_with_timeout(c: &mut Command, max: Duration) -> bool {
    // trial (Nov 16): its own process group, so on timeout everything it launched is closed
    #[cfg(unix)]
    { use std::os::unix::process::CommandExt; c.process_group(0); }
    let Ok(mut child) = c.stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) if start.elapsed() < max => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                #[cfg(unix)]
                unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL); }
                let _ = child.kill(); let _ = child.wait(); return false;
            }
        }
    }
}

fn with_qlmanage(p: &str, k: &str, dir: &Path, png: &Path) -> bool {
    let tmp = dir.join(format!("ql-{k}"));
    let _ = fs::create_dir_all(&tmp);
    run_with_timeout(crate::new_command("/usr/bin/qlmanage").args(["-t", "-s", SIDE, "-o"]).arg(&tmp).arg(p), Duration::from_secs(12));
    let name = format!("{}.png", Path::new(p).file_name().and_then(|x| x.to_str()).unwrap_or(""));
    let ok = fs::rename(tmp.join(name), png).is_ok();
    let _ = fs::remove_dir_all(&tmp);
    ok
}

/// Cached thumbnail (long side SIDE px): images with sips, videos (and images sips cannot read) with qlmanage.
fn thumbnail(p: &str, md: &fs::Metadata, video: bool) -> Option<PathBuf> {
    let dir = cache_dir();
    let k = cache_key(p, md);
    let jpg = dir.join(format!("{k}.jpg"));
    let png = dir.join(format!("{k}.png"));
    if jpg.exists() { return Some(jpg); }
    if png.exists() { return Some(png); }
    if no_mark(&dir, &k).exists() { return None; }   // trial (Nov 16): already tried, it cannot be read
    let _ = fs::create_dir_all(&dir);
    if !video {
        let part = dir.join(format!("{k}.parte.jpg"));
        let ok = run_with_timeout(crate::new_command("/usr/bin/sips").args(["-Z", SIDE, "-s", "format", "jpeg", "-s", "formatOptions", "75", p, "--out"]).arg(&part), Duration::from_secs(10));
        if ok && fs::rename(&part, &jpg).is_ok() { return Some(jpg); }
        let _ = fs::remove_file(&part);
    }
    if with_qlmanage(p, &k, &dir, &png) { return Some(png); }
    // trial (Nov 16): neither sips nor qlmanage (corrupted file, e.g. an mp4 without index): mark it in the cache and do not retry
    // until the file changes (the key contains size and date). Before, it retried on every round, 12 s each.
    let _ = fs::write(no_mark(&dir, &k), b"");
    None
}

fn no_mark(dir: &Path, k: &str) -> PathBuf { dir.join(format!("{k}.no")) }

/// trial (Nov 16): the thumbnail if already cached; None also if the file is marked as unreadable.
/// The second value says whether it still has to be made (it is not there and not marked).
fn cached_thumbnail(p: &str, md: &fs::Metadata) -> (Option<PathBuf>, bool) {
    let dir = cache_dir();
    let k = cache_key(p, md);
    for e in ["jpg", "png"] { let f = dir.join(format!("{k}.{e}")); if f.exists() { return (Some(f), false); } }
    (None, !no_mark(&dir, &k).exists())
}

// trial (Nov 16): new thumbnails are made by ONE separate thread, outside the round's lock. Before, the round
// did it itself, holding the lock: a file that made qlmanage hang stopped every ribbon request for 12 s per round.
// When the queue empties it tells the app ("cambiato"): the ribbon reloads and the thumbnails appear.
type Job = (String, fs::Metadata, bool);
fn thumbnail_queue() -> &'static Mutex<(std::collections::VecDeque<Job>, HashSet<String>, bool)> {
    static C: OnceLock<Mutex<(std::collections::VecDeque<Job>, HashSet<String>, bool)>> = OnceLock::new();
    C.get_or_init(|| Mutex::new((Default::default(), HashSet::new(), false)))
}
fn enqueue_thumbnail(p: &str, md: &fs::Metadata, video: bool) {
    let start_thread = {
        let mut c = thumbnail_queue().lock().unwrap_or_else(|e| e.into_inner());
        if !c.1.insert(p.to_string()) { return; }
        c.0.push_back((p.to_string(), md.clone(), video));
        if c.2 { false } else { c.2 = true; true }
    };
    if !start_thread { return; }
    std::thread::spawn(|| {
        let mut done = 0usize;
        loop {
            let job = {
                let mut c = thumbnail_queue().lock().unwrap_or_else(|e| e.into_inner());
                match c.0.pop_front() { Some(l) => Some(l), None => { c.2 = false; None } }
            };
            let Some((p, md, video)) = job else { break };
            if thumbnail(&p, &md, video).is_some() { done += 1; }
            thumbnail_queue().lock().unwrap_or_else(|e| e.into_inner()).1.remove(&p);
            if done > 0 && done % 12 == 0 { notify_app(); }   // long queue: the first ones appear without waiting for the end
        }
        if done > 0 { notify_app(); }
    });
}
fn notify_app() { if let Some(app) = crate::APP.get() { use tauri::Emitter; let _ = app.emit("cambiato", ()); } }

/// trial (Nov 16): qlmanage and sips left orphaned by a previous run of Juggl (hung on a corrupted file,
/// app closed in the meantime): they lived for days. Only ours: parent 1 and our cache folder in the arguments.
pub fn close_orphans() -> usize {
    let cache = cache_dir().to_string_lossy().to_string();
    let Ok(o) = crate::new_command("/bin/ps").args(["-axo", "pid=,ppid=,command="]).output() else { return 0 };
    let mut n = 0;
    for r in String::from_utf8_lossy(&o.stdout).lines() {
        let mut c = r.split_whitespace();
        let (Some(pid), Some(ppid)) = (c.next().and_then(|x| x.parse::<i32>().ok()), c.next()) else { continue };
        let cmd: Vec<&str> = c.collect();
        let cmd = cmd.join(" ");
        if ppid != "1" || !(cmd.starts_with("/usr/bin/qlmanage ") || cmd.starts_with("/usr/bin/sips ")) || !cmd.contains(&cache) { continue; }
        #[cfg(unix)]
        unsafe { libc::kill(pid, libc::SIGKILL); }
        n += 1;
    }
    if n > 0 { crate::log_juggl(&format!("nastro: chiusi {n} programmi delle miniature rimasti orfani")); }
    n
}

/// Width and height of a thumbnail (PNG or JPEG), reading only the header.
fn dimensions(p: &Path) -> Option<(u32, u32)> {
    let mut b = vec![0u8; 65536];
    let n = fs::File::open(p).ok()?.read(&mut b).ok()?;
    b.truncate(n);
    let be16 = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]) as u32;
    if b.len() > 24 && &b[1..4] == b"PNG" {
        return Some((u32::from_be_bytes([b[16], b[17], b[18], b[19]]), u32::from_be_bytes([b[20], b[21], b[22], b[23]])));
    }
    if b.len() > 4 && b[0] == 0xFF && b[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF { i += 1; continue; }
            let m = b[i + 1];
            if m == 0xFF { i += 1; continue; }
            if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) { i += 2; continue; }
            if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) { return Some((be16(i + 7), be16(i + 5))); }
            i += 2 + be16(i + 2) as usize;
        }
    }
    None
}

/// Once an hour: away with thumbnails unused for 14 days.
fn clean_cache() {
    static LAST: Mutex<f64> = Mutex::new(0.0);
    let mut u = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if unix_now() - *u < 3600.0 { return; }
    *u = unix_now();
    for e in fs::read_dir(cache_dir()).into_iter().flatten().flatten() {
        let stale = e.metadata().ok().and_then(|m| m.accessed().or(m.modified()).ok()).and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs_f64() > 14.0 * 86400.0).unwrap_or(false);
        if stale { let p = e.path(); if p.is_dir() { let _ = fs::remove_dir_all(p); } else { let _ = fs::remove_file(p); } }
    }
}

// ---------- command ----------

/// Latest media (at most 40, most recent first) of all non-hibernated sessions.
pub fn recent_media() -> Value { recent_media_n(TOTAL) }

/// The most recent media up to `total` (the ribbon asks for more when you reach the bottom: 40, 80, 120...).
/// The per-session cap grows with the total, so scrolling also reveals sessions that have many.
pub fn recent_media_n(total: usize) -> Value {
    let total = total.clamp(TOTAL, 600);
    let per_session = PER_SESSION * total / TOTAL;
    static ROUND: Mutex<()> = Mutex::new(());
    let _one = ROUND.lock().unwrap_or_else(|e| e.into_inner());   // two requests together do not make the thumbnails twice
    // "load more" (beyond the base 40): also hibernated sessions and transcripts read in full
    let deep = total > TOTAL;
    let ss: Vec<Value> = list_sessions().into_iter().filter(|v| deep || str_of(v, "vista") != "ibernata").collect();
    let logos: HashSet<String> = ss.iter().filter_map(|v| v["progetto"]["logo"].as_str().map(String::from)).collect();
    let cache = cache_dir().to_string_lossy().to_string();
    // for each file the most recent time it appeared, and in which session
    let mut latest: HashMap<String, (String, usize)> = HashMap::new();
    let mut live: HashSet<PathBuf> = HashSet::new();
    for (i, v) in ss.iter().enumerate() {
        if let Some(p) = find_transcript(v) { live.insert(p.clone()); live.extend(fs::read_dir(p.with_extension("").join("subagents")).into_iter().flatten().flatten().map(|e| e.path())); }
        let mut voci = session_items(v);
        if deep { if let Some(p) = find_transcript(v) { voci.extend(deep_media(&p)); } }
        for x in voci {
            if logos.contains(&x.path) || x.path.starts_with(&cache) { continue; }
            if latest.get(&x.path).map(|(t, _)| *t < x.t).unwrap_or(true) { latest.insert(x.path, (x.t, i)); }
        }
    }
    // transcripts of closed sessions: away from memory
    read_states().lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| live.contains(k));
    let mut list_: Vec<(String, String, usize)> = latest.into_iter().map(|(p, (t, i))| (p, t, i)).collect();
    list_.sort_by(|a, b| b.1.cmp(&a.1));
    let mut per_count: HashMap<usize, usize> = HashMap::new();
    let mut chosen: Vec<(String, String, usize, fs::Metadata)> = vec![];
    for (p, t, i) in list_ {
        if chosen.len() >= total { break; }
        let Ok(md) = fs::metadata(&p) else { continue };
        if !md.is_file() || md.len() < MIN_BYTES { continue; }
        let n = per_count.entry(i).or_insert(0);
        if *n >= per_session { continue; }
        *n += 1;
        chosen.push((p, t, i, md));
    }
    // trial (Nov 16): only the thumbnails already cached; the new ones are made by the separate thread (the ribbon gets them next round)
    static ORPHANS: std::sync::Once = std::sync::Once::new();
    ORPHANS.call_once(|| { close_orphans(); });
    let state: Vec<(Option<PathBuf>, bool)> = chosen.iter().map(|(p, _, _, md)| {
        let (m, to_do) = cached_thumbnail(p, md);
        if to_do { enqueue_thumbnail(p, md, kind_of(p) == Some("video")); }
        let bad = m.is_none() && !to_do && !thumbnail_queue().lock().unwrap_or_else(|e| e.into_inner()).1.contains(p.as_str());
        (m, bad)
    }).collect();
    let broken: Vec<bool> = state.iter().map(|x| x.1).collect();
    let mini: Vec<Option<PathBuf>> = state.into_iter().map(|x| x.0).collect();
    clean_cache();
    let out: Vec<Value> = chosen.iter().zip(mini).zip(broken).map(|(((p, t, i, _), m), bad)| {
        let v = &ss[*i];
        let (w, h) = m.as_deref().and_then(dimensions).unwrap_or((0, 0));
        json!({"sessione": str_of(v, "sessione"), "nome": str_of(v, "nome"), "progetto": v["progetto"].clone(), "logo": v["progetto"]["logo"].clone(),
            "path": p, "mini": m.map(|x| x.to_string_lossy().to_string()), "w": w, "h": h, "t": t, "tipo": kind_of(p).unwrap_or("immagine"),
            "non_leggibile": bad})   // trial (Nov 16): neither sips nor qlmanage can read it (corrupted file)
    }).collect();
    json!(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// trial (Nov 16): ribbon timings on real data (outside the app): cargo test --release ribbon_timings -- --ignored --nocapture
    #[test]
    #[ignore]
    fn ribbon_timings() {
        for round in 0..3 {
            let t = std::time::Instant::now();
            let v = recent_media_n(TOTAL);
            let l = v.as_array().cloned().unwrap_or_default();
            let without = l.iter().filter(|m| m["mini"].is_null()).count();
            let broken = l.iter().filter(|m| m["non_leggibile"] == json!(true)).count();
            println!("TEMPI nastro giro {round}: {:?}, {} voci, senza miniatura {without}, non leggibili {broken}, in coda {}", t.elapsed(), l.len(), thumbnail_queue().lock().unwrap().0.len());
            let t = std::time::Instant::now();
            while thumbnail_queue().lock().unwrap().2 && t.elapsed() < Duration::from_secs(60) { std::thread::sleep(Duration::from_millis(100)); }
            println!("TEMPI miniature fatte in {:?}", t.elapsed());
        }
    }

    /// trial (Nov 16): lines in chunks: a line crossing two chunks read whole, a huge line skipped, last half line
    /// left for the next round, a start in the middle of a line skipped.
    #[test]
    fn lines_in_chunks_correct() {
        let d = std::env::temp_dir().join(format!("juggl-righe-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        let p = d.join("t.jsonl");
        let long_ok = "b".repeat(300_000);
        let mut t = String::new();
        t.push_str("primo\n");
        while t.len() < CHUNK - 100 { t.push_str(&"r".repeat(1000)); t.push('\n'); }
        t.push_str(&format!("{long_ok}\n"));            // crosses the 4 MB boundary
        t.push_str(&format!("{}\n", "x".repeat(LINE_MAX + 10)));   // huge: skipped
        t.push_str("ultima intera\n");
        t.push_str("a meta");
        fs::write(&p, &t).unwrap();
        let n = t.len() as u64;
        let mut seen: Vec<String> = vec![];
        let fine = lines_in_chunks(&p, 0, n, false, |r| seen.push(r.to_string()));
        assert_eq!(seen.first().map(|x| x.as_str()), Some("primo"));
        assert!(seen.iter().any(|x| x == &long_ok), "riga a cavallo dei pezzi letta intera");
        assert!(!seen.iter().any(|x| x.starts_with("xxx")), "riga enorme saltata");
        assert_eq!(seen.last().map(|x| x.as_str()), Some("ultima intera"));
        assert_eq!(fine, n - "a meta".len() as u64, "si riprende dalla riga a meta'");
        // start in the middle of a line: that one is skipped
        let mut v2: Vec<String> = vec![];
        lines_in_chunks(&p, 2, n, true, |r| v2.push(r.to_string()));
        assert_ne!(v2.first().map(|x| x.as_str()), Some("imo"));
        let _ = fs::remove_dir_all(&d);
    }

    /// trial (Nov 16): on timeout the whole group is closed, the command's children too.
    #[test]
    fn run_closes_the_group() {
        let t = std::time::Instant::now();
        let ok = run_with_timeout(crate::new_command("/bin/sh").args(["-c", "sleep 31.7 & sleep 31.7"]), Duration::from_millis(300));
        assert!(!ok && t.elapsed() < Duration::from_secs(5));
        // the killed child may linger for a moment to be reaped: I wait up to 3 s
        let live = || !String::from_utf8_lossy(&crate::new_command("/usr/bin/pgrep").args(["-f", "sleep 31.7"]).output().unwrap().stdout).trim().is_empty();
        let t = std::time::Instant::now();
        while live() && t.elapsed() < Duration::from_secs(3) { std::thread::sleep(Duration::from_millis(100)); }
        assert!(!live(), "figli rimasti vivi");
    }

    /// trial (Nov 16): a corrupted video is tried only once, then stays marked until the file changes.
    #[test]
    fn corrupted_video_marked_once() {
        let d = std::env::temp_dir().join(format!("juggl-provino-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        let p = d.join("rovinato.mp4");
        fs::write(&p, vec![7u8; 64 * 1024]).unwrap();
        let ps = p.to_string_lossy().to_string();
        let md = fs::metadata(&p).unwrap();
        assert!(thumbnail(&ps, &md, true).is_none());
        assert_eq!(cached_thumbnail(&ps, &md), (None, false), "segnato: non va rifatto");
        let t = std::time::Instant::now();
        assert!(thumbnail(&ps, &md, true).is_none());
        assert!(t.elapsed() < Duration::from_millis(50), "la seconda volta non rilancia niente");
        let _ = fs::remove_file(no_mark(&cache_dir(), &cache_key(&ps, &md)));
        let _ = fs::remove_dir_all(&d);
    }
    /// Prints the ribbon with real data: cargo test --release ribbon -- --ignored --nocapture
    #[test]
    #[ignore]
    fn count_ribbon() {
        for q in [40usize, 80, 120, 200, 400, 600] {
            let v = super::recent_media_n(q);
            println!("CONTA chiesti {q} -> {}", v.as_array().map(|a| a.len()).unwrap_or(0));
        }
    }
    #[test]
    #[ignore]
    fn print_ribbon() {
        let t = std::time::Instant::now();
        let v = super::recent_media();
        let first = t.elapsed();
        let t2 = std::time::Instant::now();
        let _ = super::recent_media();
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        println!("media: {} · primo giro {:?} · secondo giro {:?}", v.as_array().map(|a| a.len()).unwrap_or(0), first, t2.elapsed());
        if let Ok(d) = std::env::var("NASTRO_DUMP") {
            let _ = std::fs::write(format!("{d}/media.json"), v.to_string());
            let tv = crate::desk_data();
            for x in tv["sessioni"].as_array().into_iter().flatten() {
                println!("{:<22} vista={:<8} turno={:<8} sotto_attivi={}", crate::str_of(x, "nome"), crate::str_of(x, "vista"), crate::turn_view(x), x["sotto_attivi"]);
            }
            let _ = std::fs::write(format!("{d}/tavolo.json"), tv.to_string());
        }
    }
}
