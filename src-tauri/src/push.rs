// Real push notifications (APNs) to the iPhone app, even when the app is closed.
// The app sends its device token (POST /api/iphone/token): it is saved in ~/tavolo/iphone-push.json.
// Direct delivery to Apple: ES256 JWT signed with the team key (~/.config/juggl/apns, see apns.env), HTTP/2
// to api.push.apple.com (TestFlight, App Store) or api.sandbox.push.apple.com (direct installs).
// No new dependencies: signing is done by `openssl`, HTTP/2 by `curl`, both present on macOS.
use crate::{base_dir, load_json, unix_now, str_of, write_state};
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Stdio};
use std::sync::Mutex;

fn token_file() -> std::path::PathBuf { base_dir().join("iphone-push.json") }

fn config() -> Option<(String, String, String, String)> {
    let h = std::env::var("HOME").ok()?;
    let text = std::fs::read_to_string(format!("{h}/.config/juggl/apns/apns.env")).ok()?;
    let v = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}=")).map(|x| x.trim().to_string()));
    Some((v("APNS_KEY_ID")?, v("APNS_TEAM_ID")?, v("APNS_TOPIC")?, v("APNS_KEY_PATH")?))
}

/// POST /api/iphone/token {token, ambiente: "sandbox"|"produzione", bundle}
pub fn register_token(b: &Value) -> Value {
    let token = str_of(b, "token").trim().to_lowercase();
    if token.len() < 32 || !token.chars().all(|c| c.is_ascii_hexdigit()) { return json!({"ok": false, "msg": "token non valido"}) }
    let environment = if str_of(b, "ambiente") == "sandbox" { "sandbox" } else { "produzione" };
    write_state(&token_file(), &json!({"token": token, "ambiente": environment, "bundle": str_of(b, "bundle"), "t": unix_now()}));
    json!({"ok": true, "push": config().is_some()})
}

pub fn is_available() -> bool { apns_available() || android::is_available() }
fn apns_available() -> bool { config().is_some() && load_json(&token_file()).map(|v| !str_of(&v, "token").is_empty()).unwrap_or(false) }

fn b64url(d: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(d)
}

/// openssl DER signature -> "raw" r||s signature (64 bytes) required by ES256.
fn der_to_raw(der: &[u8]) -> Option<Vec<u8>> {
    if der.len() < 8 || der[0] != 0x30 { return None; }
    let mut i = 2;
    if der[1] & 0x80 != 0 { i = 2 + (der[1] & 0x7f) as usize; }
    let mut read_part = |i: &mut usize| -> Option<Vec<u8>> {
        if *i >= der.len() || der[*i] != 0x02 { return None; }
        let n = der[*i + 1] as usize;
        let mut v = der.get(*i + 2..*i + 2 + n)?.to_vec();
        *i += 2 + n;
        while v.len() > 32 && v[0] == 0 { v.remove(0); }
        if v.len() > 32 { return None; }
        let mut p = vec![0u8; 32 - v.len()]; p.extend(v); Some(p)
    };
    let mut r = read_part(&mut i)?;
    let sv = read_part(&mut i)?;
    r.extend(sv);
    Some(r)
}

/// JWT for APNs: valid up to 60 minutes, Apple asks not to renew it more than once every 20. I keep it 45 minutes.
fn jwt(key_id: &str, team: &str, key: &str) -> Result<String, String> {
    static CACHE: Mutex<(f64, String)> = Mutex::new((0.0, String::new()));
    if let Ok(c) = CACHE.lock() { if unix_now() - c.0 < 45.0 * 60.0 && !c.1.is_empty() { return Ok(c.1.clone()); } }
    let header = b64url(json!({"alg": "ES256", "kid": key_id}).to_string().as_bytes());
    let body = b64url(json!({"iss": team, "iat": unix_now() as u64}).to_string().as_bytes());
    let to_sign = format!("{header}.{body}");
    let mut p = crate::new_command("/usr/bin/openssl").args(["dgst", "-sha256", "-sign", key])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    p.stdin.take().ok_or("stdin")?.write_all(to_sign.as_bytes()).map_err(|e| e.to_string())?;
    let out = p.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(format!("openssl: {}", String::from_utf8_lossy(&out.stderr).trim())); }
    let sig = der_to_raw(&out.stdout).ok_or("firma non valida")?;
    let t = format!("{to_sign}.{}", b64url(&sig));
    if let Ok(mut c) = CACHE.lock() { *c = (unix_now(), t.clone()); }
    Ok(t)
}

