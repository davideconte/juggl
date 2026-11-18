// Juggl side of the session keeper (the program is in src/programs/juggl-keeper.rs).
//
// Every hosted session lives in its own `juggl-custode` process, detached from Juggl (setsid): closing, reinstalling or
// crashing Juggl stops neither Claude nor its sub-agents. Juggl talks to the keeper on ~/tavolo/sock/<id>.sock and,
// when it restarts, reconnects and asks for the events it missed.
//
// The keeper program lives in the app bundle (next to Juggl). I copy it to ~/tavolo/custode/ with
// "write aside and rename": keepers already running keep their file and are never touched by a
// reinstall (overwriting a running executable on macOS gets it killed by the signature check).
#[cfg(windows)] use crate::sys as libc;
use crate::{base_dir, log_juggl};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)] pub use std::os::unix::net::UnixStream;
#[cfg(windows)] pub use uds_windows::UnixStream;
#[cfg(unix)] use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub fn socket_dir() -> PathBuf { base_dir().join("sock") }
pub fn socket(id: &str) -> PathBuf { socket_dir().join(format!("{id}.sock")) }

/// The keeper program, installed in ~/tavolo/custode/juggl-custode. None = not available (the old mode is used,
/// Claude as a direct child of Juggl). JUGGL_SENZA_CUSTODE=1 switches it off.
pub fn program() -> Option<PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        if std::env::var("JUGGL_SENZA_CUSTODE").map(|x| x == "1").unwrap_or(false) { return None; }
        let name = format!("juggl-custode{}", std::env::consts::EXE_SUFFIX);
        let source = std::env::current_exe().ok()?.parent()?.join(&name);
        let dest = base_dir().join("custode").join(&name);
        match install(&source, &dest) {
            Ok(()) => Some(dest),
            Err(e) => {
                log_juggl(&format!("custode: non installato ({e})"));
                // already installed by an earlier launch: fine anyway
                if dest.exists() { Some(dest) } else { None }
            }
        }
    }).clone()
}

fn install(source: &Path, dest: &Path) -> Result<(), String> {
    let new_bin = std::fs::read(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let d = dest.parent().ok_or("cartella")?;
    #[cfg(windows)] remove_old(d);
    if std::fs::read(dest).map(|v| v == new_bin).unwrap_or(false) { UPDATED.store(true, std::sync::atomic::Ordering::SeqCst); return Ok(()); }
    std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    let tmp = d.join(format!(".juggl-custode.{}.tmp", std::process::id()));
    std::fs::write(&tmp, &new_bin).map_err(|e| e.to_string())?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?; }
    if let Err(e) = std::fs::rename(&tmp, dest) {
        // Windows (12/11, popup reported by a tester): a running .exe cannot be overwritten ("Access denied") but can be renamed.
        // Before, we stayed with the old keeper as long as one was running; now the one in use is moved aside (chats
        // stay alive) and the new one takes its place. Old ones are deleted when they stop running (remove_old).
        #[cfg(windows)] {
            let aside = d.join(format!("juggl-custode.vecchio.{}.{}.exe", std::process::id(), (crate::unix_now() * 1000.0) as u64));
            std::fs::rename(dest, &aside).and_then(|_| std::fs::rename(&tmp, dest))
                .map_err(|e2| { let _ = std::fs::remove_file(&tmp); format!("{e}; anche spostandolo da parte: {e2}") })?;
        }
        #[cfg(not(windows))] { let _ = std::fs::remove_file(&tmp); return Err(e.to_string()); }
    }
    UPDATED.store(true, std::sync::atomic::Ordering::SeqCst);
    log_juggl(&format!("custode: programma aggiornato in {}/custode (versione {VERSION})", crate::data_dir_short()));   // rename (14/11)
    Ok(())
}

/// Windows: deletes the keepers moved aside that no longer run (those in use cannot be deleted).
#[cfg(windows)]
fn remove_old(d: &Path) {
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with("juggl-custode.vecchio.") { let _ = std::fs::remove_file(e.path()); }
    }
}

/// Connection to a keeper: reader (with what it has already read) and writer.
pub struct Connection {
    pub lettore: BufReader<UnixStream>,
    pub scrittore: UnixStream,
    pub pid: u32,
    pub pid_custode: u32,
    pub vivo: bool,
    /// keeper version (VERSIONE of src/programs/juggl-keeper.rs) and of the app it was built with
    pub versione: u64,
    pub app: String,
}

