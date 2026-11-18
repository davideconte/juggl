//! live edits (Nov 12): the file changes of each reply, the text the agent is writing, Undo.
//!
//! Neutral structure, the same for Claude and Codex: for each reply one event
//! `{"tipo": "modifiche", "da", "a", "elenco": [{path, stato, piu, meno, pezzi: [{a, b, righe}], ...}], "piu", "meno", "n"}`
//! where `righe` are diff lines with the sign in front (" ", "-", "+"), `a`/`b` the first line of the chunk before/after.
//! - Claude: from the Edit/Write results in the transcript (`toolUseResult`: originalFile, oldString/newString, content).
//!   The count is the net diff between the file at the first change of the reply and the file after the last one.
//! - Codex (and other engines): a line `{"type": "juggl_modifiche", "timestamp", "cwd", "diff": "<unified diff of the turn>"}`
//!   in the transcript mirror; the last one of the reply wins.
//! No original file is saved: the texts stay in memory only while the transcript is read.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Above this size a file is not compared (no diff, only the count from Claude's chunks).
const MAX_FILE: usize = 2_000_000;
/// Max diff lines inside one event (the rest is visible in the editor); max characters per line.
const MAX_LINES: usize = 400;
const MAX_CHARS: usize = 400;
/// Context lines around each changed chunk.
const CONTEXT: usize = 3;
/// Above this number of differences the line-by-line comparison stops: the middle block becomes "all removed, all added".
const MAX_D: usize = 1500;

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str { v.get(k).and_then(|x| x.as_str()).unwrap_or("") }

// ---------------------------------------------------------------- files not to show

/// Files with keys, passwords and certificates: lines are counted, the content never leaves (not even to the iPhone).
pub fn is_reserved(path: &str) -> bool {
    if ["/.config/", "/.ssh/", "/.aws/", "/.gnupg/", "/.kube/", "/.docker/", "/Library/Keychains/"].iter().any(|d| path.contains(d)) { return true; }
    let n = Path::new(path).file_name().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
    n == ".env" || n.starts_with(".env.") || n.ends_with(".env") || n.starts_with("id_rsa") || n.starts_with("id_ed25519")
        || [".pem", ".key", ".p12", ".pfx", ".p8", ".keychain", ".mobileprovision"].iter().any(|e| n.ends_with(e))
        || ["secret", "credential", "password", "token"].iter().any(|p| n.contains(p))
        || matches!(n.as_str(), ".netrc" | ".npmrc" | ".pypirc" | "auth.json" | "oauth_creds.json")
}

// ---------------------------------------------------------------- line-by-line comparison

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op { Uguale, Tolta, Messa }

/// Lines with their newline (to also compare "missing newline at the end").
fn split_lines(t: &str) -> Vec<&str> { t.split_inclusive('\n').collect() }

/// Myers O(ND) with the trace kept only on the useful window (memory ~D²). None above `cap` differences.
fn myers(a: &[&str], b: &[&str], cap: usize) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let lim = (a.len() + b.len()).min(cap) as isize;
    let off = lim + 1;
    let mut v = vec![0isize; (2 * lim + 3) as usize];
    let mut trace: Vec<Vec<isize>> = vec![];
    for d in 0..=lim {
        trace.push(v[(off - d) as usize..=(off + d) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[(off + k - 1) as usize] < v[(off + k + 1) as usize]) { v[(off + k + 1) as usize] } else { v[(off + k - 1) as usize] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] { x += 1; y += 1; }
            v[(off + k) as usize] = x;
            if x >= n && y >= m { return Some(backtrack(&trace, n, m)); }
            k += 2;
        }
    }
    None
}

