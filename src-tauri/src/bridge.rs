// Bridge for the iPhone app: a small HTTP server inside the Mac app, reachable on the local network and on Tailscale.
// Every request must carry the key (Authorization: Bearer header, or ?t= for images).
// Updates arrive in real time with Server-Sent Events on /api/eventi.
use crate::{base_dir, load_json, unix_now, write_state};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server, StatusCode};

/// Bridge port: 47800, or JUGGL_PORTA / "porta" in the settings (for tests with another HOME).
pub fn port() -> u16 { match ACTUAL_PORT.load(std::sync::atomic::Ordering::Relaxed) { 0 => crate::profile::port(), p => p } }

/// Port the bridge really listens on (0 = the chosen one). On Windows WSL and Hyper-V reserve blocks of ports
/// that show as "already in use" even with nobody listening (studio PC: 45000-48900): then the bridge tries
/// others and the phone QR carries the real one.
static ACTUAL_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
const FALLBACK_PORTS: [u16; 4] = [38470, 39470, 41470, 43470];

/// The bridge listens on the network (0.0.0.0) and not only on this Mac: the iPhone can connect (Settings > iPhone).
pub static ON_NETWORK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn clients() -> &'static Mutex<Vec<Sender<String>>> {
    static C: OnceLock<Mutex<Vec<Sender<String>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(vec![]))
}

/// Sends an event to all connected iPhones (those that no longer answer are removed).
pub fn broadcast(event: &str, data: &str) {
    let msg = format!("event: {event}\ndata: {}\n\n", data.replace('\n', " "));
    clients().lock().unwrap().retain(|c| c.send(msg.clone()).is_ok());
}

pub fn key() -> String {
    let p = base_dir().join("ponte.json");
    if let Some(v) = load_json(&p) { if let Some(t) = v.get("token").and_then(|x| x.as_str()) { return t.to_string(); } }
    let mut b = [0u8; 24];
    #[cfg(windows)] crate::sys::random_bytes(&mut b);
    #[cfg(unix)]
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") { let _ = f.read_exact(&mut b); }
    let t: String = b.iter().map(|x| format!("{x:02x}")).collect();
    write_state(&p, &json!({"token": t, "porta": port(), "creata": unix_now()}));
    t
}

fn h(k: &str, v: &str) -> Header { Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap() }

fn reply_json(req: tiny_http::Request, code: u16, v: &Value) {
    let _ = req.respond(Response::from_string(v.to_string()).with_status_code(StatusCode(code))
        .with_header(h("Content-Type", "application/json; charset=utf-8")).with_header(h("Cache-Control", "no-store")));
}

fn query_param(url: &str, name: &str) -> Option<String> {
    let q = url.split('?').nth(1)?;
    for kv in q.split('&') {
        let mut it = kv.splitn(2, '=');
        if it.next()? == name { return Some(url_decode(it.next().unwrap_or(""))); }
    }
    None
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => { out.push(b' '); i += 1; }
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(&s[i + 1..i + 3], 16) { Ok(x) => { out.push(x); i += 3; } Err(_) => { out.push(b'%'); i += 1; } }
            }
            c => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

const MEDIA: [&str; 11] = ["png", "jpg", "jpeg", "gif", "webp", "heic", "svg", "mp4", "mov", "m4v", "webm"];

fn file_mime(p: &str) -> &'static str {
    match p.rsplit('.').next().unwrap_or("").to_lowercase().as_str() {
        "png" => "image/png", "jpg" | "jpeg" => "image/jpeg", "gif" => "image/gif", "webp" => "image/webp", "heic" => "image/heic",
        "svg" => "image/svg+xml", "mp4" | "m4v" => "video/mp4", "mov" => "video/quicktime", "webm" => "video/webm", _ => "application/octet-stream",
    }
}

pub fn start() {
    let token = key();
    #[allow(non_snake_case)] let mut port = port();
    // the bridge must never stay off: if the server closes (8/11: port vanished after hours,
    // iPhone "refused"), I reopen it. A guardian checks every 20 s that the port really answers.
    std::thread::spawn(move || loop {
        let mut attempts = 0u32;
        let server = loop {
            // on the network only with the iPhone app on; otherwise only this Mac (no firewall prompt)
            let bind_addr = if crate::profile::is_enabled("iphone") { "0.0.0.0" } else { "127.0.0.1" };
            match Server::http((bind_addr, port)) {
                Ok(s) => { ON_NETWORK.store(bind_addr == "0.0.0.0", std::sync::atomic::Ordering::Relaxed); break std::sync::Arc::new(s) }
                Err(e) => {
                    attempts += 1;
                    // Windows: port reserved by the system -> try the fallback ports, one per attempt
                    if cfg!(windows) && attempts >= 2 && (attempts as usize) < 2 + FALLBACK_PORTS.len() {
                        port = FALLBACK_PORTS[attempts as usize - 2];
                        ACTUAL_PORT.store(port, std::sync::atomic::Ordering::Relaxed);
                    }
                    if attempts == 1 || attempts % 30 == 0 { crate::log_juggl(&format!("ponte: porta {port} non disponibile ({e}), riprovo")); }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        };
        crate::log_juggl(&format!("ponte: in ascolto sulla porta {port}{}", if attempts > 0 { format!(" dopo {attempts} tentativi") } else { String::new() }));
        let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        {
            let (srv, alive) = (server.clone(), alive.clone());
            std::thread::spawn(move || {
                let mut failures = 0;
                while alive.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(20));
                    let ok = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(3)).is_ok();
                    failures = if ok { 0 } else { failures + 1 };
                    if failures >= 2 { crate::log_juggl("ponte: la porta non risponde, lo riapro"); alive.store(false, std::sync::atomic::Ordering::Relaxed); srv.unblock(); break; }
                }
            });
        }
        loop {
            match server.recv() {
                Ok(req) => { let token = token.clone(); std::thread::spawn(move || handle(req, &token)); }
                Err(e) => { crate::log_juggl(&format!("ponte: ascolto interrotto ({e})")); break; }
            }
            if !alive.load(std::sync::atomic::Ordering::Relaxed) { break; }
        }
        alive.store(false, std::sync::atomic::Ordering::Relaxed);
        drop(server);
        crate::log_juggl("ponte: chiuso, lo riapro");
        std::thread::sleep(Duration::from_secs(1));
    });
}

