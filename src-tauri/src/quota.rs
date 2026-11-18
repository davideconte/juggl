// Claude's weekly quota: history, forecast and who consumes the most.
//
// - History: one point per hour of the 7-day limit (from the status bar, ~/.claude/regista/usage/limiti.json)
//   in ~/tavolo/consumi-storico.json, written with a temp file + rename. Keeps 35 days.
// - Spend: from the transcripts in ~/.claude/projects (sessions and sub-agents of the last 8 days), the equivalent
//   cost of every Claude reply (pay-as-you-go prices per model family). Incremental reading:
//   of each file only the new part is re-read.
// - Forecast: pace from the history if it covers at least 12 hours of the current week, otherwise the spend of the
//   last 3 days turned into percentage points with this week's percentage/spend ratio.

#[cfg(windows)] use crate::sys as libc;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const DAYS: f64 = 8.0;
const WEEK: f64 = 7.0 * 86400.0;

fn home_dir() -> PathBuf { PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into())) }
fn now_secs() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
fn read_json(p: &Path) -> Option<Value> { serde_json::from_str(&fs::read_to_string(p).ok()?).ok() }
fn history_file() -> PathBuf { crate::base_dir().join("consumi-storico.json") }   // rename (Nov 14)

fn write_safe(p: &Path, v: &Value) -> bool {
    // the folder may not exist (new computer, Windows): without this, the quota silently stayed "not available"
    if let Some(d) = p.parent() { let _ = fs::create_dir_all(d); }
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(v).unwrap_or_default()).is_ok() && fs::rename(&tmp, p).is_ok()
}

// ---------- limits of the active account (Nov 11) ----------
// Before, the value came only from Claude Code's status bar (~/.claude/statusline.sh), which exists only in the
// terminal sessions: with all chats inside Juggl (claude -p) the file stayed still (Nov 9 23:08, "1%" while
// the account was at 96%). Now Juggl asks Anthropic for the connected account (the same data as /usage) every
// 2 minutes and right after an account switch, and writes it to the same file: everything that reads it stays the same.
// Readers take the value only if it is fresh (20 minutes) and from the current account: otherwise "not available".

fn limits_file() -> PathBuf { home_dir().join(".claude/regista/usage/limiti.json") }
const FRESH: f64 = 20.0 * 60.0;

/// ISO date ("2026-11-15T03:59:59.926+00:00") in Unix seconds.
pub(crate) fn epoch_of(t: &str) -> Option<f64> {
    let b = t.get(..19)?;
    let n = |a: usize, z: usize| b.get(a..z).and_then(|x| x.parse::<i64>().ok());
    let (y, m, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400); let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1; let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let mut s = (era * 146097 + doe - 719468) * 86400 + n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19)?;
    // "+hh:mm" time zone at the end
    if let Some(i) = t.rfind(|c| c == '+' || c == '-').filter(|i| *i > 18) {
        let z = &t[i..]; let sg = if z.starts_with('-') { -1 } else { 1 };
        if let (Some(h), Some(mi)) = (z.get(1..3).and_then(|x| x.parse::<i64>().ok()), z.get(4..6).and_then(|x| x.parse::<i64>().ok())) { s -= sg * (h * 3600 + mi * 60); }
    }
    Some(s as f64)
}

/// Token of the connected account, from the Keychain (never printed nor saved elsewhere).
fn token() -> Option<String> {
    // Windows and Linux: Claude Code keeps the login in ~/.claude/.credentials.json (or in CLAUDE_CONFIG_DIR)
    #[cfg(not(target_os = "macos"))]
    {
        let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| home_dir().join(".claude"));
        let v: Value = serde_json::from_slice(&std::fs::read(dir.join(".credentials.json")).ok()?).ok()?;
        return v["claudeAiOauth"]["accessToken"].as_str().map(|x| x.to_string()).filter(|x| !x.is_empty());
    }
    #[allow(unreachable_code)]
    let o = crate::new_command("/usr/bin/security").args(["find-generic-password", "-s", "Claude Code-credentials", "-w"]).output().ok()?;
    let v: Value = serde_json::from_slice(&o.stdout).ok()?;
    v["claudeAiOauth"]["accessToken"].as_str().map(|x| x.to_string()).filter(|x| !x.is_empty())
}

// credentials change (Nov 12): same account but new token or different plan (e.g. Pro -> Max). Limits read with the
// old token are no longer valid: limiti.json is thrown away, the waits are reset (even after a 429) and it is re-read at once.
static CHANGE_AT: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);

/// Credentials signature that does NOT change when the token is renewed: plan and tier (subscriptionType, rateLimitTier).
/// The token stays in memory, is not printed nor saved. Empty if there is no login.
pub fn credentials_signature() -> String {
    #[cfg(not(target_os = "macos"))]
    let raw: Option<Vec<u8>> = {
        let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| home_dir().join(".claude"));
        std::fs::read(dir.join(".credentials.json")).ok()
    };
    #[cfg(target_os = "macos")]
    let raw: Option<Vec<u8>> = crate::new_command("/usr/bin/security").args(["find-generic-password", "-s", "Claude Code-credentials", "-w"]).output().ok().map(|o| o.stdout);
    let Some(v) = raw.and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return String::new() };
    let o = &v["claudeAiOauth"];
    if o["accessToken"].as_str().map(|x| x.is_empty()).unwrap_or(true) { return String::new(); }
    format!("{}/{}", o["subscriptionType"].as_str().unwrap_or(""), o["rateLimitTier"].as_str().unwrap_or(""))
}

/// Credentials changed: old limits and waits are dropped, and limits are re-read at once (in a thread). Returns immediately.
pub fn credentials_changed() {
    *NEXT_CHECK.lock().unwrap_or_else(|e| e.into_inner()) = (0.0, 0);
    *CHANGE_AT.lock().unwrap_or_else(|e| e.into_inner()) = now_secs();
    let _ = fs::remove_file(limits_file());
    crate::log_juggl("quota: credenziali cambiate, limiti vecchi scartati e riletti");
    std::thread::spawn(|| { if let Ok(p) = refresh_from_server() { notify_thresholds(p); } });
}

/// After a logout: the limits of the account that no longer exists are dropped, without asking the server anything.
pub fn credentials_changed_no_reread() {
    *NEXT_CHECK.lock().unwrap_or_else(|e| e.into_inner()) = (0.0, 0);
    let _ = fs::remove_file(limits_file());
}

fn window_of(w: &Value) -> Value {
    match (w["utilization"].as_f64(), w["resets_at"].as_str().and_then(epoch_of)) {
        (Some(p), Some(r)) => json!({"used_percentage": p, "resets_at": r}),
        (Some(p), None) => json!({"used_percentage": p}),
        _ => Value::Null,
    }
}

