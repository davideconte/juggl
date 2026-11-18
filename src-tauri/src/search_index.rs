// Full-text search index of all messages: SQLite FTS5 in ~/tavolo/indice.db.
// Inside: the user's messages and the agents' messages of all sessions (even closed and old ones), messages read during the
// turn, sub-agents and, with the personal features, the external messages of the enabled chats. Outside: base64, system-reminder, internal commands, long command results,
// the transcripts of the pilot and of requests (claude -p "You are the assistant of ...").
// Built the first time in the background and then updated by reading only the new lines (offset per file).
#[cfg(windows)] use crate::sys as libc;
use crate::{base_dir, home, str_of};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
#[cfg(feature = "personale")]
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

const VERSION: &str = "2";
const MAX_LINE: usize = 24 * 1024 * 1024;   // longer lines: only embedded images, skipped
const MAX_RESULT: usize = 1200;          // longer command results are not indexed

static BUILDING: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
static ROWS: AtomicU64 = AtomicU64::new(0);

fn db() -> &'static Mutex<Option<Connection>> { static D: OnceLock<Mutex<Option<Connection>>> = OnceLock::new(); D.get_or_init(|| Mutex::new(open_db())) }
/// One update at a time (the initial build, the periodic round, the one before a search).
fn updating() -> &'static Mutex<()> { static A: Mutex<()> = Mutex::new(()); &A }

fn open_db() -> Option<Connection> {
    let c = Connection::open(base_dir().join("indice.db")).ok()?;
    let _ = c.busy_timeout(Duration::from_secs(5));
    // secure_delete: deleted rows are zeroed on disk, they do not stay in free pages (local chats, 10/11)
    let _ = c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY; PRAGMA secure_delete=ON;");
    let v: String = c.query_row("SELECT v FROM meta WHERE k='versione'", [], |r| r.get(0)).unwrap_or_default();
    if v != VERSION {
        let _ = c.execute_batch("DROP TABLE IF EXISTS msg; DROP TABLE IF EXISTS file; DROP TABLE IF EXISTS sess; DROP TABLE IF EXISTS wa; DROP TABLE IF EXISTS meta;");
    }
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);
         CREATE TABLE IF NOT EXISTS file(id INTEGER PRIMARY KEY, path TEXT UNIQUE, off INTEGER, len INTEGER, mtime INTEGER, sessione TEXT, fonte TEXT);
         CREATE TABLE IF NOT EXISTS sess(sessione TEXT PRIMARY KEY, cwd TEXT, titolo TEXT, fonte TEXT, ultimo TEXT);
         CREATE TABLE IF NOT EXISTS wa(jid TEXT PRIMARY KEY, ultimo INTEGER);
         CREATE VIRTUAL TABLE IF NOT EXISTS msg USING fts5(testo, sessione UNINDEXED, t UNINDEXED, chi UNINDEXED, img UNINDEXED, f UNINDEXED,
             tokenize = 'unicode61 remove_diacritics 2');").ok()?;
    let _ = c.execute("INSERT OR REPLACE INTO meta(k, v) VALUES('versione', ?1)", [VERSION]);
    // the search index (FTS5) too really deletes: without this, removed text stays in the segments until the next merge
    let _ = c.execute("INSERT INTO msg(msg, rank) VALUES('secure-delete', 1)", []);
    READY.store(c.query_row("SELECT v FROM meta WHERE k='costruito'", [], |r| r.get::<_, String>(0)).is_ok(), Ordering::Relaxed);
    Some(c)
}

// ---------- text cleanup ----------