/// Real-time channel: I write directly on the connection, no buffer, every event goes out at once.
fn events(req: tiny_http::Request) {
    let (tx, rx) = channel::<String>();
    clients().lock().unwrap().push(tx);
    let mut w = req.into_writer();
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\nevent: pronto\ndata: {}\n\n";
    if w.write_all(head.as_bytes()).is_err() || w.flush().is_err() { return; }
    loop {
        let m = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => ": vivo\n\n".into(),   // keeps the connection open
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if w.write_all(m.as_bytes()).is_err() || w.flush().is_err() { break; }
    }
}

/// List of texts from a JSON field (attachments: paths). Strings only, at most 10.
fn string_list(b: &Value, k: &str) -> Vec<String> {
    b.get(k).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).take(10).collect()).unwrap_or_default()
}

/// Only from the Mac itself, the local network (studio, home) and Tailscale: never from the internet.
fn allowed(req: &tiny_http::Request) -> bool {
    match req.remote_addr().map(|a| a.ip()) {
        Some(std::net::IpAddr::V4(ip)) => {
            let o = ip.octets();
            ip.is_loopback() || ip.is_private() || (o[0] == 100 && (64..=127).contains(&o[1]))   // 100.64/10 = Tailscale (CGNAT)
        }
        Some(std::net::IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 || ip.to_ipv4_mapped().map(|v| v.is_private() || v.is_loopback()).unwrap_or(false),
        None => false,
    }
}

fn handle(mut req: tiny_http::Request, token: &str) {
    if !allowed(&req) { return reply_json(req, 403, &json!({"errore": "solo rete locale o Tailscale"})); }
    let url = req.url().to_string();
    let auth_ok = req.headers().iter().any(|x| x.field.equiv("Authorization") && x.value.as_str() == format!("Bearer {token}"))
        || query_param(&url, "t").as_deref() == Some(token);
    if !auth_ok { return reply_json(req, 401, &json!({"errore": "chiave mancante o sbagliata"})); }
    if crate::messages::from_phone(&req) { crate::messages::heartbeat(); }   // the iPhone is alive (messages.rs)
    let path = url.split('?').next().unwrap_or("").to_string();
    // without a valid license (license.rs) the bridge only answers what is needed to connect: iPhone app state,
    // notification token and license state. Everything else 403 {"errore":"licenza"}, and the iPhone says so.
    if !crate::license::is_valid() && !matches!(path.as_str(), "/api/iphone" | "/api/iphone/token" | "/api/android/token" | "/api/licenza") {
        return reply_json(req, 403, &json!({"errore": "licenza"}));
    }
    if path == "/api/licenza" {
        let st = crate::license::status_json();
        return reply_json(req, 200, &json!({"valida": st["valida"], "stato": st["stato"], "motivo": st["motivo"]}));
    }
    let mut body = String::new();
    if *req.method() == Method::Post { let _ = req.as_reader().take(90_000_000).read_to_string(&mut body); }
    let b: Value = serde_json::from_str(&body).unwrap_or(json!({}));
    let st = |k: &str| b.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    match (req.method().clone(), path.as_str()) {
        (Method::Get, "/api/tavolo") => { let mut d = crate::desk_data(); crate::local_models::for_iphone(&mut d); reply_json(req, 200, &d) }
        (Method::Get, "/api/media-recenti") => reply_json(req, 200, &crate::media_strip::recent_media()),
        (Method::Get, "/api/sessione") => {
            let mut r = crate::session_data(query_param(&url, "id").unwrap_or_default(), query_param(&url, "dal"), query_param(&url, "prima").and_then(|x| x.parse().ok()));   // fast chats (14/11): + prima
            // ?ultimi=N: only the last N events (fast first open from the iPhone); without it, all as before
            if let Some(n) = query_param(&url, "ultimi").and_then(|x| x.parse::<usize>().ok()) { keep_last(&mut r, n); }
            if query_param(&url, "leggero").as_deref() == Some("1") { lighten(&mut r); slim(&mut r); }
            reply_json(req, 200, &r)
        }
        (Method::Get, "/api/sottoagente") => {
            let mut r = crate::sottoagente(query_param(&url, "id").unwrap_or_default(), query_param(&url, "agente").unwrap_or_default());
            if query_param(&url, "leggero").as_deref() == Some("1") { lighten(&mut r); }
            reply_json(req, 200, &r)
        }
        (Method::Post, "/api/invia") => {
            let imgs: Vec<String> = b.get("immagini").and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let immediately = b.get("subito").and_then(|x| x.as_bool()).unwrap_or(false);
            // a message between chats (bin/juggl: "da": "chat", or the Python program that sends it) is not a user request
            let from_script = req.headers().iter().any(|h| h.field.equiv("User-Agent") && h.value.as_str().starts_with("Python-urllib"));
            let between_chats = imgs.is_empty() && (b.get("da").and_then(|x| x.as_str()) == Some("chat") || from_script);
            let r = if between_chats { crate::send_between_chats(st("id"), st("testo"), immediately) } else { crate::invia(st("id"), st("testo"), Some(imgs), Some(immediately)) };
            reply_json(req, 200, &r)
        }
        // live edits (12/11): Undo a reply or a file from the iPhone
        (Method::Post, "/api/modifiche/annulla") => reply_json(req, 200, &crate::live_edits::undo_session(&st("id"), &st("da"), &st("a"), b.get("path").and_then(|x| x.as_str()).filter(|x| !x.is_empty()))),
        // change account (12/11): Claude login from the iPhone. Never tokens in the responses (src/claude_login.rs)
        // commands (13/11): "/" menu of the iPhone and running Juggl commands (/api/invia intercepts them too)
        (Method::Get, "/api/comandi") => reply_json(req, 200, &crate::commands::list(&query_param(&url, "id").unwrap_or_default())),
        (Method::Get, "/api/modelli") => reply_json(req, 200, &crate::commands::models()),   // models (14/11): full list from the CLI
        (Method::Post, "/api/comando") => reply_json(req, 200, &crate::commands::run(&st("id"), &st("testo"))),
        (Method::Get, "/api/accesso/stato") => reply_json(req, 200, &crate::claude_login::status()),
        (Method::Post, "/api/accesso/avvia") => reply_json(req, 200, &crate::claude_login::start()),
        (Method::Post, "/api/accesso/codice") => reply_json(req, 200, &crate::claude_login::code(&st("codice"))),
        (Method::Post, "/api/accesso/annulla") => reply_json(req, 200, &crate::claude_login::cancel()),
        (Method::Post, "/api/accesso/esci") => reply_json(req, 200, &crate::claude_login::logout(b.get("conferma").and_then(|x| x.as_bool()).unwrap_or(false))),
        (Method::Post, "/api/interrompi") => reply_json(req, 200, &crate::interrompi(st("id"))),
        // live activity (12/11): the iPhone registers the token of its Live Activity ("" when it ends) (src/live.rs)
        (Method::Post, "/api/live/token") => reply_json(req, 200, &crate::live::register(&st("token"), &st("attivita"), &st("ambiente"))),
        // live activity (12/11): token to start the activity with the app closed (push-to-start) (src/live.rs)
        (Method::Post, "/api/live/avvio") => reply_json(req, 200, &crate::live::register_start(&st("token"), &st("ambiente"))),
        (Method::Post, "/api/sottoagente/ferma") => reply_json(req, 200, &crate::ferma_sottoagente(st("id"), st("agente"))),
        // open as chat (12/11): {id, agente, ferma}. If it is working and ferma is not true it answers {conferma: true} (src/open_as_chat.rs)
        (Method::Post, "/api/sottoagente/apri") => reply_json(req, 200, &crate::open_as_chat::open(st("id"), st("agente"), b.get("ferma").and_then(|x| x.as_bool()).unwrap_or(false))),
        (Method::Post, "/api/coda/ora") => reply_json(req, 200, &crate::invia_ora_dalla_coda(st("id"), b.get("indice").and_then(|x| x.as_u64()).unwrap_or(0) as usize, b.get("mid").and_then(|x| x.as_str()).map(|x| x.to_string()))),
        (Method::Post, "/api/coda/togli") => reply_json(req, 200, &crate::togli_dalla_coda(st("id"), b.get("indice").and_then(|x| x.as_u64()).unwrap_or(0) as usize, b.get("mid").and_then(|x| x.as_str()).map(|x| x.to_string()))),
        (Method::Post, "/api/pilota/annulla") => reply_json(req, 200, &crate::pilota_annulla(st("id"), b.get("spegni").and_then(|x| x.as_bool()).unwrap_or(false))),
        (Method::Post, "/api/pilota/ora") => reply_json(req, 200, &crate::pilota_ora(st("id"))),
        (Method::Post, "/api/pilota/imposta") => reply_json(req, 200, &crate::pilota_imposta(st("id"), b.get("acceso").and_then(|x| x.as_bool()).unwrap_or(true))),
        (Method::Post, "/api/scelta") => reply_json(req, 200, &crate::rispondi_scelta(st("id"), st("n"), Some(st("testo")))),
        // permissions (12/11): pending permission requests of all chats, and the answer (scelta: consenti | sempre | nega).
        // /api/scelta works too (n 1/2/3): the box arrives in /api/sessione as domanda_terminale.
        (Method::Get, "/api/permessi") => reply_json(req, 200, &crate::permissions::list()),
        (Method::Post, "/api/permesso") => reply_json(req, 200, &crate::permissions::answer(&st("id"), Some(st("richiesta").as_str()), &st("scelta"), &st("testo"))),
        (Method::Get, "/api/locale/modelli") => reply_json(req, 200, &crate::modelli_locali()),
        (Method::Post, "/api/locale/nuova") => reply_json(req, 200, &crate::nuova_locale(st("modello"), st("testo"))),
        (Method::Get, "/api/mac") => reply_json(req, 200, &crate::stato_mac()),
        (Method::Post, "/api/mac/pulisci") => reply_json(req, 200, &crate::pulisci_mac(b.get("urgente").and_then(|x| x.as_bool()).unwrap_or(false))),
        (Method::Post, "/api/mac/esci") => reply_json(req, 200, &crate::esci_app(st("nome"))),
        (Method::Get, "/api/mac/scansione") => {
            let p = matches!(query_param(&url, "profonda").as_deref(), Some("1") | Some("true") | Some("si"));
            reply_json(req, 200, &crate::mac_scansione(p))
        }
        (Method::Get, "/api/mac/ultima") => reply_json(req, 200, &crate::mac_ultima()),
        (Method::Post, "/api/mac/libera") => reply_json(req, 200, &crate::mac_libera(st("id"), b.get("conferma").and_then(|x| x.as_bool()).unwrap_or(false))),
        (Method::Post, "/api/mac/libera-auto") => reply_json(req, 200, &crate::mac_libera_auto(b.get("obiettivo_gb").and_then(|x| x.as_f64()).unwrap_or(0.0))),
        (Method::Get, "/api/mac/ram") => reply_json(req, 200, &crate::mac_ram()),
        (Method::Get, "/api/mac/storico") => reply_json(req, 200, &crate::mac_storico()),
        (Method::Post, "/api/mac/ripristina") => reply_json(req, 200, &crate::mac_ripristina(st("n"))),
        // space (14/11): the Space view on the iPhone too. GET = items; POST {"args": [...]} = same closed list as the Mac
        (Method::Get, "/api/mac/spazio") => reply_json(req, 200, &crate::session_care::mac_space(&["voci".to_string()])),
        (Method::Post, "/api/mac/spazio") => {
            let args: Vec<String> = b.get("args").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            reply_json(req, 200, &crate::session_care::mac_space(&args))
        }
        (Method::Get, "/api/cerca") => reply_json(req, 200, &crate::cerca_ovunque(query_param(&url, "q").unwrap_or_default())),
        // search of ⌘K (local index) with its filters, and inside a single conversation
        (Method::Get, "/api/cerca-tutto") => {
            let si = |k: &str| matches!(query_param(&url, k).as_deref(), Some("1") | Some("true") | Some("si"));
            let num = |k: &str| query_param(&url, k).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
            let r = crate::search_for_iphone(&query_param(&url, "q").unwrap_or_default(), &query_param(&url, "progetto").unwrap_or_default(),
                si("solo_mie"), si("immagini"), num("giorni"), query_param(&url, "limite").and_then(|x| x.parse::<u64>().ok()).unwrap_or(50));
            reply_json(req, 200, &r)
        }
        (Method::Get, "/api/cerca-sessione") => {
            let id = query_param(&url, "id").unwrap_or_default();
            if !crate::storage::is_valid_id(&id) { return reply_json(req, 200, &json!({"risultati": [], "ms": 0, "errore": "id non valido"})); }
            reply_json(req, 200, &crate::cerca_in_sessione(id, query_param(&url, "q").unwrap_or_default()))
        }
        (Method::Get, "/api/chiusa") => reply_json(req, 200, &crate::sessione_chiusa(query_param(&url, "id").unwrap_or_default())),
        (Method::Get, "/api/notifiche") => reply_json(req, 200, &crate::notifiche()),
        (Method::Post, "/api/notifiche/lette") => reply_json(req, 200, &crate::notifiche_lette()),
        (Method::Post, "/api/messaggio") => reply_json(req, 200, &crate::messages::receive(&b)),
        (Method::Get, "/api/messaggio/stato") => reply_json(req, 200, &crate::messages::status()),
        (Method::Post, "/api/iphone") => reply_json(req, 200, &crate::messages::app_state(&b)),
        (Method::Post, "/api/iphone/token") => reply_json(req, 200, &crate::push::register_token(&b)),
        // Android: FCM token for notifications (src/push.rs, android module)
        (Method::Post, "/api/android/token") => reply_json(req, 200, &crate::push::android::register_token(&b)),
        (Method::Post, "/api/iphone/push-prova") => reply_json(req, 200, &crate::push::test_push(&b)),
        (Method::Post, "/api/visore/chiudi") => {
            let r = match crate::APP.get() { Some(app) => crate::close_viewer_from(app.clone()), None => json!({"ok": false}) };
            reply_json(req, 200, &r)
        }
        (Method::Post, "/api/visore") if b.get("avviso").and_then(|x| x.as_bool()).unwrap_or(false) => {
            // from `mostra` (one session): no viewer opening while the user is doing something else. Light sound and mark
            // on the chat and on the strip; the viewer opens only when they ask for it (click on the mark or on the strip).
            let media = b.get("media").cloned().unwrap_or(json!([]));
            let sessione = b.get("sessione").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if crate::user_at_mac() { let _ = crate::new_command("afplay").args(["-v", "0.35", "/System/Library/Sounds/Tink.aiff"]).spawn(); }
            if let Some(app) = crate::APP.get() { use tauri::Emitter; let _ = app.emit("media-nuovo", json!({"sessione": sessione, "media": media})); }
            reply_json(req, 200, &json!({"ok": true, "avviso": true}))
        }
        (Method::Post, "/api/visore") => {
            // opens a photo large on the Mac monitor (also from the iPhone)
            let r = match crate::APP.get() { Some(app) => crate::apri_visore(app.clone(), b.get("media").cloned().unwrap_or(json!([])), b.get("indice").and_then(|x| x.as_u64()).unwrap_or(0) as usize), None => json!({"ok": false}) };
            reply_json(req, 200, &r)
        }
        (Method::Get, "/api/consumi") => reply_json(req, 200, &crate::consumi()),
        (Method::Get, "/api/consumi/analisi") => reply_json(req, 200, &crate::consumi_analisi()),
        (Method::Post, "/api/regola") => reply_json(req, 200, &crate::regola(st("id"), st("modello"), st("effort"))),
        (Method::Post, "/api/regola-locale") => reply_json(req, 200, &crate::regola_locale(st("id"), st("modello"), b.get("temperatura").and_then(|x| x.as_f64()).unwrap_or(0.7), b.get("contesto").and_then(|x| x.as_u64()).unwrap_or(8192), b.get("think").and_then(|x| x.as_bool()))),
        (Method::Post, "/api/letta") => reply_json(req, 200, &crate::mark_seen_by_user(&st("id"))),
        (Method::Post, "/api/non-serve") => reply_json(req, 200, &crate::non_serve(st("id"))),
        (Method::Post, "/api/nota-spostato") => reply_json(req, 200, &crate::nota_spostato(st("id"))),
        (Method::Post, "/api/elimina") => reply_json(req, 200, &crate::elimina_chat(st("id"))),
        (Method::Get, "/api/ok") => reply_json(req, 200, &crate::ok_voci()),
        (Method::Post, "/api/riassunto-azioni") => {
            let righe: Vec<String> = b.get("righe").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            reply_json(req, 200, &crate::summaries::summary(&st("chiave"), &righe, &st("richiesta")))
        }
        // iPhone voice mode: 1-3 sentences to say aloud (summaries.rs voce_breve, never for locale:)
        (Method::Post, "/api/voce-breve") => reply_json(req, 200, &crate::summaries::short_voice(&st("id"), &st("testo"), &st("richiesta"))),
        (Method::Post, "/api/ok/manda") => reply_json(req, 200, &crate::ok_manda(st("sessione"), st("azione"), st("titolo"), Some(st("testo")))),
        (Method::Post, "/api/ok/azione") => reply_json(req, 200, &crate::ok_azione(st("id"), st("azione"), Some(st("testo")))),
        (Method::Get, "/api/richieste-ai") => reply_json(req, 200, &crate::richieste_ai()),
        (Method::Post, "/api/richieste-ai/azione") => reply_json(req, 200, &crate::richiesta_ai_azione(st("id"), st("azione"))),
        (Method::Post, "/api/richieste-ai/lette") => reply_json(req, 200, &crate::ai_requests_mark_read(st("sessione"))),
        (Method::Post, "/api/richieste-ai/rivaluta") => reply_json(req, 200, &crate::richieste_ai_rivaluta()),
        (Method::Get, "/api/consumi/dettaglio") => reply_json(req, 200, &crate::consumi_dettaglio()),
        // help and reports (17/11): the same help chat as the Mac, from the iPhone (support.rs). Preview and ok for every send.
        (Method::Get, "/api/aiuto") => reply_json(req, 200, &crate::support::aiuto_storia()),
        (Method::Post, "/api/aiuto/immagine") => reply_json(req, 200, &crate::support::aiuto_salva_immagine(st("b64"))),
        (Method::Post, "/api/aiuto/chiedi") => reply_json(req, 200, &crate::support::aiuto_chiedi(st("testo"), crate::support::only_attachments(string_list(&b, "allegati")))),
        (Method::Post, "/api/aiuto/anteprima") => reply_json(req, 200, &crate::support::preview(b.get("bozza").cloned().unwrap_or(json!({})), crate::support::only_attachments(string_list(&b, "allegati")), b.get("con_diagnostica").and_then(|x| x.as_bool()).unwrap_or(true), &st("da"))),
        (Method::Post, "/api/aiuto/invia") => reply_json(req, 200, &crate::support::aiuto_invia(st("id_locale"), b.get("bozza").cloned().unwrap_or(json!({})))),
        (Method::Post, "/api/aiuto/stati") => reply_json(req, 200, &crate::support::aiuto_stati()),
        (Method::Post, "/api/aiuto/svuota") => reply_json(req, 200, &crate::support::aiuto_svuota()),
        (Method::Get, "/api/aiutanti") => reply_json(req, 200, &crate::aiutanti_stato()),
        (Method::Post, "/api/aiutanti") => reply_json(req, 200, &crate::aiutanti_imposta(b.clone())),
        (Method::Post, "/api/nuova") => reply_json(req, 200, &crate::new_without_app(st("cartella"), st("testo"), st("modello"), st("nome"), b.get("forza").and_then(|x| x.as_bool()).unwrap_or(false) || matches!(st("forza").as_str(), "1" | "true"))),
        (Method::Post, "/api/chiudi") => reply_json(req, 200, &crate::chiudi(st("id"))),
        (Method::Get, "/api/progetti") => reply_json(req, 200, &crate::progetti()),
        (Method::Post, "/api/iberna") => reply_json(req, 200, &crate::iberna(st("id"))),
        // sessions from the cmux terminal to the Juggl keeper (src/migrate.rs)
        (Method::Post, "/api/migra") => reply_json(req, 200, &crate::migrate::command(&b)),
        (Method::Post, "/api/esci-da-tutto") => { if let Some(a) = crate::APP.get() { crate::hosted::quit_all(a.clone()); } reply_json(req, 200, &json!({"ok": true})) }
        (Method::Post, "/api/riprendi") => reply_json(req, 200, &crate::resume_without_app(st("id"))),
        // archive: the list of archived ones is the "archiviate" field of /api/tavolo
        (Method::Post, "/api/archivia") => reply_json(req, 200, &crate::archivia(st("id"))),
        (Method::Post, "/api/riprendi-archiviata") => reply_json(req, 200, &crate::resume_archived_without_app(st("id"))),
        (Method::Post, "/api/togli-archivio") => reply_json(req, 200, &crate::togli_archivio(st("id"))),
        (Method::Post, "/api/allegato") => reply_json(req, 200, &crate::salva_allegato(st("nome"), st("dati"))),
        (Method::Post, "/api/vocale") => reply_json(req, 200, &crate::voice_notes::receive(&st("nome"), &st("dati"))),
        (Method::Get, "/api/vocale/esito") => reply_json(req, 200, &crate::voice_notes::outcome(&query_param(&url, "percorso").unwrap_or_default())),
        (Method::Get, "/api/file") => {
            let p = crate::documents::norm(&query_param(&url, "path").unwrap_or_default());
            let ext = p.rsplit('.').next().unwrap_or("").to_lowercase();
            // Windows: inside the user folder, in the temp folder or on a disk that is not the system one
            let win_ok = cfg!(windows) && crate::documents::is_absolute(&p) && {
                let l = p.to_lowercase();
                let casa = crate::documents::norm(&std::env::var("USERPROFILE").unwrap_or_default()).to_lowercase();
                let tmp = crate::documents::norm(&std::env::temp_dir().to_string_lossy()).to_lowercase();
                (!casa.is_empty() && l.starts_with(&format!("{casa}/"))) || l.starts_with(&tmp) || !l.starts_with("c:/")
            };
            let allowed = MEDIA.contains(&ext.as_str()) && !p.contains("..")
                && (win_ok || ["/Users/", "/private/tmp/", "/tmp/", "/Volumes/"].iter().any(|r| p.starts_with(r)));
            match (allowed, std::fs::File::open(&p)) {
                (true, Ok(f)) => {
                    let _ = req.respond(Response::from_file(f).with_header(h("Content-Type", file_mime(&p))).with_header(h("Cache-Control", "max-age=3600")));
                }
                _ => reply_json(req, 404, &json!({"errore": "file non disponibile"})),
            }
        }
        // documents in replies (documents.rs): only files that appeared in a card, never the secret folders
        (Method::Get, "/api/documento") => {
            let p = query_param(&url, "path").unwrap_or_default();
            if !crate::documents::is_allowed(&p) { return reply_json(req, 403, &json!({"ok": false, "msg": "Questo file non è tra quelli citati dalle sessioni."})); }
            reply_json(req, 200, &crate::documents::read_document(&p, crate::documents::IPHONE_LIMIT))
        }
        (Method::Get, "/api/documento/file") => {
            let p = query_param(&url, "path").unwrap_or_default();
            if !crate::documents::is_allowed(&p) { return reply_json(req, 403, &json!({"ok": false, "msg": "Questo file non è tra quelli citati dalle sessioni."})); }
            // anteprima=1: Quick Look image (Pages, Keynote, Excel, PDFs too large)
            let (file, kind) = if query_param(&url, "anteprima").as_deref() == Some("1") {
                (crate::documents::image_preview(&p).map(|x| x.to_string_lossy().to_string()), "image/png")
            } else { (Some(p.clone()), crate::documents::mime(&p)) };
            let Some(file) = file else { return reply_json(req, 404, &json!({"ok": false, "msg": "Anteprima non disponibile."})) };
            let peso = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
            if peso > crate::documents::MAX_RAW { return reply_json(req, 413, &json!({"ok": false, "msg": "File troppo grande per il telefono."})); }
            match std::fs::File::open(&file) {
                Ok(f) => { let _ = req.respond(Response::from_file(f).with_header(h("Content-Type", kind)).with_header(h("Cache-Control", "no-store"))); }
                Err(_) => reply_json(req, 404, &json!({"ok": false, "msg": "Il file non c'è più."})),
            }
        }
        (Method::Get, "/api/eventi") => events(req),
        _ => reply_json(req, 404, &json!({"errore": "non trovato"})),
    }
}

// ---------- ?leggero=1 (iPhone app): no base64 images inside the conversation ----------
// Embedded images (tool screenshots, pasted photos) become files in a disk cache, served by
// /api/file like the others; every media event also gets "misure" [[width, height]] aligned with "file", so
// the phone reserves the right space before downloading them. Without the parameter the response stays as always.

fn media_dir() -> std::path::PathBuf {
    let h = std::env::var("HOME").unwrap_or_default();
    // same folder as always: Library/Caches/<app identifier>/media
    let id = crate::APP.get().map(|a| a.config().identifier.clone()).unwrap_or_else(|| "juggl".into());
    std::path::Path::new(&h).join("Library/Caches").join(id).join("media")
}

/// 64-bit FNV-1a: stable name for the same image (copies are not duplicated).
fn fingerprint(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for x in b { h ^= *x as u64; h = h.wrapping_mul(0x100000001b3); }
    h
}

/// Width and height reading only the header (PNG, JPEG, GIF, WebP).
fn image_size(b: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| -> u32 { ((b[i] as u32) << 8) | b[i + 1] as u32 };
    let be32 = |i: usize| -> u32 { ((b[i] as u32) << 24) | ((b[i + 1] as u32) << 16) | ((b[i + 2] as u32) << 8) | b[i + 3] as u32 };
    let le16 = |i: usize| -> u32 { b[i] as u32 | ((b[i + 1] as u32) << 8) };
    if b.len() >= 24 && b.starts_with(&[0x89, b'P', b'N', b'G']) { return Some((be32(16), be32(20))); }
    if b.len() >= 10 && b.starts_with(b"GIF") { return Some((le16(6), le16(8))); }
    if b.len() >= 30 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
        return match &b[12..16] {
            b"VP8X" => Some((1 + (b[24] as u32 | (b[25] as u32) << 8 | (b[26] as u32) << 16), 1 + (b[27] as u32 | (b[28] as u32) << 8 | (b[29] as u32) << 16))),
            b"VP8 " => Some((le16(26) & 0x3fff, le16(28) & 0x3fff)),
            b"VP8L" => { let v = b[21] as u32 | (b[22] as u32) << 8 | (b[23] as u32) << 16 | (b[24] as u32) << 24; Some((1 + (v & 0x3fff), 1 + ((v >> 14) & 0x3fff))) }
            _ => None,
        };
    }
    if b.len() >= 4 && b[0] == 0xff && b[1] == 0xd8 {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xff { i += 1; continue; }
            let m = b[i + 1];
            if (0xc0..=0xcf).contains(&m) && m != 0xc4 && m != 0xc8 && m != 0xcc { return Some((be16(i + 7), be16(i + 5))); }
            if m == 0xd8 || m == 0x01 || (0xd0..=0xd7).contains(&m) { i += 2; continue; }
            i += 2 + be16(i + 2) as usize;
        }
    }
    None
}

