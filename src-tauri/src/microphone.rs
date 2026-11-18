// Microphone in the input box: records from the Mac (CoreAudio via cpal), transcribes with Whisper and puts the text in the
// input box, editable. It never sends anything by itself.
//
// - Recording: the audio stream lives in its own thread; samples are immediately reduced to 16 kHz mono i16
//   (32 KB per second, 15 minutes = 29 MB) and stay in memory. Levels at 20 Hz with the "mic-livello" event.
// - Transcription on the Mac: whisper-cli (whisper.cpp with Metal, static, in Juggl.app/Contents/MacOS) with the
//   large-v3-turbo q5_0 model (548 MB) in ~/Library/Application Support/Juggl/whisper. Audio goes through stdin: on the Mac
//   it never touches the disk. The process starts for each voice note and exits at the end: when idle it uses no RAM
//   (in use about 900 MB for 3-6 s; loading the model costs 0.5-1 s).
// - Fallback: the studio PC's Whisper (~/.local/bin/trascrivi, src/voice_notes.rs) if the model is not there yet or
//   if the Mac cannot do it. The temporary file lives in a private folder and is deleted right after.
// - No cloud: the model is downloaded once from Hugging Face (SHA-256 fingerprint verified), the audio never leaves.
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Emitter;

const RATE: u32 = 16_000;
/// Past 15 minutes the recording stops by itself (the UI closes it and transcribes).
const MAX_S: u32 = 15 * 60;
/// A Whisper model: file, URL, size and fingerprint (verified at every download).
#[derive(Clone, Copy)]
struct WhisperModel { nome: &'static str, url: &'static str, byte: u64, sha: &'static str }
const ACCURATE: WhisperModel = WhisperModel { nome: "ggml-large-v3-turbo-q5_0.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
    byte: 574_041_195, sha: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2" };
/// Windows and Ubuntu (Nov 12): the bundled Whisper runs on the CPU, without a GPU. The accurate model takes
/// ~75 s for 7 s of audio (measured on a GitHub Windows runner), "small" 19 s with the same correct sentence.
#[cfg(not(target_os = "macos"))]
const FAST: WhisperModel = WhisperModel { nome: "ggml-small-q5_1.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",
    byte: 190_085_487, sha: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb" };
/// The model in use: always accurate on the Mac; "fast" on Windows/Ubuntu unless "precisa" is chosen in Settings > Dictation.
fn chosen_model() -> WhisperModel {
    #[cfg(not(target_os = "macos"))]
    { if setting("dettatura_qualita").and_then(|v| v.as_str().map(String::from)).as_deref() != Some("precisa") { return FAST; } }
    ACCURATE
}
/// Maximum wait in seconds for a transcription: on the Mac 60 + 0.6 per second of audio, elsewhere 60 + 3.
fn max_wait_secs(audio_secs: f64) -> f64 { 60.0 + audio_secs * if cfg!(target_os = "macos") { 0.6 } else { 3.0 } }
/// Voice activity detection (Silero, 885 KB): Whisper transcribes only the spoken parts. Without it, after a long pause
/// it skipped the next sentence (test of Nov 10 on 3 minutes: 4 sentences lost and 2 cut out of 30; with it: 30 of 30, 6 s).
const VAD: &str = "ggml-silero-v5.1.2.bin";
const VAD_URL: &str = "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin";
const VAD_BYTES: u64 = 885_098;
const VAD_SHA256: &str = "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf";
/// Words Whisper gets wrong without help (project and product names). They can be changed with "microfono_parole"
/// in ~/tavolo/impostazioni.json.
/// Default: Juggl, Claude, Codex and the project names (also "microfono_parole" in ~/.config/juggl/personale.json).
fn default_vocabulary() -> String {
    if let Some(p) = crate::profile::personal().get("microfono_parole").and_then(|x| x.as_str()) { return p.to_string(); }
    let mut v = vec!["Juggl".to_string(), "Claude".into(), "Codex".into(), "deploy".into()];
    v.extend(crate::session_care::projects().iter().map(|p| crate::str_of(p, "nome").to_string()).filter(|n| !n.is_empty()).take(10));
    v.join(", ") + "."
}

fn whisper_dir() -> PathBuf {
    // Windows: %LOCALAPPDATA%\Juggl\whisper; Linux: ~/.local/share/juggl/whisper; Mac: Application Support
    #[cfg(windows)]
    { return std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("AppData/Local")).join("Juggl").join("whisper"); }
    #[cfg(target_os = "linux")]
    { return std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share")).join("juggl").join("whisper"); }
    #[allow(unreachable_code)]
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Application Support/Juggl/whisper")
}
fn model_path() -> PathBuf { whisper_dir().join(chosen_model().nome) }
pub(crate) fn model_ready() -> bool { model_path().is_file() && whisper_cli().is_some() }
/// The voice activity detection model: if missing it is downloaded now (once, 1 MB, fingerprint verified).
fn vad() -> Option<PathBuf> {
    let p = whisper_dir().join(VAD);
    if std::fs::metadata(&p).map(|m| m.len() == VAD_BYTES).unwrap_or(false) { return Some(p); }
    let partial = whisper_dir().join(format!("{VAD}.part"));
    let _ = std::fs::create_dir_all(whisper_dir());
    let ok = crate::license::curl_command().args(["-fsSL", "--retry", "2", "--max-time", "30", "-o"]).arg(&partial).arg(VAD_URL)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    if ok && std::fs::metadata(&partial).map(|m| m.len() == VAD_BYTES).unwrap_or(false) && sha256(&partial).ok().as_deref() == Some(VAD_SHA256) {
        if std::fs::rename(&partial, &p).is_ok() { return Some(p); }
    }
    let _ = std::fs::remove_file(&partial);
    None
}
fn setting(k: &str) -> Option<Value> { crate::load_json(&crate::base_dir().join("impostazioni.json")).and_then(|v| v.get(k).cloned()) }

/// whisper-cli next to the executable (installed app, or target/ in development), otherwise in the model folder.
fn whisper_cli() -> Option<PathBuf> {
    let name = format!("whisper-cli{}", std::env::consts::EXE_SUFFIX);
    let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join(&name)));
    [beside, Some(whisper_dir().join(&name))].into_iter().flatten().find(|p| p.is_file())
}

// ---------- microphone permission (AVFoundation) ----------
#[cfg(target_os = "macos")]
mod mic_permission {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" { static AVMediaTypeAudio: *const AnyObject; }
    /// 0 not asked yet, 1 restricted, 2 denied, 3 granted.
    pub fn status() -> isize {
        unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: &*AVMediaTypeAudio] }
    }
    /// Shows the macOS prompt and waits for the answer.
    pub fn request() -> bool {
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let block = block2::RcBlock::new(move |ok: Bool| { let _ = tx.send(ok.as_bool()); });
        unsafe { let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: &*AVMediaTypeAudio, completionHandler: &*block]; }
        rx.recv_timeout(std::time::Duration::from_secs(180)).unwrap_or(false)
    }
}
#[cfg(not(target_os = "macos"))]
mod mic_permission { pub fn status() -> isize { 3 } pub fn request() -> bool { true } }

