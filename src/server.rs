//! The stats server: read-only JSON on 127.0.0.1:9002, never the LAN. Every request but
//! /health needs the token written to token.js at start.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::http::{self, Request, Response};
use crate::logos;
use crate::paths::{PAGE_ASSETS, Paths};
use crate::platform;
use crate::sampler::{Sampler, StatsSource};
use crate::share;
use crate::style;
use crate::theme;
use crate::util::{self, Map, Value};

pub const HOST: &str = "127.0.0.1";
pub const PORT: u16 = 9002;

/// 9002, or $TERMWALL_PORT (1024-65535) to run a second server next to a running one, for tests
/// and for a host that tells its page where the API is. Wallpaper Engine and Lively pages always
/// ask 9002.
pub fn port() -> u16 {
    std::env::var("TERMWALL_PORT").ok().and_then(|p| p.parse().ok()).filter(|p| *p >= 1024).unwrap_or(PORT)
}

/// A random token per start, written next to the page as token.js. The wallpaper (a local file)
/// can load it; a web page in a browser can't read your disk, so it can't call the API even
/// though the API answers on 127.0.0.1. Written in place, no temp file and rename.
pub fn write_token(path: &Path) -> std::io::Result<String> {
    let token = util::token_urlsafe(24);
    util::write_in_place(path, &util::native_newlines(&format!("window.TERMWALL_TOKEN = \"{token}\";\n")))?;
    Ok(token)
}

fn host_ok(host: Option<&str>) -> bool {
    let h = host.unwrap_or("").split(':').next().unwrap_or("");
    h == "127.0.0.1" || h == "localhost"
}

/// Token in ?t= and a loopback Host header (blocks DNS rebinding).
pub fn request_ok(path_qs: &str, host: Option<&str>, token: &str) -> bool {
    host_ok(host) && util::compare_digest(&util::query_param(path_qs, "t"), token)
}

/// The mtime of index.html: the page reloads itself when this changes, so edits show up without
/// re-adding the wallpaper.
pub fn page_version(paths: &Paths) -> Value {
    fs::metadata(paths.page_file()).ok().and_then(|m| util::mtime_ns(&m)).map(|n| Value::from(n as u64)).unwrap_or(Value::Null)
}

fn json(code: u16, body: &Map) -> Response {
    Response::new(code)
        .header("Content-Type", "application/json")
        .header("Access-Control-Allow-Origin", "*") // file:// is origin "null"; without the token only a 403 is readable
        .header("Cache-Control", "no-store")
        .body(serde_json::to_vec(body).unwrap_or_default())
}

pub fn make_handler(source: Arc<dyn StatsSource>, token: String, paths: Arc<Paths>) -> http::Handler {
    let ok = Arc::new(AtomicU64::new(0));
    let forbidden = Arc::new(AtomicU64::new(0));
    Arc::new(move |req: &Request| {
        let path = req.path();
        if path == "/health" && host_ok(req.header("Host")) {
            // "is it running?": counters only, so no token needed
            let body = serde_json::json!({"termwall": env!("CARGO_PKG_VERSION"), "ok": ok.load(Ordering::Relaxed), "forbidden": forbidden.load(Ordering::Relaxed)});
            return Response::new(200).header("Content-Type", "application/json").body(body.to_string());
        }
        if !request_ok(&req.target, req.header("Host"), &token) {
            forbidden.fetch_add(1, Ordering::Relaxed);
            return Response::new(403).header("Access-Control-Allow-Origin", "*"); // the page sees it and reloads
        }
        let screen = style::screen_of(&req.target);
        let st = || style::read_style(&paths.style_file, &paths.theme_file, screen.as_deref());
        let palette = |st: &Map| theme::resolve_palette(st, &paths.theme_file).map(Value::Object).unwrap_or(Value::Null);
        match path {
            "/logo-image" => match logos::logo_image_bytes(&paths) {
                Some((bytes, kind)) => {
                    Response::new(200).header("Access-Control-Allow-Origin", "*").header("Content-Type", kind).body(bytes)
                }
                None => Response::new(404).header("Access-Control-Allow-Origin", "*").header("Content-Type", "text/plain"),
            },
            "/stats" => {
                let st = st();
                let mut body = source.get();
                body.insert("theme".into(), palette(&st));
                body.insert("style".into(), Value::Object(st.clone()));
                body.insert("page".into(), page_version(&paths));
                body.extend(logos::page_extras(&st, &paths));
                ok.fetch_add(1, Ordering::Relaxed);
                json(200, &body)
            }
            "/theme" => {
                // tiny, polled often so palette switches land fast
                let st = st();
                let mut body = Map::new();
                body.insert("theme".into(), palette(&st));
                body.insert("style".into(), Value::Object(st));
                body.insert("page".into(), page_version(&paths));
                ok.fetch_add(1, Ordering::Relaxed);
                json(200, &body)
            }
            _ => Response::new(404),
        }
    })
}