fn image_size_file(p: &str) -> Value {
    let mut head = vec![0u8; 256 * 1024];
    let n = std::fs::File::open(p).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    head.truncate(n);
    match image_size(&head) { Some((w, h)) if w > 0 && h > 0 => json!([w, h]), _ => Value::Null }
}

/// Writes an embedded image to the cache and returns the path (None if the data is not a valid image).
fn save_media(uri: &str) -> Option<String> {
    use base64::Engine;
    let (head, b64) = uri.split_once(',')?;
    let ext = match head.trim_start_matches("data:").split(';').next().unwrap_or("") {
        "image/png" => "png", "image/jpeg" | "image/jpg" => "jpg", "image/gif" => "gif", "image/webp" => "webp", _ => return None,
    };
    let dir = media_dir();
    let p = dir.join(format!("{:016x}-{}.{ext}", fingerprint(b64.as_bytes()), b64.len()));
    if !p.exists() {
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?;
        let _ = std::fs::create_dir_all(&dir);
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, bytes).ok()?;
        std::fs::rename(&tmp, &p).ok()?;
        clean_media(&dir);
    }
    Some(p.to_string_lossy().to_string())
}

/// At most once an hour: removes cache files unused for 14 days.
fn clean_media(dir: &std::path::Path) {
    static LAST_RUN: Mutex<f64> = Mutex::new(0.0);
    { let mut u = LAST_RUN.lock().unwrap(); if unix_now() - *u < 3600.0 { return; } *u = unix_now(); }
    let cutoff = std::time::SystemTime::now() - Duration::from_secs(14 * 86400);
    if let Ok(l) = std::fs::read_dir(dir) {
        for e in l.flatten() {
            if e.metadata().and_then(|m| m.modified()).map(|t| t < cutoff).unwrap_or(false) { let _ = std::fs::remove_file(e.path()); }
        }
    }
}