// ---------- recording ----------
struct Recording {
    id: u64,
    ferma: Arc<AtomicBool>,
    campioni: Arc<Mutex<Vec<i16>>>,
    filo: std::thread::JoinHandle<()>,
    inizio: Instant,
    dispositivo: String,
}
static REG: Mutex<Option<Recording>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// One transcription at a time on the Mac: two whispers together would double the RAM.
static TRANSCRIBING: Mutex<()> = Mutex::new(());

/// Downsamples to 16 kHz by averaging the samples that fall into each step (box filter: good enough for voice).
struct Resampler { passo: f64, pos: f64, somma: f32, n: u32, ultimo: f32 }
impl Resampler {
    fn new(rate_in: u32) -> Self { Resampler { passo: rate_in as f64 / RATE as f64, pos: 0.0, somma: 0.0, n: 0, ultimo: 0.0 } }
    fn push_sample(&mut self, x: f32, out: &mut Vec<i16>) {
        self.somma += x; self.n += 1; self.pos += 1.0;
        while self.pos >= self.passo {
            let v = if self.n > 0 { self.somma / self.n as f32 } else { self.ultimo };
            self.ultimo = v; self.somma = 0.0; self.n = 0; self.pos -= self.passo;
            out.push((v.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
    }
}

fn build_stream<T>(dev: &cpal::Device, cfg: cpal::StreamConfig, samples: Arc<Mutex<Vec<i16>>>, peak: Arc<AtomicU32>) -> Result<cpal::Stream, String>
where T: cpal::SizedSample + Send + 'static, f32: cpal::FromSample<T> {
    use cpal::traits::DeviceTrait;
    use cpal::Sample;
    let channels = cfg.channels.max(1) as usize;
    let mut rc = Resampler::new(cfg.sample_rate);
    let max = (MAX_S * RATE) as usize;
    let mut out_buf: Vec<i16> = Vec::with_capacity(4096);
    dev.build_input_stream::<T, _, _>(cfg, move |data: &[T], _| {
        out_buf.clear();
        let mut p = 0f32;
        for fr in data.chunks(channels) {
            let x = fr.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / fr.len() as f32;
            p = p.max(x.abs());
            rc.push_sample(x, &mut out_buf);
        }
        // the highest peak since the last read by the levels thread (positive floats: bit order matches numeric order)
        peak.fetch_max(p.to_bits(), Ordering::Relaxed);
        if let Ok(mut c) = samples.lock() {
            if c.len() < max { let room = max - c.len(); c.extend_from_slice(&out_buf[..out_buf.len().min(room)]); }
        }
    }, |e| eprintln!("microfono: {e}"), None).map_err(|e| e.to_string())
}

/// Starts recording. Asks for permission the first time; clear error if denied or if there is no microphone.
#[tauri::command(async)]
pub fn mic_avvia(app: tauri::AppHandle) -> Value {
    if REG.lock().map(|r| r.is_some()).unwrap_or(false) { return json!({"ok": false, "msg": "Sto gia' registrando"}) }
    match mic_permission::status() {
        3 => {}
        0 => if !mic_permission::request() { return json!({"ok": false, "permesso": false, "msg": "Microfono non consentito"}) },
        _ => return json!({"ok": false, "permesso": false, "msg": "Microfono non consentito: Impostazioni di Sistema > Privacy e sicurezza > Microfono > Juggl"}),
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let ferma = Arc::new(AtomicBool::new(false));
    let campioni = Arc::new(Mutex::new(Vec::<i16>::with_capacity((RATE * 60) as usize)));
    let peak = Arc::new(AtomicU32::new(0));
    let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
    let (f2, c2, a2) = (ferma.clone(), campioni.clone(), app.clone());
    let filo = std::thread::Builder::new().name("microfono".into()).spawn(move || {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let Some(dev) = host.default_input_device() else { let _ = tx.send(Err("Nessun microfono trovato".into())); return };
        let name = dev.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "microfono".into());
        let sup = match dev.default_input_config() { Ok(c) => c, Err(e) => { let _ = tx.send(Err(e.to_string())); return } };
        let cfg = sup.config();
        let stream = match sup.sample_format() {
            cpal::SampleFormat::F32 => build_stream::<f32>(&dev, cfg, c2.clone(), peak.clone()),
            cpal::SampleFormat::I16 => build_stream::<i16>(&dev, cfg, c2.clone(), peak.clone()),
            cpal::SampleFormat::I32 => build_stream::<i32>(&dev, cfg, c2.clone(), peak.clone()),
            cpal::SampleFormat::U16 => build_stream::<u16>(&dev, cfg, c2.clone(), peak.clone()),
            f => Err(format!("formato audio non gestito: {f}")),
        };
        let stream = match stream.and_then(|s| s.play().map(|_| s).map_err(|e| e.to_string())) { Ok(s) => s, Err(e) => { let _ = tx.send(Err(e)); return } };
        let _ = tx.send(Ok(name));
        let t0 = Instant::now();
        let mut full = false;
        while !f2.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
            let p = f32::from_bits(peak.swap(0, Ordering::Relaxed));
            // almost logarithmic scale: normal voice (peak 0.05-0.3) fills the upper half of the bar
            let v = if p <= 0.0 { 0.0 } else { ((20.0 * p.log10() + 54.0) / 50.0).clamp(0.0, 1.0) };
            let _ = a2.emit("mic-livello", json!({"id": id, "v": v, "s": t0.elapsed().as_secs_f64()}));
            if !full && c2.lock().map(|c| c.len() >= (MAX_S * RATE) as usize).unwrap_or(false) {
                full = true; let _ = a2.emit("mic-pieno", json!({"id": id}));
            }
        }
        drop(stream);
    });
    let Ok(filo) = filo else { return json!({"ok": false, "msg": "thread non avviato"}) };
    match rx.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(device)) => {
            let mut r = REG.lock().unwrap_or_else(|e| e.into_inner());
            *r = Some(Recording { id, ferma, campioni, filo, inizio: Instant::now(), dispositivo: device.clone() });
            json!({"ok": true, "id": id, "dispositivo": device})
        }
        Ok(Err(e)) => { let _ = filo.join(); json!({"ok": false, "msg": format!("Microfono non avviato: {e}")}) }
        Err(_) => { ferma.store(true, Ordering::Relaxed); json!({"ok": false, "msg": "Il microfono non risponde"}) }
    }
}

