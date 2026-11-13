//! Everything outside termwall's own files: Wallpaper Engine and Lively, other running copies,
//! updates from GitHub and the editor for termwall.toml.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::paths::Paths;
use crate::style;
use crate::util::{self, Map, Value};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// ─── Wallpaper Engine ────────────────────────────────────────────────────────

/// Library folders from Steam's libraryfolders.vdf (every "path" entry).
pub fn steam_libraries(vdf_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = vdf_text;
    while let Some(i) = rest.find("\"path\"") {
        rest = &rest[i + 6..];
        let after = rest.trim_start();
        if after.len() == rest.len() || !after.starts_with('"') {
            continue; // "path" has to be followed by whitespace and a quoted value
        }
        let body = &after[1..];
        if let Some(end) = body.find('"') {
            if end > 0 {
                out.push(body[..end].replace("\\\\", "\\"));
            }
            rest = &body[end + 1..];
        }
    }
    out
}

/// Wallpaper Engine's projects/myprojects, found through Steam (registry, then libraries).
pub fn we_projects() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use crate::platform::reg;
        let steam = reg::read(reg::HKCU, r"Software\Valve\Steam", "SteamPath")?;
        let mut libs = vec![steam.clone()];
        if let Ok(b) = fs::read(Path::new(&steam).join("steamapps").join("libraryfolders.vdf")) {
            libs.extend(steam_libraries(&String::from_utf8_lossy(&b)));
        }
        for lib in libs {
            let we = Path::new(&lib).join("steamapps").join("common").join("wallpaper_engine");
            if we.join("wallpaper64.exe").is_file() {
                return Some(we.join("projects").join("myprojects"));
            }
        }
        None
    }
    #[cfg(not(windows))]
    None
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn lexists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// A junction projects/myprojects/termwall -> this folder, so Wallpaper Engine lists termwall and
/// plays this very copy. An existing termwall there that isn't ours is left alone.
pub fn link_we(here: &Path) -> String {
    let Some(projects) = we_projects() else {
        return "Wallpaper Engine not found — open index.html from this folder in your wallpaper app".into();
    };
    let link = projects.join("termwall");
    if lexists(&link) {
        if same_file(&link, here) {
            return format!("already linked: {}", link.display());
        }
        return format!("left alone: {} exists and isn't this install (your own termwall?)", link.display());
    }
    let _ = fs::create_dir_all(&projects);
    let mut cmd = Command::new("cmd");
    cmd.arg("/c").arg("mklink").arg("/J").arg(&link).arg(here);
    match util::hide_window(&mut cmd).output() {
        Ok(o) if o.status.success() => format!("linked {} -> {}", link.display(), here.display()),
        Ok(o) => format!("mklink failed: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
            .trim()
            .to_string(),
        Err(e) => format!("mklink failed: {e}"),
    }
}

pub fn unlink_we(here: &Path) -> String {
    let link = we_projects().map(|p| p.join("termwall"));
    let Some(link) = link.filter(|l| lexists(l)) else { return "nothing linked".into() };
    if !same_file(&link, here) {
        return format!("left alone: {} isn't this install", link.display());
    }
    match fs::remove_dir(&link) {
        // removes the junction itself, never what it points to
        Ok(()) => format!("unlinked {}", link.display()),
        Err(e) => format!("could not remove {}: {e}", link.display()),
    }
}

// ─── Lively Wallpaper ────────────────────────────────────────────────────────

pub const LIVELY_PKG: &str = "12030rocksdanister.LivelyWallpaper"; // the Microsoft Store build

fn read_json_sig(path: &Path) -> Option<Value> {
    let b = fs::read(path).ok()?;
    let s = String::from_utf8(b).ok()?;
    serde_json::from_str(s.strip_prefix('\u{feff}').unwrap_or(&s)).ok()
}

/// Lively's wallpapers folder on disk (the Store build's writes are redirected into its package
/// folder), or None when Lively hasn't run yet. `local` is %LOCALAPPDATA%.
pub fn lively_library(local: &Path) -> Option<PathBuf> {
    let virt = local.join("Lively Wallpaper");
    let mut real = None;
    let packages = local.join("Packages");
    if let Ok(rd) = fs::read_dir(&packages) {
        let mut dirs: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        dirs.sort();
        for d in dirs {
            let cand = packages.join(&d).join("LocalCache").join("Local").join("Lively Wallpaper");
            if d.starts_with(&format!("{LIVELY_PKG}_")) && cand.join("Settings.json").is_file() {
                real = Some(cand);
                break;
            }
        }
    }
    if real.is_none() && virt.join("Settings.json").is_file() {
        real = Some(virt.clone());
    }
    let real = real?;
    let wd = read_json_sig(&real.join("Settings.json"))
        .and_then(|v| v.get("WallpaperDir").and_then(Value::as_str).filter(|s| !s.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| virt.join("Library"));
    let (wd_s, virt_s) = (wd.to_string_lossy().to_lowercase(), virt.to_string_lossy().to_lowercase());
    if wd_s.starts_with(&virt_s) {
        let rel = wd.to_string_lossy()[virt_s.len()..].trim_start_matches(['\\', '/']).to_string();
        Some(real.join(rel).join("wallpapers"))
    } else {
        Some(wd.join("wallpapers"))
    }
}

pub fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default()
}

fn lively_info(here: &Path) -> Map {
    let v = serde_json::json!({
        "AppVersion": "1.0.0.0", "Title": "termwall", "Desc": "live system stats, fastfetch style",
        "Author": "PantoYT", "License": "MIT", "Contact": "https://github.com/PantoYT/termwall",
        "Type": 1, "FileName": here.join("index.html").to_string_lossy(), "Arguments": null, "IsAbsolutePath": true
    });
    v.as_object().cloned().unwrap_or_default()
}

fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) { a.to_lowercase().replace('/', "\\") == b.to_lowercase().replace('/', "\\") } else { a == b }
}

