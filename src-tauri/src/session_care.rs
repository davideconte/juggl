// Session management: memory used by each (process + children), warnings before the Mac locks up,
// automatic hibernation of sessions idle for too long, new sessions, closing, clipboard for photos and videos.
use crate::{base_dir, f64_of, home, load_json, unix_now, str_of};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub fn settings() -> Value {
    let mut d = json!({"ibernazione": true, "iberna_dopo_ore": 3.0, "avviso_sessione_gb": 2.5, "avviso_mac_libera_pct": 12.0, "avvisi": true, "pilota": true, "pilota_attesa_s": 60.0, "archivia_dopo_giorni": 0.0});
    if let Some(Value::Object(o)) = load_json(&base_dir().join("impostazioni.json")) {
        for (k, v) in o { d[k] = v; }
    }
    d
}

pub fn expand_path(p: &str) -> String {
    if let Some(r) = p.strip_prefix("~/") { home().join(r).to_string_lossy().to_string() } else { p.to_string() }
}

// ---------- projects (logos, folders) ----------

pub fn projects() -> Vec<Value> {
    load_json(&base_dir().join("progetti.json")).and_then(|v| v.get("progetti").cloned()).and_then(|v| v.as_array().cloned()).unwrap_or_default()
        .into_iter().map(|mut p| {
            let logo = expand_path(str_of(&p, "logo"));
            let ok = !logo.is_empty() && Path::new(&logo).is_file();
            p["logo"] = json!(if ok { logo } else { String::new() });
            p["cartella"] = json!(expand_path(str_of(&p, "cartella")));
            p
        }).collect()
}

/// Real path of a folder (links resolved: ~/Documents/WORKSPACE -> /Volumes/MacDev/...), cached.
/// If the folder does not exist it stays as it is.
pub fn real_path(p: &str) -> String {
    static C: std::sync::OnceLock<std::sync::Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    let p = p.trim_end_matches('/');
    if p.is_empty() { return String::new(); }
    let c = C.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Some(x) = c.lock().unwrap_or_else(|e| e.into_inner()).get(p) { return x.clone(); }
    match std::fs::canonicalize(p) {
        Ok(v) => {
            let v = v.to_string_lossy().to_string();
            let mut g = c.lock().unwrap_or_else(|e| e.into_inner());
            if g.len() > 5000 { g.clear(); }
            g.insert(p.to_string(), v.clone());
            v
        }
        Err(_) => p.to_string(),
    }
}

/// `cwd` is folder `c` or one of its subfolders, even if one of the two goes through a link.
pub fn is_inside(cwd: &str, c: &str) -> bool {
    // Windows: same comparisons with "/" (C:\\a\\b and C:/a/b are the same folder, case included)
    #[cfg(windows)]
    let (cwd, c) = (&cwd.replace('\\', "/").to_lowercase(), &c.replace('\\', "/").to_lowercase());
    let (cwd, c) = (cwd.trim_end_matches('/'), c.trim_end_matches('/'));
    if cwd.is_empty() || c.is_empty() { return false; }
    let under = |a: &str, b: &str| a == b || a.starts_with(&format!("{b}/"));
    under(cwd, c) || under(&real_path(cwd), &real_path(c))
}

/// Which project a session belongs to: first the folder (the current one, then the starting one: a `cd` into a
/// subfolder or a path with resolved links does not make it leave the project), then the words in the name.
pub fn project_of(v: &Value, name: &str, pp: &[Value]) -> Value {
    for cwd in [str_of(v, "cwd"), str_of(v, "cwd_origine")] {
        if cwd.is_empty() || cwd == "/" { continue; }
        // the project with the longest folder that contains this one (projects inside other projects)
        let mut best: Option<&Value> = None;
        for p in pp {
            let c = str_of(p, "cartella");
            if is_inside(cwd, c) && best.map(|m| c.len() > str_of(m, "cartella").len()).unwrap_or(true) { best = Some(p); }
        }
        if let Some(p) = best { return p.clone(); }
    }
    let n = name.to_lowercase();
    let words: Vec<&str> = n.split(|c: char| !c.is_alphanumeric()).filter(|x| !x.is_empty()).collect();
    for p in pp {
        let ok = p.get("parole").and_then(|x| x.as_array()).map(|a| a.iter().any(|w| words.contains(&w.as_str().unwrap_or("").to_lowercase().as_str()))).unwrap_or(false);
        if ok { return p.clone(); }
    }
    Value::Null
}

// ---------- memory ----------
// RSS underestimates idle sessions (macOS compresses their memory): I use the footprint from `top` (like Activity
// Monitor), read in the background every 20 s because it costs about a second.
pub static FOOTPRINT: std::sync::Mutex<Option<HashMap<i64, f64>>> = std::sync::Mutex::new(None);