fn take_recording() -> Option<(Vec<i16>, f64, String)> {
    let r = REG.lock().ok()?.take()?;
    r.ferma.store(true, Ordering::Relaxed);
    let _ = r.filo.join();
    let c = std::mem::take(&mut *r.campioni.lock().unwrap_or_else(|e| e.into_inner()));
    Some((c, r.inizio.elapsed().as_secs_f64(), r.dispositivo))
}

/// Esc: throws the recording away, no transcription.
#[tauri::command(async)]
pub fn mic_annulla() -> Value {
    let c = take_recording();
    json!({"ok": true, "annullata": c.is_some()})
}

/// Stops, transcribes and returns the text. On the Mac if the model is there, otherwise (or if the Mac fails) on the PC.
#[tauri::command(async)]
pub fn mic_ferma(app: tauri::AppHandle) -> Value {
    let Some((samples, duration, device)) = take_recording() else { return json!({"ok": false, "msg": "Nessuna registrazione in corso"}) };
    let audio_secs = samples.len() as f64 / RATE as f64;
    let Some(voice) = trim_silence(&samples) else {
        return json!({"ok": false, "vuoto": true, "msg": format!("Non ho sentito niente da {device}"), "secondi_audio": audio_secs})
    };
    let wav = wav_16k(voice);
    drop(samples);
    let t0 = Instant::now();
    let engine = setting("microfono_motore").and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "auto".into());
    let mut notes = vec![];
    if engine != "pc" {
        if model_path().is_file() {
            let _ = app.emit("mic-fase", json!({"fase": "mac"}));
            match transcribe_mac(&wav, audio_secs) {
                Ok(t) => return json!({"ok": true, "testo": t, "fonte": "mac", "secondi_audio": audio_secs, "durata": duration, "secondi": t0.elapsed().as_secs_f64()}),
                Err(e) => notes.push(e),
            }
        } else { notes.push("modello non ancora scaricato".to_string()); }
    }
    let _ = app.emit("mic-fase", json!({"fase": "pc"}));
    match transcribe_on_pc(&wav, audio_secs) {
        Ok(t) => json!({"ok": true, "testo": t, "fonte": "pc", "secondi_audio": audio_secs, "durata": duration, "secondi": t0.elapsed().as_secs_f64(), "nota": notes.join("; ")}),
        Err(e) => {
            notes.push(e);
            // the audio is not lost: it stays in ~/tavolo/allegati (this user only) for "Riprova"
            let saved_path = save_for_retry(&wav);
            json!({"ok": false, "msg": format!("Trascrizione non riuscita: {}", notes.join("; ")), "secondi_audio": audio_secs, "percorso": saved_path})
        }
    }
}