/// Reads the connected account's limits from Anthropic and writes them to limiti.json. Returns the weekly percentage.
/// Error = the file stays as it was (readers will see it is no longer fresh).
// limits from several computers (Nov 12): with the same account on Mac and PC the endpoint answers 429. It is asked at most every
// 5 minutes (plus some randomness, so the computers do not ask together) and, after a 429, it waits 5, 10, 20 minutes
// (or more if Anthropic says so with Retry-After). Meanwhile the last good value holds.
const LIMITS_EVERY: f64 = 300.0;
const WAITS_429: [f64; 3] = [300.0, 600.0, 1200.0];
static NEXT_CHECK: std::sync::Mutex<(f64, usize)> = std::sync::Mutex::new((0.0, 0));

fn jitter(max: f64) -> f64 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    (n % 1000) as f64 / 1000.0 * max
}

/// Percentage of the last good value (same account), if any.
fn last_pct() -> Option<f64> {
    let v = read_json(&limits_file())?;
    let acct = v["account"].as_str().unwrap_or("");
    if !acct.is_empty() && !crate::claude_account::same_account(acct, &crate::claude_account::current()) { return None; }   // account switch (Nov 12)
    v["rate_limits"]["seven_day"]["used_percentage"].as_f64()
}

pub fn refresh_from_server() -> Result<f64, String> {
    // it is not time yet (or we are waiting after a 429): the last good value holds. With a different account it asks at once.
    let now = now_secs();
    let (next_at, _) = *NEXT_CHECK.lock().unwrap_or_else(|e| e.into_inner());
    if now < next_at {
        if let Some(p) = last_pct() { return Ok(p); }
        if next_at - now > LIMITS_EVERY { return Err(format!("in attesa dopo troppe richieste (riprovo tra {:.0} minuti)", (next_at - now) / 60.0)); }
    }
    let tok = token().ok_or("token dell'account non disponibile")?;
    // system curl (ureq here has no HTTPS): the headers with the token go through stdin, not the arguments
    // (a process's arguments are visible with ps)
    use std::io::Write;
    let mut c = crate::license::curl_command()
        .args(["-sS", "--max-time", "15", "-D", "-", "-w", "\n%{http_code}", "-H", "@-", "https://api.anthropic.com/api/oauth/usage"])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .spawn().map_err(|e| format!("curl non parte ({e})"))?;
    if let Some(mut i) = c.stdin.take() { let _ = write!(i, "Authorization: Bearer {tok}\nanthropic-beta: oauth-2025-04-20\n"); }
    let o = c.wait_with_output().map_err(|e| e.to_string())?;
    if !o.status.success() { reschedule(false, 0.0); return Err(format!("Anthropic non risponde ({})", String::from_utf8_lossy(&o.stderr).trim().chars().take(80).collect::<String>())); }
    let all_out = String::from_utf8_lossy(&o.stdout).to_string();
    let (rest, code) = all_out.rsplit_once('\n').unwrap_or((all_out.as_str(), ""));
    let (headers, body) = rest.rsplit_once("\r\n\r\n").unwrap_or(("", rest));
    if code.trim() == "429" {
        let ra = headers.lines().find_map(|l| { let (k, v) = l.split_once(':')?; k.trim().eq_ignore_ascii_case("retry-after").then(|| v.trim().parse::<f64>().ok()).flatten() }).unwrap_or(0.0);
        let wait = reschedule(true, ra);
        return Err(format!("Anthropic: troppe richieste (429), riprovo tra {:.0} minuti", wait / 60.0));
    }
    let v: Value = serde_json::from_str(body.trim()).map_err(|_| { reschedule(false, 0.0); "risposta di Anthropic non leggibile".to_string() })?;
    if v.get("error").is_some() { return Err(format!("Anthropic: {}", v["error"]["message"].as_str().unwrap_or("errore"))); }
    let seven = window_of(&v["seven_day"]);
    let pct = seven["used_percentage"].as_f64().ok_or("risposta senza limite settimanale")?;
    let mut rl = json!({"seven_day": seven});
    let five = window_of(&v["five_hour"]);
    if !five.is_null() { rl["five_hour"] = five; }
    write_safe(&limits_file(), &json!({"at": now_secs(), "fonte": "anthropic", "account": crate::claude_account::current(), "rate_limits": rl}));
    *NEXT_CHECK.lock().unwrap_or_else(|e| e.into_inner()) = (now_secs() + LIMITS_EVERY + jitter(60.0), 0);
    Ok(pct)
}

/// Next check after an error: with a 429 a growing wait (5, 10, 20 minutes, or Retry-After if longer),
/// otherwise the normal round. Returns the wait in seconds.
fn reschedule(too_many: bool, retry_after: f64) -> f64 {
    let mut p = NEXT_CHECK.lock().unwrap_or_else(|e| e.into_inner());
    let (wait, level) = if too_many {
        let l = (p.1 + 1).min(WAITS_429.len());
        (WAITS_429[l - 1].max(retry_after), l)
    } else { (LIMITS_EVERY, p.1) };
    let wait = wait + jitter(60.0);
    *p = (now_secs() + wait, level);
    wait
}

/// Limits valid now (fresh and from the connected account), in the status bar's shape; Null if not available.
pub fn valid_limits() -> Value {
    let Some(v) = read_json(&limits_file()) else { return Value::Null };
    let at = v["at"].as_f64().unwrap_or(0.0);
    let acct = v["account"].as_str().unwrap_or("");
    let same = acct.is_empty() || crate::claude_account::same_account(acct, &crate::claude_account::current());   // account switch (Nov 12)
    if !same { return Value::Null; }
    let mut rl = v["rate_limits"].clone();
    // just re-read after a credentials change: the line under the input box says "updated now" for 5 minutes
    let changed_at = *CHANGE_AT.lock().unwrap_or_else(|e| e.into_inner());
    if changed_at > 0.0 && at >= changed_at && now_secs() - changed_at < 300.0 && rl["seven_day"].is_object() { rl["seven_day"]["aggiornato_adesso"] = json!(true); }
    // stale data (too many requests, or no network): the last good value stays, with its age (Nov 12)
    let age = now_secs() - at;
    if age >= FRESH {
        if age > 3.0 * 86400.0 || rl["seven_day"]["resets_at"].as_f64().map(|r| r < now_secs()).unwrap_or(false) { return Value::Null; }
        if rl["seven_day"].is_object() { rl["seven_day"]["aggiornato_min"] = json!((age / 60.0).round()); }
    }
    rl
}

/// When the limits we see were read (Unix seconds), if the file exists (Claude Account panel, Nov 14).
pub fn limits_read_at() -> Option<f64> { read_json(&limits_file())?["at"].as_f64() }