pub fn refresh_footprint() {
    let Ok(o) = crate::new_command("top").args(["-l", "1", "-n", "600", "-o", "mem", "-stats", "pid,mem"]).output() else { return };
    let mut m = HashMap::new();
    for r in String::from_utf8_lossy(&o.stdout).lines() {
        let c: Vec<&str> = r.split_whitespace().collect();
        if c.len() != 2 { continue; }
        let Ok(pid) = c[0].parse::<i64>() else { continue };
        let v = c[1].trim_end_matches(|x: char| x == '+' || x == '-');
        let (n, u) = v.split_at(v.len().saturating_sub(1));
        let n: f64 = n.parse().unwrap_or(0.0);
        let mb = match u { "G" => n * 1024.0, "M" => n, "K" => n / 1024.0, "B" => n / 1048576.0, _ => 0.0 };
        m.insert(pid, mb);
    }
    if !m.is_empty() { *FOOTPRINT.lock().unwrap() = Some(m); }
}

pub struct Processes { figli: HashMap<i64, Vec<i64>>, rss: HashMap<i64, u64>, cpu: HashMap<i64, f64> }

pub fn processes() -> Processes {
    let mut p = Processes { figli: HashMap::new(), rss: HashMap::new(), cpu: HashMap::new() };
    let Ok(o) = crate::new_command("ps").args(["-axo", "pid=,ppid=,rss=,%cpu="]).output() else { return p };
    for r in String::from_utf8_lossy(&o.stdout).lines() {
        let c: Vec<&str> = r.split_whitespace().collect();
        if c.len() < 4 { continue; }
        let (Ok(pid), Ok(ppid)) = (c[0].parse::<i64>(), c[1].parse::<i64>()) else { continue };
        p.rss.insert(pid, c[2].parse().unwrap_or(0));
        p.cpu.insert(pid, c[3].replace(',', ".").parse().unwrap_or(0.0));
        p.figli.entry(ppid).or_default().push(pid);
    }
    p
}

impl Processes {
    /// (memory in MB, cpu %) of the process and all its children (MCP servers, commands, builds).
    pub fn tree_usage(&self, pid: i64) -> (f64, f64) {
        let settings = FOOTPRINT.lock().unwrap().clone();
        let (mut mb, mut cpu, mut queue) = (0.0f64, 0.0, vec![pid]);
        while let Some(x) = queue.pop() {
            let rss = self.rss.get(&x).copied().unwrap_or(0) as f64 / 1024.0;
            mb += settings.as_ref().and_then(|m| m.get(&x).copied()).map(|v| v.max(rss)).unwrap_or(rss);
            cpu += self.cpu.get(&x).copied().unwrap_or(0.0);
            if let Some(ff) = self.figli.get(&x) { queue.extend(ff); }
        }
        (mb, cpu)
    }
}

/// Memory of the Mac: total in GB and free percentage (free + inactive + reclaimable, like Activity Monitor).
/// What this computer is called in texts for people: "Mac" on the Mac, "PC" on Windows, "computer" on Linux.
pub fn computer_name() -> &'static str {
    if cfg!(target_os = "macos") { "Mac" } else if cfg!(windows) { "PC" } else { "computer" }
}
/// "Apri la vista Mac" / "Apri la vista PC" / "Apri la vista computer" (the view is named like the computer).
fn open_view_label() -> String { format!("Apri la vista {}", computer_name()) }
fn low_memory_title() -> String { format!("Juggl: {} quasi senza memoria", computer_name()) }
fn low_disk_title(threshold: f64) -> String { format!("Disco {} sotto il {threshold:.0}%", computer_name()) }

pub fn mac_memory() -> Value {
    #[cfg(target_os = "linux")]
    {
        let m = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let kb = |k: &str| m.lines().find(|r| r.starts_with(k)).and_then(|r| r.split_whitespace().nth(1)).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) * 1024.0;
        let (tot, disp) = (kb("MemTotal:"), kb("MemAvailable:"));
        let pct = if tot > 0.0 { (disp / tot * 100.0).min(100.0) } else { 100.0 };
        return json!({"totale_gb": (tot / 1073741824.0 * 10.0).round() / 10.0, "libera_pct": pct.round()});
    }
    #[cfg(windows)]
    {
        let (tot, disp) = crate::sys::memory();
        let pct = if tot > 0.0 { (disp / tot * 100.0).min(100.0) } else { 100.0 };
        return json!({"totale_gb": (tot / 1073741824.0 * 10.0).round() / 10.0, "libera_pct": pct.round()});
    }
    #[allow(unreachable_code)]
    let tot = crate::new_command("sysctl").args(["-n", "hw.memsize"]).output().ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok()).unwrap_or(0.0);
    let mut page_size = 16384.0;
    let mut free_pages = 0.0;
    if let Ok(o) = crate::new_command("vm_stat").output() {
        for r in String::from_utf8_lossy(&o.stdout).lines() {
            if let Some(i) = r.find("page size of ") { page_size = r[i + 13..].split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(page_size); }
            let n = r.rsplit(':').next().unwrap_or("").trim().trim_end_matches('.').parse::<f64>().unwrap_or(0.0);
            if r.starts_with("Pages free") || r.starts_with("Pages inactive") || r.starts_with("Pages speculative") || r.starts_with("Pages purgeable") { free_pages += n; }
        }
    }
    let pct = if tot > 0.0 { (free_pages * page_size / tot * 100.0).min(100.0) } else { 100.0 };
    json!({"totale_gb": (tot / 1073741824.0 * 10.0).round() / 10.0, "libera_pct": pct.round()})
}