fn save_for_retry(wav: &[u8]) -> String {
    let dir = crate::base_dir().join("allegati");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(format!("vocale-mac-{}.wav", (crate::unix_now() * 1000.0) as u64));
    if std::fs::write(&p, wav).is_err() { return String::new(); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)); }
    p.to_string_lossy().to_string()
}

/// Retries the transcription of a Mac voice note saved after an error. Success = the file is deleted.
#[tauri::command(async)]
pub fn mic_riprova(percorso: String) -> Value {
    let p = std::path::Path::new(&percorso);
    let inside = p.parent() == Some(crate::base_dir().join("allegati").as_path()) && p.file_name().map(|n| n.to_string_lossy().starts_with("vocale-mac-")).unwrap_or(false);
    if !inside || percorso.contains("..") { return json!({"ok": false, "msg": "percorso non ammesso"}) }
    let Ok(wav) = std::fs::read(p) else { return json!({"ok": false, "msg": "audio non trovato"}) };
    let audio_secs = wav.len().saturating_sub(44) as f64 / (RATE as f64 * 2.0);
    let r = if model_ready() { transcribe_mac(&wav, audio_secs) } else { Err("modello non pronto".into()) }
        .or_else(|e| transcribe_on_pc(&wav, audio_secs).map_err(|e2| format!("{e}; {e2}")));
    match r {
        Ok(t) => { let _ = std::fs::remove_file(p); json!({"ok": true, "testo": t}) }
        Err(e) => json!({"ok": false, "msg": format!("Trascrizione non riuscita: {e}"), "percorso": percorso}),
    }
}

