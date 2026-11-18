// Profile of whoever uses Juggl: name, enabled features, link with Claude Code (hook), installed scripts, first launch.
//
// Juggl must work for anyone. Features that depend on tools only some people have (iPhone app with push, Mac cleanup engine, cmux terminal, local models on a PC) switch on
// only if they find their dependencies, and can be switched off from Settings ("funzioni" in impostazioni.json).
// Without dependencies they stay off and hidden, without errors.
//
// The personal data these features need (addresses, own tools) is not in the program: it is read from
// ~/.config/juggl/personale.json, which exists only on the Mac of whoever has it.
use crate::NoConsoleWindow;
use crate::{base_dir, home, load_json, str_of};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)] use std::os::windows::process::CommandExt;
use std::sync::OnceLock;

/// Line that marks scripts and files installed by Juggl: only these update themselves. The others (written by
/// hand by whoever uses Juggl) are never touched.
pub const MARK: &str = "juggl-gestito";

pub fn settings_file() -> PathBuf { base_dir().join("impostazioni.json") }
fn user_settings() -> Value { load_json(&settings_file()).unwrap_or(json!({})) }

/// Optional personal data (~/.config/juggl/personale.json): never inside the program.
pub fn personal() -> Value {
    load_json(&home().join(".config/juggl/personale.json")).unwrap_or(json!({}))
}

/// A path from personale.json ("~/..." expanded), if it really exists.
fn personal_path(key: &str) -> Option<PathBuf> {
    let v = personal();
    let p = str_of(&v, key);
    if p.is_empty() { return None; }
    let p = PathBuf::from(crate::session_care::expand_path(p));
    p.exists().then_some(p)
}

/// Port of the local bridge (API used by scripts and the iPhone app): JUGGL_PORTA, then settings, then 47800.
pub fn port() -> u16 {
    static P: OnceLock<u16> = OnceLock::new();
    *P.get_or_init(|| {
        std::env::var("JUGGL_PORTA").ok().and_then(|x| x.parse().ok())
            .or_else(|| user_settings().get("porta").and_then(|x| x.as_u64()).map(|x| x as u16))
            .filter(|p| *p > 1024).unwrap_or(47800)
    })
}

/// First name of whoever uses Juggl, for texts and prompts ("le richieste di <name>").
/// Settings > Name, otherwise the macOS account name, otherwise "l'utente".
pub fn user_name() -> String {
    let n = str_of(&user_settings(), "nome").trim().to_string();
    if !n.is_empty() { return n; }
    account_name().unwrap_or_else(|| "l'utente".into())
}

fn account_name() -> Option<String> {
    static N: OnceLock<Option<String>> = OnceLock::new();
    N.get_or_init(|| {
        let o = crate::new_command("/usr/bin/id").arg("-F").output().ok()?;
        let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
        t.split_whitespace().next().map(String::from).filter(|x| !x.is_empty())
    }).clone()
}

/// Replaces «NOME» in fixed texts (prompts, prefixes) with the name of whoever uses Juggl.
pub fn with_name(t: &str) -> String { t.replace("«NOME»", &user_name()) }

// ---------- optional features ----------

/// (id, title, description)
const FEATURES: &[(&str, &str, &str)] = &[
    ("iphone", "App iPhone", "Ponte in rete locale e notifiche push verso l'app iPhone"),
    ("mac", "Pulizia del Mac", "Scansione e pulizia dello spazio con il motore mac"),
    ("spazio", "Spazio del disco", "Trova build e cache che si rifanno da sole e le sposta nel Cestino, sempre con conferma"),
    ("cmux", "Terminale cmux", "Apri e rispondi alle sessioni nel terminale cmux, e spostale in Juggl"),
    ("pc", "Modelli locali", "Chat con i modelli di un PC in rete locale (Ollama)"),
];