/// Removes system-reminder, technical blocks and long strings without spaces (base64, hash): leaves the text to search.
fn clean(t: &str) -> String {
    let mut s = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(i) = rest.find("<system-reminder>") {
        s.push_str(&rest[..i]);
        match rest[i..].find("</system-reminder>") { Some(j) => rest = &rest[i + j + 18..], None => { rest = ""; } }
    }
    s.push_str(rest);
    s.split_whitespace().filter(|w| w.len() <= 120).collect::<Vec<_>>().join(" ")
}
/// "Your" messages that are really from the app or Claude Code: commands, task notifications, interruptions.
fn noise(t: &str) -> bool {
    let t = t.trim_start();
    t.is_empty() || t.starts_with("<command-") || t.starts_with("<local-command") || t.starts_with("<task-notification") || t.starts_with("<bash-")
        || t.starts_with("Caveat:") || t.starts_with("[Request interrupted") || t.starts_with("<user-prompt-submit-hook")
}
fn text_of(v: &Value) -> String {
    match v { Value::String(t) => t.clone(), Value::Array(a) => a.iter().filter(|x| str_of(x, "type") == "text").map(|x| str_of(x, "text")).collect::<Vec<_>>().join("\n"), _ => String::new() }
}
/// Civil day from Unix seconds (no dependencies): ISO as in the transcripts.
pub fn iso(ts: i64) -> String {
    let (d, sec) = (ts.div_euclid(86400), ts.rem_euclid(86400));
    let z = d + 719468; let era = z.div_euclid(146097); let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let dd = doy - (153 * mp + 2) / 5 + 1; let mm = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z", if mm <= 2 { y + 1 } else { y }, mm, dd, sec / 3600, sec % 3600 / 60, sec % 60)
}

// ---------- reading transcripts ----------

struct Row { testo: String, t: String, chi: &'static str, img: bool }

/// What is there to search in a transcript line. `turn_img`: an image was seen in the current turn.
fn extract(d: &Value, sub: bool, turn_img: &mut bool, out: &mut Vec<Row>) {
    let t = str_of(d, "timestamp").to_string();
    let chi_tu: &'static str = if sub { "sotto" } else { "tu" };
    let chi_ag: &'static str = if sub { "sotto" } else { "agente" };
    match str_of(d, "type") {
        "queue-operation" => {
            if str_of(d, "operation") == "remove" && str_of(d, "reason") == "absorbed_mid_turn" {
                let x = clean(str_of(d, "content"));
                if !noise(&x) && !x.starts_with('<') { out.push(Row { testo: x, t, chi: chi_tu, img: false }); }
            }
        }
        "user" => {
            let c = d.get("message").and_then(|m| m.get("content")).cloned().unwrap_or(Value::Null);
            let mut img = false;
            let mut own = String::new();
            if let Value::Array(a) = &c {
                for b in a {
                    match str_of(b, "type") {
                        "text" => { own.push_str(str_of(b, "text")); own.push('\n'); }
                        "image" => img = true,
                        "tool_result" => {
                            let cont = b.get("content").cloned().unwrap_or(Value::Null);
                            if let Value::Array(x) = &cont { if x.iter().any(|y| str_of(y, "type") == "image") { *turn_img = true; } }
                            let r = clean(&text_of(&cont));
                            if !r.is_empty() && r.len() <= MAX_RESULT { out.push(Row { testo: r, t: t.clone(), chi: "strumento", img: false }); }
                        }
                        _ => {}
                    }
                }
            } else if let Value::String(x) = &c { own = x.clone(); }
            let x = clean(&own);
            if !noise(&x) {
                if x.contains("Allegato: /") { img = true; }
                *turn_img = img;   // a user message opens a new turn
                out.push(Row { testo: x, t, chi: chi_tu, img });
            }
        }
        "assistant" => {
            let c = d.get("message").and_then(|m| m.get("content")).cloned().unwrap_or(Value::Null);
            if let Value::Array(a) = &c {
                for b in a {
                    match str_of(b, "type") {
                        "text" => { let x = clean(str_of(b, "text")); if !x.is_empty() { out.push(Row { testo: x, t: t.clone(), chi: chi_ag, img: *turn_img }); } }
                        "tool_use" => {
                            let i = &b["input"];
                            let what = [str_of(i, "description"), str_of(i, "file_path"), str_of(i, "pattern"), str_of(i, "prompt")].into_iter().find(|x| !x.is_empty()).unwrap_or("");
                            let what: String = what.chars().take(300).collect();
                            if !what.is_empty() { out.push(Row { testo: clean(&format!("{} {}", str_of(b, "name"), what)), t: t.clone(), chi: "strumento", img: false }); }
                        }
                        _ => {}
                    }
                }
            } else if let Value::String(x) = &c { let x = clean(x); if !x.is_empty() { out.push(Row { testo: x, t, chi: chi_ag, img: *turn_img }); } }
        }
        _ => {}
    }
}

fn mtime(md: &fs::Metadata) -> i64 { md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0) }