/// A Lively library entry (LivelyInfo.json, a web wallpaper) pointing at this folder's
/// index.html: termwall shows up in Lively after Lively's next start.
pub fn link_lively(local: &Path, here: &Path) -> String {
    let Some(entry) = lively_library(local).map(|l| l.join("termwall")) else {
        return "Lively not found (or never started) — nothing to do".into();
    };
    let info = lively_info(here);
    let f = entry.join("LivelyInfo.json");
    if f.exists() {
        match read_json_sig(&f) {
            Some(v) => {
                let theirs = v.get("FileName").and_then(Value::as_str).unwrap_or("");
                if !same_name(theirs, info["FileName"].as_str().unwrap_or("")) {
                    return format!("left alone: {} is another termwall", entry.display());
                }
            }
            None => return format!("left alone: {}", entry.display()),
        }
    }
    let _ = fs::create_dir_all(&entry);
    let text = serde_json::to_string_pretty(&info).unwrap_or_default() + "\n";
    match util::write_in_place(&f, &util::native_newlines(&text)) {
        Ok(()) => format!("added to Lively's library: {} (restart Lively to see it)", entry.display()),
        Err(e) => format!("could not write {}: {e}", f.display()),
    }
}

pub fn unlink_lively(local: &Path, here: &Path) -> String {
    let entry = lively_library(local).map(|l| l.join("termwall"));
    let Some(entry) = entry.filter(|e| e.join("LivelyInfo.json").exists()) else {
        return "nothing in Lively's library".into();
    };
    let ours = read_json_sig(&entry.join("LivelyInfo.json"))
        .and_then(|v| v.get("FileName").and_then(Value::as_str).map(String::from))
        .is_some_and(|n| same_name(&n, &here.join("index.html").to_string_lossy()));
    if !ours {
        return format!("left alone: {} isn't this install", entry.display());
    }
    let _ = fs::remove_dir_all(&entry);
    format!("removed {}", entry.display())
}

// ─── other running copies ───────────────────────────────────────────────────

/// Stop the other running copies of this termwall: this very binary, and the Python server of
/// the same folder (an update from the Python version replaces it). Returns how many.
pub fn stop_running(here: &Path) -> usize {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let Ok(mut sys) = System::new() else { return 0 };
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always).with_cmd(UpdateKind::Always),
    );
    let me = std::process::id();
    let my_exe = std::env::current_exe().ok().and_then(|p| fs::canonicalize(p).ok());
    let script = fs::canonicalize(here.join("termwall_api.py")).ok();
    let mut n = 0;
    for p in sys.processes().values() {
        if p.pid().as_u32() == me {
            continue;
        }
        let exe_match = my_exe.as_ref().is_some_and(|mine| p.exe().and_then(|e| fs::canonicalize(e).ok()).as_ref() == Some(mine));
        let py_match = script.as_ref().is_some_and(|s| {
            p.cmd().iter().skip(1).any(|a| {
                let a = a.to_string_lossy();
                a.ends_with(".py") && fs::canonicalize(a.as_ref()).ok().as_ref() == Some(s)
            })
        });
        if (exe_match || py_match) && p.kill() {
            n += 1;
        }
    }
    n
}