/// Ok if the feature's dependencies are there, otherwise what is missing.
fn dependencies(id: &str) -> Result<(), String> {
    let c = |p: PathBuf, what: &str| if p.exists() { Ok(()) } else { Err(what.to_string()) };
    match id {
        "iphone" => c(home().join(".config/juggl/apns/apns.env"), "la chiave APNs"),
        "mac" => c(base_dir().join("bin/mac"), "il motore mac"),
        "spazio" => if cfg!(target_os = "macos") { Ok(()) } else { Err("per ora solo su Mac".into()) },
        "cmux" => c(PathBuf::from(crate::CMUX), "cmux"),
        "pc" => if pc_ollama().is_some() { Ok(()) } else { Err("un PC con Ollama in personale.json".into()) },
        _ => Err("funzione sconosciuta".into()),
    }
}

/// Ollama address on the PC (personale.json: {"pc": {"ollama": "http://...:11434", "modello": "..."}}).
pub fn pc_ollama() -> Option<String> {
    let v = personal();
    let u = v.get("pc").map(|p| str_of(p, "ollama").trim_end_matches('/').to_string()).unwrap_or_default();
    (!u.is_empty()).then_some(u)
}
/// Fallback address of the PC via Tailscale (away from the studio, or studio network down): "pc.ollama_tailscale".
pub fn pc_ollama_fallback() -> Option<String> {
    let u = personal().get("pc").map(|p| str_of(p, "ollama_tailscale").trim_end_matches('/').to_string()).unwrap_or_default();
    (!u.is_empty()).then_some(u)
}

/// Between the studio address and the PC's Tailscale one, the one that answers: studio first (300 ms to open the
/// connection), then Tailscale. The choice holds for a minute for each pair of addresses.
pub fn pc_address(primario: &str, fallback: Option<&str>) -> String {
    use std::sync::{Mutex, OnceLock};
    static CHOSEN: OnceLock<Mutex<std::collections::HashMap<String, (String, std::time::Instant)>>> = OnceLock::new();
    let Some(fallback) = fallback.filter(|r| !r.is_empty() && *r != primario) else { return primario.to_string() };
    let m = CHOSEN.get_or_init(|| Mutex::new(Default::default()));
    if let Some((x, t)) = m.lock().unwrap().get(primario) { if t.elapsed().as_secs() < 60 { return x.clone(); } }
    let reachable = |u: &str| -> bool {
        let hp = u.split("://").nth(1).unwrap_or(u).split('/').next().unwrap_or("");
        use std::net::ToSocketAddrs;
        hp.to_socket_addrs().ok().and_then(|mut a| a.next())
            .map(|a| std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_millis(300)).is_ok()).unwrap_or(false)
    };
    let picked = if reachable(primario) || !reachable(fallback) { primario.to_string() } else { fallback.to_string() };
    m.lock().unwrap().insert(primario.to_string(), (picked.clone(), std::time::Instant::now()));
    picked
}

pub fn pc_model() -> String { personal().get("pc").map(|p| str_of(p, "modello").to_string()).unwrap_or_default() }

/// The feature is on: dependencies present and not switched off from Settings.
pub fn is_enabled(id: &str) -> bool {
    let choice = user_settings().get("funzioni").and_then(|f| f.get(id)).and_then(|x| x.as_bool());
    // iPhone app (11/11, App Store): also switches on without an APNs key, by hand from Settings > iPhone (push stays
    // only where the key exists). By default on only if the key exists (the owner's Mac, as before).
    if id == "iphone" { return choice.unwrap_or_else(|| dependencies(id).is_ok()); }
    dependencies(id).is_ok() && choice != Some(false)
}

/// Map id -> on, for the interface.
pub fn enabled_map() -> Value {
    let mut m = Map::new();
    for &(id, _, _) in FEATURES { m.insert(id.into(), json!(is_enabled(id))); }
    Value::Object(m)
}

/// An optional personal tool (e.g. "mostra", "trascrivi", "pulizia"): path from personale.json, or
/// ~/.local/bin/<name> if it exists.
pub fn tool(nome: &str) -> Option<PathBuf> {
    personal_path(nome).or_else(|| { let p = home().join(".local/bin").join(nome); p.is_file().then_some(p) })
}

// ---------- system programs ----------