/// All transcripts: main ones (~/.claude/projects/*/<id>.jsonl) and sub-agent ones (<id>/subagents/*.jsonl).
fn transcript() -> Vec<(PathBuf, String, bool)> {
    let mut v = vec![];
    for dir in fs::read_dir(home().join(".claude/projects")).into_iter().flatten().flatten() {
        for e in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                let sid = p.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string();
                v.push((p, sid, false));
            } else if p.is_dir() {
                let sid = p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
                for q in fs::read_dir(p.join("subagents")).into_iter().flatten().flatten() {
                    let q = q.path();
                    if q.extension().and_then(|x| x.to_str()) == Some("jsonl") { v.push((q, sid.clone(), true)); }
                }
            }
        }
    }
    v
}

/// Indexes the new lines of a file (from the last offset). Returns the added lines.
fn index_file(c: &mut Connection, p: &Path, sid: &str, sub: bool) -> usize {
    let Ok(md) = fs::metadata(p) else { return 0 };
    let ps = p.to_string_lossy().to_string();
    let (len, mt) = (md.len() as i64, mtime(&md));
    let before: Option<(i64, i64, i64, String)> = c.query_row("SELECT id, off, len, fonte FROM file WHERE path=?1", [&ps], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).ok();
    if let Some((_, off, l, fonte)) = &before { if fonte == "rumore" || (*off == len && *l == len) { return 0; } }
    let (fid, mut off) = match &before {
        Some((id, off, _, _)) if *off <= len => (*id, *off),
        Some((id, _, _, _)) => { let _ = c.execute("DELETE FROM msg WHERE f=?1", [id]); (*id, 0) }   // file rewritten from scratch
        None => { let _ = c.execute("INSERT INTO file(path, off, len, mtime, sessione, fonte) VALUES(?1, 0, 0, 0, ?2, ?3)", params![ps, sid, if sub { "sotto" } else { "claude" }]); (c.last_insert_rowid(), 0) }
    };
    let Ok(f) = fs::File::open(p) else { return 0 };
    let mut r = BufReader::with_capacity(1 << 20, f);
    if r.seek(SeekFrom::Start(off as u64)).is_err() { return 0; }
    let mut rows: Vec<Row> = vec![];
    let (mut cwd, mut title, mut last) = (String::new(), String::new(), String::new());
    let mut turn_img = false;
    let mut buf: Vec<u8> = Vec::new();
    let mut first_mine = off == 0;
    loop {
        buf.clear();
        let Ok(n) = r.read_until(b'\n', &mut buf) else { break };
        if n == 0 || buf.last() != Some(&b'\n') { break; }   // half line: I pick it up on the next round
        off += n as i64;
        if n > MAX_LINE { continue; }
        let Ok(riga) = std::str::from_utf8(&buf) else { continue };
        if riga.contains("\"isMeta\":true") { continue; }
        let useful = riga.contains("\"type\":\"user\"") || riga.contains("\"type\":\"assistant\"") || riga.contains("\"queue-operation\"") || riga.contains("Title\"");
        if !useful && !(cwd.is_empty() && riga.contains("\"cwd\"")) { continue; }
        let Ok(d) = serde_json::from_str::<Value>(riga) else { continue };
        if cwd.is_empty() { cwd = str_of(&d, "cwd").to_string(); }
        if !str_of(&d, "customTitle").is_empty() { title = str_of(&d, "customTitle").to_string(); } else if title.is_empty() && !str_of(&d, "aiTitle").is_empty() { title = str_of(&d, "aiTitle").to_string(); }
        if !sub && d.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
        let k = rows.len();
        extract(&d, sub, &mut turn_img, &mut rows);
        // pilot and request transcripts (claude -p) are not conversations: out of the index, forever
        if first_mine && rows.iter().skip(k).any(|x| x.chi == "tu") {
            first_mine = false;
            let x = &rows.iter().skip(k).find(|x| x.chi == "tu").unwrap().testo;
            if x.starts_with("Sei l'assistente di ") || x.starts_with("Sei il pilota automatico di ") || x.starts_with("Classifica") {
                let _ = c.execute("UPDATE file SET fonte='rumore', off=?2, len=?2, mtime=?3 WHERE id=?1", params![fid, len, mt]);
                return 0;
            }
        }
        if let Some(x) = rows.last() { if !x.t.is_empty() { last = x.t.clone(); } }
    }
    let n = rows.len();
    let Ok(tx) = c.transaction() else { return 0 };
    {
        if let Ok(mut st) = tx.prepare_cached("INSERT INTO msg(testo, sessione, t, chi, img, f) VALUES(?1, ?2, ?3, ?4, ?5, ?6)") {
            for x in &rows { let _ = st.execute(params![x.testo, sid, x.t, x.chi, x.img as i64, fid]); }
        }
        let _ = tx.execute("UPDATE file SET off=?2, len=?3, mtime=?4 WHERE id=?1", params![fid, off, len, mt]);
        if !sub {
            let _ = tx.execute("INSERT INTO sess(sessione, cwd, titolo, fonte, ultimo) VALUES(?1, ?2, ?3, 'claude', ?4)
                ON CONFLICT(sessione) DO UPDATE SET cwd = CASE WHEN excluded.cwd != '' THEN excluded.cwd ELSE sess.cwd END,
                titolo = CASE WHEN excluded.titolo != '' THEN excluded.titolo ELSE sess.titolo END,
                ultimo = CASE WHEN excluded.ultimo > coalesce(sess.ultimo, '') THEN excluded.ultimo ELSE sess.ultimo END", params![sid, cwd, title, last]);
        }
    }
    let _ = tx.commit();
    ROWS.fetch_add(n as u64, Ordering::Relaxed);
    n
}

/// Removes the PC's local chats from the index (rule: never to Claude nor other cloud services). If it removed anything
/// it also cleans up the file (no trace in free pages). Returns how many rows it removed.
fn remove_local(c: &mut Connection) -> usize {
    let n = c.execute("DELETE FROM msg WHERE sessione LIKE 'locale:%'", []).unwrap_or(0)
        + c.execute("DELETE FROM sess WHERE sessione LIKE 'locale:%' OR fonte='locale'", []).unwrap_or(0)
        + c.execute("DELETE FROM file WHERE fonte='locale' OR sessione LIKE 'locale:%'", []).unwrap_or(0);
    if n > 0 { compact(c); }
    n
}

/// Rewrites the index file without free space: deleted text (local chats) stays nowhere,
/// neither in the search index segments, nor in free pages, nor in the WAL log. A few seconds on 150 MB.
fn compact(c: &mut Connection) {
    let t0 = std::time::Instant::now();
    let _ = c.execute("INSERT INTO msg(msg) VALUES('optimize')", []);
    let _ = c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);");
    crate::log_juggl(&format!("indice: ripulito dalle chat locali in {:.1} s", t0.elapsed().as_secs_f64()));
}