// ─── updates: only when asked, from GitHub's latest release ─────────────────

pub const RELEASES_API: &str = "https://api.github.com/repos/PantoYT/termwall/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/PantoYT/termwall/releases/latest";

/// "v1.10.0" -> [1, 10, 0]: versions compare as numbers.
pub fn version_tuple(v: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in v.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(cur.parse().unwrap_or(0));
            cur.clear();
        }
    }
    out.truncate(3);
    if out.is_empty() { vec![0] } else { out }
}

#[derive(Debug, PartialEq)]
pub struct Installer {
    pub name: String,
    pub url: Option<String>,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

/// The release's Windows installer: name, url, size and sha256 (when GitHub gives one).
pub fn pick_installer(release: &Value) -> Option<Installer> {
    for a in release.get("assets").and_then(Value::as_array)? {
        let name = a.get("name").and_then(Value::as_str).unwrap_or("");
        if name.starts_with("termwall-setup-") && name.ends_with(".exe") {
            let digest = a.get("digest").and_then(Value::as_str).unwrap_or("");
            return Some(Installer {
                name: name.into(),
                url: a.get("browser_download_url").and_then(Value::as_str).map(String::from),
                size: a.get("size").and_then(Value::as_u64),
                sha256: digest.strip_prefix("sha256:").map(String::from),
            });
        }
    }
    None
}

fn curl() -> Result<PathBuf, String> {
    util::which("curl").ok_or_else(|| "curl not found (it comes with Windows 10 and most Linux systems)".to_string())
}

pub fn latest_release(timeout_s: u64) -> Result<Value, String> {
    let mut cmd = Command::new(curl()?);
    cmd.args(["-fsSL", "--max-time", &timeout_s.to_string()])
        .args(["-H", "Accept: application/vnd.github+json", "-H", &format!("User-Agent: termwall/{VERSION}")])
        .arg(RELEASES_API);
    let out = util::run_with_timeout(util::hide_window(&mut cmd), std::time::Duration::from_secs(timeout_s + 2))
        .ok_or("no answer in time")?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

/// The latest release if it's newer than this copy, else None.
pub fn newer_release(timeout_s: u64) -> Result<Option<Value>, String> {
    let rel = latest_release(timeout_s)?;
    let tag = rel.get("tag_name").and_then(Value::as_str).unwrap_or("").to_string();
    Ok((version_tuple(&tag) > version_tuple(VERSION)).then_some(rel))
}

/// installer (the uninstaller sits next to the binary), git (a clone) or copy (anything else).
pub fn install_kind(here: &Path) -> &'static str {
    if here.join(".git").is_dir() {
        return "git";
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    if exe_dir.is_some_and(|d| d.join("unins000.exe").exists()) {
        return "installer";
    }
    "copy"
}

/// termwall update [--check]: the latest release, downloaded, checked and installed.
pub fn cmd_update(args: &[String], paths: &Paths) -> i32 {
    let rel = match newer_release(10) {
        Ok(Some(r)) => r,
        Ok(None) => {
            println!("termwall {VERSION} is the latest");
            return 0;
        }
        Err(e) => {
            println!("can't reach GitHub: {e}");
            return 1;
        }
    };
    let tag = rel.get("tag_name").and_then(Value::as_str).unwrap_or("?").trim_start_matches('v').to_string();
    let page = rel.get("html_url").and_then(Value::as_str).unwrap_or(RELEASES_PAGE);
    println!("termwall {tag} is out (this is {VERSION}): {page}");
    if args.iter().any(|a| a == "--check") {
        return 0;
    }
    let kind = install_kind(&paths.app_dir);
    if kind == "git" {
        println!(
            "this is a git clone: git pull (and cargo build --release), then restart the server (termwall stop, then termwall)"
        );
        return 0;
    }
    if paths.system_install {
        println!(
            "installed by your package manager: update it there (AUR: termwall), then systemctl --user restart termwall-api"
        );
        return 0;
    }
    let asset = pick_installer(&rel);
    let (Some(asset), true, "installer") = (asset, cfg!(windows), kind) else {
        println!("download it from {RELEASES_PAGE}");
        return 0;
    };
    let Some(url) = asset.url.clone() else {
        println!("download it from {RELEASES_PAGE}");
        return 0;
    };
    let dest = std::env::temp_dir().join(&asset.name);
    println!("downloading {} ({:.1} MB)", asset.name, asset.size.unwrap_or(0) as f64 / 1e6);
    let curl = match curl() {
        Ok(c) => c,
        Err(e) => {
            println!("download failed: {e}");
            return 1;
        }
    };
    let mut cmd = Command::new(curl);
    cmd.args(["-fL", "--max-time", "600", "-H", &format!("User-Agent: termwall/{VERSION}"), "-o"]).arg(&dest).arg(&url);
    match util::hide_window(&mut cmd).output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            println!("download failed: {}", String::from_utf8_lossy(&o.stderr).trim());
            return 1;
        }
        Err(e) => {
            println!("download failed: {e}");
            return 1;
        }
    }
    let Ok(data) = fs::read(&dest) else {
        println!("download failed: {} can't be read", dest.display());
        return 1;
    };
    if asset.size.is_some_and(|s| s != data.len() as u64) {
        println!("download incomplete; nothing installed");
        return 1;
    }
    if let Some(want) = &asset.sha256 {
        use sha2::{Digest, Sha256};
        let got: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
        if &got != want {
            let _ = fs::remove_file(&dest);
            println!("the download doesn't match the release's checksum; nothing installed");
            return 1;
        }
    }
    // The installer replaces this binary, so it runs on its own after we exit; it stops the
    // server, keeps termwall.toml and starts the new server when it's done.
    let mut cmd = Command::new(&dest);
    cmd.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    if let Err(e) = cmd.spawn() {
        println!("could not start the installer: {e}");
        return 1;
    }
    println!("installing {tag} in the background; the wallpaper comes back in about half a minute. Your settings stay.");
    0
}