/// Notification log (the app's notifications area): last 200, on the Mac.
pub fn record(title: &str, text: &str, session: &str, kind: &str) {
    record_with(title, text, session, kind, json!({}));
}

/// Like `registra`, with extra fields (project, link, file of the notice scripts' messages). Returns the written entry.
pub fn record_with(title: &str, text: &str, session: &str, kind: &str, extra: Value) -> Value {
    // notifiche.json is also written by the scripts: file lock, not only a process lock
    let p = base_dir().join("notifiche.json");
    let mut entry = json!({"t": unix_now(), "titolo": title, "testo": text, "sessione": session, "tipo": kind});
    if let (Some(o), Some(e)) = (entry.as_object_mut(), extra.as_object()) { for (k, v) in e { o.insert(k.clone(), v.clone()); } }
    let r = crate::storage::modify(&p, Some(json!([])), |l| {
        if !l.is_array() { *l = json!([]); }
        let a = l.as_array_mut().unwrap();
        a.push(entry.clone());
        let n = a.len();
        if n > 200 { a.drain(0..n - 200); }
        true
    });
    if let Err(e) = r { crate::log_juggl(&format!("notifica non registrata: {e}")); }
    entry
}

pub fn notify(title: &str, text: &str) {
    record(title, text, "", "sistema");
    if crate::settings_chat::is_quiet() { return; }   // chat-style settings (Nov 16): night notices, it only stays in the list
    // real Juggl notification (app name and icon; clicking it opens Juggl)
    use tauri_plugin_notification::NotificationExt;
    if let Some(app) = crate::APP.get() {
        let _ = app.notification().builder().title(title).body(text).show();
    }
}

// ---------- hibernation, closing, new sessions ----------

fn state_file(id: &str) -> std::path::PathBuf {
    let clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).collect();
    base_dir().join("stato").join(format!("{clean}.json"))
}

/// SIGTERM and after 8 s SIGKILL. If the process is leader of its group (hosted, terminal commands) the signal
/// goes to the whole group: MCP servers and builds are not left orphaned.
pub fn terminate(pid: i64) { crate::hosted::terminate_group(pid); }

/// Stops the process but leaves the session resumable (the conversation is all in the transcript).
pub fn hibernate(id: &str, motivo: &str) -> Value {
    let p = state_file(id);
    let mut pid = 0;
    // before stopping it: the end-of-session hook finds it already hibernated and does not close it
    let r = crate::storage::modify(&p, None, |v| {
        pid = v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
        v["stato_prima"] = json!(str_of(v, "stato"));
        v["stato"] = json!("ibernata");
        v["ibernata_il"] = json!(unix_now());
        v["motivo_ibernazione"] = json!(motivo);
        true
    });
    if r.is_err() { return json!({"ok": false, "msg": "sessione non trovata"}) }
    terminate(pid);
    json!({"ok": true, "msg": "ibernata"})
}

pub fn close(id: &str) -> Value {
    let p = state_file(id);
    let mut pid = 0;
    let r = crate::storage::modify(&p, None, |v| {
        pid = v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0);
        v["stato"] = json!("chiusa");
        v["da"] = json!(unix_now());
        true
    });
    if r.is_err() { return json!({"ok": false, "msg": "sessione non trovata"}) }
    terminate(pid);
    json!({"ok": true, "msg": "chiusa"})
}

// ---------- guardian: every 30 seconds ----------

pub struct Watchdog { avvisi: HashMap<String, f64>, mac_avvisato: f64 }

impl Watchdog {
    pub fn new() -> Self { Watchdog { avvisi: HashMap::new(), mac_avvisato: 0.0 } }

