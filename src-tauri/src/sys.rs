// Windows: the few libc functions Juggl uses on the Mac (signals, process groups, file locks, local time),
// with the same names. In the files that use them `#[cfg(windows)] use crate::sys as libc;` is enough and the code stays
// identical to the Mac. Windows only: on the Mac the real libc is used.
#![cfg(windows)]
#![allow(non_camel_case_types, dead_code, clippy::missing_safety_doc)]

use std::os::windows::process::CommandExt;
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, SYSTEMTIME, HANDLE, STILL_ACTIVE};
use windows_sys::Win32::Storage::FileSystem::{LockFileEx, UnlockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};
use windows_sys::Win32::System::IO::OVERLAPPED;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime, TzSpecificLocalTimeToSystemTime};

pub type c_int = i32;
pub type time_t = i64;
pub type uid_t = u32;
pub type gid_t = u32;
pub type sighandler_t = usize;

pub const SIGHUP: c_int = 1;
pub const SIGINT: c_int = 2;
pub const SIGKILL: c_int = 9;
pub const SIGPIPE: c_int = 13;
pub const SIGTERM: c_int = 15;
pub const SIG_IGN: sighandler_t = 1;
pub const PRIO_PROCESS: c_int = 0;
pub const LOCK_EX: c_int = 2;
pub const LOCK_NB: c_int = 4;
pub const LOCK_UN: c_int = 8;

/// No black window for commands launched by the app.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Same fields of `struct tm` that Juggl reads.
#[derive(Clone, Copy, Default, Debug)]
pub struct tm {
    pub tm_sec: c_int, pub tm_min: c_int, pub tm_hour: c_int, pub tm_mday: c_int, pub tm_mon: c_int,
    pub tm_year: c_int, pub tm_wday: c_int, pub tm_yday: c_int, pub tm_isdst: c_int,
    /// difference of local time from UTC in seconds (as on macOS)
    pub tm_gmtoff: i64,
}

fn is_alive(pid: u32) -> bool {
    unsafe {
        let h: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() { return false; }
        let mut c: u32 = 0;
        let ok = GetExitCodeProcess(h, &mut c) != 0 && c == STILL_ACTIVE as u32;
        CloseHandle(h);
        ok
    }
}

fn terminate(pid: u32) -> bool {
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() { return false; }
        let ok = TerminateProcess(h, 1) != 0;
        CloseHandle(h);
        ok
    }
}

/// `kill(pid, 0)` = is it alive?; `kill(-pid, sig)` = the whole process tree; otherwise only that process.
/// Windows has no "close gently" for processes without a window: SIGTERM and SIGKILL close immediately.
pub unsafe fn kill(pid: i32, sig: c_int) -> c_int {
    let p = pid.unsigned_abs();
    if p == 0 { return -1; }
    if sig == 0 { return if is_alive(p) { 0 } else { -1 }; }
    if pid < 0 {
        let ok = std::process::Command::new("taskkill").args(["/T", "/F", "/PID", &p.to_string()])
            .creation_flags(CREATE_NO_WINDOW).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false);
        return if ok || terminate(p) { 0 } else { -1 };
    }
    if terminate(p) { 0 } else { -1 }
}

/// On Windows every process launched with `process_group(0)` is the leader of its tree.
pub unsafe fn getpgid(pid: i32) -> i32 { pid }
pub unsafe fn setsid() -> i32 { 0 }
pub unsafe fn setpriority(_: c_int, _: u32, _: c_int) -> c_int { 0 }
pub unsafe fn umask(_: u32) -> u32 { 0 }
pub unsafe fn getuid() -> uid_t { 0 }
pub unsafe fn signal(_: c_int, _: sighandler_t) -> sighandler_t { 0 }