// ─── termwall config: the editor ────────────────────────────────────────────

/// Split a command line like a shell (POSIX quoting; on Windows backslashes stay as they are).
fn split_command(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote, mut any) = (Vec::new(), String::new(), None::<char>, false);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') if !cfg!(windows) && matches!(chars.peek(), Some('"' | '\\' | '$' | '`')) => {
                cur.extend(chars.next());
            }
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, '\\') if !cfg!(windows) => cur.extend(chars.next()),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() || any {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    out
}

/// How to open a text file for editing: $TERMWALL_EDITOR / $EDITOR, else the app Windows
/// associates with the extension, else VS Code, else Notepad (Linux: xdg-open). None = the
/// association (ShellExecute).
pub fn editor_for(path: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<Vec<String>> {
    let p = path.to_string_lossy().into_owned();
    for var in ["TERMWALL_EDITOR", "EDITOR"] {
        if let Some(v) = env(var).filter(|v| !v.is_empty()) {
            let mut cmd = split_command(&v);
            cmd.push(p);
            return Some(cmd);
        }
    }
    if !cfg!(windows) {
        return Some(vec!["xdg-open".into(), p]);
    }
    #[cfg(windows)]
    {
        use crate::platform::reg;
        let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        // the user's own "open with" choice, or a registered type with an open command
        if reg::key_exists(reg::HKCU, &format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{ext}\UserChoice")) {
            return None;
        }
        if let Some(prog) = reg::read(reg::HKCR, &ext, "").filter(|p| !p.is_empty())
            && reg::key_exists(reg::HKCR, &format!(r"{prog}\shell\open\command"))
        {
            return None;
        }
    }
    Some(match util::which("code") {
        Some(code) => vec![code.to_string_lossy().into_owned(), p],
        None => vec!["notepad.exe".into(), p],
    })
}

/// Create termwall.toml if needed and open it in an editor.
pub fn open_config(paths: &Paths) -> String {
    let path = match style::ensure_config(&paths.style_file) {
        Ok(p) => p,
        Err(e) => return format!("{}\n(could not create it: {e})", paths.style_file.display()),
    };
    style::refresh_config(&path);
    let env = |k: &str| std::env::var(k).ok();
    let shown = path.display().to_string();
    let result = match editor_for(&path, &env) {
        None => crate::platform::shell_open(&path),
        Some(cmd) if std::env::var("EDITOR").is_ok_and(|e| !e.is_empty()) && !cfg!(windows) => {
            // a terminal editor takes this terminal
            Command::new(&cmd[0]).args(&cmd[1..]).status().map(|_| ())
        }
        Some(cmd) => {
            let mut c = Command::new(&cmd[0]);
            c.args(&cmd[1..]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            if cmd[0].to_lowercase().ends_with(".cmd") {
                util::hide_window(&mut c);
            }
            c.spawn().map(|_| ())
        }
    };
    match result {
        Ok(()) => shown,
        Err(e) => format!("{shown}\n(could not open an editor: {e}; set EDITOR, or open the file yourself)"),
    }
}