fn backtrack(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<Op> {
    let (mut x, mut y) = (n, m);
    let mut ops = vec![];
    for d in (1..trace.len() as isize).rev() {
        let vv = &trace[d as usize];
        let get = |k: isize| vv[(k + d) as usize];
        let k = x - y;
        let pk = if k == -d || (k != d && get(k - 1) < get(k + 1)) { k + 1 } else { k - 1 };
        let px = get(pk);
        let py = px - pk;
        while x > px && y > py { ops.push(Op::Uguale); x -= 1; y -= 1; }
        if x == px { ops.push(Op::Messa); y -= 1; } else { ops.push(Op::Tolta); x -= 1; }
    }
    while x > 0 && y > 0 { ops.push(Op::Uguale); x -= 1; y -= 1; }
    ops.reverse();
    ops
}

/// Sequence of operations between two texts: equal parts at the head and tail are removed before comparing.
fn diff_ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Op> {
    let mut p = 0;
    while p < a.len() && p < b.len() && a[p] == b[p] { p += 1; }
    let mut q = 0;
    while q < a.len() - p && q < b.len() - p && a[a.len() - 1 - q] == b[b.len() - 1 - q] { q += 1; }
    let (ma, mb) = (&a[p..a.len() - q], &b[p..b.len() - q]);
    let middle = myers(ma, mb, MAX_D).unwrap_or_else(|| {
        let mut o = vec![Op::Tolta; ma.len()]; o.extend(vec![Op::Messa; mb.len()]); o
    });
    let mut ops = vec![Op::Uguale; p];
    ops.extend(middle);
    ops.extend(vec![Op::Uguale; q]);
    ops
}

fn strip_eol(r: &str) -> &str { r.strip_suffix('\n').map(|x| x.strip_suffix('\r').unwrap_or(x)).unwrap_or(r) }

/// Changed chunks with `CONTEXT` lines around: [{a, b, righe: [" x", "-y", "+z"]}] and the counts.
pub fn chunks(before: &str, after: &str) -> (Vec<Value>, u64, u64) {
    let (ra, rb) = (split_lines(before), split_lines(after));
    let ops = diff_ops(&ra, &rb);
    // position of each operation in the two texts
    let mut pos = Vec::with_capacity(ops.len());
    let (mut ia, mut ib) = (0usize, 0usize);
    for o in &ops {
        pos.push((ia, ib));
        match o { Op::Uguale => { ia += 1; ib += 1; } Op::Tolta => ia += 1, Op::Messa => ib += 1 }
    }
    let changes: Vec<usize> = ops.iter().enumerate().filter(|(_, o)| **o != Op::Uguale).map(|(i, _)| i).collect();
    let (mut added, mut removed) = (0u64, 0u64);
    let mut out = vec![];
    let mut i = 0;
    while i < changes.len() {
        let start = changes[i].saturating_sub(CONTEXT);
        let mut end = changes[i];
        while i + 1 < changes.len() && changes[i + 1] - end <= 2 * CONTEXT + 1 { i += 1; end = changes[i]; }
        let end = (end + CONTEXT).min(ops.len() - 1);
        let mut rr = vec![];
        for j in start..=end {
            let (x, y) = pos[j];
            match ops[j] {
                Op::Uguale => rr.push(format!(" {}", strip_eol(ra[x]))),
                Op::Tolta => { removed += 1; rr.push(format!("-{}", strip_eol(ra[x]))) }
                Op::Messa => { added += 1; rr.push(format!("+{}", strip_eol(rb[y]))) }
            }
        }
        let (x, y) = pos[start];
        out.push(json!({"a": x + 1, "b": y + 1, "righe": rr}));
        i += 1;
    }
    (out, added, removed)
}

// ---------------------------------------------------------------- unified diff (Codex)

/// A unified diff (also with several files, git style) split by file: [(path, stato, pezzi, piu, meno)].
pub fn read_unified(diff: &str, cwd: &str) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    let (mut old_path, mut new_path) = (String::new(), String::new());
    let mut cur: Option<Value> = None;
    let close = |cur: &mut Option<Value>, out: &mut Vec<Value>| { if let Some(c) = cur.take() { out.push(c); } };
    let absolute = |p: &str| -> String {
        let p = p.trim();
        let p = p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p);
        if p.starts_with('/') || cwd.is_empty() { p.to_string() } else { Path::new(cwd).join(p).to_string_lossy().to_string() }
    };
    for r in diff.lines() {
        if r.starts_with("diff --git ") { close(&mut cur, &mut out); old_path.clear(); new_path.clear(); continue; }
        if let Some(p) = r.strip_prefix("--- ") { close(&mut cur, &mut out); old_path = p.split('\t').next().unwrap_or("").to_string(); continue; }
        if let Some(p) = r.strip_prefix("+++ ") {
            new_path = p.split('\t').next().unwrap_or("").to_string();
            let (state, path) = if old_path == "/dev/null" { ("creato", absolute(&new_path)) } else if new_path == "/dev/null" { ("tolto", absolute(&old_path)) } else { ("modificato", absolute(&new_path)) };
            cur = Some(json!({"path": path, "stato": state, "pezzi": [], "piu": 0, "meno": 0}));
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        if let Some(t) = r.strip_prefix("@@ ") {
            // @@ -a,b +c,d @@
            let mut it = t.split_whitespace();
            let num = |x: Option<&str>| x.and_then(|v| v[1..].split(',').next()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(1);
            let (a, b) = (num(it.next()), num(it.next()));
            c["pezzi"].as_array_mut().unwrap().push(json!({"a": a.max(1), "b": b.max(1), "righe": []}));
            continue;
        }
        let Some(p) = c["pezzi"].as_array_mut().and_then(|p| p.last_mut()) else { continue };
        match r.chars().next() {
            Some('+') => { p["righe"].as_array_mut().unwrap().push(json!(r)); c["piu"] = json!(c["piu"].as_u64().unwrap_or(0) + 1); }
            Some('-') => { p["righe"].as_array_mut().unwrap().push(json!(r)); c["meno"] = json!(c["meno"].as_u64().unwrap_or(0) + 1); }
            Some(' ') => p["righe"].as_array_mut().unwrap().push(json!(r)),
            None => p["righe"].as_array_mut().unwrap().push(json!(" ")),
            _ => {}   // "\ No newline at end of file"
        }
    }
    close(&mut cur, &mut out);
    out
}

// ---------------------------------------------------------------- collection per reply

#[derive(Default)]
struct TurnFile {
    path: String,
    /// the file at the first change of the reply: None = unknown (large file or missing data), Some(None) = did not exist
    prima: Option<Option<String>>,
    dopo: Option<String>,
    grande: bool,
    /// Claude's chunks summed (only if the net comparison cannot be done)
    somma: Vec<Value>,
    piu: u64,
    meno: u64,
}

/// Collects the changes of one reply while the transcript is read.
#[derive(Default)]
pub struct Collector {
    file: Vec<TurnFile>, motore: Option<Vec<Value>>, da: String, a: String,
    /// chat (sessionId of the lines) and start of the reply (time of the user's message): needed for the git snapshots
    sessione: String, inizio: String,
    /// files changed in the git folder during the reply (also by commands), from the snapshots
    git: Vec<Value>,
}

impl Collector {
    pub fn new() -> Self { Self::default() }

    /// One transcript line: result of a Claude Edit/Write/MultiEdit, or a neutral line from another engine.
    pub fn see(&mut self, d: &Value) {
        let when = str_of(d, "timestamp");
        if self.sessione.is_empty() { let id = str_of(d, "sessionId"); if !id.is_empty() { self.sessione = id.to_string(); } }
        if str_of(d, "type") == "juggl_modifiche" {
            self.motore = Some(read_unified(str_of(d, "diff"), str_of(d, "cwd")));
            self.mark(when);
            return;
        }
        let Some(r) = d.get("toolUseResult").filter(|r| r.is_object()) else { return };
        let path = str_of(r, "filePath");
        if path.is_empty() || r.get("structuredPatch").is_none() && r.get("content").is_none() { return; }
        // NotebookEdit and similar: no text to compare, only the count from the chunks
        let orig = r.get("originalFile").and_then(|x| x.as_str());
        let large = orig.map(|o| o.len() > MAX_FILE).unwrap_or(false) || str_of(r, "content").len() > MAX_FILE;
        let created = str_of(r, "type") == "create" || (r.get("originalFile").map(|x| x.is_null()).unwrap_or(false) && r.get("content").is_some());
        let before_here: Option<Option<String>> = if large { None } else if created { Some(None) } else { orig.map(|o| Some(o.to_string())) };
        let after_here: Option<String> = if large { None } else if let Some(c) = r.get("content").and_then(|x| x.as_str()) { Some(c.to_string()) }
            else if let (Some(o), Some(old_str), Some(new_str)) = (orig, r.get("oldString").and_then(|x| x.as_str()), r.get("newString").and_then(|x| x.as_str())) {
                if old_str.is_empty() { None }
                else if r.get("replaceAll").and_then(|x| x.as_bool()).unwrap_or(false) { Some(o.replace(old_str, new_str)) } else { Some(o.replacen(old_str, new_str, 1)) }
            } else { None };
        let (mut added, mut removed) = (0, 0);
        let mut summed = vec![];
        if let Some(Value::Array(sp)) = r.get("structuredPatch") {
            for h in sp {
                let rr: Vec<Value> = h.get("lines").and_then(|l| l.as_array()).cloned().unwrap_or_default();
                for l in &rr { match l.as_str().and_then(|x| x.chars().next()) { Some('+') => added += 1, Some('-') => removed += 1, _ => {} } }
                summed.push(json!({"a": h.get("oldStart").cloned().unwrap_or(json!(1)), "b": h.get("newStart").cloned().unwrap_or(json!(1)), "righe": rr}));
            }
        }
        let i = match self.file.iter().position(|f| f.path == path) {
            Some(i) => i,
            None => { self.file.push(TurnFile { path: path.to_string(), prima: before_here.clone(), ..Default::default() }); self.file.len() - 1 }
        };
        let f = &mut self.file[i];
        f.grande |= large;
        if f.prima.is_none() && f.dopo.is_none() && f.somma.is_empty() { f.prima = before_here; }
        f.dopo = after_here;
        f.somma.extend(summed);
        f.piu += added; f.meno += removed;
        self.mark(when);
    }

    fn mark(&mut self, when: &str) {
        if self.da.is_empty() || (!when.is_empty() && when < self.da.as_str()) { self.da = when.to_string(); }
        if when > self.a.as_str() { self.a = when.to_string(); }
    }

    /// End of the reply (`fine` = time of the message that opens the next one, "" = it is the last one, maybe still running):
    /// the "modifiche" event, None if no file really changed. The collection restarts empty for the next reply.
    pub fn close(&mut self, end: &str) -> Option<Value> {
        let mut r = std::mem::take(self);
        self.sessione = r.sessione.clone();
        self.inizio = end.to_string();
        // start of the reply: the user's message; if the read window starts mid-reply, the first change
        let start = if r.inizio.is_empty() { r.da.clone() } else { r.inizio.clone() };
        let mut t = r.a.clone();
        if !r.sessione.is_empty() && !start.is_empty() {
            let (g, tg) = git_folder::for_turn(&r.sessione, &start, end);
            r.git = g;
            if tg > t { t = tg; }
        }
        let list = r.file_list(true);
        if list.is_empty() { return None; }
        let (added, removed) = list.iter().fold((0, 0), |(p, m), f| (p + f["piu"].as_u64().unwrap_or(0), m + f["meno"].as_u64().unwrap_or(0)));
        Some(json!({"tipo": "modifiche", "t": t, "da": start, "a": end, "n": list.len(), "piu": added, "meno": removed, "elenco": list}))
    }

    /// Per file: net (or summed) chunks, counts, state. `trim`: limits for the interface (lines, sensitive files).
    fn file_list(&self, trim: bool) -> Vec<Value> {
        let mut out: Vec<Value> = vec![];
        if let Some(m) = &self.motore { out = m.clone(); }
        else {
            for f in &self.file {
                let (pz, added, removed, state) = match (&f.prima, &f.dopo) {
                    (Some(p), Some(d)) if !f.grande => {
                        let (pz, added, removed) = chunks(p.as_deref().unwrap_or(""), d);
                        (pz, added, removed, if p.is_none() { "creato" } else { "modificato" })
                    }
                    _ => (f.somma.clone(), f.piu, f.meno, if matches!(f.prima, Some(None)) { "creato" } else { "modificato" }),
                };
                if added == 0 && removed == 0 && state != "creato" { continue; }   // changes later put back as they were
                out.push(json!({"path": f.path, "stato": state, "piu": added, "meno": removed, "pezzi": pz, "grande": f.grande}));
            }
        }
        // the git folder also sees files changed by commands: for the files it has, its (net) diff wins
        for g in &self.git {
            let p = str_of(g, "path");
            match out.iter().position(|f| str_of(f, "path") == p) { Some(i) => out[i] = g.clone(), None => out.push(g.clone()) }
        }
        if !trim { return out; }
        // the line limit is split among the files: the last file also gets lines to show
        let per_file = (MAX_LINES / out.len().max(1)).max(80);
        for f in out.iter_mut() {
            let mut remaining = per_file;
            let p = str_of(f, "path").to_string();
            if is_reserved(&p) { f["pezzi"] = json!([]); f["nascosto"] = json!(true); f["piu"] = json!(0); f["meno"] = json!(0); continue; }
            let mut truncated = false;
            let mut new_chunks = vec![];
            for pz in f["pezzi"].as_array().cloned().unwrap_or_default() {
                if remaining == 0 { truncated = true; break; }
                let rr: Vec<Value> = pz["righe"].as_array().cloned().unwrap_or_default();
                let kept: Vec<Value> = rr.iter().take(remaining).map(|x| {
                    let t = x.as_str().unwrap_or("");
                    if t.chars().count() > MAX_CHARS { json!(format!("{}…", t.chars().take(MAX_CHARS).collect::<String>())) } else { x.clone() }
                }).collect();
                if kept.len() < rr.len() { truncated = true; }
                remaining -= kept.len();
                new_chunks.push(json!({"a": pz["a"], "b": pz["b"], "righe": kept}));
            }
            f["pezzi"] = json!(new_chunks);
            if truncated { f["tagliato"] = json!(true); }
        }
        out
    }
}

// ---------------------------------------------------------------- live preview (what is being written)

/// Value (even incomplete) of a string key inside a JSON that is still arriving.
fn partial_string(j: &str, key: &str) -> Option<(String, bool)> {
    let k = format!("\"{key}\"");
    let i = j.find(&k)?;
    let rest = j[i + k.len()..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut it = rest.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some((out, true)),
            '\\' => match it.next() {
                Some('n') => out.push('\n'), Some('t') => out.push('\t'), Some('r') => {}, Some('"') => out.push('"'),
                Some('\\') => out.push('\\'), Some('/') => out.push('/'), Some('b') | Some('f') => {},
                Some('u') => {
                    let h: String = it.by_ref().take(4).collect();
                    if h.len() < 4 { return Some((out, false)); }
                    if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) { out.push(ch); }
                }
                Some(x) => out.push(x),
                None => return Some((out, false)),
            },
            c => out.push(c),
        }
    }
    Some((out, false))
}

/// Line of the file where the text to replace starts (Edit): read once per tool, then from the cache.
fn line_of(path: &str, needle: &str) -> Option<usize> {
    static CACHE: Mutex<Option<HashMap<(String, u64), Option<usize>>>> = Mutex::new(None);
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    needle.hash(&mut h);
    let k = (path.to_string(), h.finish());
    let mut g = CACHE.lock().ok()?;
    let c = g.get_or_insert_with(HashMap::new);
    if let Some(r) = c.get(&k) { return *r; }
    if c.len() > 200 { c.clear(); }
    let r = std::fs::metadata(path).ok().filter(|m| m.len() as usize <= MAX_FILE)
        .and_then(|_| std::fs::read_to_string(path).ok())
        .and_then(|t| t.find(needle).map(|i| t[..i].matches('\n').count() + 1));
    c.insert(k, r);
    r
}

/// The last lines of the text the agent is writing (Edit/Write/MultiEdit) from the still incomplete input JSON.
/// `{"righe": [...], "da": number of the first line shown or null, "piu", "meno"}`; Null for the other tools
/// and for sensitive files.
pub fn preview(name: &str, partial_json: &str) -> Value {
    if !matches!(name, "Edit" | "Write" | "MultiEdit") { return Value::Null; }
    let path = partial_string(partial_json, "file_path").filter(|x| x.1).map(|x| x.0).unwrap_or_default();
    if path.is_empty() || is_reserved(&path) { return Value::Null; }
    // Codex (codex_engine.rs): the input is {file_path, diff} with the diff of the single file; show the added lines
    if let Some((diff, _)) = partial_string(partial_json, "diff") {
        let mut b: Option<usize> = None;
        let mut added_lines: Vec<(Option<usize>, String)> = vec![];
        let (mut added, mut removed) = (0u64, 0u64);
        for r in diff.split('\n') {
            if let Some(t) = r.strip_prefix("@@ ") { b = t.split_whitespace().nth(1).and_then(|x| x[1..].split(',').next()).and_then(|x| x.parse().ok()); continue; }
            if r.starts_with("+++") || r.starts_with("---") { continue; }
            match r.chars().next() {
                Some('+') => { added += 1; added_lines.push((b, r[1..].to_string())); b = b.map(|x| x + 1); }
                Some('-') => removed += 1,
                Some(' ') => b = b.map(|x| x + 1),
                _ => {}
            }
        }
        let last: Vec<&(Option<usize>, String)> = added_lines.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
        let from = last.first().and_then(|x| x.0);
        return json!({"righe": last.iter().map(|x| x.1.chars().take(200).collect::<String>()).collect::<Vec<_>>(), "da": from, "piu": added, "meno": removed});
    }
    let (text, key) = if name == "Write" { (partial_string(partial_json, "content"), "content") } else {
        // MultiEdit: the last new_string arriving
        let i = partial_json.rfind("\"new_string\"").unwrap_or(0);
        (partial_string(&partial_json[i..], "new_string"), "new_string")
    };
    let _ = key;
    let Some((text, _)) = text else { return json!({"righe": [], "da": Value::Null, "piu": 0, "meno": 0}) };
    let (start, removed) = if name == "Write" { (Some(1), 0) } else {
        let i = partial_json.rfind("\"old_string\"").unwrap_or(0);
        match partial_string(&partial_json[i..], "old_string") {
            Some((v, true)) if !v.is_empty() => (line_of(&path, &v), v.lines().count().max(1) as u64),
            _ => (None, 0),
        }
    };
    let all_lines: Vec<&str> = text.split('\n').collect();
    let n = all_lines.len();
    let count = 6usize;
    let from = n.saturating_sub(count);
    let lines: Vec<String> = all_lines[from..].iter().map(|r| { let r = r.trim_end_matches('\r'); if r.chars().count() > 200 { r.chars().take(200).collect() } else { r.to_string() } }).collect();
    json!({"righe": lines, "da": start.map(|i| i + from), "piu": n as u64, "meno": removed})
}