/// After deleting local chats (one or all): removes every trace of them from the index and compacts the file
/// anyway (leftovers of old versions). Must be called in a separate thread: VACUUM holds the lock for a few seconds.
pub fn forget_local() {
    let _u = updating().lock().unwrap_or_else(|e| e.into_inner());
    let mut g = db().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = g.as_mut() { if remove_local(c) == 0 { compact(c); } }
}

/// PC local chats: small, rebuilt whole when they change. NOT USED since 10/11 (see remove_local).
#[allow(dead_code)]
fn index_local(c: &mut Connection) {
    for e in fs::read_dir(base_dir().join("chat-locali")).into_iter().flatten().flatten() {
        let p = e.path(); if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        let Ok(md) = fs::metadata(&p) else { continue };
        let ps = p.to_string_lossy().to_string(); let mt = mtime(&md);
        let before: Option<(i64, i64)> = c.query_row("SELECT id, mtime FROM file WHERE path=?1", [&ps], |r| Ok((r.get(0)?, r.get(1)?))).ok();
        if before.map(|x| x.1 == mt).unwrap_or(false) { continue; }
        let Some(v) = crate::load_json(&p) else { continue };
        let sid = format!("locale:{}", str_of(&v, "id"));
        let fid = match before { Some((id, _)) => { let _ = c.execute("DELETE FROM msg WHERE f=?1", [id]); id }
            None => { let _ = c.execute("INSERT INTO file(path, off, len, mtime, sessione, fonte) VALUES(?1, 0, 0, 0, ?2, 'locale')", params![ps, sid]); c.last_insert_rowid() } };
        let Ok(tx) = c.transaction() else { continue };
        let mut last = String::new();
        for m in v["messaggi"].as_array().into_iter().flatten() {
            let t = m.get("t").and_then(|x| x.as_f64()).map(|x| iso(x as i64)).unwrap_or_else(|| iso(str_of(&v, "creata").parse::<f64>().unwrap_or(0.0) as i64));
            let x = clean(str_of(m, "testo")); if x.is_empty() { continue; }
            let _ = tx.execute("INSERT INTO msg(testo, sessione, t, chi, img, f) VALUES(?1, ?2, ?3, ?4, 0, ?5)", params![x, sid, t, if str_of(m, "ruolo") == "user" { "tu" } else { "agente" }, fid]);
            last = t;
        }
        let name = if str_of(&v, "nome").is_empty() { format!("Chat locale · {}", str_of(&v, "modello")) } else { str_of(&v, "nome").to_string() };
        let _ = tx.execute("INSERT OR REPLACE INTO sess(sessione, cwd, titolo, fonte, ultimo) VALUES(?1, '', ?2, 'locale', ?3)", params![sid, name, last]);
        let _ = tx.execute("UPDATE file SET mtime=?2 WHERE id=?1", params![fid, mt]);
        let _ = tx.commit();
    }
}