/// Keeps only the last `n` events of the conversation.
fn keep_last(v: &mut Value, n: usize) {
    if let Some(e) = v.get_mut("eventi").and_then(|x| x.as_array_mut()) {
        if e.len() > n { let drop_n = e.len() - n; e.drain(..drop_n); }
    }
}

/// iPhone only (?leggero=1): drops what the app does not use. "richieste_ai" it does not read; of the sub-agents
/// (even 90 in long sessions, ~20 KB) only those at work and those cited in the sent events are needed.
fn slim(v: &mut Value) {
    let Some(o) = v.as_object_mut() else { return };
    o.remove("richieste_ai");
    let cited: std::collections::HashSet<String> = o.get("eventi").and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|e| e.get("id").and_then(|x| x.as_str()).map(String::from)).collect()).unwrap_or_default();
    if let Some(Value::Array(subs)) = o.get_mut("sotto") {
        subs.retain(|a| a.get("stato").and_then(|x| x.as_str()) == Some("lavora")
            || a.get("tool").and_then(|x| x.as_str()).map(|t| cited.contains(t)).unwrap_or(false));
    }
}

fn lighten(v: &mut Value) {
    let Some(events) = v.get_mut("eventi").and_then(|x| x.as_array_mut()) else { return };
    for e in events.iter_mut() {
        if e.get("tipo").and_then(|x| x.as_str()) != Some("media") { continue; }
        let data: Vec<String> = e.get("dati").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let mut file: Vec<String> = e.get("file").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let mut remaining = vec![];
        for d in data { match save_media(&d) { Some(p) => file.push(p), None => remaining.push(d) } }
        let dims: Vec<Value> = file.iter().map(|p| image_size_file(p)).collect();
        e["file"] = json!(file);
        e["dati"] = json!(remaining);
        e["misure"] = json!(dims);
    }
}