// ---------------------------------------------------------------- Undo

/// Removes from the current text the chunks added by the agent (reverse diff), checking that each chunk is still there.
fn reverse_apply(current: &str, chunks: &[Value]) -> Result<String, String> {
    let trailing_newline = current.ends_with('\n');
    let mut rr: Vec<String> = current.lines().map(|x| x.to_string()).collect();
    for pz in chunks.iter().rev() {
        let lines: Vec<&str> = pz["righe"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
        let after: Vec<&str> = lines.iter().filter(|r| !r.starts_with('-')).map(|r| if r.is_empty() { "" } else { &r[1..] }).collect();
        let before: Vec<String> = lines.iter().filter(|r| !r.starts_with('+')).map(|r| if r.is_empty() { String::new() } else { r[1..].to_string() }).collect();
        let expected = pz["b"].as_u64().unwrap_or(1).saturating_sub(1) as isize;
        let matches_at = |p: isize| p >= 0 && (p as usize) + after.len() <= rr.len() && rr[p as usize..p as usize + after.len()].iter().zip(&after).all(|(a, b)| a.trim_end_matches('\r') == *b);
        // the chunk may have moved a few lines because of later changes elsewhere in the file
        let found = (0..=200isize).flat_map(|o| [expected + o, expected - o]).find(|p| matches_at(*p));
        let Some(p) = found else { return Err("il file è cambiato dopo in quel punto".into()) };
        rr.splice(p as usize..p as usize + after.len(), before);
    }
    let mut t = rr.join("\n");
    if trailing_newline && !t.is_empty() { t.push('\n'); }
    Ok(t)
}

/// Moves a file created by the agent to the Trash (Mac); elsewhere it renames it. Never deleted.
fn to_trash(p: &Path) -> Result<(), String> {
    let name = p.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let trash = std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".Trash")).filter(|c| c.is_dir() && cfg!(target_os = "macos"));
    let dest = match trash {
        Some(c) => { let mut d = c.join(&name); let mut i = 2; while d.exists() { d = c.join(format!("{name} {i}")); i += 1; } d }
        None => p.with_file_name(format!("{name}.juggl-annullato")),
    };
    std::fs::rename(p, &dest).map_err(|e| e.to_string())
}

/// Undoes the changes of a reply (all files or just one): rereads the transcript between `from` and `to`.
pub fn undo(transcript: &Path, session: Option<&str>, from: &str, to: &str, only: Option<&str>) -> Value {
    let Ok(t) = std::fs::read_to_string(transcript) else { return json!({"ok": false, "msg": "conversazione non trovata"}) };
    let mut r = Collector::new();
    for line in t.lines() {
        if !(line.contains("\"toolUseResult\"") || line.contains("\"juggl_modifiche\"")) { continue; }
        let Ok(d) = serde_json::from_str::<Value>(line) else { continue };
        if d.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
        let q = str_of(&d, "timestamp");
        if q < from || (!to.is_empty() && q > to) { continue; }
        r.see(&d);
    }
    if let Some(id) = session.filter(|x| !x.is_empty()) { r.git = git_folder::for_turn(id, from, to).0; }
    let list = r.file_list(false);
    let (mut done, mut skipped) = (vec![], vec![]);
    for f in &list {
        let path = str_of(f, "path");
        if only.map(|x| x != path).unwrap_or(false) { continue; }
        let p = Path::new(path);
        let current = std::fs::read_to_string(p).ok();
        // Claude: whole texts before and after (only for this check, then dropped)
        let ft = r.file.iter().find(|x| x.path == path);
        let outcome: Result<&str, String> = (|| {
            if let Some(TurnFile { prima: Some(before), dopo: Some(after), grande: false, .. }) = ft {
                match (before, &current) {
                    (None, Some(att)) if att == after => return to_trash(p).map(|_| "nel Cestino"),
                    (None, None) => return Ok("non c'era più"),
                    (Some(pr), Some(att)) if att == after => return std::fs::write(p, pr).map(|_| "rimesso com'era").map_err(|e| e.to_string()),
                    _ => {}
                }
            }
            if str_of(f, "stato") == "creato" {
                // new file seen by the folder: goes to the Trash only if it is still exactly the created one
                let added: Vec<&str> = f["pezzi"].as_array().into_iter().flatten().flat_map(|pz| pz["righe"].as_array().into_iter().flatten()).filter_map(|x| x.as_str().and_then(|r| r.strip_prefix('+'))).collect();
                return match &current {
                    Some(att) if !f["pezzi"].as_array().map(|x| x.is_empty()).unwrap_or(true) && att.lines().eq(added.iter().copied()) => to_trash(p).map(|_| "nel Cestino"),
                    None => Ok("non c'era più"),
                    _ => Err("il file è cambiato dopo".into()),
                };
            }
            if str_of(f, "stato") == "tolto" {
                if current.is_some() { return Err("il file c'è di nuovo".into()); }
                let before: Vec<String> = f["pezzi"].as_array().into_iter().flatten().flat_map(|pz| pz["righe"].as_array().cloned().unwrap_or_default()).filter_map(|x| x.as_str().and_then(|r| r.strip_prefix('-')).map(String::from)).collect();
                return std::fs::write(p, before.join("\n") + "\n").map(|_| "rimesso").map_err(|e| e.to_string());
            }
            if f["grande"].as_bool().unwrap_or(false) && ft.is_some() { return Err("file troppo grande".into()); }
            if f["nascosto"].as_bool().unwrap_or(false) || is_reserved(path) { return Err("file sensibile: si annulla a mano".into()); }
            if f["completo"].as_bool() == Some(false) { return Err("non si sa com'era prima: solo lettura".into()); }
            let att = current.as_deref().ok_or("il file non c'è più")?;
            let new_text = reverse_apply(att, f["pezzi"].as_array().map(|x| x.as_slice()).unwrap_or(&[]))?;
            std::fs::write(p, new_text).map(|_| "tolti i pezzi dell'agente").map_err(|e| e.to_string())
        })();
        match outcome {
            Ok(how) => done.push(json!({"path": path, "come": how})),
            Err(m) => skipped.push(json!({"path": path, "motivo": m})),
        }
    }
    let msg = if done.is_empty() && skipped.is_empty() { "Niente da annullare".to_string() }
        else if skipped.is_empty() { if done.len() == 1 { "Modifica annullata".into() } else { format!("Annullate le modifiche a {} file", done.len()) } }
        else { format!("{} annullati, {} lasciati com'erano: {}", done.len(), skipped.len(), skipped.iter().map(|x| format!("{} ({})", short_name(str_of(x, "path")), str_of(x, "motivo"))).collect::<Vec<_>>().join(", ")) };
    json!({"ok": !done.is_empty() || skipped.is_empty(), "fatti": done, "saltati": skipped, "msg": msg})
}

fn short_name(p: &str) -> String { Path::new(p).file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string()) }

/// Undo from the session (Mac and iPhone). Local chats have no Mac files: excluded.
pub fn undo_session(session: &str, from: &str, to: &str, path: Option<&str>) -> Value {
    if session.starts_with("locale:") || from.is_empty() { return json!({"ok": false, "msg": "non disponibile per questa chat"}) }
    let Some(v) = crate::find_session(session) else { return json!({"ok": false, "msg": "conversazione non trovata"}) };
    if crate::turn_view(&v) == "lavora" && to.is_empty() { return json!({"ok": false, "msg": "La chat sta ancora lavorando: annulla quando ha finito"}) }
    let Some(t) = crate::find_transcript(&v) else { return json!({"ok": false, "msg": "conversazione non trovata"}) };
    undo(&t, Some(session), from, to, path)
}

#[tauri::command]
pub fn modifiche_annulla(sessione: String, da: String, a: String, path: Option<String>) -> Value {
    undo_session(&sessione, &da, &a, path.as_deref())
}

// ---------------------------------------------------------------- git folders: snapshots

/// In git folders, changes made with commands (scripts, sed, generators) are seen by comparing snapshots of the
/// folder. A snapshot is a git tree written with a temporary Juggl index (~/tavolo/modifiche/indici):
/// the real index, the files, the refs, the stash and the hooks of the project are not touched; the repo only keeps loose objects
/// that git gc removes. On disk Juggl only keeps the tree hashes (foto.jsonl), never the content of the files.
/// The tick (every 3 s, on its own thread) takes a snapshot at the start of the work, every 5 s if something changes, and at the end.
pub fn tick(ss: &[Value]) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static BUSY: AtomicBool = AtomicBool::new(false);
    let working: Vec<(String, String)> = ss.iter()
        .filter(|v| !str_of(v, "sessione").starts_with("locale:") && crate::turn_view(v) == "lavora" && !str_of(v, "cwd").is_empty())
        .map(|v| (str_of(v, "sessione").to_string(), str_of(v, "cwd").to_string())).collect();
    // safety switch: ~/tavolo/modifiche/spento = no snapshots nor clones (only what the transcript says remains)
    if crate::base_dir().join("modifiche").join("spento").exists() { return; }
    if BUSY.swap(true, Ordering::SeqCst) { return; }
    std::thread::spawn(move || { let _ = std::panic::catch_unwind(|| git_folder::tick(&working)); BUSY.store(false, Ordering::SeqCst); });
}

/// Seconds from an ISO time of the transcripts ("2026-11-12T02:23:42.179Z"); 0 if unreadable.
fn seconds(iso: &str) -> f64 {
    let n = |a: usize, b: usize| iso.get(a..b).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(m), Some(d), Some(h), Some(mi), Some(sec)) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19)) else { return 0.0 };
    let ms = if iso.as_bytes().get(19) == Some(&b'.') { iso.get(20..23).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) / 1000.0 } else { 0.0 };
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400); let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1; let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    (days * 86400 + h * 3600 + mi * 60 + sec) as f64 + ms
}

fn now_iso() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let i = crate::search_index::iso(d.as_secs() as i64);
    format!("{}.{:03}Z", &i[..19], d.subsec_millis())
}