/// Notices at 50, 80 and 95% of the weekly limit: one per threshold and per week (reset), discreet.
pub fn notify_thresholds(pct: f64) {
    let p = crate::base_dir().join("limiti-soglie.json");   // rename (Nov 14)
    let reset = read_json(&limits_file()).and_then(|v| v["rate_limits"]["seven_day"]["resets_at"].as_f64()).unwrap_or(0.0);
    let mut st = read_json(&p).unwrap_or(json!({}));
    if (st["reset"].as_f64().unwrap_or(-1.0) - reset).abs() > 60.0 { st = json!({"reset": reset, "livello": 0}); }
    let seen = st["livello"].as_f64().unwrap_or(0.0);
    let Some(&threshold) = [95.0, 80.0, 50.0].iter().find(|s| pct >= **s) else { return };
    if threshold <= seen { return; }
    st["livello"] = json!(threshold);
    write_safe(&p, &st);
    let when = if reset > 0.0 { let h = ((reset - now_secs()) / 3600.0).max(0.0); if h < 24.0 { format!("si azzera tra {:.0} ore", h.ceil()) } else { format!("si azzera tra {:.0} giorni", (h / 24.0).ceil()) } } else { String::new() };
    let title = format!("Limite settimanale di Claude al {:.0}%", pct);
    let text = match threshold as u32 { 95 => format!("Quasi finito: {when}. Metti in pausa le chat che possono aspettare."), 80 => format!("Usato l'80%: {when}."), _ => format!("Metà della settimana usata: {when}.") };
    crate::session_care::record(&title, &text, "", "quota");
    crate::session_care::notify(&title, &text);
}

/// Current 7-day limit: (percentage, reset, when it was read).
fn seven_day_limit() -> Option<(f64, f64, f64)> {
    let v = read_json(&limits_file())?;
    if valid_limits().is_null() { return None; }
    let w = &v["rate_limits"]["seven_day"];
    Some((w["used_percentage"].as_f64()?, w["resets_at"].as_f64()?, v["at"].as_f64().unwrap_or(0.0)))
}

/// One point per hour in the history (called every few minutes by the background thread).
pub fn record_history() {
    let Some((pct, reset, at)) = seven_day_limit() else { return };
    if at <= 0.0 { return; }
    let p = history_file();
    let mut points: Vec<Value> = read_json(&p).and_then(|v| v["punti"].as_array().cloned()).unwrap_or_default();
    if let Some(u) = points.last() {
        let (ua, ur, up) = (u["at"].as_f64().unwrap_or(0.0), u["reset"].as_f64().unwrap_or(0.0), u["pct"].as_f64().unwrap_or(-1.0));
        if at <= ua { return; }
        // new week or account switch: at once; otherwise one per hour
        let changed = (ur - reset).abs() > 60.0;
        if !changed && at - ua < 3540.0 && (pct - up).abs() < 5.0 { return; }
    }
    points.push(json!({"at": at, "pct": pct, "reset": reset}));
    let cutoff = now_secs() - 35.0 * 86400.0;
    points.retain(|x| x["at"].as_f64().unwrap_or(0.0) >= cutoff);
    write_safe(&p, &json!({"nota": "Juggl: quota settimanale di Claude, un punto all'ora", "punti": points}));
}

// ---------- spend from the transcripts ----------

#[derive(Clone, Copy)]
struct UsageEntry { t: f64, costo: f32, fam: u8, sotto: bool, agenti: u16, modifiche: u16, out: u32, inp: u32, cr: u32, cw: u32, idh: u64 }

#[derive(Default)]
struct File { letto: u64, sessione: String, cwd: String, visti: HashSet<u64>, voci: Vec<UsageEntry>,
    // full usage (0.1.76): thread type (0 chat, 1 sub-agent, 2 Juggl helper), whether already recognised, file weight
    tipo: u8, tipo_noto: bool, peso: u64 }

static CACHE: Mutex<Option<HashMap<PathBuf, File>>> = Mutex::new(None);
static LAST_RESULT: Mutex<Option<(f64, Value)>> = Mutex::new(None);
static IN_PROGRESS: Mutex<bool> = Mutex::new(false);

fn hash(s: &str) -> u64 { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); s.hash(&mut h); h.finish() }

fn iso_to_epoch(t: &str) -> Option<f64> {
    // "2026-11-08T16:09:37.030Z"
    let b = t.as_bytes(); if b.len() < 19 { return None; }
    let n = |a: usize, l: usize| t.get(a..a + l)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (n(0, 4)?, n(5, 2)?, n(8, 2)?, n(11, 2)?, n(14, 2)?, n(17, 2)?);
    // days since 1970 (Howard Hinnant's algorithm)
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400); let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1; let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + h * 3600 + mi * 60 + s) as f64)
}

/// How the prompts of Juggl's helpers begin (extractor, pilot, names, external messages, summaries, voice, commands): used to
/// separate their calls from real chats in the usage. «NOME» is already replaced with the name of the person using Juggl in the real text.
const HELPER_PREFIXES: [&str; 6] = ["Sei l'assistente di ", "Sei il pilota automatico di ", "Dai un nome breve a questa chat", "Fai da filtro per ",
    "Sei dentro un'app che mostra a ", "Per ogni comando qui sotto"];
/// The voice summary starts with the name of the person using Juggl, so it is recognised by a fixed phrase within the first words.
fn is_helper(text: &str) -> bool {
    let t = text.trim_start();
    HELPER_PREFIXES.iter().any(|i| t.starts_with(i)) || t.chars().take(160).collect::<String>().contains(" sta parlando a voce con un agente")
}

/// Pay-as-you-go prices per million tokens: input, output, cache read, cache write 5 min, cache write 1 h.
fn prices(fam: u8) -> [f64; 5] {
    // 3 = Fable: Opus price (assumption: it is the top model; before it counted as Sonnet and its spend was underestimated)
    match fam { 0 | 3 => [5.0, 25.0, 0.5, 6.25, 10.0], 2 => [1.0, 5.0, 0.1, 1.25, 2.0], _ => [3.0, 15.0, 0.3, 3.75, 6.0] }
}