/// One update round: only files that grew since last time. `pause`: breather between files (build).
fn update(pause: bool) -> usize {
    let Ok(_one) = updating().try_lock() else { return 0 };
    let mut n = 0;
    let mut ff = transcript();
    // newest files first: today's sessions are searchable right away, even during the first build
    ff.sort_by_key(|(p, _, _)| std::cmp::Reverse(fs::metadata(p).map(|m| mtime(&m)).unwrap_or(0)));
    for (p, sid, sub) in ff {
        let mut g = db().lock().unwrap_or_else(|e| e.into_inner());
        let Some(c) = g.as_mut() else { return n };
        n += index_file(c, &p, &sid, sub);
        drop(g);
        if pause { std::thread::sleep(Duration::from_millis(2)); }
    }
    let mut g = db().lock().unwrap_or_else(|e| e.into_inner());
    // PC local chats: NOT indexed (juggl search would hand them to Claude sessions) and if any exist they are removed (10/11)
    if let Some(c) = g.as_mut() {
        // once only: leftovers of local chats indexed by old versions (free pages, segments, WAL)
        if remove_local(c) == 0 && c.query_row("SELECT v FROM meta WHERE k='locali_compattato'", [], |r| r.get::<_, String>(0)).is_err() { compact(c); }
        let _ = c.execute("INSERT OR REPLACE INTO meta(k, v) VALUES('locali_compattato', '1')", []);
    }
    n
}

/// At startup: build (the first time) and then one round every 2 minutes.
pub fn start() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(8));
        // this thread works slowly, the app stays smooth. On the Mac `setpriority(PRIO_PROCESS, 0, ..)` lowers the priority of
        // the WHOLE app (NI 10, review 14/11 point 5): here only the thread is lowered, with the "utility" service class.
        #[cfg(target_os = "macos")]
        unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0); }
        #[cfg(not(target_os = "macos"))]
        unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 10); }
        BUILDING.store(true, Ordering::Relaxed);
        let t0 = Instant::now();
        let n = update(true);
        BUILDING.store(false, Ordering::Relaxed);
        let seconds = t0.elapsed().as_secs_f64();
        if let Some(c) = db().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if !READY.load(Ordering::Relaxed) { let _ = c.execute("INSERT OR REPLACE INTO meta(k, v) VALUES('costruito', ?1)", [format!("{n} righe in {seconds:.1} s")]); }
            let _ = c.execute("INSERT OR REPLACE INTO meta(k, v) VALUES('ultimo_giro', ?1)", [format!("{n} righe in {seconds:.1} s")]);
        }
        READY.store(true, Ordering::Relaxed);
        loop { std::thread::sleep(Duration::from_secs(120)); update(false); }
    });
}

