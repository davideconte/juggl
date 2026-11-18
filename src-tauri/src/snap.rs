// snap (Nov 16): "Snap in Juggl". A global shortcut (F16 by default, even with Juggl in the background) works like ⌘⇧4:
// screencapture -i (area; Space = window; Esc = nothing), then the image goes into the open chat's attachments with the input box
// ready; with Alt (Alt+F16) it goes into the Juggl chat ("Ask Juggl"). Long side reduced to 1600 px, files in
// <data>/scatti, deleted after a day. Only wanted snaps: it starts only from the shortcut or a pressed button.
// Screen Recording permission: explained by the interface (ui/snap.js) before asking for it, only once; if missing,
// a message with the button for System Settings. Mac only: elsewhere the commands say mac:false and nothing is captured.
// Key in impostazioni.json: "scatto": {"tasto": "F16" | "Cmd+Shift+2" | "" (off), "chiesto": bool}.
use serde_json::{json, Value};
#[cfg(target_os = "macos")] use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")] use std::sync::Mutex;
use tauri::AppHandle;
#[cfg(target_os = "macos")] use tauri::{Emitter, Manager};

pub const BASE_KEY: &str = "F16";

fn settings_file() -> std::path::PathBuf { crate::profile::settings_file() }
fn snap_settings() -> Value { std::fs::read(settings_file()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v.get("scatto").cloned()).unwrap_or(json!({})) }
fn shortcut_key() -> String { snap_settings().get("tasto").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| BASE_KEY.into()) }
fn asked() -> bool { snap_settings().get("chiesto").and_then(|v| v.as_bool()).unwrap_or(false) }
fn save(k: &str, v: Value) -> bool {
    crate::storage::modify(&settings_file(), Some(json!({})), |d| { if !d["scatto"].is_object() { d["scatto"] = json!({}); } d["scatto"][k] = v.clone(); true }).is_ok()
}
fn folder() -> std::path::PathBuf { settings_file().parent().map(|p| p.join("scatti")).unwrap_or_default() }

/// "cmd+shift+2", "⌘⇧2", "f16" -> canonical form "Cmd+Shift+2", "F16". None if it is not a valid shortcut.
pub fn normalize(t: &str) -> Option<String> {
    let t = t.trim().replace('⌘', "Cmd+").replace('⇧', "Shift+").replace('⌥', "Alt+").replace('⌃', "Ctrl+");
    let parts: Vec<String> = t.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()).map(|p| match p.to_lowercase().as_str() {
        "cmd" | "command" | "comando" | "super" => "Cmd".to_string(),
        "shift" | "maiusc" | "maiuscole" => "Shift".to_string(),
        "alt" | "option" | "opzione" => "Alt".to_string(),
        "ctrl" | "control" | "controllo" => "Ctrl".to_string(),
        x if x.len() <= 3 && x.starts_with('f') && x[1..].parse::<u8>().map(|n| (1..=24).contains(&n)).unwrap_or(false) => x.to_uppercase(),
        x if x.len() == 1 && x.chars().all(|c| c.is_ascii_alphanumeric()) => x.to_uppercase(),
        _ => String::new(),
    }).collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) { return None; }
    let (mods, keys): (Vec<&String>, Vec<&String>) = parts.iter().partition(|p| ["Cmd", "Shift", "Alt", "Ctrl"].contains(&p.as_str()));
    if keys.len() != 1 { return None; }
    let k = keys[0];
    // without modifiers only function keys (a single letter would steal the keyboard from every app)
    if mods.is_empty() && !k.starts_with('F') { return None; }
    if mods.is_empty() && k.len() == 1 { return None; }
    let mut out: Vec<String> = ["Ctrl", "Alt", "Shift", "Cmd"].iter().filter(|m| mods.iter().any(|x| x.as_str() == **m)).map(|m| m.to_string()).collect();
    out.push(k.clone());
    Some(out.join("+"))
}

/// Version with Alt (for "Ask Juggl"); None if the shortcut already has Alt.
pub fn with_alt(t: &str) -> Option<String> { if t.split('+').any(|p| p == "Alt") { None } else { normalize(&format!("Alt+{t}")) } }

// ---------- Mac ----------
#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" { fn CGPreflightScreenCaptureAccess() -> bool; fn CGRequestScreenCaptureAccess() -> bool; }
#[cfg(target_os = "macos")] fn permission() -> bool { unsafe { CGPreflightScreenCaptureAccess() } }
#[cfg(not(target_os = "macos"))] fn permission() -> bool { false }