fn read_transcript(p: &Path, f: &mut File, cutoff: f64) {
    let Ok(mut fh) = fs::File::open(p) else { return };
    let n = fh.metadata().map(|m| m.len()).unwrap_or(0);
    if n < f.letto { *f = File { sessione: std::mem::take(&mut f.sessione), ..Default::default() }; }
    f.peso = n;
    if !f.tipo_noto && p.parent().and_then(|d| d.file_name()).and_then(|x| x.to_str()) == Some("subagents") { f.tipo = 1; f.tipo_noto = true; }
    if n == f.letto { return; }
    let _ = fh.seek(SeekFrom::Start(f.letto));
    let mut r = BufReader::with_capacity(1 << 20, fh);
    let mut line: Vec<u8> = Vec::with_capacity(1 << 16);
    let fu = memchr::memmem::Finder::new(b"\"usage\"");
    let mut pos = f.letto;
    loop {
        line.clear();
        let Ok(k) = r.read_until(b'\n', &mut line) else { break };
        if k == 0 || line.last() != Some(&b'\n') { break; }   // half line: I will re-read it next time
        pos += k as u64;
        if f.cwd.is_empty() {
            if let Some(i) = memchr::memmem::find(&line, b"\"cwd\":\"") {
                let rest = &line[i + 7..];
                if let Some(j) = memchr::memchr(b'"', rest) { f.cwd = String::from_utf8_lossy(&rest[..j]).to_string(); }
            }
        }
        // is the thread a Juggl helper's? The user's first message says so (helper prompts have a fixed start)
        if !f.tipo_noto && memchr::memmem::find(&line, b"\"role\":\"user\"").is_some() {
            if let Ok(d) = serde_json::from_slice::<Value>(&line) {
                let c = &d["message"]["content"];
                let text = c.as_str().map(|x| x.to_string()).or_else(|| c.as_array().and_then(|a| a.iter().find_map(|x| x["text"].as_str().map(|y| y.to_string())))).unwrap_or_default();
                if !text.is_empty() { f.tipo = if is_helper(&text) { 2 } else { 0 }; f.tipo_noto = true; }
            }
        }
        if fu.find(&line).is_none() || memchr::memmem::find(&line, b"\"assistant\"").is_none() { continue; }
        let Ok(d) = serde_json::from_slice::<Value>(&line) else { continue };
        let m = &d["message"];
        if m["role"].as_str() != Some("assistant") { continue; }
        let model = m["model"].as_str().unwrap_or("");
        if model.is_empty() || model.starts_with('<') { continue; }
        // a reply is written on several lines (one per block) with the same id: counted once
        let id = m["id"].as_str().unwrap_or("");
        let is_new = id.is_empty() || f.visti.insert(hash(id));
        // the same reply can be in several files (chat and sub-agent): in the totals it is counted once, by id + requestId
        let idh = if id.is_empty() { 0 } else { hash(&format!("{id}|{}", d["requestId"].as_str().unwrap_or(""))) };
        // tools: counted on every line (each tool_use block is on its own line)
        let (mut agenti, mut modifiche) = (0u16, 0u16);
        if let Some(a) = m["content"].as_array() {
            for c in a {
                if c["type"].as_str() != Some("tool_use") { continue; }
                match c["name"].as_str().unwrap_or("") { "Agent" | "Task" => agenti += 1, "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => modifiche += 1, _ => {} }
            }
        }
        let Some(t) = d["timestamp"].as_str().and_then(iso_to_epoch) else { continue };
        if t < cutoff { continue; }
        let fam = if model.contains("fable") { 3 } else if model.contains("opus") { 0 } else if model.contains("haiku") { 2 } else { 1 };
        let sotto = d["isSidechain"].as_bool().unwrap_or(false);
        if !is_new {
            if agenti + modifiche > 0 { f.voci.push(UsageEntry { t, costo: 0.0, fam, sotto, agenti, modifiche, out: 0, inp: 0, cr: 0, cw: 0, idh: 0 }); }
            continue;
        }
        let u = &m["usage"];
        let g = |k: &str| u[k].as_f64().unwrap_or(0.0);
        let c1 = u["cache_creation"]["ephemeral_1h_input_tokens"].as_f64().unwrap_or(0.0);
        let c5 = (g("cache_creation_input_tokens") - c1).max(0.0);
        let pz = prices(fam);
        let costo = (g("input_tokens") * pz[0] + g("output_tokens") * pz[1] + g("cache_read_input_tokens") * pz[2] + c5 * pz[3] + c1 * pz[4]) / 1e6;
        f.voci.push(UsageEntry { t, costo: costo as f32, fam, sotto, agenti, modifiche, out: g("output_tokens") as u32, inp: g("input_tokens") as u32, cr: g("cache_read_input_tokens") as u32, cw: g("cache_creation_input_tokens") as u32, idh });
    }
    f.letto = pos;
}

fn recent_files(cutoff: f64) -> Vec<(PathBuf, String)> {
    let mut out = vec![];
    let Ok(rd) = fs::read_dir(home_dir().join(".claude/projects")) else { return out };
    let recente = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64() >= cutoff).unwrap_or(false);
    for d in rd.flatten() {
        let dir = d.path();
        let Ok(rd2) = fs::read_dir(&dir) else { continue };
        for e in rd2.flatten() {
            let p = e.path();
            let nome = p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
            if nome.ends_with(".jsonl") {
                if recente(&p) { out.push((p.clone(), nome.trim_end_matches(".jsonl").to_string())); }
            } else if p.is_dir() {
                // <session>/subagents/agent-*.jsonl: count for the parent session
                if let Ok(rd3) = fs::read_dir(p.join("subagents")) {
                    for s in rd3.flatten() {
                        let q = s.path();
                        if q.extension().and_then(|x| x.to_str()) == Some("jsonl") && recente(&q) { out.push((q, nome.clone())); }
                    }
                }
            }
        }
    }
    out
}

#[derive(Default)]
struct Sum { costo: f64, sotto: f64, opus: f64, agenti: u32, modifiche: u32, risposte: u32, out: f64, ultimo: f64, cwd: String,
    // short windows (Nov 11): who is consuming right now
    m30: f64, ora1: f64, sotto_ora1: f64, oggi: f64 }