// ---------- search ----------

/// The FTS5 query: each word as a prefix ("ross" finds "Rossi"), quoted phrases stay phrases.
fn fts_query(q: &str) -> String {
    let mut out = vec![];
    let mut rest = q.trim();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('"') {
            let (phrase, after) = r.split_once('"').unwrap_or((r, ""));
            let p: Vec<&str> = phrase.split(|c: char| !c.is_alphanumeric()).filter(|x| !x.is_empty()).collect();
            if !p.is_empty() { out.push(format!("\"{}\"", p.join(" "))); }
            rest = after.trim_start();
        } else {
            let (w, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            for p in w.split(|c: char| !c.is_alphanumeric()).filter(|x| !x.is_empty()) { out.push(format!("\"{p}\"*")); }
            rest = after.trim_start();
        }
    }
    out.join(" ")
}

/// Searches all messages. filters: sessione, cartella (project), chi ("tu"), img (true), giorni, limite, ordine ("tempo").
pub fn search(q: &str, filters: &Value) -> Value {
    let t0 = Instant::now();
    let m = fts_query(q);
    if m.is_empty() { return json!({"risultati": [], "ms": 0}); }
    // before searching: the new transcript lines (a few ms), if the build is not already working
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = (crate::unix_now() * 1000.0) as u64;
    if READY.load(Ordering::Relaxed) && !BUILDING.load(Ordering::Relaxed) && now.saturating_sub(LAST.swap(now, Ordering::Relaxed)) > 3000 { update(false); }
    let g = db().lock().unwrap_or_else(|e| e.into_inner());
    let Some(c) = g.as_ref() else { return json!({"risultati": [], "errore": "indice non disponibile"}) };
    let mut sql = String::from("SELECT msg.sessione, msg.t, msg.chi, msg.img, snippet(msg, 0, char(2), char(3), '…', 16), coalesce(s.cwd, ''), coalesce(s.titolo, ''), coalesce(s.fonte, ''), coalesce(s.ultimo, ''), msg.rowid
        FROM msg LEFT JOIN sess s ON s.sessione = msg.sessione WHERE msg MATCH ?1");
    let mut args: Vec<String> = vec![m];
    let session = str_of(filters, "sessione");
    if !session.is_empty() { args.push(session.to_string()); sql += &format!(" AND msg.sessione = ?{}", args.len()); }
    let folder = str_of(filters, "cartella");
    if !folder.is_empty() { args.push(folder.trim_end_matches('/').to_string()); sql += &format!(" AND (s.cwd = ?{0} OR s.cwd LIKE ?{0} || '/%')", args.len()); }
    if str_of(filters, "chi") == "tu" { sql += " AND msg.chi = 'tu'"; }
    else if !filters.get("strumenti").and_then(|x| x.as_bool()).unwrap_or(true) { sql += " AND msg.chi != 'strumento'"; }
    if filters.get("img").and_then(|x| x.as_bool()).unwrap_or(false) { sql += " AND msg.img = 1"; }
    if let Some(gg) = filters.get("giorni").and_then(|x| x.as_f64()).filter(|x| *x > 0.0) {
        args.push(iso(crate::unix_now() as i64 - (gg * 86400.0) as i64)); sql += &format!(" AND msg.t >= ?{}", args.len());
    }
    sql += if str_of(filters, "ordine") == "tempo" { " ORDER BY msg.t DESC" }
        else { " ORDER BY bm25(msg) + (julianday('now') - julianday(substr(msg.t, 1, 19))) * 0.06 + CASE msg.chi WHEN 'strumento' THEN 4 WHEN 'sotto' THEN 1.5 ELSE 0 END" };
    let limit = filters.get("limite").and_then(|x| x.as_u64()).unwrap_or(200).min(2000);
    sql += &format!(" LIMIT {limit}");
    let Ok(mut st) = c.prepare(&sql) else { return json!({"risultati": [], "errore": "domanda non valida"}) };
    let rows: Vec<Value> = st.query_map(rusqlite::params_from_iter(args.iter()), |r| Ok(json!({
        "sessione": r.get::<_, String>(0)?, "t": r.get::<_, String>(1)?, "chi": r.get::<_, String>(2)?, "img": r.get::<_, i64>(3)? == 1,
        "frase": r.get::<_, String>(4)?, "cwd": r.get::<_, String>(5)?, "titolo": r.get::<_, String>(6)?, "fonte": r.get::<_, String>(7)?,
        "ultimo": r.get::<_, String>(8)?, "id": r.get::<_, i64>(9)? }))).map(|it| it.flatten().collect()).unwrap_or_default();
    json!({"risultati": rows, "ms": t0.elapsed().as_millis() as u64, "costruendo": BUILDING.load(Ordering::Relaxed), "pronto": READY.load(Ordering::Relaxed)})
}

/// Index state (for the UI and the report).
pub fn status() -> Value {
    let g = db().lock().unwrap_or_else(|e| e.into_inner());
    let Some(c) = g.as_ref() else { return json!({"ok": false}) };
    let n: i64 = c.query_row("SELECT count(*) FROM msg", [], |r| r.get(0)).unwrap_or(0);
    let meta = |k: &str| c.query_row("SELECT v FROM meta WHERE k=?1", [k], |r| r.get::<_, String>(0)).unwrap_or_default();
    json!({"ok": true, "righe": n, "costruendo": BUILDING.load(Ordering::Relaxed), "pronto": READY.load(Ordering::Relaxed),
           "costruito": meta("costruito"), "ultimo_giro": meta("ultimo_giro"), "peso_mb": fs::metadata(base_dir().join("indice.db")).map(|m| m.len() / 1048576).unwrap_or(0)})
}

#[cfg(test)]
mod tests {
    /// Builds the real index and tries a few searches: cargo test --release indice -- --ignored --nocapture
    #[test]
    #[ignore]
    fn build_and_search() {
        let t = std::time::Instant::now();
        let n = super::update(false);
        println!("costruzione: {n} righe in {:.1} s", t.elapsed().as_secs_f64());
        let t = std::time::Instant::now();
        let n2 = super::update(false);
        println!("giro incrementale: {n2} righe in {} ms", t.elapsed().as_millis());
        for q in ["Rossi", "box", "coda di rondine", "\"non mi piace che non ha inizio\"", "pellicola", "fattura"] {
            let r = super::search(q, &serde_json::json!({}));
            let res = r["risultati"].as_array().cloned().unwrap_or_default();
            println!("\n«{q}»: {} risultati in {} ms", res.len(), r["ms"]);
            for x in res.iter().take(4) { println!("  {} {} [{}] {} · {}", &crate::str_of(x, "t").chars().take(16).collect::<String>(), crate::str_of(x, "chi"), crate::str_of(x, "titolo"), crate::str_of(x, "frase").replace(['\u{2}', '\u{3}'], "|"), crate::str_of(x, "sessione")); }
        }
        println!("\n{}", super::status());
    }
}

/// Tests only (local_models.rs): puts a local chat in the index as old versions did.
#[cfg(test)]
pub fn test_insert_local(sid: &str, testo: &str) {
    let mut g = db().lock().unwrap_or_else(|e| e.into_inner());
    let c = g.as_mut().expect("indice");
    c.execute("INSERT INTO msg(testo, sessione, t, chi, img, f) VALUES(?1, ?2, '2026-11-10T00:00:00Z', 'tu', 0, 0)", rusqlite::params![testo, sid]).unwrap();
    c.execute("INSERT OR REPLACE INTO sess(sessione, cwd, titolo, fonte, ultimo) VALUES(?1, '', ?2, 'locale', '')", rusqlite::params![sid, testo]).unwrap();
}