/// Juggl elements (```juggl-stato, -scelta, ...) in the notice become one line: title or question, not the block.
pub fn without_blocks(t: &str) -> String {
    let mut out: Vec<String> = vec![];
    let mut inside: Option<String> = None;
    let mut taken = false;
    for r in t.lines() {
        let x = r.trim();
        if let Some(l) = x.strip_prefix("```") {
            if inside.is_some() { inside = None; continue; }
            if let Some(kind) = l.trim().strip_prefix("juggl-") { inside = Some(kind.to_string()); taken = false; continue; }
            continue;
        }
        if inside.is_some() {
            if !taken { if let Some(v) = x.strip_prefix("titolo:").or_else(|| x.strip_prefix("domanda:")) { out.push(v.trim().to_string()); taken = true; } }
            continue;
        }
        out.push(r.to_string());
    }
    out.join("\n").trim().to_string()
}

#[cfg(test)]
#[test]
fn notice_without_blocks() {
    assert_eq!(without_blocks("Fatto.\n```juggl-stato\ntitolo: Build\n- [x] a\n```\nCiao"), "Fatto.\nBuild\nCiao");
}

/// Sends a notification. `sessione` and `progetto` go in the payload: a tap opens that chat (like local notifications).
/// Returns Ok if Apple accepted it (HTTP 200).
/// Sends to all registered phones: iPhone (APNs) and Android (FCM). Ok if at least one received it.
pub fn send(title: &str, text: &str, session: &str, project: &str) -> Result<(), String> {
    if crate::settings_chat::is_quiet() { return Err("avvisi di notte: in silenzio".into()); }   // chat-style settings (Nov 16)
    let mut errors = vec![];
    let mut ok = false;
    if apns_available() { match send_iphone(title, text, session, project) { Ok(()) => ok = true, Err(e) => errors.push(format!("iPhone: {e}")) } }
    if android::is_available() { match android::send(title, text, session, project) { Ok(()) => ok = true, Err(e) => errors.push(format!("Android: {e}")) } }
    if ok { return Ok(()); }
    if errors.is_empty() { return Err("nessun telefono registrato per le notifiche".into()); }
    Err(errors.join("; "))
}

fn send_iphone(title: &str, text: &str, session: &str, project: &str) -> Result<(), String> {
    let (key_id, team, topic, key) = config().ok_or("chiave APNs mancante (~/.config/juggl/apns/apns.env)")?;
    let reg = load_json(&token_file()).ok_or("l'iPhone non ha ancora mandato il token")?;
    let token = str_of(&reg, "token").to_string();
    if token.is_empty() { return Err("token vuoto".into()); }
    let host = if str_of(&reg, "ambiente") == "sandbox" { "api.sandbox.push.apple.com" } else { "api.push.apple.com" };
    let thread = if session.is_empty() { if project.is_empty() { "juggl".to_string() } else { project.to_string() } } else { session.to_string() };
    let body: String = without_blocks(text).chars().take(240).collect();
    let payload = json!({
        "aps": {"alert": {"title": title, "body": body}, "sound": "default", "thread-id": thread},
        "sessione": session, "progetto": project,
    });
    let t = jwt(&key_id, &team, &key)?;
    let mut p = crate::new_command("/usr/bin/curl")
        .args(["-s", "--http2", "--max-time", "10", "-o", "-", "-w", "\n%{http_code}",
               "-H", &format!("authorization: bearer {t}"), "-H", &format!("apns-topic: {topic}"),
               "-H", "apns-push-type: alert", "-H", "apns-priority: 10", "--data-binary", "@-",
               &format!("https://{host}/3/device/{token}")])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    p.stdin.take().ok_or("stdin")?.write_all(payload.to_string().as_bytes()).map_err(|e| e.to_string())?;
    let out = p.wait_with_output().map_err(|e| e.to_string())?;
    let response = String::from_utf8_lossy(&out.stdout).to_string();
    let (response_text, code) = response.rsplit_once('\n').unwrap_or(("", response.as_str()));
    match code.trim() {
        "200" => Ok(()),
        "410" | "400" if response_text.contains("Unregistered") || response_text.contains("BadDeviceToken") => {
            // expired token or one from the wrong environment: remove it, the app sends a new one at next launch
            let _ = std::fs::remove_file(token_file());
            Err(format!("token non piu' valido ({}): {}", code.trim(), response_text.trim()))
        }
        c => Err(format!("APNs HTTP {c}: {}", response_text.trim())),
    }
}