/// Full computation (slow only the first time: reads the transcripts of the last 8 days).
fn compute(sessioni: &[Value]) -> Value {
    let now = now_secs();
    let cutoff = now - DAYS * 86400.0;
    let listing = recent_files(cutoff);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = g.get_or_insert_with(HashMap::new);
    let live: HashSet<&PathBuf> = listing.iter().map(|(p, _)| p).collect();
    cache.retain(|p, _| live.contains(p));
    for (p, sid) in &listing {
        let f = cache.entry(p.clone()).or_insert_with(|| File { sessione: sid.clone(), ..Default::default() });
        read_transcript(p, f, cutoff);
        f.voci.retain(|v| v.t >= cutoff);
    }
    // sums per session, per day and per model
    let mut per: HashMap<String, Sum> = HashMap::new();
    let mut fam = [0f64; 4];
    let lim7 = seven_day_limit();
    let inizio_sett = lim7.map(|(_, r, _)| r - WEEK).unwrap_or(now - WEEK);
    let (mut in_sett, mut ultimi3) = (0f64, 0f64);
    let mut days: HashMap<i64, f64> = HashMap::new();
    let tz_offset = local_offset();
    let midnight = ((now + tz_offset) / 86400.0).floor() * 86400.0 - tz_offset;
    let (mut tot_ora1, mut tot_oggi) = (0f64, 0f64);
    let mut seen: HashSet<u64> = HashSet::new();
    for f in cache.values() {
        let s = per.entry(f.sessione.clone()).or_default();
        if s.cwd.is_empty() && !f.cwd.is_empty() { s.cwd = f.cwd.clone(); }
        for v in &f.voci {
            let c = v.costo as f64;
            s.agenti += v.agenti as u32; s.modifiche += v.modifiche as u32;
            if c <= 0.0 { continue; }
            if v.idh != 0 && !seen.insert(v.idh) { continue; }
            if v.t >= now - WEEK {
                s.costo += c; s.risposte += 1; s.out += v.out as f64;
                if v.sotto { s.sotto += c; }
                if v.fam == 0 { s.opus += c; }
                fam[v.fam as usize] += c;
            }
            if v.t > s.ultimo { s.ultimo = v.t; }
            if v.t >= now - 1800.0 { s.m30 += c; }
            if v.t >= now - 3600.0 { s.ora1 += c; tot_ora1 += c; if v.sotto { s.sotto_ora1 += c; } }
            if v.t >= midnight { s.oggi += c; tot_oggi += c; }
            if v.t >= inizio_sett { in_sett += c; }
            if v.t >= (now - 3.0 * 86400.0).max(inizio_sett) { ultimi3 += c; }
            *days.entry(((v.t + tz_offset) / 86400.0).floor() as i64).or_default() += c;
        }
    }
    drop(g);
    let totale: f64 = fam.iter().sum();
    // percentage points per equivalent dollar, measured on this week (at least 12 hours needed)
    let ratio = match lim7 { Some((pct, _, _)) if in_sett > 1.0 && now - inizio_sett > 12.0 * 3600.0 => Some(pct / in_sett), _ => None };

    // ---- forecast ----
    let history: Vec<Value> = read_json(&history_file()).and_then(|v| v["punti"].as_array().cloned()).unwrap_or_default();
    let mut prev = json!(null);
    let mut series: Vec<Value> = vec![];
    if let Some((pct, reset, at)) = lim7 {
        let week_pts: Vec<(f64, f64)> = history.iter().filter(|x| (x["reset"].as_f64().unwrap_or(0.0) - reset).abs() < 60.0)
            .filter_map(|x| Some((x["at"].as_f64()?, x["pct"].as_f64()?))).collect();
        series = week_pts.iter().map(|(a, p)| json!([a, p])).collect();
        let da72: Vec<&(f64, f64)> = week_pts.iter().filter(|(a, _)| *a >= now - 72.0 * 3600.0).collect();
        let (mut pace, mut method, mut hours) = (None, "", 0.0);
        if let (Some(a), Some(b)) = (da72.first(), da72.last()) {
            if b.0 - a.0 >= 12.0 * 3600.0 && b.1 >= a.1 { pace = Some((b.1 - a.1) / (b.0 - a.0) * 86400.0); method = "storico"; hours = (b.0 - a.0) / 3600.0; }
        }
        if pace.is_none() {
            if let Some(k) = ratio {
                let dal = (now - 3.0 * 86400.0).max(inizio_sett);
                if now - dal > 6.0 * 3600.0 { pace = Some(k * ultimi3 / (now - dal) * 86400.0); method = "transcript"; hours = (now - dal) / 3600.0; }
            }
        }
        if pace.is_none() && now - inizio_sett > 6.0 * 3600.0 {
            pace = Some(pct / (now - inizio_sett) * 86400.0); method = "media"; hours = (now - inizio_sett) / 3600.0;
        }
        if let Some(r) = pace {
            let remaining = (reset - now).max(0.0);
            let fine = pct + r * remaining / 86400.0;
            let full = if fine >= 100.0 && r > 0.0 { json!(now + (100.0 - pct) / r * 86400.0) } else { json!(null) };
            prev = json!({"metodo": method, "ore": hours.round(), "ritmo_giorno": (r * 10.0).round() / 10.0, "a_fine": fine.round(), "pieno_il": full});
        }
        let _ = at;
    }

    // ---- per day (last 7, oldest first) ----
    let oggi = ((now + tz_offset) / 86400.0).floor() as i64;
    let gg: Vec<Value> = (0..7).rev().map(|i| {
        let d = oggi - i;
        let c = days.get(&d).copied().unwrap_or(0.0);
        json!({"giorno": d * 86400 - tz_offset as i64, "costo": (c * 100.0).round() / 100.0, "pct": ratio.map(|k| (k * c * 10.0).round() / 10.0)})
    }).collect();

    // ---- per session, with Juggl's data ----
    let di: HashMap<&str, &Value> = sessioni.iter().map(|v| (v["sessione"].as_str().unwrap_or(""), v)).collect();
    let mut rows: Vec<(&String, &Sum)> = per.iter().filter(|(_, s)| s.costo > 0.01).collect();
    rows.sort_by(|a, b| b.1.costo.partial_cmp(&a.1.costo).unwrap_or(std::cmp::Ordering::Equal));
    let pc = |c: f64| ratio.map(|k| (k * c * 10.0).round() / 10.0);
    let r2 = |c: f64| (c * 100.0).round() / 100.0;
    // today per project (evening summary): Juggl's project name, otherwise the folder
    let mut projects: HashMap<String, f64> = HashMap::new();
    for (sid, s) in &rows {
        if s.oggi <= 0.0 { continue; }
        let j = di.get(sid.as_str());
        let nome = j.and_then(|v| v["progetto"]["nome"].as_str()).filter(|x| !x.is_empty()).map(|x| x.to_string())
            .unwrap_or_else(|| s.cwd.trim_end_matches('/').rsplit('/').next().filter(|x| !x.is_empty()).unwrap_or("Altro").to_string());
        *projects.entry(nome).or_default() += s.oggi;
    }
    let mut pr: Vec<(String, f64)> = projects.into_iter().collect();
    pr.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let progetti_oggi: Vec<Value> = pr.iter().map(|(n, c)| json!({"nome": n, "costo": r2(*c), "pct": pc(*c)})).collect();
    // at this pace (last hour) the limit runs out in... (hours), only if the percentage/spend ratio is known
    let fine_ora = match (lim7, ratio) { (Some((p, _, _)), Some(k)) if tot_ora1 > 0.0 && p < 100.0 => Some(((100.0 - p) / (k * tot_ora1) * 10.0).round() / 10.0), _ => None };
    let ss: Vec<Value> = rows.iter().take(40).map(|(sid, s)| {
        let j = di.get(sid.as_str());
        json!({"sessione": sid, "costo": (s.costo * 100.0).round() / 100.0, "quota": if totale > 0.0 { (s.costo / totale * 1000.0).round() / 10.0 } else { 0.0 },
            "pct": ratio.map(|k| (k * s.costo * 10.0).round() / 10.0), "sotto_quota": if s.costo > 0.0 { (s.sotto / s.costo * 100.0).round() } else { 0.0 },
            "opus_quota": if s.costo > 0.0 { (s.opus / s.costo * 100.0).round() } else { 0.0 }, "agenti": s.agenti, "modifiche": s.modifiche, "risposte": s.risposte,
            "out_medio": if s.risposte > 0 { (s.out / s.risposte as f64).round() } else { 0.0 }, "ultimo": s.ultimo, "cwd": s.cwd,
            "juggl": j.is_some(), "nome": j.map(|v| v["nome"].clone()).unwrap_or(Value::Null), "progetto": j.map(|v| v["progetto"].clone()).unwrap_or(Value::Null),
            "vista": j.map(|v| v["vista"].clone()).unwrap_or(Value::Null),
            "m30": r2(s.m30), "ora": r2(s.ora1), "oggi": r2(s.oggi), "pct_m30": pc(s.m30), "pct_ora": pc(s.ora1), "pct_oggi": pc(s.oggi),
            "sotto_ora": if s.ora1 > 0.0 { (s.sotto_ora1 / s.ora1 * 100.0).round() } else { 0.0 }})
    }).collect();

    json!({"ok": true, "calcolato": now, "limite": lim7.map(|(p, r, a)| json!({"pct": p, "reset": r, "letto": a})),
        "previsione": prev, "storico": series, "giorni": gg, "sessioni": ss,
        "modelli": {"opus": fam[0].round(), "sonnet": fam[1].round(), "haiku": fam[2].round(), "fable": fam[3].round()}, "totale": totale.round(), "rapporto": ratio,
        "ultima_ora": {"costo": r2(tot_ora1), "pct": pc(tot_ora1)}, "oggi": {"costo": r2(tot_oggi), "pct": pc(tot_oggi)},
        "fine_ore_ritmo_ora": fine_ora, "progetti_oggi": progetti_oggi})
}