pub unsafe fn time(_: *mut time_t) -> time_t {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

const FILETIME_EPOCH: i64 = 116_444_736_000_000_000; // 1601 -> 1970 in units of 100 ns

fn filetime(t: time_t) -> FILETIME {
    let v = (t * 10_000_000 + FILETIME_EPOCH) as u64;
    FILETIME { dwLowDateTime: v as u32, dwHighDateTime: (v >> 32) as u32 }
}

fn day_of_year(a: i32, m: i32, g: i32) -> i32 {
    const CUM: [i32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let bis = (a % 4 == 0 && a % 100 != 0) || a % 400 == 0;
    CUM[m as usize] + g - 1 + if bis && m > 1 { 1 } else { 0 }
}

pub unsafe fn localtime_r(t: *const time_t, out: *mut tm) -> *mut tm {
    let ft = filetime(*t);
    let mut utc: SYSTEMTIME = std::mem::zeroed();
    let mut loc: SYSTEMTIME = std::mem::zeroed();
    if FileTimeToSystemTime(&ft, &mut utc) == 0 || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut loc) == 0 {
        return std::ptr::null_mut();
    }
    let a = loc.wYear as i32; let m = loc.wMonth as i32 - 1; let g = loc.wDay as i32;
    let mut fl: FILETIME = std::mem::zeroed();
    let off = if SystemTimeToFileTime(&loc, &mut fl) != 0 { ((((fl.dwHighDateTime as u64) << 32 | fl.dwLowDateTime as u64) as i64 - FILETIME_EPOCH) / 10_000_000) - *t } else { 0 };
    *out = tm { tm_gmtoff: off, tm_sec: loc.wSecond as i32, tm_min: loc.wMinute as i32, tm_hour: loc.wHour as i32, tm_mday: g, tm_mon: m,
        tm_year: a - 1900, tm_wday: loc.wDayOfWeek as i32, tm_yday: day_of_year(a, m, g), tm_isdst: -1 };
    out
}

/// Local time -> Unix seconds, normalising out-of-range fields (like mktime: day 32 = first of next month).
pub unsafe fn mktime(t: *mut tm) -> time_t {
    let x = *t;
    // normalise with arithmetic on days: start from the first of the month, then add days/hours/minutes/seconds
    let mut year = x.tm_year + 1900 + x.tm_mon.div_euclid(12);
    let month = x.tm_mon.rem_euclid(12);
    if year < 1601 { year = 1601; }
    let base = SYSTEMTIME { wYear: year as u16, wMonth: (month + 1) as u16, wDayOfWeek: 0, wDay: 1, wHour: 0, wMinute: 0, wSecond: 0, wMilliseconds: 0 };
    let mut utc: SYSTEMTIME = std::mem::zeroed();
    if TzSpecificLocalTimeToSystemTime(std::ptr::null(), &base, &mut utc) == 0 { return -1; }
    let mut ft: FILETIME = std::mem::zeroed();
    if SystemTimeToFileTime(&utc, &mut ft) == 0 { return -1; }
    let v = ((ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64) as i64;
    let first = (v - FILETIME_EPOCH) / 10_000_000;
    let r = first + (x.tm_mday as i64 - 1) * 86_400 + x.tm_hour as i64 * 3600 + x.tm_min as i64 * 60 + x.tm_sec as i64;
    // a DST change within the month shifts by one hour: fix it by re-reading the resulting local time
    let mut chk = tm::default();
    if !localtime_r(&r, &mut chk).is_null() {
        let wanted = x.tm_hour.rem_euclid(24);
        let diff = chk.tm_hour - wanted;
        if diff == 1 || diff == -23 { let r2 = r - 3600; localtime_r(&r2, &mut *t); return r2; }
        if diff == -1 || diff == 23 { let r2 = r + 3600; localtime_r(&r2, &mut *t); return r2; }
    }
    localtime_r(&r, &mut *t);
    r
}

/// File handle as "fd" for `flock`.
pub trait AsRawFd { fn as_raw_fd(&self) -> isize; }
impl AsRawFd for std::fs::File {
    fn as_raw_fd(&self) -> isize { use std::os::windows::io::AsRawHandle; self.as_raw_handle() as isize }
}

pub unsafe fn flock(h: isize, op: c_int) -> c_int {
    let h = h as HANDLE;
    let mut ov: OVERLAPPED = std::mem::zeroed();
    if op & LOCK_UN != 0 { return if UnlockFileEx(h, 0, u32::MAX, u32::MAX, &mut ov) != 0 { 0 } else { -1 }; }
    let mut f = 0;
    if op & LOCK_EX != 0 { f |= LOCKFILE_EXCLUSIVE_LOCK; }
    if op & LOCK_NB != 0 { f |= LOCKFILE_FAIL_IMMEDIATELY; }
    if LockFileEx(h, f, 0, u32::MAX, u32::MAX, &mut ov) != 0 { 0 } else { -1 }
}

/// True in the keeper started with its hidden console (avvia_nascosto): children inherit it, so it must NOT be removed
/// (without a console each of their commands would open a new visible one).
pub static CONSOLE_HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The Mac's `Command::process_group(0)`: on Windows a new group; with no hidden console to inherit, no window.
pub trait CommandExtJuggl { fn process_group(&mut self, _: i32) -> &mut Self; }
impl CommandExtJuggl for std::process::Command {
    fn process_group(&mut self, _: i32) -> &mut Self {
        if CONSOLE_HIDDEN.load(std::sync::atomic::Ordering::Relaxed) { self.creation_flags(CREATE_NEW_PROCESS_GROUP) }
        else { self.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_roundtrip() {
        let n = format!("prova-{}", std::process::id());
        assert!(vault_save(&n, "segreto-è"));
        assert_eq!(vault_read(&n).as_deref(), Some("segreto-è"));
        assert!(vault_remove(&n));
        assert!(vault_read(&n).is_none());
    }
    #[test]
    fn local_time_roundtrip() {
        unsafe {
            let t = 1_794_355_200; // 11/11/2026 00:00 UTC
            let mut x = tm::default();
            assert!(!localtime_r(&t, &mut x).is_null());
            assert_eq!(x.tm_year, 126); assert_eq!(x.tm_mon, 10); assert_eq!(x.tm_gmtoff % 1800, 0);
            let mut y = x; assert_eq!(mktime(&mut y), t);
            let mut z = x; z.tm_mday += 30; // spills into the next month
            let r = mktime(&mut z); assert_eq!(z.tm_mon, 11); assert_eq!(r, t + 30 * 86_400);
        }
    }
}

/// Names of the executables of all descendants of a process (children, grandchildren...): to know whether Claude has
/// background commands (hosted::occupata). A single snapshot of the processes.
#[allow(dead_code)]
pub fn descendants(pid: u32) -> Vec<String> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut all: Vec<(u32, u32, String)> = vec![];
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 { return vec![]; }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                all.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
                if Process32NextW(snap, &mut e) == 0 { break; }
            }
        }
        CloseHandle(snap);
    }
    let mut parents = vec![pid];
    let mut out = vec![];
    let mut i = 0;
    while i < parents.len() && parents.len() < 4096 {
        let p = parents[i];
        for (c, pp, name) in &all { if *pp == p && *c != pid && !parents.contains(c) { parents.push(*c); out.push(name.clone()); } }
        i += 1;
    }
    out
}

