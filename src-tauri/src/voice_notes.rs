// Voice notes from the iPhone (and the Mac): the audio arrives as base64 on /api/vocale, is saved in ~/tavolo/allegati and
// transcribed with the studio PC's Whisper through ~/.local/bin/trascrivi (writes <name>.txt next to the file,
// paragraphs "[mm:ss] text"; exit 75 = PC busy). If the PC does not answer within the wait, the phone keeps its
// own transcription: here we answer source "nessuna" and the transcription goes on by itself, the .txt arrives later.
use crate::{base_dir, load_json, unix_now};
use serde_json::{json, Value};
use std::process::{Stdio};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

/// Seconds to wait for the PC's Whisper (can be overridden with "vocale_attesa_s" in ~/tavolo/impostazioni.json).
const WAIT_S: u64 = 25;
/// Past this time the transcription left in the background is stopped (hung ssh, PC turned off halfway).
const MAX_S: u64 = 600;

fn wait_secs() -> u64 {
    load_json(&base_dir().join("impostazioni.json")).and_then(|v| v.get("vocale_attesa_s").and_then(|x| x.as_u64())).unwrap_or(WAIT_S).clamp(5, 120)
}

/// Any audio (iPhone m4a) to 16 kHz mono WAV with macOS afconvert, in a private temporary folder.
fn to_wav_16k(p: &std::path::Path) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("juggl-vocale-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)); }
    let out = dir.join(format!("{}.wav", (unix_now() * 1000.0) as u64));
    let ok = crate::new_command("/usr/bin/afconvert").args(["-f", "WAVE", "-d", "LEI16@16000", "-c", "1"]).arg(p).arg(&out)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    let r = if ok { std::fs::read(&out).ok() } else { None };
    let _ = std::fs::remove_file(&out);
    r
}

/// From trascrivi's .txt ("[00:03] sentence\n\n[00:12] another") to clean text, paragraphs separated by an empty line.
fn clean_transcript(txt: &str) -> String {
    txt.split("\n\n").map(|p| {
        let p = p.trim();
        match (p.starts_with('['), p.find("] ")) { (true, Some(i)) if i < 12 => p[i + 2..].trim().to_string(), _ => p.to_string() }
    }).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n\n")
}

pub fn receive(name: &str, data: &str) -> Value {
    use base64::Engine;
    let b64 = data.split(',').last().unwrap_or("").trim();
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else { return json!({"ok": false, "msg": "audio non valido", "fonte": "nessuna"}) };
    if bytes.len() < 200 { return json!({"ok": false, "msg": "audio vuoto", "fonte": "nessuna"}) }
    let dir = base_dir().join("allegati");
    let _ = std::fs::create_dir_all(&dir);
    let ext = name.rsplit('.').next().map(|e| e.to_lowercase()).filter(|e| ["m4a", "aac", "wav", "caf", "mp3", "ogg", "opus", "webm", "mp4"].contains(&e.as_str())).unwrap_or("m4a".into());
    let p = dir.join(format!("vocale-{}.{ext}", (unix_now() * 1000.0) as u64));
    if let Err(e) = std::fs::write(&p, &bytes) { return json!({"ok": false, "msg": e.to_string(), "fonte": "nessuna"}) }
    let path_str = p.to_string_lossy().to_string();
    let txt = p.with_extension("txt");
    // the Mac's Whisper first (a few seconds even for long notes, with voice activity detection): the PC took
    // ~1 minute for 3 minutes of audio, beyond the wait, and the phone kept its broken transcription (Nov 10)
    if crate::microphone::model_ready() {
        let t0 = Instant::now();
        if let Some(wav) = to_wav_16k(&p) {
            let audio_secs = wav.len().saturating_sub(44) as f64 / 32_000.0;
            if let Ok(text) = crate::microphone::transcribe_mac(&wav, audio_secs) {
                let _ = std::fs::write(&txt, &text);
                let source = if text.trim().is_empty() { "nessuna" } else { "whisper" };
                return json!({"ok": true, "testo": text, "fonte": source, "percorso": path_str, "secondi": (t0.elapsed().as_millis() as f64 / 100.0).round() / 10.0, "motore": "mac"});
            }
        }
    }

    let tool = crate::profile::tool("trascrivi").unwrap_or_default();
    let t0 = Instant::now();
    let child = crate::new_command(&tool).arg(&p).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        // the app launched from Finder has a minimal PATH: ssh and scp live in /usr/bin, but better be explicit
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin")
        .spawn();
    let Ok(mut child) = child else { return json!({"ok": true, "testo": "", "fonte": "nessuna", "percorso": path_str, "msg": "trascrivi non trovato"}) };
    let (tx, rx) = channel::<i32>();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(MAX_S);
        loop {
            match child.try_wait() {
                Ok(Some(st)) => { let _ = tx.send(st.code().unwrap_or(-1)); return; }
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(150)),
                _ => { let _ = child.kill(); let _ = child.wait(); let _ = tx.send(-1); return; }
            }
        }
    });
    let elapsed_s = |t: Instant| (t.elapsed().as_millis() as f64 / 100.0).round() / 10.0;
    match rx.recv_timeout(Duration::from_secs(wait_secs())) {
        Ok(0) => {
            let text = std::fs::read_to_string(&txt).map(|t| clean_transcript(&t)).unwrap_or_default();
            let source = if text.is_empty() { "nessuna" } else { "whisper" };
            json!({"ok": true, "testo": text, "fonte": source, "percorso": path_str, "secondi": elapsed_s(t0)})
        }
        Ok(75) => json!({"ok": true, "testo": "", "fonte": "nessuna", "percorso": path_str, "secondi": elapsed_s(t0), "msg": "PC occupato"}),
        Ok(c) => json!({"ok": true, "testo": "", "fonte": "nessuna", "percorso": path_str, "secondi": elapsed_s(t0), "msg": format!("trascrizione non riuscita ({c})")}),
        // the transcription goes on: the phone can ask for the text later with /api/vocale/esito
        Err(_) => json!({"ok": true, "testo": "", "fonte": "nessuna", "percorso": path_str, "secondi": elapsed_s(t0), "in_corso": true, "msg": "il PC non ha risposto in tempo"}),
    }
}