/// Trims the silence before and after (fewer Whisper hallucinations on silent endings). None if there is no voice.
fn trim_silence(c: &[i16]) -> Option<&[i16]> {
    const F: usize = 480; // 30 ms
    if c.len() < RATE as usize / 3 { return None }
    let rms: Vec<f32> = c.chunks(F).map(|f| (f.iter().map(|&x| (x as f32 / 32768.0).powi(2)).sum::<f32>() / f.len() as f32).sqrt()).collect();
    let max = rms.iter().cloned().fold(0f32, f32::max);
    if max < 0.006 { return None }   // about -44 dBFS: only background noise or permission denied (zeros)
    let threshold = (max * 0.08).max(0.004);
    let first = rms.iter().position(|&r| r > threshold)?;
    let last = rms.iter().rposition(|&r| r > threshold)?;
    let pad = 10; // 300 ms
    let from_idx = first.saturating_sub(pad) * F;
    let a = ((last + 1 + pad) * F).min(c.len());
    Some(&c[from_idx..a])
}

fn wav_16k(c: &[i16]) -> Vec<u8> {
    let data_len = (c.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF"); w.extend_from_slice(&(36 + data_len).to_le_bytes()); w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes()); w.extend_from_slice(&1u16.to_le_bytes()); w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&RATE.to_le_bytes()); w.extend_from_slice(&(RATE * 2).to_le_bytes()); w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes()); w.extend_from_slice(b"data"); w.extend_from_slice(&data_len.to_le_bytes());
    for s in c { w.extend_from_slice(&s.to_le_bytes()); }
    w
}

/// Sentences Whisper invents on silence (subtitles of the videos it was trained on).
fn strip_hallucinations(t: &str) -> String {
    let mut t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    for suffix in ["Sottotitoli creati dalla comunità Amara.org", "Sottotitoli a cura di QTSS", "Grazie per la visione!", "Grazie per la visione."] {
        if let Some(r) = t.strip_suffix(suffix) { t = r.trim_end().to_string(); }
    }
    t.trim().to_string()
}

pub(crate) fn transcribe_mac(wav: &[u8], audio_secs: f64) -> Result<String, String> {
    let cli = whisper_cli().ok_or("whisper-cli non trovato")?;
    let _turno = TRANSCRIBING.lock().unwrap_or_else(|e| e.into_inner());
    let vocabulary = setting("microfono_parole").and_then(|v| v.as_str().map(String::from)).unwrap_or_else(default_vocabulary);
    let threads = std::thread::available_parallelism().map(|n| n.get().min(4)).unwrap_or(4).to_string();
    let mut cmd = crate::new_command(&cli);
    #[cfg(windows)] { use std::os::windows::process::CommandExt; cmd.creation_flags(crate::sys::CREATE_NO_WINDOW); }
    cmd.args(["-m"]).arg(model_path()).args(["-l", "it", "-t", &threads, "-nt", "-np", "-otxt", "--prompt", &vocabulary]);
    if let Some(v) = vad() { cmd.args(["--vad", "-vm"]).arg(v); }
    // Windows (Nov 12, tested on the runner): with audio from stdin ("-f -") Whisper reads and processes it but does not write
    // the text. With a file it does. The file lives in Juggl's folder and is deleted right after (the .txt from -otxt too).
    #[cfg(windows)]
    let audio_file = {
        let dir = whisper_dir().join("tmp");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let p = dir.join(format!("dettatura-{}.wav", (crate::unix_now() * 1000.0) as u64));
        std::fs::write(&p, wav).map_err(|e| e.to_string())?;
        cmd.arg("-f").arg(&p);
        p
    };
    #[cfg(not(windows))] cmd.args(["-f", "-"]);
    let child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    #[cfg(windows)] let remove_temp_files = || { let _ = std::fs::remove_file(&audio_file); let _ = std::fs::remove_file(audio_file.with_extension("wav.txt")); };
    #[cfg(not(windows))] let remove_temp_files = || {};
    let mut child = match child { Ok(f) => f, Err(e) => { remove_temp_files(); return Err(format!("whisper-cli: {e}")) } };
    let mut stdin_pipe = child.stdin.take().ok_or("stdin")?;
    let audio = if cfg!(windows) { Vec::new() } else { wav.to_vec() };
    let writer = std::thread::spawn(move || { if !audio.is_empty() { let _ = stdin_pipe.write_all(&audio); } });
    let mut stdout_pipe = child.stdout.take().ok_or("stdout")?;
    let reader = std::thread::spawn(move || { let mut s = String::new(); let _ = stdout_pipe.read_to_string(&mut s); s });
    let deadline = Instant::now() + Duration::from_secs_f64(max_wait_secs(audio_secs));
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(60)),
            _ => { let _ = child.kill(); let _ = child.wait(); remove_temp_files(); return Err("whisper sul Mac troppo lento".into()) }
        }
    };
    let _ = writer.join();
    let text = reader.join().unwrap_or_default();
    remove_temp_files();
    if !status.success() { return Err(format!("whisper sul Mac uscito con {}", status.code().unwrap_or(-1))) }
    Ok(strip_hallucinations(&text))
}