/// live activity (Nov 12): the APNs key is there (the activity token is separate, src/live.rs).
pub fn config_available() -> bool { config().is_some() }

/// live activity (Nov 12): sends a "liveactivity" update to the activity token. The environment (sandbox or
/// production) is the app's one, registered with the notifications token.
pub fn send_live(token: &str, payload: &Value, priority: u8, environment: &str) -> Result<(), String> {
    let (key_id, team, topic, key) = config().ok_or("chiave APNs mancante")?;
    // environment sent by the iPhone with the activity token (direct builds and simulator = sandbox); if missing, the notifications one
    let sandbox = match environment { "sandbox" => true, "production" => false, _ => load_json(&token_file()).map(|r| str_of(&r, "ambiente") == "sandbox").unwrap_or(false) };
    let host = if sandbox { "api.sandbox.push.apple.com" } else { "api.push.apple.com" };
    let t = jwt(&key_id, &team, &key)?;
    let mut p = crate::new_command("/usr/bin/curl")
        .args(["-s", "--http2", "--max-time", "10", "-o", "-", "-w", "\n%{http_code}",
               "-H", &format!("authorization: bearer {t}"), "-H", &format!("apns-topic: {topic}.push-type.liveactivity"),
               "-H", "apns-push-type: liveactivity", "-H", &format!("apns-priority: {priority}"), "--data-binary", "@-",
               &format!("https://{host}/3/device/{token}")])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    p.stdin.take().ok_or("stdin")?.write_all(payload.to_string().as_bytes()).map_err(|e| e.to_string())?;
    let out = p.wait_with_output().map_err(|e| e.to_string())?;
    let response = String::from_utf8_lossy(&out.stdout).to_string();
    let (response_text, code) = response.rsplit_once('\n').unwrap_or(("", response.as_str()));
    match code.trim() {
        "200" => Ok(()),
        "410" | "400" if response_text.contains("Unregistered") || response_text.contains("BadDeviceToken") || response_text.contains("ExpiredToken") =>
            Err(format!("token non piu' valido ({}): {}", code.trim(), response_text.trim())),
        c => Err(format!("APNs HTTP {c}: {}", response_text.trim())),
    }
}

/// Should the push be sent? Only if the user is not at the Mac and the iPhone app is not open and connected (everything arrives there already).
pub fn is_needed() -> bool { is_available() && !crate::user_at_mac() && !crate::messages::iphone_connected() }

/// Like `manda`, in a thread: for places that must not wait for Apple. Writes errors to Juggl's log.
pub fn send_in_background(title: String, text: String, session: String, project: String) {
    std::thread::spawn(move || {
        if let Err(e) = send(&title, &text, &session, &project) { crate::log_juggl(&format!("push non partita: {e}")); }
    });
}