#[cfg(target_os = "macos")] static REGISTERED: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());
#[cfg(target_os = "macos")] static IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Registers the shortcut (and its Alt version). Readable error if another app holds it.
#[cfg(target_os = "macos")]
fn register_shortcut(app: &AppHandle, t: &str) -> Result<(), String> {
    use std::str::FromStr;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
    let gs = app.global_shortcut();
    let mut reg = REGISTERED.lock().map_err(|_| "blocco".to_string())?;
    for (s, _) in reg.drain(..) { if let Ok(sc) = Shortcut::from_str(&s) { let _ = gs.unregister(sc); } }
    if t.is_empty() { return Ok(()); }
    let base = Shortcut::from_str(t).map_err(|_| "scorciatoia non valida".to_string())?;
    gs.register(base).map_err(|_| "questa scorciatoia è già usata da un'altra app".to_string())?;
    reg.push((t.to_string(), false));
    if let Some(a) = with_alt(t) { if let Ok(sc) = Shortcut::from_str(&a) { if gs.register(sc).is_ok() { reg.push((a, true)); } } }
    Ok(())
}

/// From the shortcuts handler (main.rs): true if it was the snap (pressed = capture in another thread).
pub fn handle(app: &AppHandle, sc: &tauri_plugin_global_shortcut::Shortcut, pressed: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::str::FromStr;
        let alt = REGISTERED.lock().ok().and_then(|r| r.iter().find(|(s, _)| tauri_plugin_global_shortcut::Shortcut::from_str(s).map(|x| x.id() == sc.id()).unwrap_or(false)).map(|(_, a)| *a));
        let Some(alt) = alt else { return false };
        if pressed { let a = app.clone(); std::thread::spawn(move || capture(&a, alt)); }
        return true;
    }
    #[allow(unreachable_code)]
    { let _ = (app, sc, pressed); false }
}

/// At startup (Mac only): old snaps removed, shortcut registered.
pub fn start(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        cleanup();
        if let Err(e) = register_shortcut(app, &shortcut_key()) { crate::log_juggl(&format!("scatto: {} non registrata: {e}", shortcut_key())); }
    }
    let _ = app;
}

/// Removes snaps older than a day.
pub fn cleanup() {
    let Ok(rd) = std::fs::read_dir(folder()) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs() > 86400).unwrap_or(false);
        if old && e.file_name().to_string_lossy().starts_with("scatto-") { let _ = std::fs::remove_file(e.path()); }
    }
}

#[cfg(target_os = "macos")]
fn bring_to_front(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") { let _ = w.unminimize(); let _ = w.show(); let _ = w.set_focus(); }
}

/// Long side over 1600 px -> 1600 (sips, in place).
#[cfg(target_os = "macos")]
fn shrink(f: &std::path::Path) {
    let o = crate::new_command("/usr/bin/sips").args(["-g", "pixelWidth", "-g", "pixelHeight"]).arg(f).output();
    let long_side = o.ok().map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|r| r.split(':').nth(1)?.trim().parse::<u32>().ok()).max().unwrap_or(0)).unwrap_or(0);
    if long_side > 1600 { let _ = crate::new_command("/usr/bin/sips").args(["-Z", "1600"]).arg(f).output(); }
}

#[cfg(target_os = "macos")]
fn capture(app: &AppHandle, alt: bool) {
    if IN_PROGRESS.swap(true, Ordering::SeqCst) { return; }   // one at a time
    struct Done; impl Drop for Done { fn drop(&mut self) { IN_PROGRESS.store(false, Ordering::SeqCst); } }
    let _f = Done;
    if !permission() {
        // no capture without permission: the interface explains (first time) or leads to System Settings
        bring_to_front(app);
        let _ = app.emit("scatto-permesso", json!({"stato": if asked() { "negato" } else { "da_chiedere" }, "alt": alt}));
        return;
    }
    cleanup();
    let dir = folder(); let _ = std::fs::create_dir_all(&dir);
    let f = dir.join(format!("scatto-{}.png", (crate::unix_now() * 1000.0) as u64));
    let status = crate::new_command("/usr/sbin/screencapture").arg("-i").arg(&f).status();
    if status.is_err() || !f.is_file() { return; }   // Esc: no file, no message
    shrink(&f);
    bring_to_front(app);
    let _ = app.emit("scatto", json!({"path": f.to_string_lossy(), "alt": alt}));
}

// ---------- commands for the interface ----------
#[tauri::command(async)]
pub fn scatto_stato() -> Value {
    let t = shortcut_key();
    json!({"mac": cfg!(target_os = "macos"), "tasto": t, "alt": with_alt(&t), "base": BASE_KEY, "permesso": permission(), "chiesto": asked()})
}