/// (parent pid, executable name, start in Unix seconds) of a process: used by the hook to trace back to Claude.
pub fn process_info(pid: u32) -> Option<(u32, String, Option<f64>)> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 { return None; }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                if e.th32ProcessID == pid {
                    let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                    found = Some((e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
                    break;
                }
                if Process32NextW(snap, &mut e) == 0 { break; }
            }
        }
        CloseHandle(snap);
        let (pp, name) = found?;
        let mut started = None;
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if !h.is_null() {
            let (mut c, mut x, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
            if GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) != 0 {
                let v = ((c.dwHighDateTime as u64) << 32 | c.dwLowDateTime as u64) as i64;
                started = Some((v - FILETIME_EPOCH) as f64 / 10_000_000.0);
            }
            CloseHandle(h);
        }
        Some((pp, name, started))
    }
}

/// Opens a file, a folder or a link with the default program (like `open` on the Mac).
pub fn open(what: &str) -> bool {
    std::process::Command::new("explorer").arg(what).creation_flags(CREATE_NO_WINDOW).spawn().is_ok()
}

/// Shows the selected file in File Explorer (like `open -R`).
pub fn reveal_in_folder(p: &str) -> bool {
    std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", p.replace('/', "\\"))).creation_flags(CREATE_NO_WINDOW).spawn().is_ok()
}