/// POST /api/iphone/push-prova {testo?}: test notification (to check key, token and environment).
pub fn test_push(b: &Value) -> Value {
    let test_text = format!("Notifica di prova da Juggl sul {}", crate::session_care::computer_name());
    let text = if str_of(b, "testo").is_empty() { test_text.as_str() } else { str_of(b, "testo") };
    match send("Juggl", text, "", "generale") {
        Ok(()) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "msg": e}),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn der_signature_to_raw() {
        // r with leading zero (33 bytes), s short (31 bytes)
        let mut der = vec![0x30, 0x44, 0x02, 0x21, 0x00];
        der.extend([0xAA; 32]);
        der.extend([0x02, 0x1F]);
        der.extend([0xBB; 31]);
        let g = super::der_to_raw(&der).unwrap();
        assert_eq!(g.len(), 64);
        assert_eq!(g[0], 0xAA);
        assert_eq!(g[32], 0x00);
        assert_eq!(g[33], 0xBB);
    }
}

#[cfg(test)]
mod apple_test {
    /// cargo test apple_test -- --ignored --nocapture: real JWT to Apple with a fake token.
    /// Expected "BadDeviceToken" (authentication accepted), not "InvalidProviderToken".
    #[test]
    #[ignore]
    fn jwt_accepted_by_apple() {
        let (k, t, topic, key) = super::config().expect("apns.env");
        let jwt = super::jwt(&k, &t, &key).expect("jwt");
        for host in ["api.sandbox.push.apple.com", "api.push.apple.com"] {
            let out = crate::new_command("/usr/bin/curl").args(["-s", "--http2", "--max-time", "10", "-w", " HTTP %{http_code}",
                "-H", &format!("authorization: bearer {jwt}"), "-H", &format!("apns-topic: {topic}"), "-H", "apns-push-type: alert",
                "-d", r#"{"aps":{"alert":"prova"}}"#, &format!("https://{host}/3/device/{}", "0".repeat(64))]).output().unwrap();
            println!("{host}: {}", String::from_utf8_lossy(&out.stdout));
        }
    }
}


// ---------- Android: Firebase Cloud Messaging (HTTP v1) ----------
// The Android app sends its FCM token (POST /api/android/token): it is saved in ~/tavolo/android-push.json.
// Delivery with the service account of the Firebase project "Juggl" (~/.config/juggl/fcm/service-account.json, 600, only on
// this computer): OAuth token from an RS256 JWT signed here with ring (no openssl: works on Windows too).
pub mod android {
    use crate::{base_dir, load_json, unix_now, str_of, write_state};
    use serde_json::{json, Value};
    use std::sync::Mutex;

    fn token_file() -> std::path::PathBuf { base_dir().join("android-push.json") }
    fn key() -> Option<Value> {
        let h = std::env::var("HOME").ok()?;
        serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(&h).join(".config/juggl/fcm/service-account.json")).ok()?).ok()
    }
    pub fn is_available() -> bool { key().is_some() && load_json(&token_file()).map(|v| !str_of(&v, "token").is_empty()).unwrap_or(false) }

    /// POST /api/android/token {token}
    pub fn register_token(b: &Value) -> Value {
        let token = str_of(b, "token").trim().to_string();
        if token.len() < 20 || token.len() > 4096 || !token.chars().all(|c| c.is_ascii_alphanumeric() || "-_:".contains(c)) {
            return json!({"ok": false, "msg": "token non valido"});
        }
        write_state(&token_file(), &json!({"token": token, "t": unix_now()}));
        json!({"ok": true, "push": key().is_some()})
    }

    fn b64url(d: &[u8]) -> String { use base64::Engine; base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(d) }