/// Dimensions of an image file, remembered (the same image appears at every chat reload).
fn remembered_size(p: &str) -> Value {
    if crate::fast_open::is_network(p) { return Value::Null; }   // fast chats (15/11): no reads on network disks when opening a chat
    static M: OnceLock<Mutex<std::collections::HashMap<String, Value>>> = OnceLock::new();
    let m = M.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(v) = m.lock().unwrap().get(p) { if !v.is_null() { return v.clone(); } }
    let v = image_size_file(p);
    let mut g = m.lock().unwrap();
    if g.len() > 4000 { g.clear(); }
    g.insert(p.to_string(), v.clone());
    v
}

/// Embedded image already written to the cache: no fingerprint to recompute at every reload.
fn remembered_media(uri: &str) -> Option<String> {
    static M: OnceLock<Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
    let m = M.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let n = uri.len();
    let chiave = format!("{n}:{}:{}", uri.get(..n.min(96)).unwrap_or(""), uri.get(n.saturating_sub(96)..).unwrap_or(""));
    if let Some(p) = m.lock().unwrap().get(&chiave) { if std::path::Path::new(p).exists() { return Some(p.clone()); } }
    let p = save_media(uri)?;
    let mut g = m.lock().unwrap();
    if g.len() > 2000 { g.clear(); }
    g.insert(chiave, p.clone());
    Some(p)
}