/// Folders synced with the cloud: never scanned (a scan on Oct 20-21 downloaded over 100 GB).
fn in_cloud(p: &str) -> bool {
    ["/Library/Mobile Documents", "/Library/CloudStorage", "/Dropbox", "/iCloud", "/Google Drive", "/OneDrive", "/Box/"].iter().any(|x| p.contains(x))
}
/// Cloud placeholder file not downloaded (SF_DATALESS): reading it would download it.
#[cfg(target_os = "macos")]
fn is_placeholder(m: &std::fs::Metadata) -> bool { use std::os::macos::fs::MetadataExt; m.st_flags() & 0x4000_0000 != 0 }
#[cfg(not(target_os = "macos"))]
fn is_placeholder(_m: &std::fs::Metadata) -> bool { false }

/// Adaptive tick (Nov 12): every 5 s; a folder or repo whose check takes over 500 ms (very large repos)
/// moves to 30 s, and goes back to 5 s when it drops below 200 ms. Each change ends up in the log (~/tavolo/juggl.log).
static INTERVALS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
fn interval(key: &str) -> u64 { INTERVALS.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(key).copied())).unwrap_or(5) }
fn record_timing(key: &str, ms: u128) {
    let Ok(mut g) = INTERVALS.lock() else { return };
    let m = g.get_or_insert_with(HashMap::new);
    let current = m.get(key).copied().unwrap_or(5);
    let next = if ms > 500 { 30 } else if ms < 200 { 5 } else { current };
    if next != current {
        m.insert(key.to_string(), next);
        crate::log_juggl(&format!("modifiche: {key} controllo in {ms} ms, ora ogni {next} s"));
    }
}

mod git_folder {
    use super::*;
    use std::io::Write;
    use std::process::{Stdio};
    use std::time::Instant;

    fn data_dir() -> PathBuf { crate::base_dir().join("modifiche") }
    fn snapshots_file() -> PathBuf { data_dir().join("foto.jsonl") }

    /// git with the Juggl index (if given), without optional locks, without fsmonitor, without automatic gc.
    fn git(repo: &str, index: Option<&Path>, args: &[&str], input: Option<&[u8]>) -> Option<Vec<u8>> {
        let mut c = crate::new_command("git");
        crate::NoConsoleWindow::no_console_window(&mut c);   // Windows: no black window at each tick
        c.arg("-C").arg(repo).args(["-c", "core.fsmonitor=false", "-c", "core.quotepath=false", "-c", "gc.auto=0", "-c", "maintenance.auto=false"]).args(args)
            .env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0")
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(i) = index { c.env("GIT_INDEX_FILE", i); }
        let mut child = c.spawn().ok()?;
        if let Some(b) = input { child.stdin.take()?.write_all(b).ok()?; }
        let o = child.wait_with_output().ok()?;
        o.status.success().then_some(o.stdout)
    }
    fn text(b: Vec<u8>) -> String { String::from_utf8_lossy(&b).trim().to_string() }

    /// Git root of the chat folder (cached). Never the whole home folder.
    fn repo_of(cwd: &str) -> Option<String> {
        static CACHE: Mutex<Option<HashMap<String, (Option<String>, Instant)>>> = Mutex::new(None);
        let mut g = CACHE.lock().ok()?;
        let c = g.get_or_insert_with(HashMap::new);
        if let Some((r, when)) = c.get(cwd) { if when.elapsed().as_secs() < 300 { return r.clone(); } }
        if in_cloud(cwd) { c.insert(cwd.to_string(), (None, Instant::now())); return None; }
        let r = git(cwd, None, &["rev-parse", "--show-toplevel"], None).map(text).filter(|x| !x.is_empty() && !in_cloud(x))
            .filter(|x| Some(x.as_str()) != std::env::var("HOME").ok().as_deref() && x != "/");
        c.insert(cwd.to_string(), (r.clone(), Instant::now()));
        r
    }

    fn index_path(repo: &str) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        repo.hash(&mut h);
        data_dir().join("indici").join(format!("{:016x}", h.finish()))
    }

    /// Snapshot of the folder: hash of the tree with all the files (also new untracked ones, respecting .gitignore).
    /// Files above MAX_FILE excluded: they stay as in the previous snapshot.
    pub(super) fn snapshot(repo: &str) -> Option<String> {
        let idx = index_path(repo);
        if !idx.exists() {
            std::fs::create_dir_all(idx.parent()?).ok()?;
            let real = text(git(repo, None, &["rev-parse", "--git-path", "index"], None)?);
            let real = if real.starts_with('/') { PathBuf::from(real) } else { Path::new(repo).join(real) };
            if real.exists() { std::fs::copy(&real, &idx).ok()?; }
        }
        let st = git(repo, Some(&idx), &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--no-renames", "--ignore-submodules"], None)?;
        let mut list: Vec<u8> = vec![];
        let mut n = 0;
        for entry in st.split(|b| *b == 0) {
            if entry.len() < 4 { continue; }
            let (xy, p) = (&entry[..2], &entry[3..]);
            // Y = folder differs from the Juggl index; "??" = new
            if xy[1] == b' ' && xy != b"??" { continue; }
            let ps = String::from_utf8_lossy(p).to_string();
            if std::fs::symlink_metadata(Path::new(repo).join(&ps)).map(|m| m.len() as usize > MAX_FILE || is_placeholder(&m)).unwrap_or(false) { continue; }
            list.extend_from_slice(p); list.push(0); n += 1;
            if n >= 5000 { break; }
        }
        if !list.is_empty() { git(repo, Some(&idx), &["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"], Some(&list))?; }
        git(repo, Some(&idx), &["write-tree"], None).map(text).filter(|x| x.len() >= 40)
    }

    fn write_entry(v: Value) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(snapshots_file()) { let _ = writeln!(f, "{v}"); }
    }

    /// Chats working now -> (repo, last tick, last tree).
    static WORKING: Mutex<Option<HashMap<String, (String, Instant, String)>>> = Mutex::new(None);

    pub(super) fn tick(working: &[(String, String)]) {
        let mut done: HashMap<String, Option<String>> = HashMap::new();   // one repo, one snapshot per tick
        let mut snap = |repo: &str| done.entry(repo.to_string()).or_insert_with(|| { let t = Instant::now(); let a = snapshot(repo); record_timing(repo, t.elapsed().as_millis()); a }).clone();
        let Ok(mut g) = WORKING.lock() else { return };
        let work = g.get_or_insert_with(HashMap::new);
        shadow::tick(working);
        for (id, cwd) in working {
            let Some(repo) = repo_of(cwd) else { continue };
            match work.get(id).cloned() {
                None => if let Some(a) = snap(&repo) {
                    write_entry(json!({"sessione": id, "repo": repo, "t": now_iso(), "albero": a, "tipo": "inizio"}));
                    work.insert(id.clone(), (repo, Instant::now(), a));
                },
                Some((r, when, last)) if when.elapsed().as_secs() >= interval(&r) => if let Some(a) = snap(&r) {
                    if a != last { write_entry(json!({"sessione": id, "repo": r, "t": now_iso(), "albero": a, "tipo": "durante"})); }
                    work.insert(id.clone(), (r, Instant::now(), a));
                },
                _ => {}
            }
        }
        let finished: Vec<String> = work.keys().filter(|k| !working.iter().any(|(id, _)| id == *k)).cloned().collect();
        for id in finished {
            if let Some((r, _, last)) = work.remove(&id) {
                let a = snap(&r).unwrap_or(last);
                write_entry(json!({"sessione": id, "repo": r, "t": now_iso(), "albero": a, "tipo": "fine"}));
            }
        }
        drop(g);
        prune();
    }

    /// foto.jsonl above 2 MB: keep the last two weeks.
    fn prune() {
        let p = snapshots_file();
        if std::fs::metadata(&p).map(|m| m.len() < 2_000_000).unwrap_or(true) { return; }
        let cutoff = seconds(&now_iso()) - 14.0 * 86400.0;
        let Ok(t) = std::fs::read_to_string(&p) else { return };
        let kept: String = t.lines().filter(|r| serde_json::from_str::<Value>(r).map(|v| seconds(str_of(&v, "t")) >= cutoff).unwrap_or(false)).map(|r| format!("{r}\n")).collect();
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, kept).is_ok() { let _ = std::fs::rename(tmp, p); }
    }

    /// Snapshots of a chat (read from the file, cached until it changes).
    fn snapshots_of(session: &str) -> Vec<Value> {
        static CACHE: Mutex<Option<(u64, std::time::SystemTime, Vec<Value>)>> = Mutex::new(None);
        let p = snapshots_file();
        let Ok(m) = std::fs::metadata(&p) else { return vec![] };
        let key = (m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH));
        let Ok(mut g) = CACHE.lock() else { return vec![] };
        if g.as_ref().map(|c| (c.0, c.1) != key).unwrap_or(true) {
            let all = std::fs::read_to_string(&p).unwrap_or_default().lines().filter_map(|r| serde_json::from_str::<Value>(r).ok()).collect();
            *g = Some((key.0, key.1, all));
        }
        g.as_ref().map(|c| c.2.iter().filter(|v| str_of(v, "sessione") == session).cloned().collect()).unwrap_or_default()
    }

    fn diff_trees(repo: &str, a: &str, b: &str) -> Vec<Value> {
        static CACHE: Mutex<Option<HashMap<(String, String), Vec<Value>>>> = Mutex::new(None);
        let k = (a.to_string(), b.to_string());
        if let Some(v) = CACHE.lock().ok().and_then(|g| g.as_ref().and_then(|c| c.get(&k).cloned())) { return v; }
        let d = git(repo, None, &["diff", "--no-color", "--no-ext-diff", "--no-renames", "--src-prefix=a/", "--dst-prefix=b/", a, b], None).unwrap_or_default();
        let d = if d.len() > 4_000_000 { d[..4_000_000].to_vec() } else { d };
        let mut v = read_unified(&String::from_utf8_lossy(&d), repo);
        for f in v.iter_mut() { f["fonte"] = json!("cartella"); }
        if let Ok(mut g) = CACHE.lock() { let c = g.get_or_insert_with(HashMap::new); if c.len() > 100 { c.clear(); } c.insert(k, v.clone()); }
        v
    }

    /// Files changed in the chat's git folders between `start` and `end` ("" = until now), and the time of the last snapshot.
    pub(super) fn for_turn(session: &str, start: &str, end: &str) -> (Vec<Value>, String) {
        if session.starts_with("locale:") { return (vec![], String::new()); }
        let all = snapshots_of(session);
        let (ti, tf) = (seconds(start), if end.is_empty() { f64::MAX } else { seconds(end) });
        let mut repos: Vec<String> = all.iter().map(|v| str_of(v, "repo").to_string()).collect();
        repos.sort(); repos.dedup();
        let (mut out, mut latest) = (vec![], String::new());
        for repo in repos {
            let here: Vec<&Value> = all.iter().filter(|v| str_of(v, "repo") == repo).collect();
            // base: the start-of-work snapshot taken within 20 s of the user's message, otherwise the last one before
            let base = here.iter().find(|v| { let t = seconds(str_of(v, "t")); t >= ti && t <= ti + 20.0 && str_of(v, "tipo") == "inizio" })
                .or_else(|| here.iter().filter(|v| seconds(str_of(v, "t")) <= ti).last());
            let end_snap = here.iter().filter(|v| seconds(str_of(v, "t")) <= tf).last();
            let (Some(b), Some(f)) = (base, end_snap) else { continue };
            if seconds(str_of(f, "t")) < seconds(str_of(b, "t")) || str_of(b, "albero") == str_of(f, "albero") { continue; }
            if str_of(f, "t") > latest.as_str() { latest = str_of(f, "t").to_string(); }
            out.extend(diff_trees(&repo, str_of(b, "albero"), str_of(f, "albero")));
        }
        // non-git folders: changes made by commands seen from the cloned copy
        let (o, t) = shadow::for_turn(session, ti, tf);
        out.extend(o);
        if t > latest { latest = t; }
        (out, latest)
    }

    pub(super) fn repo_of_pub(cwd: &str) -> Option<String> { repo_of(cwd) }
}