/// Claude Code: the first one found among the installer's usual places, npm's and Homebrew's, then the login shell.
pub fn claude() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let ad = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
        let candidates = [home().join(".local/bin/claude.exe"), ad.join("npm/claude.cmd")];
        if let Some(p) = candidates.into_iter().find(|p| p.is_file()) { return Some(p); }
        static FROM_WHERE: OnceLock<Option<PathBuf>> = OnceLock::new();
        return FROM_WHERE.get_or_init(|| {
            let o = crate::new_command("where").arg("claude").creation_flags(crate::sys::CREATE_NO_WINDOW).output().ok()?;
            // `where` also lists the extensionless "claude" script npm puts next to claude.cmd (e.g. npm prefix
            // different from %APPDATA%\npm): Windows cannot start it ("Claude non parte"). Only .exe, .cmd, .bat.
            String::from_utf8_lossy(&o.stdout).lines().map(|l| PathBuf::from(l.trim()))
                .find(|p| p.is_file() && p.extension().map(|e| ["exe", "cmd", "bat"].contains(&e.to_string_lossy().to_ascii_lowercase().as_str())).unwrap_or(false))
        }).clone();
    }
    #[allow(unreachable_code)]
    let candidates = [home().join(".local/bin/claude"), home().join(".claude/local/claude"), PathBuf::from("/opt/homebrew/bin/claude"), PathBuf::from("/usr/local/bin/claude")];
    if let Some(p) = candidates.into_iter().find(|p| p.is_file()) { return Some(p); }
    // Ubuntu (12/11): usually no zsh, and with nvm claude lives in ~/.nvm/...: the login shell PATH finds it
    #[cfg(target_os = "linux")]
    {
        static FROM_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
        return FROM_PATH.get_or_init(|| std::env::split_paths(&linux_shell_path()).map(|d| d.join("claude")).find(|p| p.is_file())).clone();
    }
    static FROM_SHELL: OnceLock<Option<PathBuf>> = OnceLock::new();
    FROM_SHELL.get_or_init(|| {
        let o = crate::new_command("/bin/zsh").args(["-ilc", "command -v claude"]).output().ok()?;
        let p = String::from_utf8_lossy(&o.stdout).lines().last().unwrap_or("").trim().to_string();
        (p.starts_with('/') && Path::new(&p).is_file()).then(|| PathBuf::from(p))
    }).clone()
}