/// Fallback on the studio PC: temporary file in a private folder, deleted together with its .txt/.srt.
fn transcribe_on_pc(wav: &[u8], audio_secs: f64) -> Result<String, String> {
    let dir = whisper_dir().join("tmp");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)); }
    let p = dir.join(format!("vocale-{}.wav", (crate::unix_now() * 1000.0) as u64));
    std::fs::write(&p, wav).map_err(|e| e.to_string())?;
    let r = crate::voice_notes::transcribe_file(&p, Duration::from_secs_f64(60.0 + audio_secs));
    for ext in ["wav", "txt", "srt"] { let _ = std::fs::remove_file(p.with_extension(ext)); }
    match r {
        Some(t) => Ok(strip_hallucinations(&t.replace("\n\n", " "))),
        None if crate::profile::tool("trascrivi").is_none() => Err("Riconoscimento vocale non ancora pronto sul Mac".into()),
        None => Err("PC dello studio non raggiungibile o occupato".into()),
    }
}

// ---------- model: status and download ----------
static DOWNLOADING: AtomicBool = AtomicBool::new(false);
static DOWNLOADED: AtomicU64 = AtomicU64::new(0);

#[tauri::command(async)]
pub fn mic_stato() -> Value {
    let engine = setting("microfono_motore").and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "auto".into());
    json!({
        "pronto": model_path().is_file() && whisper_cli().is_some(),
        "modello": model_path().is_file(), "cli": whisper_cli().is_some(),
        "scarica": DOWNLOADING.load(Ordering::Relaxed), "fatti": DOWNLOADED.load(Ordering::Relaxed), "totale": chosen_model().byte,
        "qualita": if cfg!(target_os = "macos") { "" } else if chosen_model().nome == ACCURATE.nome { "precisa" } else { "veloce" },
        "senza_gpu": !cfg!(target_os = "macos"),
        "motore": engine, "permesso": mic_permission::status(),
        "registra": REG.lock().map(|r| r.as_ref().map(|r| r.id)).ok().flatten(),
    })
}