/// Difference of the local time zone from UTC in seconds (to group by calendar day).
fn local_offset() -> f64 {
    let t = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm.tm_gmtoff as f64
}

/// Ready result (at most 2 minutes old); if missing it is computed now.
pub fn analysis(sessioni: Vec<Value>) -> Value {
    let pronto = LAST_RESULT.lock().ok().and_then(|g| g.clone());
    if let Some((t, v)) = &pronto { if now_secs() - t < 120.0 { return v.clone(); } }
    {
        let mut c = IN_PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
        if *c { return pronto.map(|x| x.1).unwrap_or(json!({"ok": false, "msg": "Sto leggendo i consumi"})); }
        *c = true;
    }
    // even if the computation panics, the "in progress" signal goes off
    struct Done;
    impl Drop for Done { fn drop(&mut self) { *IN_PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = false; } }
    let _fine = Done;
    let v = compute(&sessioni);
    if let Ok(mut g) = LAST_RESULT.lock() { *g = Some((now_secs(), v.clone())); }
    v
}

// ---------- full usage (0.1.76): where and when tokens were consumed ----------
// Real data from the .jsonl transcripts (same reading as `compute`, every reply counted once by id): input, output
// and cache tokens; per chat, per project, per hour, per model (Opus, Sonnet, Haiku, Fable) and per thread type
// (chat, sub-agents, Juggl helpers, coordinator). Local chats excluded (they have no Claude transcript).
//
// "TOKEN QUOTA": Anthropic gives only the weekly total, not the per-chat one. Each chat's quota is its share
// of the WEIGHTED tokens over the week's total. Weights (fixed ratios of the pay-as-you-go prices, equal for all models):
//   input 1, output 5, cache read 0.1, cache write 1.25 (1 hour: 2); then per model: Haiku 0.33, Sonnet 1, Opus 1.67, Fable 1.67
// which is exactly the equivalent cost of `prices()`. It is not the official percentage: it is a share of the weighted tokens.
fn week_start(now: f64) -> f64 { seven_day_limit().map(|(_, r, _)| r - WEEK).unwrap_or(now - WEEK) }