/// Ubuntu: the app PATH plus that of the user's login shell ($SHELL or bash, interactive: that is where nvm,
/// ~/.npm-global and the like add their folders). Opened from the menu, the app only has the graphical session PATH.
#[cfg(target_os = "linux")]
pub(crate) fn linux_shell_path() -> String {
    static P: OnceLock<String> = OnceLock::new();
    P.get_or_init(|| {
        let app = std::env::var("PATH").unwrap_or_default();
        let sh = std::env::var("SHELL").ok().filter(|s| Path::new(s).is_file()).unwrap_or_else(|| "/bin/bash".into());
        let login = crate::new_command(&sh).args(["-ilc", "printf %s \"$PATH\""]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .output().ok().map(|o| String::from_utf8_lossy(&o.stdout).lines().last().unwrap_or("").trim().to_string()).unwrap_or_default();
        let mut seen: Vec<String> = vec![];
        for d in login.split(':').chain(app.split(':')) { if d.starts_with('/') && !seen.iter().any(|x| x == d) { seen.push(d.to_string()); } }
        if seen.is_empty() { app } else { seen.join(":") }
    }).clone()
}

/// Path of Claude to launch (if not found, "claude" and the PATH takes care of it).
pub fn claude_cmd() -> PathBuf { claude().unwrap_or_else(|| PathBuf::from("claude")) }

/// Are Apple's command line tools there? Without them, /usr/bin/python3 opens the install window.
fn apple_tools() -> bool {
    crate::new_command("/usr/bin/xcode-select").arg("-p").output().map(|o| o.status.success()).unwrap_or(false)
}

/// A Python 3.9 or newer for the hooks: Homebrew if present, otherwise the system one (Homebrew is not needed).
pub fn python() -> Option<(PathBuf, String)> {
    let mut c: Vec<PathBuf> = vec!["/opt/homebrew/bin/python3".into(), "/usr/local/bin/python3".into()];
    if apple_tools() { c.push("/usr/bin/python3".into()); }
    for p in c.into_iter().filter(|p| p.is_file()) {
        let Ok(o) = crate::new_command(&p).args(["-I", "-c", "import sys;print('%d.%d' % sys.version_info[:2]);sys.exit(0 if sys.version_info >= (3, 9) else 1)"]).output() else { continue };
        if o.status.success() { return Some((p, String::from_utf8_lossy(&o.stdout).trim().to_string())); }
    }
    None
}

fn claude_version(p: &Path) -> String {
    crate::new_command(p).no_console_window().arg("--version").env("TAVOLO_INTERNO", "1").output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

// ---------- Claude Code hooks ----------

const EVENTS: [&str; 5] = ["UserPromptSubmit", "Stop", "Notification", "SessionStart", "SessionEnd"];

// automatic coordination (12/11): settings() and salva_settings() are pub(crate), also used by src/coordinator.rs for its hook
pub(crate) fn settings() -> PathBuf { home().join(".claude/settings.json") }
/// rename (14/11): the hook script on the Mac. Whoever was already there keeps bin/tavolo (and their hooks stay identical);
/// new users have bin/juggl-hook.
pub(crate) fn script_hook() -> PathBuf { let v = base_dir().join("bin/tavolo"); if v.exists() { v } else { base_dir().join("bin/juggl-hook") } }

/// The program to put in the hooks (Windows and Linux). AppImage: the .AppImage file (the APPIMAGE variable), not the
/// path it runs from, which changes at every launch (/tmp/.mount_...).
#[cfg(not(target_os = "macos"))]
fn hook_executable() -> Option<std::path::PathBuf> {
    if let Some(a) = std::env::var_os("APPIMAGE") { return Some(a.into()); }
    std::env::current_exe().ok()
}

/// A hook command is Juggl's if it launches the Juggl hook script (the earlier bin/tavolo or bin/juggl-hook, even
/// with another Python) or bin/cartelle.py, in the data folder or in a ~/tavolo.
fn is_ours(cmd: &str) -> bool {
    let c = cmd.replace(['\'', '"'], "");
    #[cfg(not(target_os = "macos"))]
    if c.trim_end().ends_with(" hook") && hook_executable().map(|e| c.replace('\\', "/").contains(&e.to_string_lossy().replace('\\', "/"))).unwrap_or(false) { return true; }
    // automatic coordination (12/11): also the folders hook (bin/cartelle.py), so "Disinstalla collegamento" removes it too
    // rename (14/11): + bin/juggl-hook and the data folder whatever it is (~/.juggl for new users)
    let b = base_dir().join("bin").to_string_lossy().replace('\\', "/");
    let c = c.replace('\\', "/");
    c.trim_end().ends_with(" hook") && (c.contains("/tavolo/bin/tavolo ") || c.contains(&format!("{} ", script_hook().to_string_lossy().replace('\\', "/"))) || c.contains("/tavolo/bin/cartelle.py ")
        || c.contains("/bin/juggl-hook ") || ["tavolo", "juggl-hook", "cartelle.py"].iter().any(|x| c.contains(&format!("{b}/{x} "))))
}

#[cfg(target_os = "macos")]
fn hook_command(py: &Path) -> String {
    let q = |p: &Path| { let t = p.to_string_lossy().to_string(); if t.contains(' ') { format!("'{t}'") } else { t } };
    format!("{} {} hook", q(py), q(&script_hook()))
}

/// Which events already have Juggl's hook.
fn linked_events(v: &Value) -> Vec<String> {
    EVENTS.iter().filter(|ev| {
        v.get("hooks").and_then(|h| h.get(**ev)).and_then(|l| l.as_array()).map(|l| l.iter().any(|g| {
            g.get("hooks").and_then(|x| x.as_array()).map(|hh| hh.iter().any(|x| is_ours(str_of(x, "command")))).unwrap_or(false)
        })).unwrap_or(false)
    }).map(|x| x.to_string()).collect()
}

pub fn hook_status() -> Value {
    let v = load_json(&settings()).unwrap_or(json!({}));
    let ev = linked_events(&v);
    json!({"collegati": ev.len(), "totale": EVENTS.len(), "completo": ev.len() == EVENTS.len(), "file": settings()})
}

pub(crate) fn save_settings(v: &Value) -> Result<String, String> {
    // settings.json may be a link (dotfiles): I write to the real file, not over the link
    let p = fs::canonicalize(settings()).unwrap_or_else(|_| settings());
    let _ = fs::create_dir_all(p.parent().unwrap());
    let mut backup = String::new();
    if p.exists() {
        let b = p.with_file_name(format!("settings.json.bak-juggl-{}", crate::unix_now() as u64));
        fs::copy(&p, &b).map_err(|e| format!("backup non riuscito: {e}"))?;
        backup = b.to_string_lossy().to_string();
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())? + "\n";
    crate::storage::write(&p, text.as_bytes()).map_err(|e| format!("scrittura non riuscita: {e}"))?;
    Ok(backup)
}

/// Adds Juggl's hook to the events that lack it. The other hooks stay as they are, no duplicates.
pub fn install_hook() -> Value {
    // Windows and Linux: the hook is the program itself (`Juggl.exe hook` / `juggl hook`, src/hook.rs), no Python
    #[cfg(not(target_os = "macos"))]
    let cmd = match hook_executable() { Some(e) => format!("\"{}\" hook", e.to_string_lossy().replace('\\', "/")), None => return json!({"ok": false, "msg": "programma di Juggl non trovato"}) };
    #[cfg(target_os = "macos")]
    let cmd = {
        let Some((py, _)) = python() else { return json!({"ok": false, "msg": "Manca Python 3"}) };
        if !script_hook().is_file() { return json!({"ok": false, "msg": "Script di Juggl non installati"}) }
        hook_command(&py)
    };
    let p = settings();
    let mut v = if p.exists() {
        match fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(v) if v.is_object() => v,
            _ => return json!({"ok": false, "msg": "settings.json non si legge: lo lascio com'è"}),
        }
    } else { json!({}) };
    let already = linked_events(&v);
    if already.len() == EVENTS.len() { return json!({"ok": true, "msg": "già collegato", "backup": ""}) }
    if !v["hooks"].is_object() { v["hooks"] = json!({}); }
    for ev in EVENTS.iter().filter(|e| !already.contains(&e.to_string())) {
        if !v["hooks"][*ev].is_array() { v["hooks"][*ev] = json!([]); }
        v["hooks"][*ev].as_array_mut().unwrap().push(json!({"hooks": [{"type": "command", "command": cmd, "timeout": 10, "async": true}]}));
    }
    match save_settings(&v) { Ok(b) => json!({"ok": true, "backup": b}), Err(e) => json!({"ok": false, "msg": e}) }
}

/// Removes only Juggl's hooks (Settings > Disinstalla collegamento). Groups and events left empty disappear.
pub fn remove_hook() -> Value {
    let p = settings();
    let Some(mut v) = fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return json!({"ok": true, "msg": "niente da togliere"}) };
    if linked_events(&v).is_empty() { return json!({"ok": true, "msg": "niente da togliere"}) }
    if let Some(h) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (_, l) in h.iter_mut() {
            if let Some(groups) = l.as_array_mut() {
                for g in groups.iter_mut() {
                    if let Some(hh) = g.get_mut("hooks").and_then(|x| x.as_array_mut()) { hh.retain(|x| !is_ours(str_of(x, "command"))); }
                }
                groups.retain(|g| g.get("hooks").and_then(|x| x.as_array()).map(|a| !a.is_empty()).unwrap_or(true));
            }
        }
        h.retain(|_, l| l.as_array().map(|a| !a.is_empty()).unwrap_or(true));
    }
    match save_settings(&v) { Ok(b) => json!({"ok": true, "backup": b}), Err(e) => json!({"ok": false, "msg": e}) }
}