/// Mac app: as for the iPhone embedded images (base64) become cache files, and every image carries its dimensions.
/// A chat with a few screenshots weighed megabytes at every reload (dozens of times while the session works, and every time
/// the interface stopped to read them); now a few KB, and the interface reserves the right space before the images arrive.
/// "copia": the event was only the embedded copy (usually of a file just read), the interface does not repeat it.
/// "misure_allegati": the dimensions of images attached to your messages ("Allegato: /path").
pub(crate) fn lighten_for_mac(v: &mut Value) {
    let Some(events) = v.get_mut("eventi").and_then(|x| x.as_array_mut()) else { return };
    let string_list = |e: &Value, k: &str| -> Vec<String> { e.get(k).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() };
    for e in events.iter_mut() {
        let kind = e.get("tipo").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let mut file = string_list(e, "file");
        if kind == "media" {
            let data = string_list(e, "dati");
            if !data.is_empty() {
                let already = file.len();
                let mut remaining = vec![];
                // as the interface did: embedded copies of files already in the event are not shown twice
                for d in data.into_iter().skip(already) { match remembered_media(&d) { Some(p) => file.push(p), None => remaining.push(d) } }
                if already == 0 && !file.is_empty() { e["copia"] = json!(true); }
                e["file"] = json!(file);
                e["dati"] = json!(remaining);
            }
        }
        if !file.is_empty() { e["misure"] = json!(file.iter().map(|p| remembered_size(p)).collect::<Vec<_>>()); }
        if kind == "tu" {
            let t = e.get("testo").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let mut m = serde_json::Map::new();
            for parte in t.split("Allegato: ").skip(1) {
                let p = parte.split_whitespace().next().unwrap_or("");
                let l = p.to_lowercase();
                if p.starts_with('/') && [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|x| l.ends_with(x)) {
                    let v = remembered_size(p); if !v.is_null() { m.insert(p.to_string(), v); }
                }
            }
            if !m.is_empty() { e["misure_allegati"] = Value::Object(m); }
        }
    }
}