#[tauri::command(async)]
pub fn scatto_imposta(app: AppHandle, tasto: String) -> Value {
    if !cfg!(target_os = "macos") { return json!({"ok": false, "msg": "Solo sul Mac"}); }
    let new_key = if tasto.trim().is_empty() || tasto.trim().eq_ignore_ascii_case("spento") { String::new() } else {
        match normalize(&tasto) { Some(t) => t, None => return json!({"ok": false, "msg": "Scorciatoia non valida: usa un tasto funzione (F13-F19) o una combinazione con ⌘, ⌥, ⌃ o ⇧"}) }
    };
    let previous = self::shortcut_key();
    #[cfg(target_os = "macos")]
    if let Err(e) = register_shortcut(&app, &new_key) { let _ = register_shortcut(&app, &previous); return json!({"ok": false, "msg": e}); }
    let _ = &app;
    if !save("tasto", json!(new_key)) { return json!({"ok": false, "msg": "Non salvato"}); }
    json!({"ok": true, "tasto": new_key, "prima": previous, "alt": with_alt(&new_key)})
}

/// Asks macOS for the permission (once: after the first no macOS does not ask again, System Settings is needed).
#[tauri::command(async)]
pub fn scatto_permesso_chiedi() -> Value {
    save("chiesto", json!(true));
    #[cfg(target_os = "macos")] let ok = unsafe { CGRequestScreenCaptureAccess() };
    #[cfg(not(target_os = "macos"))] let ok = false;
    json!({"ok": ok})
}

/// Snap started from an interface button (after the permission, or "Try"): always and only at the user's request.
#[tauri::command(async)]
pub fn scatto_ora(app: AppHandle, alt: bool) -> Value {
    #[cfg(target_os = "macos")]
    { std::thread::spawn(move || capture(&app, alt)); return json!({"ok": true}); }
    #[allow(unreachable_code)]
    { let _ = (app, alt); json!({"ok": false}) }
}

#[tauri::command(async)]
pub fn scatto_apri_impostazioni() -> Value {
    #[cfg(target_os = "macos")] let ok = crate::new_command("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture").spawn().is_ok();
    #[cfg(not(target_os = "macos"))] let ok = false;
    json!({"ok": ok})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcuts() {
        assert_eq!(normalize("f16").as_deref(), Some("F16"));
        assert_eq!(normalize("cmd+shift+2").as_deref(), Some("Shift+Cmd+2"));
        assert_eq!(normalize("⌘⇧2").as_deref(), Some("Shift+Cmd+2"));
        assert_eq!(normalize("Alt+F16").as_deref(), Some("Alt+F16"));
        assert_eq!(normalize("a"), None);          // a single letter: no
        assert_eq!(normalize("cmd+shift"), None);  // without a key: no
        assert_eq!(normalize("ciao"), None);
        assert_eq!(with_alt("F16").as_deref(), Some("Alt+F16"));
        assert_eq!(with_alt("Alt+F16"), None);
        assert_eq!(with_alt("Shift+Cmd+2").as_deref(), Some("Alt+Shift+Cmd+2"));
        // the canonical forms read as plugin shortcuts
        use std::str::FromStr;
        for t in ["F16", "Alt+F16", "Shift+Cmd+2", "Alt+Shift+Cmd+2", "Ctrl+F13"] { assert!(tauri_plugin_global_shortcut::Shortcut::from_str(t).is_ok(), "{t}"); }
    }
    /// shrink(): a 3000x2000 image (made from the icon, no screen) comes out with the long side at 1600; a small one stays as it is.
    #[cfg(target_os = "macos")]
    #[test]
    fn reduction() {
        let dir = std::env::temp_dir().join(format!("scatto-prova-{}", std::process::id())); let _ = std::fs::create_dir_all(&dir);
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/icon.png");
        let sides = |f: &std::path::Path| { let o = crate::new_command("/usr/bin/sips").args(["-g", "pixelWidth", "-g", "pixelHeight"]).arg(f).output().unwrap();
            String::from_utf8_lossy(&o.stdout).lines().filter_map(|r| r.split(':').nth(1)?.trim().parse::<u32>().ok()).collect::<Vec<_>>() };
        let large = dir.join("grande.png");
        crate::new_command("/usr/bin/sips").args(["-z", "2000", "3000"]).arg(&icon).arg("--out").arg(&large).output().unwrap();
        assert_eq!(sides(&large), vec![3000, 2000]);
        shrink(&large);
        assert_eq!(sides(&large), vec![1600, 1066]);   // sips rounds down
        let small = dir.join("piccola.png");
        crate::new_command("/usr/bin/sips").args(["-z", "300", "400"]).arg(&icon).arg("--out").arg(&small).output().unwrap();
        shrink(&small);
        assert_eq!(sides(&small), vec![400, 300]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
