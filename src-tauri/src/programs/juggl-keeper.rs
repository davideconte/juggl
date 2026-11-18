// juggl-custode (keeper): keeps ONE hosted Juggl session alive even when the app closes or is reinstalled.
//
// Juggl launches it detached (setsid, new process session). The keeper starts `claude -p ... stream-json` as its
// child, owns its stdin and stdout and exposes the session on a local Unix socket:
//     ~/tavolo/sock/<id>.sock   (folder 700, socket 600, only processes of the same user are accepted)
// No network. If Juggl closes, Claude keeps working (sub-agents too); when Juggl restarts it reconnects
// and receives the events it missed from the buffer.
//
// Protocol (text lines):
//   Juggl -> keeper:   "#ciao {"dopo": N}"   first line: I want the events with number > N
//                      "{...}"               JSON line to pass as-is to Claude's stdin
//                      "#ferma"              stop Claude (TERM to the group, KILL after 8 s) and close
//   keeper -> Juggl:   "# {...}"             information: {"custode":1,"pid":..,"pid_custode":..,"seq":..,"vivo":..}
//                                            at the end {"fine": code}
//                      "N {...}"             line N of Claude's stdout
//
// One connection at a time: a new one replaces the old one. The keeper does not read the content (only the "result"
// type so as never to lose the last answer), so it does not need updating when Juggl changes.
//
// Usage:  juggl-custode <socket> <id> <claude> [claude arguments...]
//        juggl-custode --ferma-tutti <socket folder>      stops all sessions (Quit everything, even with Juggl closed)
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)] use std::os::unix::fs::PermissionsExt;
#[cfg(unix)] use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)] use std::os::unix::process::CommandExt;
// Windows: local AF_UNIX sockets (Windows 10 1803 and later) and the libc functions needed, from src/sys.rs
#[cfg(windows)] use uds_windows::{UnixListener, UnixStream};
#[cfg(windows)] #[path = "../sys.rs"] mod sys;
#[cfg(windows)] use sys as libc;
#[cfg(windows)] use sys::CommandExtJuggl;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
// engines (Nov 12): Codex (and later Gemini) translated into Claude's format, see codex_engine.rs
mod codex_engine;
/// Input of the hosted process: Claude's stdin or the translator's entry (engines Nov 12)
type InputSink = Box<dyn Write + Send>;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const VERSION: u32 = 3;   // engines (Nov 12): 2 = can also host Codex; 3 = Windows with hidden console (0.1.50): Juggl restarts keepers below 3 when the chat is idle
/// events buffer: enough for a long turn with images
const MAX_BYTES: usize = 48 * 1024 * 1024;
const MAX_LINES: usize = 60_000;
/// a connection that does not read (Juggl stuck) must never stop Claude: beyond this queue I detach it
const MAX_QUEUE: usize = 96 * 1024 * 1024;
/// Juggl closed for 12 hours and Claude silent for 2 hours: I close, so as not to leave forgotten processes
const ORPHAN_AFTER: Duration = Duration::from_secs(12 * 3600);
const SILENT_AFTER: Duration = Duration::from_secs(2 * 3600);
/// Claude finished with nobody connected: I wait a bit for Juggl to come and pick up the last lines
const FINAL_WAIT: Duration = Duration::from_secs(10 * 60);

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn on_signal(_: libc::c_int) { STOP.store(true, Ordering::SeqCst); }

fn lock_mutex<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

fn log(r: &str) {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // Windows: the keeper has a hidden console, the log goes to the file given by Juggl
    #[cfg(windows)]
    if let Some(f) = std::env::var_os("JUGGL_CUSTODE_LOG") {
        if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(f) { let _ = writeln!(h, "[custode {} {t}] {r}", std::process::id()); return; }
    }
    eprintln!("[custode {} {t}] {r}", std::process::id());
}

/// Where the launched programs' errors go: on Windows to Juggl's log (the console is hidden).
fn child_stderr() -> Stdio {
    #[cfg(windows)]
    if let Some(f) = std::env::var_os("JUGGL_CUSTODE_LOG") {
        if let Ok(h) = std::fs::OpenOptions::new().create(true).append(true).open(f) { return Stdio::from(h); }
    }
    Stdio::inherit()
}

struct Link { gen: u64, tx: Sender<Arc<String>>, flusso: UnixStream, scrittore: Option<std::thread::JoinHandle<()>>, coda: Arc<AtomicUsize> }

struct State {
    buffer: VecDeque<(u64, Arc<String>)>,
    byte: usize,
    seq: u64,
    ultimo_result: Option<(u64, Arc<String>)>,
    client: Option<Link>,
    gen: u64,
    fine: Option<i32>,
    ultimo_output: Instant,
    ultimo_client: Instant,
}