// ---------- scripts in the package ----------

/// Copies the package scripts (Contents/Resources/resources) to the Juggl folder: at first launch and after every
/// update. An existing file is replaced only if Juggl put it there (line with SEGNO).
pub fn install_scripts(app: &tauri::AppHandle) {
    use tauri::Manager;
    let Ok(r) = app.path().resource_dir() else { return };
    let r = r.join("resources");
    if !r.is_dir() { return; }
    // automatic coordination (12/11): + bin/cartelle.py ("who touches what" hook and `juggl cartelle`)
    // rename (14/11): the hook script in the package is bin/juggl-hook; whoever already has bin/tavolo receives it under that
    // name (same content), new users only as juggl-hook.
    let hook_rel = if base_dir().join("bin/tavolo").exists() { "bin/tavolo" } else { "bin/juggl-hook" };
    let pairs = [(hook_rel, true), ("bin/juggl", true), ("bin/comune.py", false), ("generale-juggl/CLAUDE.md", false), ("bin/cartelle.py", false)];
    for (rel, executable) in pairs {
        // English names in the package (18/11); in the data folder they stay as always (hooks and scripts look for them that way)
        let in_package = match rel {
            "bin/tavolo" => "bin/juggl-hook",
            "bin/comune.py" => "bin/common.py",
            "bin/cartelle.py" => "bin/folders.py",
            "generale-juggl/CLAUDE.md" => "juggl-assistant/CLAUDE.md",
            x => x,
        };
        let (da, a) = (r.join(in_package), base_dir().join(rel));
        let Ok(mut new_bytes) = fs::read(&da) else { continue };
        // rename (14/11): the general assistant's instructions with the real folder (~/.juggl/bin/juggl for new users)
        if rel == "generale-juggl/CLAUDE.md" { new_bytes = instructions_with_folder(&String::from_utf8_lossy(&new_bytes), &crate::data_dir_short()).into_bytes(); }
        if let Ok(c) = fs::read(&a) {
            if c == new_bytes || !String::from_utf8_lossy(&c).contains(MARK) { continue; }
        } else if rel == "bin/comune.py" && base_dir().join("bin/tavolo").exists() && !marked(&base_dir().join("bin/tavolo")) {
            continue;   // hand-written folder without comune.py: I do not touch it
        }
        let _ = fs::create_dir_all(a.parent().unwrap());
        if crate::storage::write(&a, &new_bytes).is_ok() && executable {
            #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = fs::set_permissions(&a, fs::Permissions::from_mode(0o755)); }
        }
    }
}

