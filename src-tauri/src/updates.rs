// Automatic update (tauri-plugin-updater): checks at startup and every 4 hours, downloads and installs the new version
// into the app bundle, then the UI shows "Aggiornamento pronto" with Restart now / Later.
// Juggl never restarts by itself: the new version starts on the chosen restart, or on the next launch.
// Hosted sessions live in the keepers, so a restart does not stop them.
// Since 11/11 the update goes through the account (license.rs, ~/tavolo/account/CONTRATTO.md):
// GET https://juggl.dev/api/aggiornamento/{{target}}/{{arch}}/{{current_version}} with Authorization: Bearer <device
// token>; the server reads the latest.json uploaded by pubblica.sh and returns a signed link to the package.
// Without a token (no account signed in on this Mac) nothing is checked. The packages' public key stays in
// tauri.conf.json (plugins.updater).
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

struct State { pronto: Option<Value>, ultimo: f64, errore: String, in_corso: bool }
static STATE: Mutex<State> = Mutex::new(State { pronto: None, ultimo: 0.0, errore: String::new(), in_corso: false });

const EVERY: Duration = Duration::from_secs(4 * 3600);

/// The app runs from a .app bundle (not cargo run) and updates are not switched off for testing.
/// Windows: the installed app (not a program built in a target folder). Linux: only the AppImage (the .deb
/// updates through its own package).
fn possible() -> bool {
    if !cfg!(feature = "aggiornamenti") { return false; }
    if std::env::var("JUGGL_SENZA_AGGIORNAMENTI").map(|x| x == "1").unwrap_or(false) { return false; }
    if cfg!(windows) {
        return std::env::current_exe().map(|p| { let t = p.to_string_lossy().replace('\\', "/").to_lowercase(); !t.contains("/target/") }).unwrap_or(false);
    }
    if cfg!(target_os = "linux") { return std::env::var_os("APPIMAGE").is_some(); }
    std::env::current_exe().map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/")).unwrap_or(false)
}

/// Windows: the installer closes Juggl as soon as it starts, so here we only download; installation happens on "Restart now"
/// (aggiornamento_installa). This way on Windows too Juggl never restarts without asking.
static DOWNLOADED: Mutex<Option<(tauri_plugin_updater::Update, Vec<u8>)>> = Mutex::new(None);

pub fn start(app: AppHandle) {
    if !possible() { return; }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(25));
        loop {
            let _ = tauri::async_runtime::block_on(check(&app));
            if STATE.lock().map(|s| s.pronto.is_some()).unwrap_or(false) { break; }   // ready: no more checks
            std::thread::sleep(EVERY);
        }
    });
}

fn state_json(app: &AppHandle) -> Value {
    let s = STATE.lock().unwrap();
    json!({"versione": app.package_info().version.to_string(), "pronto": s.pronto.clone(), "ultimo": s.ultimo,
           "errore": s.errore.clone(), "in_corso": s.in_corso, "possibile": possible()})
}

async fn check(app: &AppHandle) -> Value {
    {
        let mut s = STATE.lock().unwrap();
        if s.pronto.is_some() || s.in_corso { drop(s); return state_json(app); }
        s.in_corso = true;
    }
    let outcome: Result<Option<Value>, String> = async {
        let token = crate::license::token().ok_or_else(|| "senza account: entra con il tuo account per gli aggiornamenti".to_string())?;
        let url = format!("{}/api/aggiornamento/{{{{target}}}}/{{{{arch}}}}/{{{{current_version}}}}", crate::license::service_url());
        let url = tauri::Url::parse(&url).map_err(|e| e.to_string())?;
        let up = app.updater_builder().endpoints(vec![url]).map_err(|e| e.to_string())?
            .header("Authorization", format!("Bearer {token}")).map_err(|e| e.to_string())?
            .build().map_err(|e| e.to_string())?;
        let Some(u) = up.check().await.map_err(|e| e.to_string())? else { return Ok(None) };
        let info = json!({"versione": u.version.clone(), "note": u.body.clone().unwrap_or_default()});
        if cfg!(windows) {
            let bytes = u.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
            *DOWNLOADED.lock().unwrap_or_else(|e| e.into_inner()) = Some((u.clone(), bytes));
        } else {
            u.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
        }
        Ok(Some(info))
    }.await;
    {
        let mut s = STATE.lock().unwrap();
        s.in_corso = false;
        s.ultimo = crate::unix_now();
        match &outcome {
            Ok(Some(v)) => { s.pronto = Some(v.clone()); s.errore.clear(); }
            Ok(None) => s.errore.clear(),
            Err(e) => { s.errore = e.clone(); }
        }
    }
    match outcome {
        Ok(Some(v)) => { crate::log_juggl(&format!("aggiornamento installato: {}", v["versione"])); let _ = app.emit("aggiornamento", v); }
        Err(e) => crate::log_juggl(&format!("aggiornamento: {e}")),
        _ => {}
    }
    state_json(app)
}

#[tauri::command]
pub async fn aggiornamento_cerca(app: AppHandle) -> Value {
    if !possible() { return json!({"possibile": false, "versione": app.package_info().version.to_string()}); }
    check(&app).await
}

#[tauri::command]
pub fn aggiornamento_stato(app: AppHandle) -> Value { state_json(&app) }

/// "Restart now" on Windows: starts the installer of the already downloaded update (closes Juggl and reopens it updated).
/// Elsewhere (or with nothing downloaded) it returns false and the UI restarts as usual.
#[tauri::command]
pub fn aggiornamento_installa() -> bool {
    let taken = DOWNLOADED.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some((u, bytes)) = taken else { return false };
    match u.install(bytes) {
        Ok(()) => true,
        Err(e) => { crate::log_juggl(&format!("aggiornamento: installazione non partita: {e}")); false }
    }
}
