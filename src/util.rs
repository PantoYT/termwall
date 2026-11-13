//! Small helpers shared by every module: JSON files, in-place writes, tokens, PATH lookup and
//! the few Python behaviours the settings depend on (number formatting, rounding, splitlines).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub use serde_json::Value;
pub type Map = serde_json::Map<String, Value>;

/// A JSON file, or None when it is missing or not valid JSON (like the Python `_json_file`).
pub fn read_json(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    serde_json::from_str(&text).ok()
}

/// Text with the platform's line ends: Python wrote these files in text mode, so they have
/// CRLF on Windows. Keeping that means switching between the two versions changes no bytes.
pub fn native_newlines(text: &str) -> String {
    if cfg!(windows) { text.replace('\n', "\r\n") } else { text.to_string() }
}

/// Universal newlines, as Python's text mode reads them.
pub fn universal_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Overwrite a file in place: no temp file and rename. (On the machine termwall grew up on,
/// a broken EFS flag on AppData made every create-and-rename fail.)
pub fn write_in_place(path: &Path, text: &str) -> io::Result<()> {
    let mut f = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    f.set_len(0)?;
    f.write_all(text.as_bytes())?;
    f.flush()
}

/// The modification time in nanoseconds and the size: a coarse clock can repeat a timestamp.
pub fn stamp(path: &Path) -> Option<(u128, u64)> {
    let md = fs::metadata(path).ok()?;
    Some((mtime_ns(&md)?, md.len()))
}

pub fn mtime_ns(md: &fs::Metadata) -> Option<u128> {
    md.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_nanos())
}

pub fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `secrets.token_urlsafe(n)`: n random bytes as unpadded base64url.
pub fn token_urlsafe(nbytes: usize) -> String {
    let mut buf = vec![0u8; nbytes];
    getrandom::fill(&mut buf).expect("the operating system's random source failed");
    base64url(&buf)
}

pub fn base64url(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// `hmac.compare_digest`: the time taken doesn't depend on where the two differ.
pub fn compare_digest(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() != b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

/// `shutil.which`: a program on PATH (with PATHEXT on Windows).
pub fn which(name: &str) -> Option<PathBuf> {
    let candidates = |dir: &Path| -> Vec<PathBuf> {
        let base = dir.join(name);
        if cfg!(windows) && Path::new(name).extension().is_none() {
            let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
            exts.split(';').filter(|e| !e.is_empty()).map(|e| dir.join(format!("{name}{}", e.to_lowercase()))).collect()
        } else {
            vec![base]
        }
    };
    if name.contains('/') || name.contains('\\') {
        return candidates(Path::new("")).into_iter().find(|p| is_executable(p));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).flat_map(|d| candidates(&d)).find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Python's `round(x, digits)` for the values termwall rounds (ties to even).
pub fn round_to(x: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (x * f).round_ties_even() / f
}

/// A JSON number from a float, or null for NaN and infinities (which JSON has no word for).
pub fn num(x: f64) -> Value {
    serde_json::Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
}

/// `str(float)` / `repr(float)` in Python: always a decimal point for whole numbers.
pub fn py_float(x: f64) -> String {
    if x.is_nan() {
        "nan".into()
    } else if x.is_infinite() {
        if x > 0.0 { "inf".into() } else { "-inf".into() }
    } else if x.fract() == 0.0 && x.abs() < 1e16 {
        format!("{x:.1}")
    } else {
        format!("{x}")
    }
}

/// `str.splitlines()`: every line break Python knows, without the breaks.
pub fn splitlines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
            }
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `str.expandtabs(n)`: tabs to the next multiple of n columns, counting from each line start.
pub fn expandtabs(text: &str, n: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut col = 0;
    for c in text.chars() {
        match c {
            '\t' => {
                let pad = n - col % n;
                out.extend(std::iter::repeat_n(' ', pad));
                col += pad;
            }
            '\n' | '\r' => {
                out.push(c);
                col = 0;
            }
            _ => {
                out.push(c);
                col += 1;
            }
        }
    }
    out
}

/// `str.isprintable()`: no control, format, separator (other than the space) or private-use
/// characters. Close enough to Python's table for a one-line command.
pub fn is_printable(s: &str) -> bool {
    s.chars().all(|c| {
        if c == ' ' {
            return true;
        }
        let u = c as u32;
        let format = matches!(u, 0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x180E | 0x200B..=0x200F
            | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB);
        let private = matches!(u, 0xE000..=0xF8FF | 0xF0000..=0x10FFFF);
        !c.is_control() && !c.is_whitespace() && !format && !private
    })
}

/// Python's `parse_qs(urlsplit(path).query).get(name, [""])[0]`: the first non-empty value.
pub fn query_param(path_qs: &str, name: &str) -> String {
    let query = match path_qs.split_once('?') {
        Some((_, q)) => q.split('#').next().unwrap_or(""),
        None => return String::new(),
    };
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(k) == name && !v.is_empty() {
            return percent_decode(v);
        }
    }
    String::new()
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() => {
                let hex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
                out.push(hex(b[i + 1]) * 16 + hex(b[i + 2]));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Run a program and wait at most `timeout`; None when it can't start or runs too long.
pub fn run_with_timeout(cmd: &mut std::process::Command, timeout: Duration) -> Option<std::process::Output> {
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// No console window for a helper program (nvidia-smi, lspci) started from the GUI-less server.
pub fn hide_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// A temporary folder removed when dropped (the selftest's only need; no extra crate).
pub struct TempDir(PathBuf);

impl TempDir {
    /// Under the system's temp folder, or next to the binary when that can't be written (a
    /// sandboxed or packaged parent process can refuse new folders in %TEMP%). `cargo test`
    /// keeps its folders in target/test-tmp.
    pub fn new() -> io::Result<TempDir> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let name = format!("termwall-test-{}-{}-{nanos}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
        let mut bases = Vec::new();
        if cfg!(test) {
            bases.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("test-tmp"));
        }
        bases.push(std::env::temp_dir());
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            bases.push(dir.join("selftest-tmp"));
        }
        let mut last = io::Error::other("no folder for temporary files");
        for base in bases {
            let p = base.join(&name);
            match fs::create_dir_all(&p) {
                Ok(()) => return Ok(TempDir(p)),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn join(&self, p: impl AsRef<Path>) -> PathBuf {
        self.0.join(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