/// rename (14/11): in the package CLAUDE.md the commands are written with ~/.juggl/; I bring them to the real data
/// folder (for whoever already had ~/tavolo/: the file stays identical to before).
fn instructions_with_folder(text: &str, folder: &str) -> String { text.replace("~/.juggl/", &format!("{}/", folder.trim_end_matches('/'))) }

fn marked(p: &Path) -> bool { fs::read_to_string(p).map(|t| t.contains(MARK)).unwrap_or(false) }

// ---------- first launch ----------

/// Should the welcome be shown? No if already done, or if Juggl was already in use before the welcome (hooks linked).
pub fn needs_welcome() -> bool {
    let v = user_settings();
    if v.get("benvenuto").is_some() { return false; }
    if hook_status()["collegati"].as_u64().unwrap_or(0) > 0 {
        // already in use from before: I mark it done without showing it
        let _ = crate::storage::modify(&settings_file(), Some(json!({})), |v| { v["benvenuto"] = json!({"fatto": crate::unix_now(), "da_prima": true}); true });
        return false;
    }
    true
}

pub fn mark_welcome(giro: bool) {
    let _ = crate::storage::modify(&settings_file(), Some(json!({})), |v| {
        if !v["benvenuto"].is_object() { v["benvenuto"] = json!({}); }
        // the autopilot (answers simple confirmations by itself) stays on by default even for those starting
        // from scratch (the user's choice 09/11): it is switched off from the settings.
        v["benvenuto"]["fatto"] = json!(crate::unix_now());
        if giro { v["benvenuto"]["giro"] = json!(crate::unix_now()); }
        true
    });
}

// ---------- commands for the interface ----------