/// Keeper version of this app. Older keepers restart when the chat is idle (hosted::allinea_account).
pub const VERSION: u64 = 3;
static UPDATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Should an old keeper be restarted? Only if the copy in ~/tavolo/custode is really the new one (if the install
/// failed we would restart forever with the same old program).
pub fn needs_update(versione: u64) -> bool {
    versione < VERSION && program().is_some() && UPDATED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Connects to the session keeper and asks for the events after `after`. Answers within a few seconds or fails.
pub fn connect(id: &str, after: u64) -> Result<Connection, String> {
    let c = UnixStream::connect(socket(id)).map_err(|e| e.to_string())?;
    c.set_read_timeout(Some(Duration::from_secs(8))).map_err(|e| e.to_string())?;
    let mut w = c.try_clone().map_err(|e| e.to_string())?;
    writeln!(w, "#ciao {{\"dopo\":{after}}}").map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;
    let mut r = BufReader::new(c);
    let mut first_line = String::new();
    r.read_line(&mut first_line).map_err(|e| format!("il custode non risponde: {e}"))?;
    let info: Value = first_line.strip_prefix("# ").and_then(|x| serde_json::from_str(x.trim()).ok()).ok_or("risposta del custode non valida")?;
    if info.get("custode").is_none() { return Err("risposta del custode non valida".into()); }
    r.get_ref().set_read_timeout(None).map_err(|e| e.to_string())?;
    Ok(Connection {
        lettore: r, scrittore: w,
        pid: info.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        pid_custode: info.get("pid_custode").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        vivo: info.get("vivo").and_then(|x| x.as_bool()).unwrap_or(false),
        versione: info.get("custode").and_then(|x| x.as_u64()).unwrap_or(0),
        app: info.get("app").and_then(|x| x.as_str()).unwrap_or("prima della 0.1.50").to_string(),
    })
}

/// Starts the keeper for a new or resumed session and connects. The keeper is detached from Juggl (setsid):
/// it does not receive the app group's signals and outlives it.
pub fn start(prog: &Path, id: &str, cwd: &str, claude: &str, args: &[String], path: &str, log: std::fs::File) -> Result<Connection, String> {
    let sock = socket(id);
    let d = socket_dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)); }
    // Windows: its own HIDDEN console (sys::avvia_nascosto), inherited by Claude and its children (hook, bash):
    // no windows opening and closing. The keeper log goes to the file given by JUGGL_CUSTODE_LOG.
    #[cfg(windows)]
    #[allow(unused_mut)]
    let mut spawn_child = {
        drop(log);
        let mut a = vec![sock.to_string_lossy().to_string(), id.to_string(), claude.to_string()];
        a.extend(args.iter().cloned());
        let log_path = crate::base_dir().join("ospite.log").to_string_lossy().to_string();
        let (prog, cwd, path) = (prog.to_path_buf(), cwd.to_string(), path.to_string());
        move || crate::sys::spawn_hidden(&prog, &a, &cwd, &[("PATH", &path), ("JUGGL_CUSTODE_LOG", &log_path)], &["CMUX_"])
    };
    #[cfg(unix)]
    let mut cmd = crate::new_command(prog);
    #[cfg(unix)]
    {
        cmd.arg(&sock).arg(id).arg(claude).args(args).current_dir(cwd).env("PATH", path)
            .env_remove("CMUX_SURFACE_ID").env_remove("CMUX_WORKSPACE_ID")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(log));
        unsafe { cmd.pre_exec(|| { libc::setsid(); Ok(()) }); }
    }
    #[cfg(unix)]
    let mut spawn_child = || cmd.spawn();
    let mut child = spawn_child().map_err(|e| format!("il custode non parte: {e}"))?;
    let mut retries = 0;
    // the keeper is a child of Juggl as long as Juggl lives: I reap it when it ends (no zombies)
    let started = Instant::now();
    loop {
        if let Ok(Some(es)) = child.try_wait() {
            // the previous keeper is still delivering the last lines (restart for a model change): retry
            if es.code() == Some(3) && retries < 30 {
                retries += 1;
                std::thread::sleep(Duration::from_millis(200));
                child = spawn_child().map_err(|e| format!("il custode non parte: {e}"))?;
                continue;
            }
            return Err(match es.code() { Some(3) => "la conversazione ha gia' un custode al lavoro".into(), Some(4) => "Claude non parte".into(), c => format!("il custode si e' fermato subito ({c:?})") });
        }
        if sock.exists() {
            if let Ok(c) = connect(id, 0) {
                std::thread::spawn(move || { let _ = child.wait(); });
                return Ok(c);
            }
        }
        if started.elapsed() > Duration::from_secs(20) {
            unsafe { libc::kill(child.id() as i32, libc::SIGTERM); }
            std::thread::spawn(move || { let _ = child.wait(); });
            return Err("il custode non risponde".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Sessions with a live keeper (socket that answers). Dead sockets (killed keeper) are removed.
pub fn alive() -> Vec<String> {
    let mut out = vec![];
    for e in std::fs::read_dir(socket_dir()).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("sock") { continue; }
        let id = p.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string();
        if !crate::storage::is_valid_id(&id) { continue; }
        match UnixStream::connect(&p) {
            Ok(_) => out.push(id),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => { let _ = std::fs::remove_file(&p); }
            Err(_) => {}
        }
    }
    out
}

/// Quit everything: stops all keepers (also those not connected to this Juggl) and waits for them to close.
pub fn stop_all() {
    let Some(p) = program() else { return };
    let _ = crate::new_command(p).arg("--ferma-tutti").arg(socket_dir()).output();
}