/// A system package: links in data_dir to the packaged page, so the wallpaper loads it from there
/// with token.js beside it. Links that already point at the right file stay; returns the ones
/// made. Nothing to do when the page lives where it's written (clone, installer).
pub fn prepare_page_dir(app_dir: &Path, data_dir: &Path) -> std::io::Result<Vec<String>> {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    if abs(app_dir) == abs(data_dir) {
        return Ok(vec![]);
    }
    fs::create_dir_all(data_dir)?;
    let mut made = Vec::new();
    for name in PAGE_ASSETS {
        let (target, link) = (app_dir.join(name), data_dir.join(name));
        let md = fs::symlink_metadata(&link);
        if md.as_ref().is_ok_and(|m| m.file_type().is_symlink()) && fs::read_link(&link).ok().as_deref() == Some(target.as_path())
        {
            continue;
        }
        match md {
            Ok(m) if m.file_type().is_symlink() || m.is_file() => fs::remove_file(&link)?,
            Ok(m) if m.is_dir() => fs::remove_dir_all(&link)?,
            _ => {}
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link)?;
        #[cfg(windows)]
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(&target, &link)?;
        } else {
            std::os::windows::fs::symlink_file(&target, &link)?;
        }
        made.push(link.to_string_lossy().into_owned());
    }
    Ok(made)
}

/// Why the server didn't start.
pub enum StartError {
    /// Something already listens on the port: almost always termwall's own server.
    PortInUse(u16),
    Other(String),
}

/// Everything the server needs, started without blocking: the port is bound (before anything
/// else, so a second copy can't rewrite token.js), token.js written, the sampler and
/// `termwall share` running on threads of their own. The caller answers requests with
/// `http::serve(listener, handler, ...)`.
pub fn start(paths: &Paths) -> Result<(std::net::TcpListener, http::Handler), StartError> {
    let port = port();
    let addr = format!("{HOST}:{port}").parse().expect("a valid address");
    let listener = platform::bind_exclusive(addr).map_err(|_| StartError::PortInUse(port))?;
    if paths.system_install {
        let _ = fs::create_dir_all(&paths.config_dir);
        if let Err(e) = prepare_page_dir(&paths.app_dir, &paths.data_dir) {
            eprintln!("note: could not link the page into {}: {e}", paths.data_dir.display());
        }
    }
    let token = write_token(&paths.token_file)
        .map_err(|e| StartError::Other(format!("could not write {}: {e}", paths.token_file.display())))?;
    if let Err(e) = style::ensure_config(&paths.style_file).map(|p| style::refresh_config(&p)) {
        println!("note: could not write {}: {e}", paths.style_file.display()); // an update brings new choices
    }
    let mut sampler = Sampler::new().map_err(StartError::Other)?;
    sampler.sample();
    let snapshot = sampler.snapshot();
    std::thread::spawn(move || sampler.run());
    let source: Arc<dyn StatsSource> = Arc::new(snapshot);
    let shared = Arc::new(paths.clone());
    {
        let (source, shared) = (source.clone(), shared.clone());
        std::thread::spawn(move || share::run_share(source, shared)); // termwall share
    }
    Ok((listener, make_handler(source, token, shared)))
}