    /// Returns true if it changed something (hibernations) or there is an alarm to show.
    pub fn tick(&mut self, ss: &[Value]) -> Option<Value> {
        let settings = settings();
        let now = unix_now();
        let mut alarms = vec![];
        let threshold = f64_of(&settings, "avviso_sessione_gb") * 1024.0;
        for v in ss {
            let mb = f64_of(v, "memoria_mb");
            let id = str_of(v, "sessione").to_string();
            if mb > threshold && now - self.avvisi.get(&id).copied().unwrap_or(0.0) > 1800.0 {
                self.avvisi.insert(id.clone(), now);
                notify("Juggl: memoria", &format!("{} usa {:.1} GB. Ibernala se non ti serve adesso.", str_of(v, "nome"), mb / 1024.0));
                alarms.push(json!({"sessione": id, "mb": mb}));
            }
        }
        // disk almost full. Below 15 GB: notice (leads to the Mac view) and hourly cleanup in energetic mode.
        // Below 10 GB: instead of the hourly cleanup, `mac libera auto` up to 15 GB with only the safe items.
        // Both take the same lock: if one is already running the other does not start.
        DISK.lock().unwrap().0 = 0.0;
        if let Some(free) = free_disk_gb() {
            let last = self.avvisi.get("disco").copied().unwrap_or(0.0);
            if free < 10.0 && now - last > 3600.0 {
                self.avvisi.insert("disco".into(), now);
                auto_disk_cleanup(free);
                alarms.push(json!({"disco": free}));
            } else if free < 15.0 && now - last > 3600.0 {
                self.avvisi.insert("disco".into(), now);
                let already = Path::new(CLEANUP_LOCK).exists() || AUTO_CLEANUP.load(std::sync::atomic::Ordering::SeqCst);
                notify_mac("Juggl: disco quasi pieno", &format!("Restano {free:.1} GB. {}{} per vedere cosa liberare.",
                    if already { "C'è già una pulizia in corso. " } else { "Avvio la pulizia di cache e file temporanei. " }, open_view_label()));
                if !already { let _ = cleanup(true); }
                alarms.push(json!({"disco": free}));
            }
        }
        // space (Nov 14): below the threshold (setting avviso_disco_pct, 15% if missing) notice on the Mac and the iPhone.
        // Once when it drops, then only if a day passes or if it drops another 3 points; it restarts when it rises by 2.
        // In the notification only the number: no folder names (coordinator rule).
        // On by default only where the personal engine is (the user's Mac); for the others only if they turn it on
        // (setting avviso_disco: true).
        if !crate::profile::is_enabled("mac") && crate::profile::is_enabled("spazio") { crate::disk_space::record_trend(); }   // numbers only, stays on the Mac
        let notice_on = settings.get("avviso_disco").and_then(|x| x.as_bool()).unwrap_or_else(|| crate::profile::is_enabled("mac"));
        if notice_on && now - self.avvisi.get("disco_pct_giro").copied().unwrap_or(0.0) > 300.0 {
            self.avvisi.insert("disco_pct_giro".into(), now);
            if let Some((free, pct)) = free_disk_pct() {
                let threshold = settings.get("avviso_disco_pct").and_then(|x| x.as_f64()).unwrap_or(15.0);
                let (last, pct_ultimo) = (self.avvisi.get("disco_pct").copied().unwrap_or(0.0), self.avvisi.get("disco_pct_val").copied().unwrap_or(100.0));
                if pct < threshold && (last == 0.0 || now - last > 86400.0 || pct < pct_ultimo - 3.0) {
                    self.avvisi.insert("disco_pct".into(), now);
                    self.avvisi.insert("disco_pct_val".into(), pct);
                    let title = low_disk_title(threshold);
                    let text = format!("Restano {free:.0} GB liberi ({pct:.0}%).");
                    notify_mac(&title, &format!("{text} {} per vedere cosa liberare.", open_view_label()));
                    crate::push::send_in_background(title, text, String::new(), String::new());
                } else if pct > threshold + 2.0 {
                    self.avvisi.remove("disco_pct");
                    self.avvisi.remove("disco_pct_val");
                }
            }
        }
        let mac = mac_memory();
        let libera = f64_of(&mac, "libera_pct");
        if libera < f64_of(&settings, "avviso_mac_libera_pct") && now - self.mac_avvisato > 600.0 {
            self.mac_avvisato = now;
            let mut heavy: Vec<&Value> = ss.iter().filter(|v| str_of(v, "vista") != "ibernata").collect();
            heavy.sort_by(|a, b| f64_of(b, "memoria_mb").partial_cmp(&f64_of(a, "memoria_mb")).unwrap());
            let who = heavy.iter().take(2).map(|v| format!("{} ({:.1} GB)", str_of(v, "nome"), f64_of(v, "memoria_mb") / 1024.0)).collect::<Vec<_>>().join(", ");
            notify(&low_memory_title(), &format!("Libera {libera:.0}%. Le piu' pesanti: {who}."));
            alarms.push(json!({"mac": libera}));
        }
        // hibernation: idle (or finished and unread) for too long, no recent activity, process at rest
        let mut hibernated = vec![];
        if settings["ibernazione"].as_bool().unwrap_or(true) {
            let after = f64_of(&settings, "iberna_dopo_ore") * 3600.0;
            for v in ss {
                if !matches!(str_of(v, "vista"), "ferma" | "aspetta") { continue; }
                if now - f64_of(v, "da").max(f64_of(v, "agg")) < after { continue; }
                if f64_of(v, "cpu") > 3.0 { continue; }
                // the transcript has not moved for at least the threshold (no background jobs)
                let t = str_of(v, "transcript");
                let idle = t.is_empty() || std::fs::metadata(t).and_then(|m| m.modified()).ok()
                    .and_then(|m| m.elapsed().ok()).map(|e| e.as_secs_f64() > after).unwrap_or(true);
                if !idle { continue; }
                hibernate(str_of(v, "sessione"), "ferma da troppo");
                hibernated.push(str_of(v, "nome").to_string());
            }
            if !hibernated.is_empty() {
                notify("Juggl", &format!("Ibernate per liberare memoria: {}. Le riprendi da Juggl.", hibernated.join(", ")));
            }
        }
        if alarms.is_empty() && hibernated.is_empty() { None } else { Some(json!({"allarmi": alarms, "ibernate": hibernated})) }
    }
}