#[cfg(test)]
mod mac_tests {
    use serde_json::json;
    #[test]
    fn lighten_mac_file_sizes_copy() {
        // real 3x2 PNG (header + minimal IDAT)
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAMAAAACCAIAAAASFvFNAAAAEklEQVR4nGP4z8DAwMDAwMAAAB4AA/1W2r5HAAAAAElFTkSuQmCC";
        let mut v = json!({"eventi": [
            {"tipo": "media", "da": "tu", "dati": [png], "file": []},
            {"tipo": "tu", "testo": "guarda Allegato: /non/esiste.png"},
            {"tipo": "agente", "testo": "ok", "file": []}
        ]});
        super::lighten_for_mac(&mut v);
        let e = &v["eventi"][0];
        assert_eq!(e["dati"].as_array().unwrap().len(), 0);
        assert!(e["file"][0].as_str().unwrap().ends_with(".png"));
        assert_eq!(e["misure"][0], json!([3, 2]));
        assert_eq!(e["copia"], json!(true));
        assert!(v["eventi"][1].get("misure_allegati").is_none());
        assert!(v["eventi"][2].get("misure").is_none());
    }
}

#[cfg(test)]
mod light_tests {
    #[test]
    fn sizes_png_jpeg() {
        let png = [0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 3, 0x20, 0, 0, 2, 0x58];
        assert_eq!(super::image_size(&png), Some((800, 600)));
        let jpg = [0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 17, 8, 1, 0x2c, 0x01, 0x90, 3, 0, 0];
        assert_eq!(super::image_size(&jpg), Some((400, 300)));
    }