impl State {
    fn deliver_line(&mut self, line: Arc<String>) {
        let mut detach = false;
        if let Some(c) = &self.client {
            let n = c.coda.fetch_add(line.len() + 1, Ordering::SeqCst);
            if n > MAX_QUEUE || c.tx.send(line).is_err() { detach = true; }
        }
        if detach {
            if let Some(c) = self.client.take() { log("collegamento troppo lento o chiuso: lo stacco"); let _ = c.flusso.shutdown(std::net::Shutdown::Both); }
            self.ultimo_client = Instant::now();
        }
    }
}

fn main() {
    // Windows: started by Juggl with the hidden console (JUGGL_CUSTODE_LOG): the children inherit it
    #[cfg(windows)]
    if std::env::var_os("JUGGL_CUSTODE_LOG").is_some() { sys::CONSOLE_HIDDEN.store(true, std::sync::atomic::Ordering::Relaxed); }
    let a: Vec<String> = std::env::args().collect();
    if a.len() >= 3 && a[1] == "--ferma-tutti" { std::process::exit(stop_all(Path::new(&a[2]))); }
    if a.len() >= 2 && a[1] == "--versione" { println!("{VERSION}"); return; }
    if a.len() < 4 { eprintln!("uso: juggl-custode <socket> <id> <claude> [argomenti...]"); std::process::exit(2); }
    let sock = PathBuf::from(&a[1]);
    let id = a[2].clone();
    let (prog, args) = (a[3].clone(), a[4..].to_vec());
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    // is there already a live keeper for this session? then I do not start (never two copies of the conversation)
    if UnixStream::connect(&sock).is_ok() { log(&format!("{id}: c'e' gia' un custode vivo")); std::process::exit(3); }
    let _ = std::fs::remove_file(&sock);
    if let Some(d) = sock.parent() { let _ = std::fs::create_dir_all(d); #[cfg(unix)] { let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)); } }

    // engines (Nov 12): `juggl-custode <socket> <id> --motore codex <codex program> <JSON configuration>`
    let (mut child, input, stdout): (Child, Option<InputSink>, Box<dyn Read + Send>) = if prog == "--motore" && args.len() >= 3 && args[0] == "codex" {
        let conf = serde_json::from_str(&args[2]).unwrap_or(serde_json::Value::Null);
        match codex_engine::start(&args[1], conf) {
            Ok(a) => (a.figlio, Some(a.entrata), a.uscita),
            Err(e) => { log(&format!("{id}: {e}")); std::process::exit(4); }
        }
    } else {
    let mut child = match Command::new(&prog).args(&args).process_group(0)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(child_stderr()).spawn() {
        Ok(c) => c,
        Err(e) => { log(&format!("{id}: Claude non parte: {e}")); std::process::exit(4); }
    };
    let si = child.stdin.take().map(|x| Box::new(x) as InputSink);
    let so: Box<dyn Read + Send> = Box::new(child.stdout.take().expect("stdout"));
    (child, si, so)
    };
    let pid = child.id();
    let stdin: Arc<Mutex<Option<InputSink>>> = Arc::new(Mutex::new(input));

    // socket for the user only: tight umask during bind, then explicit 600
    let old_umask = unsafe { libc::umask(0o177) };
    let listener = UnixListener::bind(&sock);
    unsafe { libc::umask(old_umask); }
    let listener = match listener {
        Ok(l) => l,
        Err(e) => { log(&format!("{id}: socket non creato: {e}")); terminate(&mut child); std::process::exit(5); }
    };
    #[cfg(unix)] { let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)); }
    log(&format!("{id}: pronto, claude pid {pid}"));

    let st = Arc::new(Mutex::new(State { buffer: VecDeque::new(), byte: 0, seq: 0, ultimo_result: None, client: None, gen: 0, fine: None,
        ultimo_output: Instant::now(), ultimo_client: Instant::now() }));

    // reader of Claude's stdout: numbers, stores, forwards. Never blocks on Juggl (queue per connection).
    let st2 = st.clone();
    let reader = std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        let mut line = Vec::new();
        loop {
            line.clear();
            match r.read_until(b'\n', &mut line) { Ok(0) | Err(_) => break, Ok(_) => {} }
            while line.last().map(|c| *c == b'\n' || *c == b'\r').unwrap_or(false) { line.pop(); }
            if line.is_empty() { continue; }
            let text = String::from_utf8_lossy(&line).to_string();
            let mut s = lock_mutex(&st2);
            s.seq += 1;
            let n = s.seq;
            let ready = Arc::new(format!("{n} {text}\n"));
            if text.starts_with("{\"type\":\"result\"") { s.ultimo_result = Some((n, ready.clone())); }
            s.byte += ready.len();
            s.buffer.push_back((n, ready.clone()));
            while s.byte > MAX_BYTES || s.buffer.len() > MAX_LINES {
                if let Some((_, x)) = s.buffer.pop_front() { s.byte -= x.len(); } else { break; }
            }
            s.ultimo_output = Instant::now();
            s.deliver_line(ready);
        }
    });

    // incoming connections
    let (st3, stdin3) = (st.clone(), stdin.clone());
    std::thread::spawn(move || {
        for c in listener.incoming() {
            let Ok(c) = c else { continue };
            if !same_user(&c) { log("collegamento da un altro utente: rifiutato"); continue; }
            let (st4, stdin4) = (st3.clone(), stdin3.clone());
            std::thread::spawn(move || connect_client(c, st4, stdin4, pid));
        }
    });

    // wait: Claude finished, signal or #ferma, orphaned for too long
    let mut stopped = false;
    let code = loop {
        if let Ok(Some(es)) = child.try_wait() { break es.code().unwrap_or(-1); }
        if STOP.load(Ordering::SeqCst) && !stopped {
            stopped = true;
            log(&format!("{id}: fermo Claude"));
            send_signal(pid, libc::SIGTERM);
            let p = pid;
            std::thread::spawn(move || { std::thread::sleep(Duration::from_secs(8)); send_signal(p, libc::SIGKILL); });
        }
        {
            let s = lock_mutex(&st);
            if !stopped && s.client.is_none() && s.ultimo_client.elapsed() > ORPHAN_AFTER && s.ultimo_output.elapsed() > SILENT_AFTER {
                drop(s);
                log(&format!("{id}: nessuno collegato da 12 ore e Claude fermo: chiudo"));
                STOP.store(true, Ordering::SeqCst);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    // the children left in Claude's group (MCP servers, commands): away with them too
    send_signal(pid, libc::SIGTERM);
    let _ = reader.join();
    *lock_mutex(&stdin) = None;
    log(&format!("{id}: Claude finito ({code})"));
    let fine = Arc::new(format!("# {{\"fine\":{code}}}\n"));
    {
        let mut s = lock_mutex(&st);
        s.fine = Some(code);
        s.deliver_line(fine);
    }
    // deliver the last lines: right away to whoever is connected, otherwise wait a bit for Juggl to come back
    let start = Instant::now();
    loop {
        let taken = { let mut s = lock_mutex(&st); s.client.take() };
        if let Some(mut c) = taken {
            // first remove the socket: whoever restarts the session right after (model change) finds the spot free
            let _ = std::fs::remove_file(&sock);
            let scrittore = c.scrittore.take();
            drop(c.tx);
            if let Some(h) = scrittore { let _ = h.join(); }
            let _ = c.flusso.shutdown(std::net::Shutdown::Both);
            break;
        }
        if start.elapsed() > FINAL_WAIT || STOP.load(Ordering::SeqCst) && start.elapsed() > Duration::from_secs(2) { break; }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = std::fs::remove_file(&sock);
    std::thread::sleep(Duration::from_millis(300));
    send_signal(pid, libc::SIGKILL);
}

/// Is the process on the other side of the socket of the same user?
#[cfg(windows)]
fn same_user(_: &UnixStream) -> bool { true }   // Windows: the socket is in the user's folder, which only they can open
#[cfg(target_os = "linux")]
fn same_user(c: &UnixStream) -> bool {
    use std::os::unix::io::AsRawFd;
    let mut cr: libc::ucred = unsafe { std::mem::zeroed() };
    let mut n = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe { libc::getsockopt(c.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cr as *mut libc::ucred as *mut libc::c_void, &mut n) };
    r == 0 && cr.uid == unsafe { libc::getuid() }
}
#[cfg(all(unix, not(target_os = "linux")))]
fn same_user(c: &UnixStream) -> bool {
    use std::os::unix::io::AsRawFd;
    let (mut uid, mut gid) = (0 as libc::uid_t, 0 as libc::gid_t);
    unsafe { libc::getpeereid(c.as_raw_fd(), &mut uid, &mut gid) == 0 && uid == libc::getuid() }
}

/// Signal to Claude's group (Claude leads its own group: it reaches MCP servers and child commands too).
/// Only to the group, never to the pid alone: after the end the pid may already belong to another process.
fn send_signal(pid: u32, sig: i32) {
    if pid <= 1 { return; }
    unsafe { libc::kill(-(pid as i32), sig); }
}

fn terminate(f: &mut Child) {
    send_signal(f.id(), libc::SIGTERM);
    for _ in 0..40 { if let Ok(Some(_)) = f.try_wait() { return; } std::thread::sleep(Duration::from_millis(200)); }
    send_signal(f.id(), libc::SIGKILL);
    let _ = f.wait();
}

/// A connection: first line "#ciao {"dopo": N}", then replay of the buffer and real-time forwarding.
fn connect_client(c: UnixStream, st: Arc<Mutex<State>>, stdin: Arc<Mutex<Option<InputSink>>>, pid: u32) {
    let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(read_half) = c.try_clone() else { return };
    let mut r = BufReader::new(read_half);
    let mut first_line = String::new();
    if r.read_line(&mut first_line).is_err() || !first_line.starts_with("#ciao") { return; }
    let _ = c.set_read_timeout(None);
    let after = first_line.trim_start_matches("#ciao").trim();
    let after: u64 = serde_json::from_str::<serde_json::Value>(after).ok().and_then(|v| v.get("dopo").and_then(|x| x.as_u64())).unwrap_or(0);
    let Ok(write_half) = c.try_clone() else { return };
    let (tx, rx) = channel::<Arc<String>>();
    let coda = Arc::new(AtomicUsize::new(0));
    let coda2 = coda.clone();
    let scrittore = std::thread::spawn(move || {
        let mut w = std::io::BufWriter::with_capacity(256 * 1024, write_half);
        loop {
            // write everything that is queued, then flush the buffer once
            let Ok(x) = rx.recv() else { break };
            coda2.fetch_sub(x.len().min(coda2.load(Ordering::SeqCst)), Ordering::SeqCst);
            if let Err(e) = w.write_all(x.as_bytes()) { log(&format!("scrittura verso Juggl interrotta ({e})")); return; }
            while let Ok(y) = rx.try_recv() {
                coda2.fetch_sub(y.len().min(coda2.load(Ordering::SeqCst)), Ordering::SeqCst);
                if w.write_all(y.as_bytes()).is_err() { return; }
            }
            if let Err(e) = w.flush() { log(&format!("invio verso Juggl interrotto ({e})")); return; }
        }
        let _ = w.flush();
    });
    let gen;
    {
        let mut s = lock_mutex(&st);
        if let Some(v) = s.client.take() { log("nuovo collegamento: stacco il precedente"); let _ = v.flusso.shutdown(std::net::Shutdown::Both); }
        s.gen += 1;
        gen = s.gen;
        let first_seq = s.buffer.front().map(|x| x.0).unwrap_or(s.seq + 1);
        let info = serde_json::json!({"custode": VERSION, "app": env!("CARGO_PKG_VERSION"), "pid": pid, "pid_custode": std::process::id(), "seq": s.seq,
            "vivo": s.fine.is_none(), "dal": first_seq, "dopo": after});
        let _ = tx.send(Arc::new(format!("# {info}\n")));
        // the last answer is never lost, even if it left the buffer
        if let Some((n, x)) = &s.ultimo_result { if *n > after && *n < first_seq { let _ = tx.send(x.clone()); } }
        for (n, x) in s.buffer.iter() { if *n > after { let _ = tx.send(x.clone()); } }
        if let Some(exit_code) = s.fine { let _ = tx.send(Arc::new(format!("# {{\"fine\":{exit_code}}}\n"))); }
        s.client = Some(Link { gen, tx, flusso: c, scrittore: Some(scrittore), coda });
    }
    // lines from Juggl to Claude
    let mut line = String::new();
    loop {
        line.clear();
        match r.read_line(&mut line) { Ok(0) | Err(_) => break, Ok(_) => {} }
        let t = line.trim_end_matches(['\n', '\r']);
        if t.starts_with('{') {
            let mut g = lock_mutex(&stdin);
            let ok = g.as_mut().map(|si| si.write_all(t.as_bytes()).and_then(|_| si.write_all(b"\n")).and_then(|_| si.flush()).is_ok()).unwrap_or(false);
            if !ok { log("Claude non accetta piu' messaggi"); }
        } else if t == "#ferma" {
            STOP.store(true, Ordering::SeqCst);
        }
    }
    let mut s = lock_mutex(&st);
    if s.client.as_ref().map(|x| x.gen) == Some(gen) { s.client = None; s.ultimo_client = Instant::now(); }
}

/// Quit everything: "#ferma" to every live keeper in the folder, then wait for them to close (at most 12 s).
fn stop_all(dir: &Path) -> i32 {
    let mut alive = vec![];
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("sock") { continue; }
        match UnixStream::connect(&p) {
            Ok(mut c) => {
                let _ = c.write_all(b"#ciao {\"dopo\":18446744073709551615}\n#ferma\n");
                alive.push((p, c));
            }
            Err(_) => { let _ = std::fs::remove_file(&p); }
        }
    }
    let n = alive.len();
    let start = Instant::now();
    for (p, mut c) in alive {
        let _ = c.set_read_timeout(Some(Duration::from_millis(500)));
        let mut b = [0u8; 4096];
        while start.elapsed() < Duration::from_secs(12) {
            match c.read(&mut b) { Ok(0) => break, Ok(_) => {}, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}, Err(_) => break }
        }
        let _ = p;
    }
    println!("fermati {n}");
    0
}