// ---------- clipboard: copy photos, videos, paths ----------

pub fn copy_file(path: &str) -> Value {
    if !Path::new(path).is_file() { return json!({"ok": false, "msg": "file non trovato"}) }
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    let q = path.replace('"', "\\\"");
    let script = match ext.as_str() {
        "png" => format!("set the clipboard to (read (POSIX file \"{q}\") as «class PNGf»)"),
        "jpg" | "jpeg" => format!("set the clipboard to (read (POSIX file \"{q}\") as JPEG picture)"),
        "gif" => format!("set the clipboard to (read (POSIX file \"{q}\") as GIF picture)"),
        "heic" | "webp" => {
            let tmp = std::env::temp_dir().join("juggl-appunti.png")   /* rename (Nov 14) */;
            let _ = crate::new_command("sips").args(["-s", "format", "png", path, "--out"]).arg(&tmp).output();
            format!("set the clipboard to (read (POSIX file \"{}\") as «class PNGf»)", tmp.to_string_lossy())
        }
        _ => format!("set the clipboard to (POSIX file \"{q}\")"),   // video and other: pasted as a file
    };
    let ok = crate::new_command("osascript").args(["-e", &script]).output().map(|o| o.status.success()).unwrap_or(false);
    json!({"ok": ok, "msg": if ok { "copiato" } else { "non riuscito" }})
}

pub fn copy_data(data_uri: &str) -> Value {
    use base64::Engine;
    let Some(b64) = data_uri.split(",").nth(1) else { return json!({"ok": false, "msg": "immagine non valida"}) };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else { return json!({"ok": false, "msg": "immagine non valida"}) };
    let ext = if data_uri.starts_with("data:image/jpeg") { "jpg" } else if data_uri.starts_with("data:image/gif") { "gif" } else if data_uri.starts_with("data:image/webp") { "webp" } else { "png" };
    let tmp = std::env::temp_dir().join(format!("juggl-incollata.{ext}"))   /* rename (Nov 14) */;
    if std::fs::write(&tmp, bytes).is_err() { return json!({"ok": false, "msg": "non riuscito"}) }
    copy_file(&tmp.to_string_lossy())
}

pub fn file_info(path: &str) -> Value {
    match std::fs::metadata(path) {
        Ok(m) => json!({"byte": m.len()}),
        Err(_) => json!({"byte": 0}),
    }
}

// ---------- "Mac" view: disk, memory, heavy apps, cleanup ----------

fn bytes_from(line: &str) -> f64 {
    line.split('(').nth(1).and_then(|x| x.split_whitespace().next()).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0)
}