    /// OAuth token for FCM (valid one hour, I keep it 50 minutes).
    fn oauth_token(sa: &Value) -> Result<String, String> {
        static CACHE: Mutex<(f64, String)> = Mutex::new((0.0, String::new()));
        if let Ok(c) = CACHE.lock() { if unix_now() - c.0 < 50.0 * 60.0 && !c.1.is_empty() { return Ok(c.1.clone()); } }
        use base64::Engine;
        let pem = str_of(sa, "private_key");
        let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = base64::engine::general_purpose::STANDARD.decode(body.trim()).map_err(|_| "chiave del service account illeggibile")?;
        let kp = ring::signature::RsaKeyPair::from_pkcs8(&der).map_err(|_| "chiave del service account non valida")?;
        let now = unix_now() as u64;
        let header = b64url(br#"{"alg":"RS256","typ":"JWT"}"#);
        let data = b64url(json!({"iss": str_of(sa, "client_email"), "scope": "https://www.googleapis.com/auth/firebase.messaging",
            "aud": str_of(sa, "token_uri"), "iat": now, "exp": now + 3600}).to_string().as_bytes());
        let to_sign = format!("{header}.{data}");
        let mut sig = vec![0u8; kp.public().modulus_len()];
        kp.sign(&ring::signature::RSA_PKCS1_SHA256, &ring::rand::SystemRandom::new(), to_sign.as_bytes(), &mut sig).map_err(|_| "firma non riuscita")?;
        let jwt = format!("{to_sign}.{}", b64url(&sig));
        let body = format!("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={jwt}");
        let (code, v) = curl_post(str_of(sa, "token_uri"), "application/x-www-form-urlencoded", None, &body)?;
        let t = v["access_token"].as_str().ok_or(format!("token OAuth non ottenuto ({code})"))?.to_string();
        if let Ok(mut c) = CACHE.lock() { *c = (unix_now(), t.clone()); }
        Ok(t)
    }

    /// POST with the system curl: headers and body on stdin (never the token on the command line).
    fn curl_post(url: &str, kind: &str, bearer: Option<&str>, body: &str) -> Result<(u16, Value), String> {
        use std::io::Write;
        let mut conf = format!("url = \"{url}\"\nrequest = \"POST\"\nheader = \"Content-Type: {kind}\"\n");
        if let Some(b) = bearer { conf.push_str(&format!("header = \"Authorization: Bearer {b}\"\n")); }
        conf.push_str("data-binary = @-\n");
        // the configuration goes in a private temp file: stdin is needed for the body
        let dir = std::env::temp_dir().join(format!("juggl-fcm-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("conf");
        std::fs::write(&f, conf).map_err(|e| e.to_string())?;
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)); }
        let mut c = crate::license::curl_command()
            .args(["-sS", "--max-time", "15", "-w", "\n%{http_code}", "-K"]).arg(&f)
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
            .spawn().map_err(|e| { let _ = std::fs::remove_dir_all(&dir); e.to_string() })?;
        if let Some(mut i) = c.stdin.take() { let _ = i.write_all(body.as_bytes()); }
        let o = c.wait_with_output();
        let _ = std::fs::remove_dir_all(&dir);
        let o = o.map_err(|e| e.to_string())?;
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        let (text, code) = out.rsplit_once('\n').unwrap_or(("", out.as_str()));
        Ok((code.trim().parse().unwrap_or(0), serde_json::from_str(text.trim()).unwrap_or(json!({}))))
    }

    pub fn send(title: &str, text: &str, session: &str, project: &str) -> Result<(), String> {
        let sa = key().ok_or("chiave FCM mancante (~/.config/juggl/fcm/service-account.json)")?;
        let reg = load_json(&token_file()).ok_or("il telefono Android non ha ancora mandato il token")?;
        let token = str_of(&reg, "token").to_string();
        let body: String = super::without_blocks(text).chars().take(240).collect();
        let msg = json!({"message": {"token": token,
            "notification": {"title": title, "body": body},
            "data": {"sessione": session, "progetto": project},
            "android": {"priority": "high", "notification": {"channel_id": "juggl", "tag": if session.is_empty() { "juggl" } else { session }}}}});
        let oauth = oauth_token(&sa)?;
        let url = format!("https://fcm.googleapis.com/v1/projects/{}/messages:send", str_of(&sa, "project_id"));
        let (code, v) = curl_post(&url, "application/json", Some(&oauth), &msg.to_string())?;
        match code {
            200 => Ok(()),
            404 | 400 if v.to_string().contains("UNREGISTERED") || v.to_string().contains("not a valid FCM registration token") => {
                let _ = std::fs::remove_file(token_file());
                Err(format!("token Android non piu' valido ({code})"))
            }
            c => Err(format!("FCM HTTP {c}: {}", v["error"]["message"].as_str().unwrap_or(""))),
        }
    }
}