/// Transcribes an audio file already on disk (voice notes from external messages): waits at most `max`,
/// None if the PC is busy or does not answer. The .txt stays next to the file, so a second call is immediate.
pub(crate) fn transcribe_file(p: &std::path::Path, max: Duration) -> Option<String> {
    let txt = p.with_extension("txt");
    if let Ok(t) = std::fs::read_to_string(&txt) { return Some(clean_transcript(&t)); }
    let tool = crate::profile::tool("trascrivi")?;
    let mut child = crate::new_command(&tool).arg(p).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin").spawn().ok()?;
    let deadline = Instant::now() + max;
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return std::fs::read_to_string(&txt).ok().map(|t| clean_transcript(&t)),
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(300)),
            _ => { let _ = child.kill(); let _ = child.wait(); return None; }
        }
    }
}

/// Late text: present if trascrivi finished after the wait. Only voice notes saved in ~/tavolo/allegati.
pub fn outcome(path_str: &str) -> Value {
    let p = std::path::Path::new(path_str);
    let inside = p.parent() == Some(base_dir().join("allegati").as_path()) && p.file_name().map(|n| n.to_string_lossy().starts_with("vocale-")).unwrap_or(false);
    if !inside || path_str.contains("..") { return json!({"ok": false, "msg": "percorso non ammesso"}) }
    match std::fs::read_to_string(p.with_extension("txt")) {
        Ok(t) => { let t = clean_transcript(&t); json!({"ok": true, "pronto": true, "testo": t, "fonte": if t.is_empty() { "nessuna" } else { "whisper" }}) }
        Err(_) => json!({"ok": true, "pronto": false, "testo": "", "fonte": "nessuna"}),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn strips_timestamps() {
        assert_eq!(super::clean_transcript("[00:00] Ciao a tutti.\n\n[01:02:03] Seconda parte\n"), "Ciao a tutti.\n\nSeconda parte");
    }
}

#[cfg(test)]
mod real_transcription {
    /// cargo test real_transcription -- --ignored --nocapture, with VOCALE_PROVA=<m4a file>
    #[test]
    #[ignore]
    fn transcribes_a_file() {
        use base64::Engine;
        let f = std::env::var("VOCALE_PROVA").expect("VOCALE_PROVA");
        let b = base64::engine::general_purpose::STANDARD.encode(std::fs::read(f).unwrap());
        let r = super::receive("prova.m4a", &b);
        println!("{r}");
        let p = r["percorso"].as_str().unwrap().to_string();
        println!("{}", super::outcome(&p));
    }
}