pub fn mac_status(sessioni: &[Value]) -> Value {
    // disk (APFS container space, like the Finder)
    let mut tot = 0.0; let mut free = 0.0;
    if let Ok(o) = crate::new_command("diskutil").args(["info", "/"]).output() {
        for r in String::from_utf8_lossy(&o.stdout).lines() {
            if r.contains("Container Total Space") { tot = bytes_from(r); }
            if r.contains("Container Free Space") { free = bytes_from(r); }
        }
    }
    // swap
    let sw = crate::new_command("sysctl").args(["-n", "vm.swapusage"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    let num = |chiave: &str| sw.split(chiave).nth(1).and_then(|x| x.trim().trim_start_matches('=').trim().split('M').next()).and_then(|x| x.replace(',', ".").parse::<f64>().ok()).unwrap_or(0.0);
    let (swap_used, swap_total) = (num("used"), num("total"));
    // heaviest apps (footprint, grouped by app: Chrome = all its processes)
    let settings = FOOTPRINT.lock().unwrap().clone().unwrap_or_default();
    let mut per_app: HashMap<String, f64> = HashMap::new();
    if let Ok(o) = crate::new_command("ps").args(["-axo", "pid=,comm="]).output() {
        for r in String::from_utf8_lossy(&o.stdout).lines() {
            let r = r.trim(); let mut it = r.splitn(2, ' ');
            let (Some(pid), Some(cmd)) = (it.next().and_then(|x| x.parse::<i64>().ok()), it.next()) else { continue };
            let mb = settings.get(&pid).copied().unwrap_or(0.0); if mb < 30.0 { continue; }
            let name = if let Some(i) = cmd.find(".app/") { cmd[..i].rsplit('/').next().unwrap_or(cmd).to_string() }
                else { cmd.rsplit('/').next().unwrap_or(cmd).to_string() };
            *per_app.entry(name).or_default() += mb;
        }
    }
    let mut app: Vec<(String, f64)> = per_app.into_iter().collect();
    app.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let app: Vec<Value> = app.into_iter().take(10).map(|(n, mb)| json!({"nome": n, "mb": mb.round()})).collect();
    // cleanup: last lines of the log
    let log = std::fs::read_to_string(home().join("Library/Logs/pulizia-disco.log")).unwrap_or_default();
    let last_lines: Vec<String> = log.lines().rev().take(4).map(String::from).collect();
    let mut sess: Vec<Value> = sessioni.iter().filter(|v| f64_of(v, "memoria_mb") > 0.0)
        .map(|v| json!({"sessione": str_of(v, "sessione"), "nome": str_of(v, "nome"), "mb": f64_of(v, "memoria_mb"), "vista": str_of(v, "vista")})).collect();
    sess.sort_by(|a, b| f64_of(b, "mb").partial_cmp(&f64_of(a, "mb")).unwrap());
    json!({"disco": {"totale_gb": (tot / 1e9).round(), "libero_gb": (free / 1e8).round() / 10.0},
           "memoria": mac_memory(), "swap": {"usato_mb": swap_used, "totale_mb": swap_total},
           "app": app, "sessioni": sess, "pulizia": last_lines})
}

pub fn cleanup(urgente: bool) -> Value {
    let Some(p) = crate::profile::tool("pulizia-disco").filter(|_| crate::profile::is_enabled("mac")) else { return json!({"ok": false, "msg": "pulizia non disponibile", "spenta": true}) };
    let mut c = crate::new_command(p);
    if urgente { c.arg("--urgente"); }
    match c.spawn() { Ok(_) => json!({"ok": true, "msg": "pulizia avviata"}), Err(e) => json!({"ok": false, "msg": e.to_string()}) }
}

// ---------- engine `~/tavolo/bin/mac`: deep scan, suggestions, active cleanup, history ----------
// The engine uses the same lock as the hourly cleanup (/tmp/pulizia-disco.lock): never two cleanups together.

const CLEANUP_LOCK: &str = "/tmp/pulizia-disco.lock";
static SCAN: std::sync::Mutex<()> = std::sync::Mutex::new(());
static AUTO_CLEANUP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DISK: std::sync::Mutex<(f64, f64)> = std::sync::Mutex::new((0.0, -1.0));   // (when, free GB)

/// Launches `~/tavolo/bin/mac` and returns its JSON (even when it exits with an error: the JSON says why).
pub fn mac(args: &[&str]) -> Value {
    if !crate::profile::is_enabled("mac") { return json!({"ok": false, "msg": "motore mac non disponibile", "spenta": true}); }
    let bin = base_dir().join("bin/mac");
    let path = format!("/opt/homebrew/bin:/usr/local/bin:{}/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin", home().display());
    match crate::new_command(&bin).args(args).env("PATH", path).output() {
        Ok(o) => match serde_json::from_str::<Value>(String::from_utf8_lossy(&o.stdout).trim()) {
            Ok(v) => v,
            Err(_) => {
                let err = String::from_utf8_lossy(&o.stderr).to_string();
                let r = err.lines().rev().find(|r| !r.trim().is_empty()).unwrap_or("risposta non valida dal motore").trim().to_string();
                json!({"ok": false, "msg": r})
            }
        },
        Err(e) => json!({"ok": false, "msg": format!("non riesco a lanciare {}: {e}", bin.display())}),
    }
}

// ---------- space (Nov 14): "Space" part of the engine (bin/spazio.py), closed list of commands and options ----------
// entries (summary, groups, history), full scan in the background with Stop, reversible actions (Trash, archive on an
// external disk, leave), undo, two-step Trash, space settings. The arguments are plain words: no
// paths nor free options from the interface or the bridge.
pub fn mac_space(args: &[String]) -> Value {
    const COMMANDS: [&str; 13] = ["voci", "avanza", "esplora-avvia", "esplora-ferma", "fai", "fai-molti", "libera-sicuro", "annulla", "svuota-cestino",
                                 "pulisci-chat", "imposta", "riproponi", "giro"];
    const OPTIONS: [&str; 7] = ["--aggiorna", "--conferma", "--modo", "--giorni", "--a-mano", "--notte", "--sfondo"];   // --sfondo (Nov 17): the long cleanup keeps going by itself
    let Some(cmd) = args.first() else { return json!({"ok": false, "msg": "manca il comando"}) };
    if !COMMANDS.contains(&cmd.as_str()) { return json!({"ok": false, "msg": "comando non consentito"}); }
    let valid = |x: &str| OPTIONS.contains(&x) || (!x.starts_with('-') && !x.is_empty() && x.len() <= 80
        && x.chars().all(|c| c.is_ascii_alphanumeric() || ":._-".contains(c)));
    if !args[1..].iter().all(|a| valid(a)) { return json!({"ok": false, "msg": "argomento non valido"}); }
    // space for everyone (Nov 14): the personal engine where present, otherwise the built-in one (strict rules)
    if !crate::profile::is_enabled("mac") {
        if !crate::profile::is_enabled("spazio") { return json!({"ok": false, "msg": "gestione dello spazio spenta", "spenta": true}); }
        return crate::disk_space::command(args);
    }
    let a: Vec<&str> = args.iter().map(|x| x.as_str()).collect();
    let r = mac(&a);
    if !matches!(cmd.as_str(), "voci" | "avanza") { DISK.lock().unwrap_or_else(|e| e.into_inner()).0 = 0.0; }
    r
}

/// space (Nov 14): free GB and percentage of the disk (APFS container, like the Finder). macOS only.
pub fn free_disk_pct() -> Option<(f64, f64)> {
    #[cfg(target_os = "macos")]
    {
        let o = crate::new_command("diskutil").args(["info", "/"]).output().ok()?;
        let t = String::from_utf8_lossy(&o.stdout).to_string();
        let free = t.lines().find(|r| r.contains("Container Free Space")).map(bytes_from)?;
        let tot = t.lines().find(|r| r.contains("Container Total Space")).map(bytes_from)?;
        if tot <= 0.0 { return None; }
        return Some((free / 1e9, free / tot * 100.0));
    }
    #[allow(unreachable_code)]
    None
}

/// Last scan saved by the engine (shown at once, while the new one loads).
pub fn mac_latest() -> Value { load_json(&base_dir().join("mac-scansione.json")).unwrap_or(Value::Null) }

/// One scan at a time: whoever arrives while another is running waits and receives the one just finished.
pub fn mac_scan(deep: bool) -> Value {
    let _g = match SCAN.try_lock() {
        Ok(g) => g,
        Err(_) => {
            let g = SCAN.lock().unwrap_or_else(|e| e.into_inner());
            let u = mac_latest();
            if !deep || u["profonda"].as_bool().unwrap_or(false) { return u; }
            g
        }
    };
    let r = mac(if deep { &["scansione", "--profonda"][..] } else { &["scansione"][..] });
    if let Some(b) = r["disco"]["libero_b"].as_f64() { *DISK.lock().unwrap() = (unix_now(), (b / 1e8).round() / 10.0); }
    r
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id != "auto" && !id.starts_with('-') && id.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

pub fn mac_free(id: &str, confirm: bool) -> Value {
    if !valid_id(id) { return json!({"ok": false, "msg": "suggerimento non valido"}) }
    let mut a = vec!["libera", id];
    if confirm { a.push("--conferma"); }
    let r = mac(&a);
    DISK.lock().unwrap().0 = 0.0;
    r
}

pub fn mac_free_auto(target_gb: f64) -> Value {
    if !(target_gb > 0.0) { return json!({"ok": false, "msg": "indica quanti GB liberare"}) }
    let n = format!("{:.1}", target_gb.min(500.0));
    let r = mac(&["libera", "auto", "--obiettivo-gb", &n]);
    DISK.lock().unwrap().0 = 0.0;
    r
}

pub fn mac_ram() -> Value { mac(&["ram"]) }
pub fn mac_history() -> Value { mac(&["storico", "30"]) }

pub fn mac_restore(n: &str) -> Value {
    if !valid_id(n) { return json!({"ok": false, "msg": "pulizia non valida"}) }
    let r = mac(&["ripristina", n]);
    DISK.lock().unwrap().0 = 0.0;
    r
}

/// Free GB on the disk (like the Finder, from diskutil), rechecked at most once a minute.
pub fn free_disk_gb() -> Option<f64> {
    let (t, v) = *DISK.lock().unwrap();
    if unix_now() - t < 60.0 && v >= 0.0 { return Some(v); }
    let o = crate::new_command("diskutil").args(["info", "/"]).output().ok()?;
    let t = String::from_utf8_lossy(&o.stdout).to_string();
    let b = t.lines().find(|r| r.contains("Container Free Space")).map(bytes_from)?;
    let gb = (b / 1e8).round() / 10.0;
    *DISK.lock().unwrap() = (unix_now(), gb);
    Some(gb)
}

/// Below 10 GB: frees by itself up to 15 GB with only the safe items, in the background, and says so.
fn auto_disk_cleanup(free: f64) {
    if Path::new(CLEANUP_LOCK).exists() {
        notify_mac("Juggl: disco quasi pieno", &format!("Restano {free:.1} GB. C'è già una pulizia in corso: {} per liberare altro spazio.", open_view_label().to_lowercase()));
        return;
    }
    if AUTO_CLEANUP.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
    notify_mac("Juggl: disco quasi pieno", &format!("Restano {free:.1} GB. Libero da solo fino a 15 GB, solo con le voci sicure (cache e file che si rigenerano)."));
    std::thread::spawn(|| {
        let r = mac(&["libera", "auto", "--obiettivo-gb", "15"]);
        DISK.lock().unwrap().0 = 0.0;
        let now_free = free_disk_gb().unwrap_or(0.0);
        let g = r["liberato_gb"].as_f64().unwrap_or(0.0).max(0.0);
        if r["ok"].as_bool().unwrap_or(false) {
            let rest = if r["raggiunto"].as_bool().unwrap_or(false) { String::new() } else { format!(" Per il resto servono voci da confermare: aprimi nella vista {}.", computer_name()) };
            notify_mac("Juggl: spazio liberato", &format!("Liberati {g:.1} GB, ora ne restano {now_free:.1}.{rest}"));
        } else {
            notify_mac("Juggl: pulizia non riuscita", &format!("{} {}.", str_of(&r, "msg"), open_view_label()));
        }
        AUTO_CLEANUP.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(app) = crate::APP.get() { use tauri::Emitter; let _ = app.emit("cambiato", ()); }
    });
}

/// Notification that, when clicked, leads to the Mac view.
pub fn notify_mac(title: &str, text: &str) {
    record(title, text, "", "mac");
    if crate::settings_chat::is_quiet() { return; }   // chat-style settings (Nov 16): night notices, it only stays in the list
    use tauri_plugin_notification::NotificationExt;
    if let Some(app) = crate::APP.get() {
        let _ = app.notification().builder().title(title).body(text).show();
    }
}

/// Asks an app to quit (like Cmd+Q): saves the work if the app supports it.
pub fn quit_app(name: &str) -> Value {
    let q = name.replace('"', "");
    let ok = crate::new_command("osascript").args(["-e", &format!("tell application \"{q}\" to quit")]).output().map(|o| o.status.success()).unwrap_or(false);
    json!({"ok": ok, "msg": if ok { "" } else { "l'app non ha risposto" }})
}

#[cfg(test)]
mod project_subfolder_tests {
    use serde_json::json;
    /// A cd into a subfolder (even through the resolved link) does not make the session leave the project.
    #[cfg(unix)]
    #[test]
    fn project_from_subfolder_and_link() {
        let base = std::env::temp_dir().join("prova-progetto-sotto");
        let real = base.join("vero/CLIENTI/ROSSI/security-audit/2026-10-03");
        let _ = std::fs::create_dir_all(&real);
        let link = base.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(base.join("vero"), &link).unwrap();
        let folder = link.join("CLIENTI/ROSSI").to_string_lossy().to_string();
        let pp = vec![json!({"id": "altro", "cartella": "/non/esiste"}), json!({"id": "rossi", "nome": "Rossi", "cartella": folder})];
        // cwd = real path of the subfolder (as Claude Code writes it after the cd), any name
        let v = json!({"cwd": std::fs::canonicalize(&real).unwrap().to_string_lossy(), "cwd_origine": ""});
        assert_eq!(super::project_of(&v, "2026-10-03", &pp)["id"], json!("rossi"));
        // cwd outside the project, but started from the project
        let v2 = json!({"cwd": "/tmp", "cwd_origine": folder});
        assert_eq!(super::project_of(&v2, "tmp", &pp)["id"], json!("rossi"));
    }
}

#[cfg(test)]
mod computer_name_tests {
    use super::*;
    #[test]
    fn texts_with_platform_name() {
        let n = computer_name();
        assert_eq!(n, if cfg!(target_os = "macos") { "Mac" } else if cfg!(windows) { "PC" } else { "computer" });
        let all_titles = [low_memory_title(), low_disk_title(12.0), open_view_label()];
        assert!(all_titles.iter().all(|t| t.contains(n)), "{all_titles:?}");
        // outside the Mac no text says "Mac"
        if !cfg!(target_os = "macos") { assert!(all_titles.iter().all(|t| !t.contains("Mac")), "{all_titles:?}"); }
        assert!(crate::license::device_name().len() > 0);
    }
}