    #[test]
    fn last_and_slim() {
        let mut v = serde_json::json!({
            "eventi": [{"tipo": "tu", "id": "a"}, {"tipo": "strumento", "nome": "Agent", "id": "t1"}, {"tipo": "agente"}],
            "richieste_ai": [1, 2],
            "sotto": [{"tool": "t1", "stato": "finito"}, {"tool": "t9", "stato": "finito"}, {"tool": "t8", "stato": "lavora"}],
            "nome": "x"
        });
        super::keep_last(&mut v, 2);
        assert_eq!(v["eventi"].as_array().unwrap().len(), 2);
        super::slim(&mut v);
        assert!(v.get("richieste_ai").is_none());
        let t: Vec<&str> = v["sotto"].as_array().unwrap().iter().map(|a| a["tool"].as_str().unwrap()).collect();
        assert_eq!(t, vec!["t1", "t8"]);
        assert_eq!(v["nome"], "x");
    }
}

#[cfg(test)]
mod real_light_test {
    /// cargo test real_light_test -- --ignored --nocapture, with SESSIONE_JSON=<saved response of /api/sessione>
    #[test]
    #[ignore]
    fn lightens_a_session() {
        let f = std::env::var("SESSIONE_JSON").expect("SESSIONE_JSON");
        let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        let prima = v.to_string().len();
        let t = std::time::Instant::now();
        super::lighten(&mut v);
        let ms = t.elapsed().as_millis();
        let dopo = v.to_string().len();
        let t2 = std::time::Instant::now();
        super::lighten(&mut v);
        println!("prima {prima} B, dopo {dopo} B, {ms} ms la prima volta, {} ms con la cache", t2.elapsed().as_millis());
        let m: Vec<_> = v["eventi"].as_array().unwrap().iter().filter(|e| e["tipo"] == "media").take(3).map(|e| (e["file"].clone(), e["misure"].clone())).collect();
        println!("{m:?}");
    }
}