/// Copies the text to the clipboard (UTF-16, so accents stay right: clip.exe ruins them).
pub fn copy_text(text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::Foundation::GlobalFree;
    const CF_UNICODETEXT: u32 = 13;
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut opened = false;
        for _ in 0..10 { if OpenClipboard(std::ptr::null_mut()) != 0 { opened = true; break; } std::thread::sleep(std::time::Duration::from_millis(30)); }
        if !opened { return false; }
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2);
        if h.is_null() { CloseClipboard(); return false; }
        let d = GlobalLock(h) as *mut u16;
        if d.is_null() { GlobalFree(h); CloseClipboard(); return false; }
        std::ptr::copy_nonoverlapping(w.as_ptr(), d, w.len());
        GlobalUnlock(h);
        let ok = !SetClipboardData(CF_UNICODETEXT, h as _).is_null();
        if !ok { GlobalFree(h); }
        CloseClipboard();
        ok
    }
}

/// Random bytes from the system (instead of /dev/urandom). If it fails it stops: a predictable key is worse than none.
pub fn random_bytes(b: &mut [u8]) {
    use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    let ok = unsafe { BCryptGenRandom(std::ptr::null_mut(), b.as_mut_ptr(), b.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) } == 0;
    assert!(ok, "BCryptGenRandom non riuscito");
}

// ---------- secure storage: Windows Credential Manager (instead of the Keychain) ----------
// Generic entries "Juggl/<name>", only for this Windows user. Used for the device token (Juggl account).

fn target_name(name: &str) -> Vec<u16> { format!("Juggl/{name}").encode_utf16().chain(std::iter::once(0)).collect() }

pub fn vault_save(name: &str, value: &str) -> bool {
    use windows_sys::Win32::Security::Credentials::{CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC};
    let mut t = target_name(name);
    let mut user: Vec<u16> = "juggl".encode_utf16().chain(std::iter::once(0)).collect();
    let mut data = value.as_bytes().to_vec();
    let c = CREDENTIALW {
        Flags: 0, Type: CRED_TYPE_GENERIC, TargetName: t.as_mut_ptr(), Comment: std::ptr::null_mut(),
        LastWritten: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
        CredentialBlobSize: data.len() as u32, CredentialBlob: data.as_mut_ptr(), Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0, Attributes: std::ptr::null_mut(), TargetAlias: std::ptr::null_mut(), UserName: user.as_mut_ptr(),
    };
    unsafe { CredWriteW(&c, 0) != 0 }
}

pub fn vault_read(name: &str) -> Option<String> {
    use windows_sys::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};
    let t = target_name(name);
    let mut p: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) == 0 || p.is_null() { return None; }
        let c = &*p;
        let v = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
        CredFree(p as *const _);
        String::from_utf8(v).ok()
    }
}

pub fn vault_remove(name: &str) -> bool {
    use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
    let t = target_name(name);
    unsafe { CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0) != 0 }
}

/// Physical memory: (total in bytes, available in bytes).
pub fn memory() -> (f64, f64) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) == 0 { return (0.0, 0.0); }
        (m.ullTotalPhys as f64, m.ullAvailPhys as f64)
    }
}

/// Seconds since the last key press or mouse move in this Windows session.
pub fn idle_secs() -> f64 {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    unsafe {
        let mut l = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if GetLastInputInfo(&mut l) == 0 { return 0.0; }
        GetTickCount().wrapping_sub(l.dwTime) as f64 / 1000.0
    }
}