#[tauri::command(async)]
pub fn profilo(app: tauri::AppHandle) -> Value {
    let imp = user_settings();
    let features: Vec<Value> = FEATURES.iter().map(|(id, t, d)| {
        let dip = dependencies(id);
        json!({"id": id, "titolo": t, "descr": d, "disponibile": dip.is_ok(), "manca": dip.err().unwrap_or_default(), "accesa": is_enabled(id)})
    }).collect();
    json!({"nome": user_name(), "nome_impostato": !str_of(&imp, "nome").is_empty(), "funzioni": features, "attive": enabled_map(),
           "versione": app.package_info().version.to_string(), "hook": hook_status(), "porta": port(),
           "benvenuto": imp.get("benvenuto").cloned().unwrap_or(Value::Null), "base": base_dir(), "dettatura": crate::microphone::mic_stato()})
}

#[tauri::command(async)]
pub fn profilo_imposta(chiave: String, valore: Value) -> Value {
    let r = crate::storage::modify(&settings_file(), Some(json!({})), |v| {
        if chiave == "nome" { v["nome"] = json!(valore.as_str().unwrap_or("").trim().chars().take(40).collect::<String>()); return true; }
        if let Some(id) = chiave.strip_prefix("funzioni.") {
            if !FEATURES.iter().any(|(x, _, _)| *x == id) { return false; }
            if !v["funzioni"].is_object() { v["funzioni"] = json!({}); }
            v["funzioni"][id] = json!(valore.as_bool().unwrap_or(true));
            return true;
        }
        // settings as chat (16/11): model of new chats (read by /api/nuova and the nuova command) and night alerts
        if chiave == "modello_nuove" {
            let m = valore.as_str().unwrap_or("").trim().to_lowercase();
            if !m.is_empty() && !crate::settings_chat::MODELS.contains(&m.as_str()) { return false; }
            v["modello_nuove"] = json!(m); return true;
        }
        if chiave == "avvisi_notte" {
            let acceso = valore.get("acceso").and_then(|x| x.as_bool()).unwrap_or(false);
            let ora = |k: &str, d: &str| { let t = valore.get(k).and_then(|x| x.as_str()).unwrap_or(d); crate::settings_chat::minutes_of(t).map(|m| format!("{:02}:{:02}", m / 60, m % 60)) };
            let (Some(da), Some(a)) = (ora("da", "23:00"), ora("a", "08:00")) else { return false };
            v["avvisi_notte"] = json!({"acceso": acceso, "da": da, "a": a}); return true;
        }
        false
    });
    match r { Ok(_) => json!({"ok": true, "attive": enabled_map(), "nome": user_name()}), Err(e) => json!({"ok": false, "msg": e.to_string()}) }
}

/// Welcome checks: are Claude Code and Python there?
#[tauri::command(async)]
pub fn controlli() -> Value {
    let cl = claude();
    // Windows and Linux: the hook is inside Juggl (src/hook.rs), Python is not needed
    let py = if cfg!(target_os = "macos") { python() } else { None };
    json!({
        "claude": {"ok": cl.is_some(), "percorso": cl.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
                   "versione": cl.as_deref().map(claude_version).unwrap_or_default()},
        "python": {"ok": py.is_some(), "percorso": py.as_ref().map(|p| p.0.to_string_lossy().to_string()).unwrap_or_default(),
                   "versione": py.map(|p| p.1).unwrap_or_default(), "strumenti_apple": apple_tools(), "non_serve": !cfg!(target_os = "macos")},
        "script": script_hook().is_file() || !cfg!(target_os = "macos"),
        "hook": hook_status(),
    })
}

#[tauri::command(async)]
pub fn hook_installa() -> Value { install_hook() }
#[tauri::command(async)]
pub fn hook_togli() -> Value { remove_hook() }