/// NON-git folders: at the start of the work Juggl clones (APFS, no space used until the file changes) the small text
/// files of the chat folder into ~/tavolo/modifiche/ombre/<chat>; during and at the end it compares. At the end of the work it keeps
/// only the diffs (cartelle.jsonl) and deletes the clones; at startup it deletes the leftover ones. Never: cloud, placeholders, whole home,
/// another disk (no real copies), sensitive files (only "modificato"), local chats.
mod shadow {
    use super::*;
    use std::time::Instant;

    const MAX_FILE_SHADOW: u64 = 1_000_000;
    const MAX_N: usize = 5000;
    const MAX_TOTAL: u64 = 200_000_000;
    const SKIP_DIRS: &[&str] = &["node_modules", ".git", "target", "build", "dist", ".next", ".nuxt", "DerivedData", "Pods", ".venv", "venv",
        "__pycache__", ".cache", "cache", ".gradle", "coverage", ".Trash", ".svn", ".hg", ".idea", ".turbo", ".parcel-cache", "vendor", "xcuserdata"];
    /// Juggl data folders when the chat works in ~/tavolo: Juggl writes them itself, not the agent
    const JUGGL_DATA_DIRS: &[&str] = &["stato",
        "coda", "regia", "modifiche", "sock", "allegati", "chat-locali", "contesti", "contesto",
        "passaggi", "custode", "custode-pacchetto", "generale-juggl", "motori", "rimandati", "account"];
    const SKIP_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "heic", "avif", "ico", "icns", "psd", "mp4", "mov", "webm", "mp3", "wav", "m4a", "aac",
        "pdf", "zip", "gz", "tgz", "xz", "bz2", "7z", "tar", "dmg", "ipa", "apk", "aab", "so", "dylib", "a", "o", "rlib", "rmeta", "exe", "dll", "bin",
        "db", "sqlite", "sqlite3", "db-wal", "db-shm", "wal", "shm", "lock", "log", "pid", "sock", "ttf", "otf", "woff", "woff2", "jsonl", "pyc",
        "class", "jar", "wasm", "glb", "3mf", "stl", "gguf", "safetensors", "onnx", "car", "nib", "xcassets", "DS_Store"];

    struct Shadow { cwd: PathBuf, dir: PathBuf, base: HashMap<String, (i64, u64)>, inizio: String, ultimo: Instant, altre: bool,
        cambiati: Vec<Value>, cache: HashMap<String, (i64, u64, Value)> }
    static SHADOWS: Mutex<Option<HashMap<String, Shadow>>> = Mutex::new(None);
    /// Folders that are not followed (reason), so the scan is not redone at each tick.
    static SKIPPED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    fn root_dir() -> PathBuf { crate::base_dir().join("modifiche").join("ombre") }
    fn diff_file() -> PathBuf { crate::base_dir().join("modifiche").join("cartelle.jsonl") }

    fn mtime(m: &std::fs::Metadata) -> i64 { m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0) }

    /// Text files followed in the folder: relative path -> (mtime, size). Err(reason) if not followed.
    pub(super) fn list_files(cwd: &Path) -> Result<HashMap<String, (i64, u64)>, String> {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
        let c = cwd.to_string_lossy().to_string();
        if cwd == home || c == "/" || cwd.parent().map(|p| p == Path::new("/Users") || p == Path::new("/")).unwrap_or(true) { return Err("cartella troppo grande".into()); }
        if in_cloud(&c) { return Err("cartella sincronizzata col cloud".into()); }
        let is_base = std::fs::canonicalize(cwd).ok() == std::fs::canonicalize(crate::base_dir()).ok();
        let mut out = HashMap::new();
        let mut total = 0u64;
        let mut stack: Vec<(PathBuf, usize)> = vec![(cwd.to_path_buf(), 0)];
        while let Some((d, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                let Ok(m) = e.metadata() else { continue };   // DirEntry::metadata does not follow links
                if m.file_type().is_symlink() || is_placeholder(&m) { continue; }
                if n.contains(".bak-") || n.starts_with("bak-") || n.ends_with(".bak") || n.ends_with("-bak") || n == ".DS_Store" { continue; }
                let p = e.path();
                if m.is_dir() {
                    // build outputs: Xcode DerivedData (also with any -derivedDataPath), bundles, marked caches
                    let build_output = [".noindex", ".xcresult", ".app", ".appex", ".framework", ".xcframework", ".dSYM", ".xcarchive", ".bundle"].iter().any(|x| n.ends_with(x))
                        || ["ModuleCache.noindex", "CompilationCache.noindex", "CACHEDIR.TAG"].iter().any(|x| p.join(x).exists());
                    if build_output || SKIP_DIRS.contains(&n.as_str()) || (is_base && depth == 0 && JUGGL_DATA_DIRS.contains(&n.as_str())) || depth >= 8 || in_cloud(&p.to_string_lossy()) { continue; }
                    stack.push((p, depth + 1));
                    continue;
                }
                if !m.is_file() || m.len() > MAX_FILE_SHADOW { continue; }
                if is_base && depth == 0 && !n.ends_with(".md") { continue; }   // Juggl state files in the root of ~/tavolo
                let ext = n.rsplit('.').next().unwrap_or("").to_lowercase();
                if n.contains('.') && SKIP_EXTS.contains(&ext.as_str()) { continue; }
                let rel = p.strip_prefix(cwd).unwrap_or(&p).to_string_lossy().to_string();
                total += m.len();
                out.insert(rel, (mtime(&m), m.len()));
                if out.len() > MAX_N || total > MAX_TOTAL { return Err("cartella troppo grande".into()); }
            }
        }
        Ok(out)
    }

    #[cfg(target_os = "macos")]
    fn clone_file(from: &Path, to: &Path) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let (Ok(x), Ok(y)) = (std::ffi::CString::new(from.as_os_str().as_bytes()), std::ffi::CString::new(to.as_os_str().as_bytes())) else { return false };
        unsafe { libc::clonefile(x.as_ptr(), y.as_ptr(), 0) == 0 }
    }
    #[cfg(not(target_os = "macos"))]
    fn clone_file(_from: &Path, _to: &Path) -> bool { false }

    #[cfg(unix)]
    fn same_disk(a: &Path, b: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        matches!((std::fs::metadata(a), std::fs::metadata(b)), (Ok(x), Ok(y)) if x.dev() == y.dev())
    }
    #[cfg(not(unix))]
    fn same_disk(_a: &Path, _b: &Path) -> bool { false }

    fn safe_name(id: &str) -> String { id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect() }

    /// Clones left over from an interrupted shutdown: deleted at startup (first tick).
    fn startup_cleanup() {
        static DONE: std::sync::Once = std::sync::Once::new();
        DONE.call_once(|| { let _ = std::fs::remove_dir_all(root_dir()); });
    }

    fn start(id: &str, cwd: &str) -> Result<Shadow, String> {
        let cwd = std::fs::canonicalize(cwd).map_err(|_| "cartella non trovata".to_string())?;
        let _ = std::fs::create_dir_all(root_dir());
        if !same_disk(&cwd, &root_dir()) { return Err("cartella su un altro disco".into()); }
        let base = list_files(&cwd)?;
        let dir = root_dir().join(safe_name(id));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut ok = 0;
        for rel in base.keys() {
            if is_reserved(&cwd.join(rel).to_string_lossy()) { continue; }   // sensitive files are never copied
            let a = dir.join(rel);
            if let Some(p) = a.parent() { let _ = std::fs::create_dir_all(p); }
            if clone_file(&cwd.join(rel), &a) { ok += 1; }
        }
        if ok == 0 && !base.is_empty() { let _ = std::fs::remove_dir_all(&dir); return Err("clonazione non disponibile su questo disco".into()); }
        Ok(Shadow { cwd, dir, base, inizio: now_iso(), ultimo: Instant::now(), altre: false, cambiati: vec![], cache: HashMap::new() })
    }

    fn read_text(p: &Path) -> Option<String> {
        let m = std::fs::symlink_metadata(p).ok()?;
        if is_placeholder(&m) || m.len() > MAX_FILE_SHADOW { return None; }
        let b = std::fs::read(p).ok()?;
        if b.contains(&0) { return None; }   // binary
        String::from_utf8(b).ok()
    }

    /// Comparison with the copy: changed, new, removed files (with diff; sensitive ones only flagged).
    /// Listing of a folder, done once per tick even if several chats work in it (e.g. all in ~/tavolo).
    type Listings = HashMap<PathBuf, Option<HashMap<String, (i64, u64)>>>;
    fn compare(o: &mut Shadow, listings: &mut Listings) {
        let Some(current) = listings.entry(o.cwd.clone()).or_insert_with(|| { let t = Instant::now(); let e = list_files(&o.cwd).ok(); record_timing(&o.cwd.to_string_lossy(), t.elapsed().as_millis()); e }).clone() else { return };
        let mut out = vec![];
        let mut keys: Vec<&String> = current.keys().chain(o.base.keys()).collect();
        keys.sort(); keys.dedup();
        for rel in keys {
            let (before, after) = (o.base.get(rel), current.get(rel));
            if before == after { continue; }
            let abs = o.cwd.join(rel);
            let path = abs.to_string_lossy().to_string();
            let (mt, sz) = after.copied().unwrap_or((0, 0));
            if let Some((m2, s2, v)) = o.cache.get(rel) { if *m2 == mt && *s2 == sz { out.push(v.clone()); continue; } }
            let state = if before.is_none() { "creato" } else if after.is_none() { "tolto" } else { "modificato" };
            let v = if is_reserved(&path) {
                json!({"path": path, "stato": state, "piu": 0, "meno": 0, "pezzi": [], "nascosto": true, "fonte": "cartella"})
            } else {
                let t_before = if before.is_some() { read_text(&o.dir.join(rel)) } else { Some(String::new()) };
                let t_after = if after.is_some() { read_text(&abs) } else { Some(String::new()) };
                match (t_before, t_after) {
                    (Some(a), Some(b)) => {
                        if a == b { continue; }
                        let (pz, added, removed) = chunks(&a, &b);
                        // huge diffs (file rewritten from scratch): keep a piece, and without the whole diff no Undo
                        let n: usize = pz.iter().map(|x| x["righe"].as_array().map(|r| r.len()).unwrap_or(0)).sum();
                        if n > 4000 { json!({"path": path, "stato": state, "piu": added, "meno": removed, "pezzi": pz.into_iter().take(3).collect::<Vec<_>>(), "fonte": "cartella", "completo": false, "tagliato": true}) }
                        else { json!({"path": path, "stato": state, "piu": added, "meno": removed, "pezzi": pz, "fonte": "cartella", "completo": true}) }
                    }
                    _ => json!({"path": path, "stato": state, "piu": 0, "meno": 0, "pezzi": [], "fonte": "cartella", "completo": false}),
                }
            };
            o.cache.insert(rel.clone(), (mt, sz, v.clone()));
            out.push(v);
        }
        if o.altre { for v in out.iter_mut() { v["altre"] = json!(true); } }
        o.cambiati = out;
    }

    fn save(id: &str, o: &Shadow) {
        if o.cambiati.is_empty() { return; }
        let _ = std::fs::create_dir_all(diff_file().parent().unwrap());
        let line = json!({"sessione": id, "cwd": o.cwd.to_string_lossy(), "inizio": o.inizio, "fine": now_iso(), "file": o.cambiati});
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(diff_file()) { use std::io::Write; let _ = writeln!(f, "{line}"); }
        // above 20 MB keep the last two weeks
        if std::fs::metadata(diff_file()).map(|m| m.len() > 20_000_000).unwrap_or(false) {
            let cutoff = seconds(&now_iso()) - 14.0 * 86400.0;
            if let Ok(t) = std::fs::read_to_string(diff_file()) {
                let kept: String = t.lines().filter(|r| serde_json::from_str::<Value>(r).map(|v| seconds(str_of(&v, "fine")) >= cutoff).unwrap_or(false)).map(|r| format!("{r}
")).collect();
                let _ = std::fs::write(diff_file(), kept);
            }
        }
    }

    pub(super) fn tick(working: &[(String, String)]) {
        startup_cleanup();
        let Ok(mut g) = SHADOWS.lock() else { return };
        let om = g.get_or_insert_with(HashMap::new);
        let Ok(mut gs) = SKIPPED.lock() else { return };
        let skipped = gs.get_or_insert_with(HashMap::new);
        let mut listings: Listings = HashMap::new();
        for (id, cwd) in working {
            if git_folder::repo_of_pub(cwd).is_some() { continue; }   // git folders have the snapshots
            // other chats working in the same folder, or in a folder inside/above this one (e.g. ~/tavolo and ~/tavolo/ios-testflight)
            let inside = |a: &str, b: &str| a == b || a.starts_with(&format!("{}/", b.trim_end_matches('/')));
            let others = working.iter().any(|(i2, c2)| i2 != id && (inside(c2, cwd) || inside(cwd, c2)));
            match om.get_mut(id) {
                None => {
                    if skipped.contains_key(cwd) { continue; }
                    match start(id, cwd) {
                        Ok(mut o) => { o.altre = others; om.insert(id.clone(), o); }
                        Err(m) => { skipped.insert(cwd.clone(), m); }
                    }
                }
                Some(o) => {
                    o.altre |= others;
                    if o.ultimo.elapsed().as_secs() >= interval(&o.cwd.to_string_lossy()) { compare(o, &mut listings); o.ultimo = Instant::now(); }
                }
            }
        }
        if skipped.len() > 200 { skipped.clear(); }
        let finished: Vec<String> = om.keys().filter(|k| !working.iter().any(|(id, _)| id == *k)).cloned().collect();
        for id in finished {
            if let Some(mut o) = om.remove(&id) {
                compare(&mut o, &mut listings);
                save(&id, &o);
                if o.dir.starts_with(root_dir()) { let _ = std::fs::remove_dir_all(&o.dir); }
            }
        }
    }

    /// Diff of the non-git folders for the reply [ti, tf]: from finished work (cartelle.jsonl) or from the one in progress.
    pub(super) fn for_turn(session: &str, ti: f64, tf: f64) -> (Vec<Value>, String) {
        let mut out = vec![];
        let mut latest = String::new();
        if tf == f64::MAX {
            if let Ok(g) = SHADOWS.lock() {
                if let Some(o) = g.as_ref().and_then(|m| m.get(session)) { let a = seconds(&o.inizio); if a >= ti - 1.0 && a <= ti + 20.0 { out.extend(o.cambiati.iter().cloned()); } }
            }
        }
        // fast (Nov 15): before, it reread all of cartelle.jsonl (25 MB) at EVERY reply of the window: with old messages
        // loaded it was 2 s at every chat update. Now the index (lines of this chat and their start),
        // rebuilt only when the file changes, and only the lines of this chat inside the reply are read.
        let p = diff_file();
        let idx = diff_index(&p);
        let rows: Vec<&(u64, u32, f64)> = idx.get(session).map(|v| v.iter().filter(|x| x.2 >= ti - 1.0 && x.2 <= ti + 20.0 && x.2 < tf).collect()).unwrap_or_default();
        if rows.is_empty() { return (out, latest); }
        if let Ok(mut fh) = std::fs::File::open(&p) {
            use std::io::{Read, Seek, SeekFrom};
            for &&(off, len, _) in &rows {
                let mut b = vec![0u8; len as usize];
                if fh.seek(SeekFrom::Start(off)).is_err() || fh.read_exact(&mut b).is_err() { continue; }
                let r = String::from_utf8_lossy(&b);
                let Ok(v) = serde_json::from_str::<Value>(&r) else { continue };
                if str_of(&v, "sessione") != session { continue; }
                let (a, b) = (seconds(str_of(&v, "inizio")), seconds(str_of(&v, "fine")));
                // the work started inside this reply (within 20 s of the user's message, the snapshot starts at the next tick)
                if a >= ti - 1.0 && a <= ti + 20.0 && a < tf {
                    if str_of(&v, "fine") > latest.as_str() { latest = str_of(&v, "fine").to_string(); }
                    out.extend(v["file"].as_array().cloned().unwrap_or_default());
                }
                let _ = b;
            }
        }
        (out, latest)
    }

    /// fast (Nov 15): for each chat the lines of cartelle.jsonl (offset, length, start of the work). Rebuilt only
    /// when the file changes (size or date). Fields are searched outside strings: inside the diffs quotes
    /// are written \" and cannot look like a key.
    pub(super) fn diff_index(p: &Path) -> std::sync::Arc<HashMap<String, Vec<(u64, u32, f64)>>> {
        type Index = std::sync::Arc<HashMap<String, Vec<(u64, u32, f64)>>>;
        static C: Mutex<Option<(PathBuf, u64, std::time::SystemTime, Index)>> = Mutex::new(None);
        let Ok(m) = std::fs::metadata(p) else { return Default::default() };
        let key = (m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH));
        if let Ok(g) = C.lock() { if let Some(c) = g.as_ref() { if c.0 == p && (c.1, c.2) == key { return c.3.clone(); } } }
        let mut idx: HashMap<String, Vec<(u64, u32, f64)>> = HashMap::new();
        if let Ok(t) = std::fs::read(p) {
            let field = |r: &[u8], k: &[u8]| -> Option<String> {
                let i = r.windows(k.len()).position(|w| w == k)? + k.len();
                let j = r[i..].iter().position(|&c| c == b'"')? + i;
                Some(String::from_utf8_lossy(&r[i..j]).to_string())
            };
            let mut off = 0usize;
            for r in t.split(|&c| c == b'\n') {
                let n = r.len();
                if let Some(sid) = field(r, b"\"sessione\":\"") {
                    let a = field(r, b"\"inizio\":\"").map(|x| seconds(&x)).unwrap_or(0.0);
                    idx.entry(sid).or_default().push((off as u64, n as u32, a));
                }
                off += n + 1;
            }
        }
        let idx: Index = std::sync::Arc::new(idx);
        if let Ok(mut g) = C.lock() { *g = Some((p.to_path_buf(), key.0, key.1, idx.clone())); }
        idx
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    /// fast (Nov 15): index of cartelle.jsonl: right lines per chat, diff strings not mistaken for keys.
    #[test]
    fn diff_index_rows_per_chat() {
        let d = std::env::temp_dir().join(format!("juggl-indice-diff-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let p = d.join("cartelle.jsonl");
        let r1 = r#"{"file":[{"righe":["+ \"sessione\":\"finta\""]}],"fine":"2026-11-10T10:05:00Z","inizio":"2026-11-10T10:00:00Z","sessione":"aaa"}"#;
        let r2 = r#"{"file":[],"fine":"2026-11-10T11:05:00Z","inizio":"2026-11-10T11:00:00Z","sessione":"bbb"}"#;
        let r3 = r#"{"file":[{"path":"/x"}],"fine":"2026-11-10T12:05:00Z","inizio":"2026-11-10T12:00:00Z","sessione":"aaa"}"#;
        std::fs::write(&p, format!("{r1}\n{r2}\n{r3}\n")).unwrap();
        let idx = super::shadow::diff_index(&p);
        assert_eq!(idx.get("aaa").map(|v| v.len()), Some(2));
        assert_eq!(idx.get("bbb").map(|v| v.len()), Some(1));
        assert!(idx.get("finta").is_none(), "la chiave dentro un diff non conta");
        let t = std::fs::read(&p).unwrap();
        let (off, len, a) = idx["aaa"][1];
        assert_eq!(&t[off as usize..(off + len as u64) as usize], r3.as_bytes());
        assert_eq!(a, super::seconds("2026-11-10T12:00:00Z"));
        // the file changes: the index is rebuilt
        std::fs::write(&p, format!("{r2}\n")).unwrap();
        assert!(super::shadow::diff_index(&p).get("aaa").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn net_diff_simple() {
        let (pz, added, removed) = chunks("uno\ndue\ntre\nquattro\n", "uno\nDUE\ntre\nquattro\ncinque\n");
        assert_eq!((added, removed), (2, 1));
        let r: Vec<&str> = pz[0]["righe"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        assert_eq!(r, vec![" uno", "-due", "+DUE", " tre", " quattro", "+cinque"]);
        assert_eq!(pz[0]["a"], 1);
    }

    #[test]
    fn distant_chunks_stay_separate() {
        let a: String = (1..=40).map(|i| format!("r{i}\n")).collect();
        let b = a.replace("r3\n", "X\n").replace("r35\n", "Y\n");
        let (pz, added, removed) = chunks(&a, &b);
        assert_eq!((added, removed, pz.len()), (2, 2, 2));
        assert_eq!(pz[1]["a"], 32);
    }

    #[test]
    fn myers_reverse_rebuilds() {
        // the diff applied in reverse on the "after" gives back the "before"
        let a = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let b = "a\nB\nc\nd\nx\ny\nf\ng\nh\ni\n";
        let (pz, _, _) = chunks(a, b);
        assert_eq!(reverse_apply(b, &pz).unwrap(), a);
    }

    #[test]
    fn collector_claude_net_and_created() {
        let mut r = Collector::new();
        let ed = |orig: &str, v: &str, n: &str, t: &str| json!({"type": "user", "timestamp": t, "toolUseResult": {"filePath": "/x/a.rs", "originalFile": orig, "oldString": v, "newString": n, "structuredPatch": []}});
        r.see(&ed("uno\ndue\n", "due", "DUE", "2026-11-12T10:00:00Z"));
        r.see(&ed("uno\nDUE\n", "DUE", "due", "2026-11-12T10:00:05Z"));   // put back as it was: does not count
        r.see(&json!({"type": "user", "timestamp": "2026-11-12T10:00:09Z", "toolUseResult": {"type": "create", "filePath": "/x/b.json", "content": "{}\n", "originalFile": null, "structuredPatch": []}}));
        let e = r.close("").unwrap();
        assert_eq!(e["n"], 1);
        assert_eq!(e["elenco"][0]["stato"], "creato");
        assert_eq!(e["piu"], 1);
        assert_eq!(e["da"], "2026-11-12T10:00:00Z");
        assert!(Collector::new().close("").is_none());
    }

    #[test]
    fn sensitive_without_content() {
        let mut r = Collector::new();
        r.see(&json!({"timestamp": "t", "toolUseResult": {"filePath": "/x/.env", "originalFile": "A=1\n", "oldString": "A=1", "newString": "A=2", "structuredPatch": []}}));
        let e = r.close("").unwrap();
        assert_eq!(e["elenco"][0]["nascosto"], true);
        assert_eq!(e["elenco"][0]["pezzi"].as_array().unwrap().len(), 0);
        assert_eq!(e["piu"], 0);   // sensitive: no numbers towards the interface (Nov 12)
    }

    #[test]
    fn unified_codex() {
        let d = "diff --git a/src/x.rs b/src/x.rs\n--- a/src/x.rs\n+++ b/src/x.rs\n@@ -1,3 +1,3 @@\n uno\n-due\n+DUE\n tre\ndiff --git a/n.txt b/n.txt\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1 @@\n+ciao\n";
        let f = read_unified(d, "/prog");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0]["path"], "/prog/src/x.rs");
        assert_eq!((f[0]["piu"].as_u64(), f[0]["meno"].as_u64()), (Some(1), Some(1)));
        assert_eq!(f[1]["stato"], "creato");
        assert_eq!(reverse_apply("uno\nDUE\ntre\n", f[0]["pezzi"].as_array().unwrap()).unwrap(), "uno\ndue\ntre\n");
    }

    #[test]
    fn reverse_refuses_if_changed() {
        let (pz, _, _) = chunks("a\nb\nc\n", "a\nB\nc\n");
        assert!(reverse_apply("a\nZ\nc\n", &pz).is_err());
        // moved a few lines: still found
        assert_eq!(reverse_apply("0\n1\na\nB\nc\n", &pz).unwrap(), "0\n1\na\nb\nc\n");
    }

    #[test]
    fn preview_while_writing() {
        let j = r#"{"file_path": "/tmp/nessuno.rs", "old_string": "fn a()", "new_string": "fn a() {\n    uno();\n    du"#;
        let a = preview("Edit", j);
        assert_eq!(a["righe"], json!(["fn a() {", "    uno();", "    du"]));
        assert_eq!(a["piu"], 3);
        assert!(preview("Bash", j).is_null());
        assert!(preview("Edit", r#"{"file_path": "/x/.env", "new_string": "A"#).is_null());
        let w = preview("Write", r#"{"file_path": "/a/b.txt", "content": "uno\ndue\u00e8"#);
        assert_eq!(w["righe"], json!(["uno", "dueè"]));
        assert_eq!(w["da"], 1);
        // Codex: {file_path, diff}
        let c = preview("Edit", r#"{"file_path": "/p/x.rs", "diff": "@@ -10,3 +10,4 @@\n uno\n-due\n+DUE\n+tre"#);
        assert_eq!(c["righe"], json!(["DUE", "tre"]));
        assert_eq!(c["da"], 11);
        assert_eq!((c["piu"].as_u64(), c["meno"].as_u64()), (Some(2), Some(1)));
    }

    /// cargo test --bin tavolo real_transcript_check -- --ignored --nocapture
    /// with MODIFICHE_TRANSCRIPT=<.jsonl file> and, optionally, MODIFICHE_OUT=<json with the "modifiche" events>
    #[test]
    #[ignore]
    fn real_transcript_check() {
        let p = std::env::var("MODIFICHE_TRANSCRIPT").expect("MODIFICHE_TRANSCRIPT");
        let t = std::time::Instant::now();
        let d = crate::session_detail_pub(&json!({"transcript": p}));
        let ms = t.elapsed().as_millis();
        let ev: Vec<Value> = d["eventi"].as_array().cloned().unwrap_or_default();
        let m: Vec<Value> = ev.iter().filter(|e| str_of(e, "tipo") == "modifiche").cloned().collect();
        println!("dettaglio in {ms} ms, {} eventi, {} con modifiche, peso {}", ev.len(), m.len(), d["peso"]);
        for e in &m {
            println!("  {} -> {}: {} file +{} -{} ({} byte)", str_of(e, "da"), str_of(e, "a"), e["n"], e["piu"], e["meno"], e.to_string().len());
            for f in e["elenco"].as_array().unwrap() { println!("     {} {} +{} -{} pezzi {}{}", str_of(f, "stato"), str_of(f, "path"), f["piu"], f["meno"], f["pezzi"].as_array().map(|x| x.len()).unwrap_or(0), if f.get("tagliato").is_some() { " tagliato" } else { "" }); }
        }
        if let Ok(o) = std::env::var("MODIFICHE_OUT") { std::fs::write(o, serde_json::to_string_pretty(&d).unwrap()).unwrap(); }
    }

    #[test]
    fn undo_real_file() {
        let dir = std::env::temp_dir().join(format!("juggl-modifiche-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "uno\nDUE\ntre\n").unwrap();
        let tr = dir.join("t.jsonl");
        let line = json!({"type": "user", "timestamp": "2026-11-12T10:00:00Z", "toolUseResult": {"filePath": f.to_string_lossy(), "originalFile": "uno\ndue\ntre\n", "oldString": "due", "newString": "DUE", "structuredPatch": []}});
        std::fs::write(&tr, format!("{line}\n")).unwrap();
        let e = undo(&tr, None, "2026-11-12T09:00:00Z", "2026-11-12T11:00:00Z", None);
        assert_eq!(e["ok"], true, "{e}");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "uno\ndue\ntre\n");
        // second time: the file is no longer the one left by the agent, it is not touched
        let e = undo(&tr, None, "2026-11-12T09:00:00Z", "2026-11-12T11:00:00Z", None);
        assert_eq!(e["fatti"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn large_file_fast() {
        let a: String = (0..20000).map(|i| format!("riga {i}\n")).collect();
        let b = a.replace("riga 100\n", "cento\n").replace("riga 15000\n", "");
        let t = std::time::Instant::now();
        let (_, added, removed) = chunks(&a, &b);
        assert_eq!((added, removed), (1, 2));
        assert!(t.elapsed().as_millis() < 500);
    }

    #[test]
    fn git_folder_sees_commands() {
        let dir = std::env::temp_dir().join(format!("juggl-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _base_guard = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (process-wide variable)
        std::env::set_var("JUGGL_BASE", dir.join("base"));
        let r = dir.join("repo");
        std::fs::create_dir_all(&r).unwrap();
        let g = |a: &[&str]| assert!(crate::new_command("git").arg("-C").arg(&r).args(a).output().unwrap().status.success());
        g(&["init", "-q"]);
        std::fs::write(r.join("a.txt"), "uno\ndue\n").unwrap();
        std::fs::write(r.join(".gitignore"), "ignorato.log\n").unwrap();
        g(&["add", "."]);
        g(&["-c", "user.name=p", "-c", "user.email=p@p", "commit", "-qm", "base"]);
        let rs = std::fs::canonicalize(&r).unwrap().to_string_lossy().to_string();
        let a1 = git_folder::snapshot(&rs).unwrap();
        // "command": change, new file, ignored file
        std::fs::write(r.join("a.txt"), "uno\nDUE\n").unwrap();
        std::fs::write(r.join("nuovo.txt"), "ciao\n").unwrap();
        std::fs::write(r.join("ignorato.log"), "x\n").unwrap();
        let a2 = git_folder::snapshot(&rs).unwrap();
        assert_ne!(a1, a2);
        // the real index and the repo state do not change
        let st = crate::new_command("git").arg("-C").arg(&r).args(["status", "--porcelain"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&st.stdout), " M a.txt\n?? nuovo.txt\n");
        let snap = |t: &str, a: &str, kind: &str| json!({"sessione": "s1", "repo": rs, "t": t, "albero": a, "tipo": kind});
        std::fs::create_dir_all(dir.join("base/modifiche")).unwrap();
        std::fs::write(dir.join("base/modifiche/foto.jsonl"), format!("{}\n{}\n", snap("2026-11-12T10:00:02.000Z", &a1, "inizio"), snap("2026-11-12T10:01:00.000Z", &a2, "fine"))).unwrap();
        let (f, latest) = git_folder::for_turn("s1", "2026-11-12T10:00:00.000Z", "");
        let names: Vec<&str> = f.iter().map(|x| str_of(x, "path")).collect();
        assert_eq!(names.len(), 2, "{f:?}");
        assert!(names[0].ends_with("/a.txt") && names[1].ends_with("/nuovo.txt"));
        assert_eq!(f[1]["stato"], "creato");
        assert_eq!(latest, "2026-11-12T10:01:00.000Z");
        // Undo from the folder: a.txt goes back as it was, nuovo.txt to the Trash (renamed here, outside the test Mac)
        let tr = dir.join("t.jsonl");
        std::fs::write(&tr, "").unwrap();
        let e = undo(&tr, Some("s1"), "2026-11-12T10:00:00.000Z", "", Some(names[0]));
        assert_eq!(e["fatti"].as_array().unwrap().len(), 1, "{e}");
        assert_eq!(std::fs::read_to_string(r.join("a.txt")).unwrap(), "uno\ndue\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seconds_iso() {
        assert_eq!(seconds("1970-01-01T00:00:01.500Z"), 1.5);
        assert_eq!(seconds(&crate::search_index::iso(1_762_912_222)), 1_762_912_222.0);
    }

    #[test]
    fn shadow_non_git_folder() {
        let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("juggl-ombra-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cwd = dir.join("progetto");
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        std::fs::create_dir_all(cwd.join("node_modules/x")).unwrap();
        let _base_guard = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (process-wide variable)
        std::env::set_var("JUGGL_BASE", dir.join("base"));
        std::fs::write(cwd.join("src/a.py"), "uno\ndue\n").unwrap();
        std::fs::write(cwd.join(".env"), "A=1\n").unwrap();
        std::fs::write(cwd.join("node_modules/x/i.js"), "x\n").unwrap();
        std::fs::write(cwd.join("foto.png"), [0u8, 1, 2]).unwrap();
        let c = cwd.to_string_lossy().to_string();
        let work = vec![("s9".to_string(), c.clone())];
        shadow::tick(&work);   // start: clone
        assert!(dir.join("base/modifiche/ombre/s9/src/a.py").exists(), "clone mancante");
        assert!(!dir.join("base/modifiche/ombre/s9/.env").exists(), "il file sensibile non si copia");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        // "command": change, new file, sensitive, excluded folder
        std::fs::write(cwd.join("src/a.py"), "uno\nDUE\n").unwrap();
        std::fs::write(cwd.join("src/b.py"), "ciao\n").unwrap();
        std::fs::write(cwd.join(".env"), "A=2\n").unwrap();
        std::fs::write(cwd.join("node_modules/x/i.js"), "y\n").unwrap();
        shadow::tick(&[]);   // end of work: diffs saved, clones deleted
        assert!(!dir.join("base/modifiche/ombre/s9").exists(), "cloni non cancellati");
        let t = std::fs::read_to_string(dir.join("base/modifiche/cartelle.jsonl")).unwrap();
        let v: Value = serde_json::from_str(t.lines().next().unwrap()).unwrap();
        let mut names: Vec<String> = v["file"].as_array().unwrap().iter().map(|x| Path::new(str_of(x, "path")).file_name().unwrap().to_string_lossy().to_string()).collect();
        names.sort();
        assert_eq!(names, vec![".env", "a.py", "b.py"], "{v}");
        let env = v["file"].as_array().unwrap().iter().find(|x| str_of(x, "path").ends_with(".env")).unwrap();
        assert_eq!(env["nascosto"], true);
        assert!(!t.contains("A=2") && !t.contains("A=1"), "il contenuto sensibile non si salva");
        // Undo of a.py from the saved diff
        let tr = dir.join("t.jsonl"); std::fs::write(&tr, "").unwrap();
        let a_py = cwd.join("src/a.py").to_string_lossy().to_string();
        let e = undo(&tr, Some("s9"), str_of(&v, "inizio"), "", Some(&a_py));
        assert_eq!(e["fatti"].as_array().unwrap().len(), 1, "{e}");
        assert_eq!(std::fs::read_to_string(cwd.join("src/a.py")).unwrap(), "uno\ndue\n");
        let e = undo(&tr, Some("s9"), str_of(&v, "inizio"), "", Some(&cwd.join(".env").to_string_lossy()));
        assert_eq!(e["saltati"].as_array().unwrap().len(), 1, "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shadow_never_home_or_cloud() {
        let home = std::env::var("HOME").unwrap();
        assert!(shadow::list_files(Path::new(&home)).is_err());
        assert!(shadow::list_files(Path::new(&format!("{home}/Library/Mobile Documents/com~apple~CloudDocs"))).is_err());
        assert!(shadow::list_files(Path::new(&format!("{home}/Dropbox"))).is_err());
    }

    /// Test on real folders (Nov 12). JUGGL_BASE points to a temporary folder: nothing written in ~/tavolo.
    /// MODIFICHE_CWD=<non-git folder> MODIFICHE_REPO=<git repo> MODIFICHE_OUT=<json> cargo test --bin tavolo real_folders_check -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_folders_check() {
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("juggl-vere-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _base_guard = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (process-wide variable)
        std::env::set_var("JUGGL_BASE", &tmp);
        if let Ok(cwd) = std::env::var("MODIFICHE_CWD") {
            let t = std::time::Instant::now();
            let list = shadow::list_files(Path::new(&cwd));
            println!("non git {cwd}: elenco {} in {} ms", list.as_ref().map(|x| x.len().to_string()).unwrap_or_else(|e| e.clone()), t.elapsed().as_millis());
            let t = std::time::Instant::now();
            shadow::tick(&[("vera".into(), cwd.clone())]);
            println!("  clonazione in {} ms, spazio occupato dai cloni: {}", t.elapsed().as_millis(),
                String::from_utf8_lossy(&crate::new_command("du").args(["-sh"]).arg(tmp.join("modifiche/ombre")).output().unwrap().stdout).trim());
            // the "command": a test file in the Juggl test folder
            let f = Path::new(&cwd).join("prova/modifiche-prova.txt");
            let _ = std::fs::create_dir_all(f.parent().unwrap());
            std::fs::write(&f, "riga uno\nriga due\n").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(20));
            std::fs::write(&f, "riga uno\nriga DUE, cambiata da un comando\nriga tre\n").unwrap();
            let t = std::time::Instant::now();
            shadow::tick(&[]);
            println!("  confronto finale in {} ms, cloni cancellati: {}", t.elapsed().as_millis(), !tmp.join("modifiche/ombre/vera").exists());
            let _ = std::fs::remove_file(&f);
            let line = std::fs::read_to_string(tmp.join("modifiche/cartelle.jsonl")).unwrap_or_default();
            if let Some(v) = line.lines().next().and_then(|r| serde_json::from_str::<Value>(r).ok()) {
                for x in v["file"].as_array().unwrap() { println!("    {} {} +{} -{}{}{}", str_of(x, "stato"), str_of(x, "path"), x["piu"], x["meno"], if x.get("nascosto").is_some() { " SENSIBILE" } else { "" }, if x["completo"] == false { " (senza prima)" } else { "" }); }
                if let Ok(o) = std::env::var("MODIFICHE_OUT") {
                    let list: Vec<Value> = v["file"].as_array().cloned().unwrap_or_default();
                    let (added, removed) = list.iter().fold((0, 0), |(p, m), f| (p + f["piu"].as_u64().unwrap_or(0), m + f["meno"].as_u64().unwrap_or(0)));
                    std::fs::write(o, json!({"tipo": "modifiche", "t": v["fine"], "da": v["inizio"], "a": v["fine"], "n": list.len(), "piu": added, "meno": removed, "elenco": list}).to_string()).unwrap();
                }
            }
        }
        if let Ok(repo) = std::env::var("MODIFICHE_REPO") {
            let g = |a: &[&str]| String::from_utf8_lossy(&crate::new_command("git").arg("-C").arg(&repo).args(a).output().unwrap().stdout).to_string();
            let before = (g(&["status", "--porcelain"]), g(&["for-each-ref"]), g(&["stash", "list"]), std::fs::metadata(Path::new(&repo).join(".git/index")).and_then(|m| m.modified()).ok());
            let t = std::time::Instant::now();
            let a1 = git_folder::snapshot(&repo);
            let t1 = t.elapsed().as_millis();
            let t = std::time::Instant::now();
            let a2 = git_folder::snapshot(&repo);
            println!("git {repo}: prima foto {t1} ms, seconda {} ms, alberi uguali senza cambi: {}", t.elapsed().as_millis(), a1.is_some() && a1 == a2);
            let after = (g(&["status", "--porcelain"]), g(&["for-each-ref"]), g(&["stash", "list"]), std::fs::metadata(Path::new(&repo).join(".git/index")).and_then(|m| m.modified()).ok());
            println!("  stato git, ref, stash e indice vero identici: {}", before == after);
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Codex chat writing a file with a command (no juggl_modifiche in the mirror): the line comes from the git snapshot.
    /// Goes through the real tick and the real chat reading (main.rs).
    #[test]
    fn codex_commands_seen_by_git_folder() {
        let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("juggl-codex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let r = dir.join("repo");
        let cwd = r.join("prove/lavoro");
        std::fs::create_dir_all(&cwd).unwrap();
        let _base_guard = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (process-wide variable)
        std::env::set_var("JUGGL_BASE", dir.join("base"));
        let g = |a: &[&str]| assert!(crate::new_command("git").arg("-C").arg(&r).args(a).output().unwrap().status.success());
        g(&["init", "-q"]);
        std::fs::write(r.join("README.md"), "x\n").unwrap();
        g(&["add", "."]);
        g(&["-c", "user.name=p", "-c", "user.email=p@p", "commit", "-qm", "base"]);
        let now_s = seconds(&now_iso()) as i64;
        let id = "c0dex000-0000-0000-0000-000000000001";
        // the user's message one second before the tick sees the chat working
        let t_user = crate::search_index::iso(now_s - 1);
        let c = cwd.to_string_lossy().to_string();
        git_folder::tick(&[(id.to_string(), c.clone())]);
        std::fs::write(cwd.join("nota.txt"), "scritta da un comando di Codex\n").unwrap();   // "printf ... > nota.txt"
        git_folder::tick(&[]);
        let t_end = crate::search_index::iso(now_s + 3);
        let mirror = dir.join("specchio.jsonl");
        let lines = [
            json!({"type": "user", "timestamp": t_user, "sessionId": id, "message": {"role": "user", "content": [{"type": "text", "text": "scrivi nota.txt"}]}}),
            json!({"type": "assistant", "timestamp": t_user, "sessionId": id, "message": {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "printf ... > nota.txt"}}]}}),
            json!({"type": "user", "timestamp": t_end, "sessionId": id, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]}}),
            json!({"type": "assistant", "timestamp": t_end, "sessionId": id, "message": {"role": "assistant", "content": [{"type": "text", "text": "Fatto."}]}}),
        ];
        std::fs::write(&mirror, lines.iter().map(|x| x.to_string() + "\n").collect::<String>()).unwrap();
        let d = crate::session_detail_pub(&json!({"transcript": mirror.to_string_lossy()}));
        let m: Vec<&Value> = d["eventi"].as_array().unwrap().iter().filter(|e| str_of(e, "tipo") == "modifiche").collect();
        assert_eq!(m.len(), 1, "{}", d["eventi"]);
        assert!(str_of(&m[0]["elenco"][0], "path").ends_with("/prove/lavoro/nota.txt"), "{}", m[0]);
        assert_eq!(m[0]["elenco"][0]["stato"], "creato");
        assert_eq!(m[0]["piu"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adaptive_tick() {
        // the test log goes to a temporary folder, never to ~/tavolo/juggl.log
        let dir = std::env::temp_dir().join(format!("juggl-adattivo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _base_guard = crate::test_data_lock(); // one test at a time changes JUGGL_BASE (process-wide variable)
        std::env::set_var("JUGGL_BASE", &dir);
        let k = "/prova/adattivo";
        assert_eq!(interval(k), 5);
        record_timing(k, 800);
        assert_eq!(interval(k), 30);
        record_timing(k, 300);   // between 200 and 500: stays as it is
        assert_eq!(interval(k), 30);
        record_timing(k, 50);
        assert_eq!(interval(k), 5);
        assert!(std::fs::read_to_string(dir.join("juggl.log")).unwrap_or_default().contains("ora ogni 30 s"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