/// Downloads the model once only, in the background, with progress in the "mic-modello" event.
#[tauri::command(async)]
pub fn mic_scarica_modello(app: tauri::AppHandle) -> Value {
    if model_path().is_file() { return json!({"ok": true, "pronto": true}) }
    if DOWNLOADING.swap(true, Ordering::SeqCst) { return json!({"ok": true, "in_corso": true}) }
    std::thread::spawn(move || {
        let r = download_model(|fatti| { DOWNLOADED.store(fatti, Ordering::Relaxed); let _ = app.emit("mic-modello", json!({"fatti": fatti, "totale": chosen_model().byte})); });
        DOWNLOADING.store(false, Ordering::SeqCst);
        match r {
            Ok(()) => { let _ = app.emit("mic-modello", json!({"pronto": true, "fatti": chosen_model().byte, "totale": chosen_model().byte})); }
            Err(e) => { let _ = app.emit("mic-modello", json!({"errore": e})); }
        }
    });
    json!({"ok": true, "in_corso": true})
}

fn download_model(progress: impl Fn(u64)) -> Result<(), String> {
    let dir = whisper_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let partial = dir.join(format!("{}.part", chosen_model().nome));
    // -C - resumes an interrupted download
    let mut child = crate::license::curl_command().args(["-fsSL", "--retry", "3", "-C", "-", "-o"]).arg(&partial).arg(chosen_model().url)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let st = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? { break st }
        progress(std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0));
        std::thread::sleep(Duration::from_millis(400));
    };
    if !st.success() {
        let mut e = String::new(); if let Some(mut s) = child.stderr.take() { let _ = s.read_to_string(&mut e); }
        return Err(format!("download non riuscito ({}): {}", st.code().unwrap_or(-1), e.trim()));
    }
    let n = std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
    progress(n);
    if n != chosen_model().byte { let _ = std::fs::remove_file(&partial); return Err(format!("dimensione sbagliata: {n} byte")) }
    let digest = sha256(&partial)?;
    if digest != chosen_model().sha { let _ = std::fs::remove_file(&partial); return Err("impronta del modello non valida".into()) }
    std::fs::rename(&partial, model_path()).map_err(|e| e.to_string())
}