/// How much each thread type and each model used, hour by hour, chat by chat and project by project. `sessioni` = Juggl's sessions.
pub fn breakdown(sessioni: Vec<Value>) -> Value {
    let now = now_secs();
    let cutoff = now - DAYS * 86400.0;
    let listing = recent_files(cutoff);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = g.get_or_insert_with(HashMap::new);
    let live: HashSet<&PathBuf> = listing.iter().map(|(p, _)| p).collect();
    cache.retain(|p, _| live.contains(p));
    for (p, sid) in &listing {
        let f = cache.entry(p.clone()).or_insert_with(|| File { sessione: sid.clone(), ..Default::default() });
        read_transcript(p, f, cutoff);
        f.voci.retain(|v| v.t >= cutoff);
    }
    let da = week_start(now);
    let tz_offset = local_offset();
    let midnight = ((now + tz_offset) / 86400.0).floor() * 86400.0 - tz_offset;
    let js: HashMap<String, &Value> = sessioni.iter().map(|v| (v["sessione"].as_str().unwrap_or("").to_string(), v)).collect();
    #[derive(Default, Clone)]
    struct Tok { costo: f64, inp: f64, out: f64, cr: f64, cw: f64, risposte: u32 }
    impl Tok { fn accumulate(&mut self, v: &UsageEntry) { self.costo += v.costo as f64; self.inp += v.inp as f64; self.out += v.out as f64; self.cr += v.cr as f64; self.cw += v.cw as f64; self.risposte += 1; }
               fn token(&self) -> f64 { self.inp + self.out + self.cr + self.cw } }
    let tipi = ["chat", "sotto_agenti", "aiutanti", "regia"];
    let mut per_tipo: [Tok; 4] = Default::default();
    let mut per_modello: [Tok; 4] = Default::default();
    let mut hours: HashMap<i64, ([f64; 4], f64)> = HashMap::new();   // start of hour -> (cost per type, tokens)
    #[derive(Default)]
    struct Chat { tot: Tok, oggi: f64, tipo: [f64; 4], peso: u64, cwd: String, ultimo: f64 }
    let mut chat: HashMap<String, Chat> = HashMap::new();
    let mut seen: HashSet<u64> = HashSet::new();
    for f in cache.values() {
        if f.sessione.starts_with("locale:") { continue; }
        let regia = js.get(&f.sessione).map(|v| !v["regia_nome"].as_str().unwrap_or("").is_empty()).unwrap_or(false);
        let tipo = match f.tipo { 1 => 1usize, 2 => 2, _ => if regia { 3 } else { 0 } };
        let c = chat.entry(f.sessione.clone()).or_default();
        if tipo != 1 && f.peso > c.peso { c.peso = f.peso; }
        if c.cwd.is_empty() && !f.cwd.is_empty() { c.cwd = f.cwd.clone(); }
        for v in &f.voci {
            if v.costo <= 0.0 || v.t < da { continue; }
            if v.idh != 0 && !seen.insert(v.idh) { continue; }
            per_tipo[tipo].accumulate(v);
            per_modello[(v.fam as usize).min(3)].accumulate(v);
            let h = ((v.t + tz_offset) / 3600.0).floor() as i64;
            let e = hours.entry(h).or_default();
            e.0[tipo] += v.costo as f64; e.1 += (v.inp + v.out + v.cr + v.cw) as f64;
            c.tot.accumulate(v); c.tipo[tipo] += v.costo as f64;
            if v.t >= midnight { c.oggi += v.costo as f64; }
            if v.t > c.ultimo { c.ultimo = v.t; }
        }
    }
    drop(g);
    let totale: f64 = per_tipo.iter().map(|x| x.costo).sum();
    let tot_token: f64 = per_tipo.iter().map(|x| x.token()).sum();
    let tot_oggi: f64 = chat.values().map(|c| c.oggi).sum();
    let r1 = |x: f64| (x * 10.0).round() / 10.0;
    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    let quota = |c: f64| if totale > 0.0 { r1(c / totale * 100.0) } else { 0.0 };
    let tok_json = |x: &Tok| json!({"costo": r2(x.costo), "token": x.token().round(), "entrata": x.inp.round(), "uscita": x.out.round(), "cache_letta": x.cr.round(), "cache_scritta": x.cw.round(), "risposte": x.risposte, "quota_token": quota(x.costo)});
    // chats: the most expensive on top, with Juggl name and project if present
    let mut elenco_chat: Vec<(&String, &Chat)> = chat.iter().filter(|(_, c)| c.tot.costo > 0.0).collect();
    elenco_chat.sort_by(|a, b| b.1.tot.costo.partial_cmp(&a.1.tot.costo).unwrap_or(std::cmp::Ordering::Equal));
    let nome_prog = |sid: &str, cwd: &str| -> String {
        let p = js.get(sid).map(|v| v["progetto"]["nome"].as_str().unwrap_or("").to_string()).filter(|x| !x.is_empty());
        p.unwrap_or_else(|| cwd.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string())
    };
    let chat_json: Vec<Value> = elenco_chat.iter().take(60).map(|(sid, c)| {
        let j = js.get(sid.as_str());
        let mut o = tok_json(&c.tot);
        o["sessione"] = json!(sid);
        o["nome"] = j.map(|v| v["nome"].clone()).unwrap_or(Value::Null);
        o["progetto"] = json!(nome_prog(sid, &c.cwd));
        o["juggl"] = json!(j.is_some());
        o["oggi_quota_token"] = json!(if tot_oggi > 0.0 { r1(c.oggi / tot_oggi * 100.0) } else { 0.0 });
        o["per_tipo"] = json!({"chat": quota(c.tipo[0]), "sotto_agenti": quota(c.tipo[1]), "aiutanti": quota(c.tipo[2]), "regia": quota(c.tipo[3])});
        o["transcript_mb"] = json!((c.peso as f64 / 1_048_576.0).round());
        o["ultimo"] = json!(c.ultimo);
        o
    }).collect();
    // projects
    let mut prog: HashMap<String, Tok> = HashMap::new();
    for (sid, c) in &chat { if c.tot.costo > 0.0 { let t = prog.entry(nome_prog(sid, &c.cwd)).or_default(); t.costo += c.tot.costo; t.inp += c.tot.inp; t.out += c.tot.out; t.cr += c.tot.cr; t.cw += c.tot.cw; t.risposte += c.tot.risposte; } }
    let mut prog: Vec<(String, Tok)> = prog.into_iter().collect();
    prog.sort_by(|a, b| b.1.costo.partial_cmp(&a.1.costo).unwrap_or(std::cmp::Ordering::Equal));
    let progetti_json: Vec<Value> = prog.iter().take(20).map(|(n, t)| { let mut o = tok_json(t); o["progetto"] = json!(n); o }).collect();
    // hours of the week, oldest first
    let mut chiavi: Vec<i64> = hours.keys().copied().collect(); chiavi.sort();
    let ore_json: Vec<Value> = chiavi.iter().map(|h| { let (c, tk) = hours[h]; json!({"t": h * 3600 - tz_offset as i64, "costo": r2(c.iter().sum::<f64>()), "token": tk.round(), "chat": r2(c[0]), "sotto_agenti": r2(c[1]), "aiutanti": r2(c[2]), "regia": r2(c[3])}) }).collect();
    // notices
    let mut avvisi: Vec<Value> = vec![];
    let nome_di = |sid: &str| js.get(sid).and_then(|v| v["nome"].as_str()).filter(|x| !x.is_empty()).map(|x| x.to_string()).unwrap_or_else(|| format!("chat {}", &sid[..sid.len().min(8)]));
    for (sid, c) in &elenco_chat {
        let q = if tot_oggi > 0.0 { c.oggi / tot_oggi * 100.0 } else { 0.0 };
        if q >= 12.0 && c.oggi > 0.5 { avvisi.push(json!({"tipo": "oggi", "sessione": sid, "valore": r1(q), "testo": format!("La chat {} ha usato il {}% dei token pesati di oggi.", nome_di(sid), q.round())})); }
        let mb = c.peso as f64 / 1_048_576.0;
        if mb >= 100.0 { avvisi.push(json!({"tipo": "contesto_enorme", "sessione": sid, "valore": mb.round(), "testo": format!("La conversazione della chat {} pesa {} MB: ogni ripresa rilegge tantissimo. Conviene aprire una chat nuova per il prossimo argomento.", nome_di(sid), mb.round())})); }
    }
    #[cfg(not(feature = "personale"))]
    let regia_non_distinta = "";
    json!({
        "ok": true, "calcolato": now, "da": da, "a": now, "fuso": tz_offset,
        "limite": seven_day_limit().map(|(p, r, a)| json!({"pct": p, "reset": r, "letto": a})),
        "totale": {"costo": r2(totale), "token": tot_token.round()},
        "per_tipo": json!({"chat": tok_json(&per_tipo[0]), "sotto_agenti": tok_json(&per_tipo[1]), "aiutanti": tok_json(&per_tipo[2]), "regia": tok_json(&per_tipo[3])}),
        "per_modello": json!({"opus": tok_json(&per_modello[0]), "sonnet": tok_json(&per_modello[1]), "haiku": tok_json(&per_modello[2]), "fable": tok_json(&per_modello[3])}),
        "ore": ore_json, "chat": chat_json, "progetti": progetti_json, "avvisi": avvisi, "tipi": tipi,
        "nota": "Quota dei token: parte dei token pesati (entrata 1, uscita 5, cache letta 0,1, cache scritta 1,25; per modello Haiku 0,33, Sonnet 1, Opus e Fable 1,67) sul totale della settimana. Non e' la percentuale ufficiale di Anthropic, che da' solo il totale.",
        "regia_non_distinta": regia_non_distinta,
    })
}

#[cfg(test)]
mod detail_tests {
    use super::*;
    #[test]
    fn recognises_helper_prompts() {
        assert!(is_helper("Sei l'assistente di Mario. Mario segue molte sessioni"));
        assert!(is_helper("  Sei il pilota automatico di Mario. Un agente chiede il permesso"));
        assert!(is_helper("Dai un nome breve a questa chat di lavoro: italiano"));
        assert!(is_helper("Fai da filtro per Mario. In un gruppo di messaggi"));
        assert!(is_helper("Sei dentro un'app che mostra a Mario il lavoro di un agente"));
        assert!(is_helper("Mario sta parlando a voce con un agente di programmazione, dal telefono"));
        assert!(!is_helper("Aggiungi una pagina al sito e controlla i test"));
        assert!(!is_helper("[Da Juggl · regia] Sei l'assistente"), "una chat vera che cita l'inizio non e' un aiutante");
    }