// ---------- process with HIDDEN console (chat keeper) ----------
// With CREATE_NO_WINDOW the process has no console: every terminal program it launches (Claude Code hooks,
// bash, cmd...) opens a NEW visible one = windows that keep opening and closing (reported by a tester, 12/11).
// Here the process is born with its own but hidden console (SW_HIDE): Claude and all children inherit it and nothing
// is seen. A console started hidden is not handed over to Windows Terminal.

pub struct HiddenProcess { h: HANDLE, pid: u32 }
unsafe impl Send for HiddenProcess {}
pub struct HiddenExit(u32);
impl HiddenExit { pub fn code(&self) -> Option<i32> { Some(self.0 as i32) } }

impl HiddenProcess {
    pub fn id(&self) -> u32 { self.pid }
    pub fn try_wait(&mut self) -> std::io::Result<Option<HiddenExit>> {
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        unsafe {
            if WaitForSingleObject(self.h, 0) != 0 { return Ok(None); }   // 0 = WAIT_OBJECT_0: finished
            let mut c: u32 = 0;
            GetExitCodeProcess(self.h, &mut c);
            Ok(Some(HiddenExit(c)))
        }
    }
    pub fn wait(&mut self) -> std::io::Result<HiddenExit> {
        use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};
        unsafe { WaitForSingleObject(self.h, INFINITE); let mut c: u32 = 0; GetExitCodeProcess(self.h, &mut c); Ok(HiddenExit(c)) }
    }
}
impl Drop for HiddenProcess { fn drop(&mut self) { unsafe { CloseHandle(self.h); } } }

/// Argument for the Windows command line (CommandLineToArgvW rules).
fn quote_arg(a: &str, out: &mut String) {
    if !a.is_empty() && !a.contains([' ', '\t', '"', '\n']) { out.push_str(a); return; }
    out.push('"');
    let mut backslashes = 0;
    for c in a.chars() {
        if c == '\\' { backslashes += 1; continue; }
        if c == '"' { out.push_str(&"\\".repeat(backslashes * 2 + 1)); out.push('"'); } else { out.push_str(&"\\".repeat(backslashes)); out.push(c); }
        backslashes = 0;
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
}

/// Starts `prog args` with a new hidden console, new process group, environment = Juggl's with the
/// `set_vars` variables added/changed and those starting with a prefix in `remove` removed.
pub fn spawn_hidden(prog: &std::path::Path, args: &[String], cwd: &str, set_vars: &[(&str, &str)], remove: &[&str]) -> std::io::Result<HiddenProcess> {
    use windows_sys::Win32::System::Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW, CREATE_NEW_CONSOLE, CREATE_UNICODE_ENVIRONMENT, STARTF_USESHOWWINDOW};
    let mut cmdline = String::new();
    quote_arg(&prog.to_string_lossy(), &mut cmdline);
    for a in args { cmdline.push(' '); quote_arg(a, &mut cmdline); }
    let mut cmdline_w: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
    let mut vars: Vec<(String, String)> = std::env::vars().filter(|(k, _)| !remove.iter().any(|t| k.to_uppercase().starts_with(&t.to_uppercase()))).collect();
    for (k, v) in set_vars { vars.retain(|(x, _)| !x.eq_ignore_ascii_case(k)); vars.push((k.to_string(), v.to_string())); }
    vars.sort_by(|a, b| a.0.to_uppercase().cmp(&b.0.to_uppercase()));
    let mut env_w: Vec<u16> = Vec::new();
    for (k, v) in &vars { env_w.extend(format!("{k}={v}").encode_utf16()); env_w.push(0); }
    env_w.push(0);
    let cwd_w: Vec<u16> = cwd.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESHOWWINDOW;
        si.wShowWindow = 0;   // SW_HIDE
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let ok = CreateProcessW(std::ptr::null(), cmdline_w.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0,
            CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT, env_w.as_ptr() as *const _, cwd_w.as_ptr(), &si, &mut pi);
        if ok == 0 { return Err(std::io::Error::last_os_error()); }
        CloseHandle(pi.hThread);
        Ok(HiddenProcess { h: pi.hProcess, pid: pi.dwProcessId })
    }
}