fn sha256(p: &std::path::Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop { let n = f.read(&mut buf).map_err(|e| e.to_string())?; if n == 0 { break } h.update(&buf[..n]); }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Opens System Settings on the microphone page (permission denied).
#[tauri::command(async)]
pub fn mic_imposta_qualita(qualita: String) -> Value {
    if cfg!(target_os = "macos") { return json!({"ok": false, "msg": "Sul Mac il riconoscimento e' sempre preciso"}); }
    let q = if qualita == "precisa" { "precisa" } else { "veloce" };
    match crate::storage::modify(&crate::base_dir().join("impostazioni.json"), Some(json!({})), |v| { v["dettatura_qualita"] = json!(q); true }) {
        Ok(_) => { let m = chosen_model(); json!({"ok": true, "qualita": q, "modello_presente": whisper_dir().join(m.nome).is_file(), "mb": m.byte / 1_000_000}) }
        Err(e) => json!({"ok": false, "msg": e}),
    }
}

#[tauri::command(async)]
pub fn mic_impostazioni() {
    #[cfg(windows)] { crate::sys::open("ms-settings:privacy-microphone"); return; }
    #[cfg(target_os = "linux")] { return; }
    #[allow(unreachable_code)]
    let _ = crate::new_command("open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone").spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resample_48k() {
        let mut r = Resampler::new(48_000); let mut o = vec![];
        for i in 0..48_000 { r.push_sample(((i as f32) * 0.01).sin() * 0.5, &mut o); }
        assert_eq!(o.len(), 16_000);
        let mut r = Resampler::new(44_100); let mut o = vec![];
        for _ in 0..44_100 { r.push_sample(0.25, &mut o); }
        assert!((o.len() as i32 - 16_000).abs() <= 1, "{}", o.len());
        assert!(o.iter().all(|&x| (x - 8191).abs() <= 1));
    }
    #[test]
    fn silence_and_trim() {
        assert!(trim_silence(&vec![0i16; 16_000]).is_none());
        let mut c = vec![0i16; 32_000];
        for i in 16_000..20_000 { c[i] = if i % 2 == 0 { 8000 } else { -8000 }; }
        let v = trim_silence(&c).unwrap();
        assert!(v.len() < 15_000 && v.len() >= 4_000, "{}", v.len());
    }
    #[test]
    fn strips_invented_phrases() {
        assert_eq!(strip_hallucinations(" Ciao a tutti.\n Ci vediamo. Sottotitoli creati dalla comunità Amara.org"), "Ciao a tutti. Ci vediamo.");
        assert_eq!(strip_hallucinations("Grazie per la visione!"), "");
    }
}

#[cfg(test)]
mod real_transcription {
    /// cargo test real_transcription -- --ignored --nocapture, with VOCALE_PROVA=<16 kHz mono wav> (the model must be there too)
    #[test]
    #[ignore]
    fn transcribes_a_wav() {
        let b = std::fs::read(std::env::var("VOCALE_PROVA").expect("VOCALE_PROVA")).unwrap();
        let samples: Vec<i16> = b[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        let voice = super::trim_silence(&samples).expect("voce");
        let t0 = std::time::Instant::now();
        let r = super::transcribe_mac(&super::wav_16k(voice), samples.len() as f64 / 16000.0);
        println!("{:?} in {:.1}s", r, t0.elapsed().as_secs_f64());
        assert!(r.unwrap().len() > 10);
    }
    /// really downloads the model into Juggl's folder (550 MB)
    #[test]
    #[ignore]
    fn downloads_the_model() {
        let t0 = std::time::Instant::now();
        let last = std::sync::atomic::AtomicU64::new(0);
        super::download_model(|f| { if f / 50_000_000 != last.load(std::sync::atomic::Ordering::Relaxed) / 50_000_000 { println!("{f}"); } last.store(f, std::sync::atomic::Ordering::Relaxed); }).unwrap();
        println!("ok in {:.0}s", t0.elapsed().as_secs_f64());
    }
}