    /// Test on real data (run by hand with --ignored): writes the detail to /tmp/dettaglio-prova.json to compare it with
    /// ~/tavolo/regia/misure/consumi24h.py on the same transcripts (same window, same deduplication).
    #[test]
    #[ignore]
    fn real_detail() {
        let v = breakdown(vec![]);
        std::fs::write("/tmp/dettaglio-prova.json", serde_json::to_string_pretty(&v).unwrap()).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn fable_has_opus_price_and_split_quota() {
        assert_eq!(prices(3), prices(0));
        assert_ne!(prices(3), prices(1));
        assert_eq!(prices(2)[0], 1.0);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn data() {
        assert_eq!(super::iso_to_epoch("1970-01-02T00:00:01.000Z"), Some(86401.0));
        assert_eq!(super::iso_to_epoch("2026-11-08T16:09:37.030Z"), Some(1794154177.0));
    }
}

#[cfg(test)]
mod limits_tests {
    use super::*;
    #[test]
    fn iso_dates() {
        assert_eq!(epoch_of("2026-11-15T03:59:59.926056+00:00"), Some(1794715199.0));
        assert_eq!(epoch_of("2026-11-15T05:59:59+02:00"), Some(1794715199.0));
        assert_eq!(epoch_of("x"), None);
    }
    /// Real read from the connected account (writes the real file, with the right data).
    #[test]
    #[ignore]
    fn real_limits() {
        let p = refresh_from_server().expect("lettura");
        println!("settimana {p}% · validi: {}", valid_limits());
        assert!(p >= 0.0 && p <= 100.0);
        assert!(!valid_limits().is_null());
    }
}


// ---------- usage notices (Nov 11) ----------
// Every 2 minutes (main.rs): a chat that burns a lot in a short time, the automatic pause (off by default) and the
// evening summary. State in ~/tavolo/limiti-avvisi.json, written safely.

fn notices_file() -> PathBuf { crate::base_dir().join("limiti-avvisi.json") }   // rename (Nov 14)

/// Thresholds for "high usage" in 30 minutes: 2% of the week, or 8 equivalent $ if the ratio is not known.
const THRESHOLD_PCT_30: f64 = 2.0;
const THRESHOLD_USD_30: f64 = 8.0;

/// Checks the usage. Returns the notices to show in the app (high usage) and the sessions to pause.
pub fn check(sessioni: Vec<Value>, imp: &Value) -> (Vec<Value>, Vec<String>) {
    let a = analysis(sessioni.clone());
    if !a["ok"].as_bool().unwrap_or(false) { return (vec![], vec![]); }
    let now = now_secs();
    let mut st = read_json(&notices_file()).unwrap_or(json!({}));
    let mut avvisi = vec![];
    let fine = a["fine_ore_ritmo_ora"].as_f64();
    // 1. chats that consume a lot in a short time (once every 2 hours per chat)
    for s in a["sessioni"].as_array().cloned().unwrap_or_default() {
        let sid = s["sessione"].as_str().unwrap_or("").to_string();
        if sid.is_empty() || sid.starts_with("locale:") { continue; }
        let (p30, c30) = (s["pct_m30"].as_f64(), s["m30"].as_f64().unwrap_or(0.0));
        let alto = match p30 { Some(p) => p >= THRESHOLD_PCT_30, None => c30 >= THRESHOLD_USD_30 };
        if !alto { continue; }
        if now - st["allarmi"][&sid].as_f64().unwrap_or(0.0) < 7200.0 { continue; }
        st["allarmi"][&sid] = json!(now);
        let nome = s["nome"].as_str().unwrap_or("Una chat").to_string();
        let quanto = match p30 { Some(p) => format!("{:.1}% della settimana negli ultimi 30 minuti", p).replace('.', ","), None => format!("circa {:.0} $ negli ultimi 30 minuti", c30) };
        let causa = if s["sotto_ora"].as_f64().unwrap_or(0.0) >= 50.0 { ", soprattutto sotto-agenti" } else { "" };
        let pace = fine.map(|h| if h < 48.0 { format!(" A questo ritmo il limite finisce tra {:.0} ore.", h.max(1.0)) } else { String::new() }).unwrap_or_default();
        let text = format!("{quanto}{causa}.{pace}");
        crate::session_care::record(&format!("{nome} sta consumando molto"), &text, &sid, "consumo");
        crate::session_care::notify(&format!("{nome} sta consumando molto"), &text);
        avvisi.push(json!({"sessione": sid, "nome": nome, "testo": text, "sotto": s["sotto_ora"]}));
    }
    // 2. automatic pause (off by default): above the limit threshold, working chats not pinned are paused
    let mut pausa = vec![];
    let pa = &imp["pausa_auto"];
    if pa["attiva"].as_bool().unwrap_or(false) {
        let threshold = pa["soglia"].as_f64().unwrap_or(95.0);
        if let Some(l) = a["limite"].as_object() {
            let (pct, reset) = (l.get("pct").and_then(|x| x.as_f64()).unwrap_or(0.0), l.get("reset").and_then(|x| x.as_f64()).unwrap_or(0.0));
            if pct >= threshold && (st["pausa_reset"].as_f64().unwrap_or(-1.0) - reset).abs() > 60.0 {
                st["pausa_reset"] = json!(reset);
                for v in &sessioni {
                    let id = v["sessione"].as_str().unwrap_or("");
                    if v["vista"].as_str() != Some("lavora") || id.is_empty() || id.starts_with("locale:") { continue; }
                    if v["fissata"].as_bool().unwrap_or(false) || v["nome"].as_str() == Some("Juggl") { continue; }
                    pausa.push(id.to_string());
                }
                if !pausa.is_empty() {
                    let t = format!("{} chat messe in pausa: limite settimanale al {:.0}%. Le riprendi quando vuoi.", pausa.len(), pct);
                    crate::session_care::record("Pausa automatica", &t, "", "quota");
                    crate::session_care::notify("Pausa automatica", &t);
                }
            }
        }
    }
    // 3. evening summary (21:00), once a day
    let tz_offset = local_offset();
    let giorno = ((now + tz_offset) / 86400.0).floor();
    let ora_locale = ((now + tz_offset) % 86400.0) / 3600.0;
    if ora_locale >= 21.0 && st["riepilogo_giorno"].as_f64() != Some(giorno) {
        st["riepilogo_giorno"] = json!(giorno);
        let pr = a["progetti_oggi"].as_array().cloned().unwrap_or_default();
        let fmt = |v: &Value| match v["pct"].as_f64() { Some(p) => format!("{} {:.1}%", v["nome"].as_str().unwrap_or("?"), p).replace('.', ","), None => format!("{} {:.0} $", v["nome"].as_str().unwrap_or("?"), v["costo"].as_f64().unwrap_or(0.0)) };
        let tot = match a["oggi"]["pct"].as_f64() { Some(p) => format!("{:.1}% della settimana", p).replace('.', ","), None => format!("{:.0} $ equivalenti", a["oggi"]["costo"].as_f64().unwrap_or(0.0)) };
        if !pr.is_empty() {
            let text = format!("Oggi {tot}. {}", pr.iter().take(4).map(fmt).collect::<Vec<_>>().join(" · "));
            crate::session_care::record("Consumi di oggi", &text, "", "quota");
            crate::session_care::notify("Consumi di oggi", &text);
        }
    }
    write_safe(&notices_file(), &st);
    (avvisi, pausa)
}

#[cfg(test)]
mod real_usage_tests {
    /// Real usage (for the usage view screenshot): writes /tmp/incorso/consumi.json.
    #[test]
    #[ignore]
    fn real_usage() {
        let _ = super::refresh_from_server();
        let mut ss = crate::list_sessions(); ss.extend(crate::archived_sessions());
        let a = super::analysis(ss);
        std::fs::write("/tmp/incorso/consumi.json", a.to_string()).unwrap();
        println!("ok={} sessioni={} ultima_ora={} oggi={} fine={} rapporto={}", a["ok"], a["sessioni"].as_array().map(|x| x.len()).unwrap_or(0), a["ultima_ora"], a["oggi"], a["fine_ore_ritmo_ora"], a["rapporto"]);
    }
}