/// Opens Apple's window that installs the command line tools (also brings Python 3).
#[tauri::command(async)]
pub fn installa_strumenti_apple() -> Value {
    json!({"ok": crate::new_command("/usr/bin/xcode-select").arg("--install").spawn().is_ok()})
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_juggl_commands() {
        assert!(is_ours("/opt/homebrew/bin/python3.12 /home/x/tavolo/bin/tavolo hook"));
        assert!(is_ours("/usr/bin/python3 '/home/a b/tavolo/bin/tavolo' hook"));
        assert!(!is_ours("python3 /home/x/.codex/hooks/guard.py"));
        assert!(!is_ours("/usr/bin/python3 /home/x/tavolo/bin/tavolo mostra"));
        // rename (14/11): new users, ~/.juggl folder
        assert!(is_ours("/usr/bin/python3 /home/x/.juggl/bin/juggl-hook hook"));
        assert!(is_ours("/usr/bin/python3 '/home/a b/.juggl/bin/juggl-hook' hook"));
        assert!(!is_ours("/usr/bin/python3 /home/x/.juggl/bin/juggl-hook presenza"));
        assert!(!is_ours("python3 /home/x/progetti/bin/cartelle.py hook"));
    }

    /// rename (14/11): the real "Collega" (installa_hook, like the welcome button) in a FAKE HOME, never in the real home.
    /// New user:    H=$(mktemp -d /private/tmp/collega-XXXX); HOME=$H cargo test --bin tavolo link_in_fake_home -- --ignored --nocapture
    /// Earlier user: as above, but first mkdir -p $H/tavolo && echo '{}' > $H/tavolo/ponte.json
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn link_in_fake_home() {
        let h = home();
        assert!(h.starts_with("/private/tmp/") || h.starts_with("/tmp/"), "HOME deve essere una cartella finta in /private/tmp: {}", h.display());
        assert!(std::env::var_os("JUGGL_BASE").is_none(), "senza JUGGL_BASE: la cartella la sceglie dati_dir()");
        let before = h.join("tavolo/ponte.json").exists();
        let b = base_dir();
        assert_eq!(b, h.join(if before { "tavolo" } else { ".juggl" }));
        let script = if before { "bin/tavolo" } else { "bin/juggl-hook" };
        fs::create_dir_all(b.join("bin")).unwrap();
        fs::write(b.join(script), include_str!("../../resources/bin/juggl-hook")).unwrap();
        assert_eq!(script_hook(), b.join(script));
        let r = install_hook();
        assert_eq!(r["ok"], json!(true), "{r}");
        let t = fs::read_to_string(settings()).unwrap();
        println!("{}", t.lines().find(|l| l.contains("\"command\"")).unwrap_or("").trim());
        if before { assert!(t.contains("/tavolo/bin/tavolo hook"), "{t}"); } else { assert!(!t.contains("tavolo"), "{t}"); assert!(t.contains("/.juggl/bin/juggl-hook hook"), "{t}"); }
        assert_eq!(hook_status()["completo"], json!(true));
        assert_eq!(install_hook()["msg"], json!("già collegato"));
        assert_eq!(remove_hook()["ok"], json!(true));
        assert_eq!(hook_status()["collegati"], json!(0));
    }

    #[test]
    fn instructions_with_the_real_folder() {   // rename (14/11)
        let t = "- `~/.juggl/bin/juggl sessioni` tutte\n- `~/.juggl/bin/juggl leggi`";
        assert_eq!(instructions_with_folder(t, "~/.juggl"), t);
        assert_eq!(instructions_with_folder(t, "~/tavolo"), "- `~/tavolo/bin/juggl sessioni` tutte\n- `~/tavolo/bin/juggl leggi`");
        // the package file, for whoever was already there, comes back identical to the one before the rename
        let package = include_str!("../../resources/juggl-assistant/CLAUDE.md");
        assert!(!package.contains("tavolo"));
        assert!(instructions_with_folder(package, "~/tavolo").contains("`~/tavolo/bin/juggl sessioni`"));
    }
}

#[cfg(test)]
mod pc_address_tests {
    #[test]
    fn studio_down_then_tailscale() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let fallback = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        // 192.0.2.1 = test address that never answers
        assert_eq!(super::pc_address("http://192.0.2.1:4096", Some(&fallback)), fallback);
        // studio answers: stays the studio
        assert_eq!(super::pc_address(&fallback, Some("http://192.0.2.1:4096")), fallback);
        // no fallback: always the primary
        assert_eq!(super::pc_address("http://192.0.2.1:4096", None), "http://192.0.2.1:4096");
    }
}