/// `termwall`: the server, until Ctrl+C.
pub fn serve(paths: &Paths) -> i32 {
    match start(paths) {
        Ok((listener, handler)) => {
            println!("termwall's server on http://{HOST}:{} - Ctrl+C stops it", port());
            http::serve(listener, handler, None);
            0
        }
        Err(StartError::PortInUse(port)) => {
            eprintln!("termwall's server is already running on {HOST}:{port} (check: http://{HOST}:{port}/health)");
            1
        }
        Err(StartError::Other(e)) => {
            eprintln!("{e}");
            1
        }
    }
}

/// The stats handler end to end (the Python selftest checked its parts, not the whole).
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    struct Fake;
    impl StatsSource for Fake {
        fn get(&self) -> Map {
            serde_json::json!({"user": "panto", "cpu": 12.5}).as_object().cloned().unwrap_or_default()
        }
    }

    #[test]
    fn stats_server_end_to_end() {
        let td = util::TempDir::new().expect("temp dir");
        let paths = Arc::new(Paths::in_dir(td.path()));
        let token = write_token(&paths.token_file).expect("token.js");
        let js = fs::read_to_string(&paths.token_file).expect("token.js");
        assert_eq!(util::universal_newlines(&js), format!("window.TERMWALL_TOKEN = \"{token}\";\n"));
        assert_eq!(token.len(), 32, "24 random bytes, base64url");
        style::set_style("theme", "gruvbox", &paths.style_file).expect("style");
        fs::OpenOptions::new()
            .append(true)
            .open(&paths.style_file)
            .and_then(|mut f| std::io::Write::write_all(&mut f, b"\n[screen.2]\nlayout = \"logo\"\n"))
            .expect("append");

        let listener = platform::bind_exclusive("127.0.0.1:0".parse().unwrap()).expect("bind");
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let handler = make_handler(Arc::new(Fake), token.clone(), paths.clone());
        let s = stop.clone();
        let srv = std::thread::spawn(move || http::serve(listener, handler, Some(s)));
        let host = format!("127.0.0.1:{port}");
        let get = |path: &str, h: &str| http::get(port, path, h, Duration::from_secs(5)).expect("request");
        let json = |b: &[u8]| serde_json::from_slice::<Value>(b).expect("json");

        let (code, body) = get("/health", &host);
        assert_eq!(code, 200);
        assert_eq!(json(&body)["termwall"], env!("CARGO_PKG_VERSION"));
        assert_eq!(get("/health", "evil.example").0, 403, "/health needs a loopback Host too");
        assert_eq!(get("/stats", &host).0, 403, "no token");

        let (code, body) = get(&format!("/stats?t={token}"), &host);
        let st = json(&body);
        assert_eq!(code, 200);
        assert_eq!(st["cpu"], 12.5, "the sampler's numbers");
        assert_eq!(st["style"]["layout"], "fetch");
        assert_eq!(st["theme"], Value::Object(theme::builtin("gruvbox").unwrap()), "the picked palette");
        assert!(st["page"].is_u64(), "index.html's mtime, for the page's self-reload");
        let (_, body) = get(&format!("/stats?t={token}&screen=2"), &format!("localhost:{port}"));
        assert_eq!(json(&body)["style"]["layout"], "logo", "[screen.2] for the monitor asking");

        let (code, body) = get(&format!("/theme?t={token}"), &host);
        let th = json(&body);
        assert_eq!(code, 200);
        assert!(th.get("cpu").is_none() && th.get("style").is_some() && th.get("theme").is_some(), "/theme is the small one");

        assert_eq!(get(&format!("/logo-image?t={token}"), &host).0, 404, "no logo picture yet");
        fs::write(paths.style_dir().join("logo.png"), crate::png::png(1, 1, &[vec![[1, 2, 3]]])).expect("png");
        let (code, body) = get(&format!("/logo-image?t={token}"), &host);
        assert!(code == 200 && body.starts_with(b"\x89PNG"), "the picture itself");
        assert_eq!(get(&format!("/nothing?t={token}"), &host).0, 404);

        let (_, body) = get("/health", &host);
        let h = json(&body);
        assert_eq!((h["ok"].as_u64(), h["forbidden"].as_u64()), (Some(3), Some(2)), "/health counts what got through");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = srv.join();
    }
}
