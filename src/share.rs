//! termwall share: the page on a phone (or any screen in the house).
//!
//! A second, read-only server on the LAN (0.0.0.0:9012). It serves the page itself, so a phone
//! needs nothing but its browser: scan the QR code, add the page to the home screen. The link
//! carries a random key (share.json; termwall share new-key replaces it). User name, computer
//! name and local IP never leave the PC on this server, and the Host header has to be an IP
//! address (a web page can't point a name at it).

use std::collections::HashMap;
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::http::{self, Request, Response};
use crate::logos;
use crate::paths::Paths;
use crate::platform;
use crate::png::png;
use crate::qr::{qr_matrix, qr_text};
use crate::sampler::{self, StatsSource};
use crate::server::page_version;
use crate::style;
use crate::theme;
use crate::util::{self, Map, Value};

pub const SHARE_PORT: u16 = 9012;

#[derive(Clone, Debug, PartialEq)]
pub struct ShareConfig {
    pub enabled: bool,
    pub key: Option<String>,
    pub port: u16,
}

pub fn read_share(path: &Path) -> ShareConfig {
    let raw = util::read_json(path).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let port = raw.get("port").and_then(Value::as_i64).filter(|p| (1024..=65535).contains(p)).map(|p| p as u16);
    ShareConfig {
        enabled: raw.get("enabled") == Some(&Value::Bool(true)),
        key: raw.get("key").and_then(Value::as_str).filter(|k| k.chars().count() >= 12).map(String::from),
        port: port.unwrap_or(SHARE_PORT),
    }
}

pub fn write_share(cfg: &Map, path: &Path) -> std::io::Result<()> {
    util::write_in_place(path, &util::native_newlines(&(serde_json::to_string_pretty(cfg).unwrap_or_default() + "\n")))
}

fn config_map(cfg: &ShareConfig) -> Map {
    let mut m = Map::new();
    m.insert("enabled".into(), Value::from(cfg.enabled));
    m.insert("key".into(), cfg.key.clone().map(Value::from).unwrap_or(Value::Null));
    m.insert("port".into(), Value::from(cfg.port));
    m
}

pub fn share_url(ip: &str, cfg: &ShareConfig) -> String {
    format!("http://{ip}:{}/?k={}", cfg.port, cfg.key.as_deref().unwrap_or(""))
}

/// index.html for a phone: the key instead of token.js, the API on this server, home-screen
/// tags (manifest, fullscreen, theme color).
pub fn share_page(key: &str, page_file: &Path) -> std::io::Result<Vec<u8>> {
    let html = fs::read_to_string(page_file)?;
    let head = format!(
        "<link rel=\"manifest\" href=\"manifest.webmanifest?k={key}\">\
<meta name=\"mobile-web-app-capable\" content=\"yes\">\
<meta name=\"apple-mobile-web-app-capable\" content=\"yes\">\
<meta name=\"apple-mobile-web-app-status-bar-style\" content=\"black\">\
<link rel=\"apple-touch-icon\" href=\"icon-192.png\">\
<meta name=\"referrer\" content=\"no-referrer\">"
    );
    let html = html.replacen("</head>", &(head + "</head>"), 1);
    let key_js = serde_json::to_string(key).unwrap_or_default();
    let html = html.replacen(
        "<script src=\"token.js\"></script>",
        &format!(
            "<script>window.TERMWALL_TOKEN = {key_js}; window.TERMWALL_API = \"stats\"; window.TERMWALL_SHARE = true;</script>"
        ),
        1,
    );
    Ok(html.into_bytes())
}

pub fn share_manifest(key: &str, theme: Option<&Map>) -> Vec<u8> {
    let bg = theme.and_then(|t| t.get("bg")).and_then(Value::as_str).unwrap_or("#171717");
    serde_json::json!({
        "name": "termwall", "short_name": "termwall", "start_url": format!("/?k={key}"), "scope": "/",
        "display": "fullscreen", "orientation": "any", "background_color": bg, "theme_color": bg,
        "icons": [{"src": "icon-192.png", "sizes": "192x192", "type": "image/png"},
                  {"src": "icon-512.png", "sizes": "512x512", "type": "image/png"}],
    })
    .to_string()
    .into_bytes()
}

fn hex_rgb(h: &str) -> [u8; 3] {
    let p = |i: usize| h.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).unwrap_or(0);
    [p(1), p(3), p(5)]
}

/// The home-screen icon: a prompt ">_" in the accent color on the background.
pub fn share_icon(size: usize, theme: Option<&Map>) -> Vec<u8> {
    type Cache = HashMap<(usize, String, String), Vec<u8>>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let mint = theme::builtin("mint").unwrap_or_default();
    let t = theme.unwrap_or(&mint);
    let color = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("#000000").to_string();
    let key = (size, color("bg"), color("accent"));
    if let Some(hit) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(&key) {
        return hit.clone();
    }
    let (bg, fg) = (hex_rgb(&key.1), hex_rgb(&key.2));
    let s = size as f64 / 100.0; // drawn on a 100x100 grid
    let seg = |px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64| {
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
    };
    let rows: Vec<Vec<[u8; 3]>> = (0..size)
        .map(|y| {
            (0..size)
                .map(|x| {
                    let (u, v) = (x as f64 / s, y as f64 / s);
                    let on = seg(u, v, 24.0, 30.0, 46.0, 50.0).min(seg(u, v, 46.0, 50.0, 24.0, 70.0)) < 6.5
                        || ((52.0..=78.0).contains(&u) && (63.0..=72.0).contains(&v));
                    if on { fg } else { bg }
                })
                .collect()
        })
        .collect();
    let img = png(size, size, &rows);
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, img.clone());
    img
}

fn send(code: u16, body: impl Into<Vec<u8>>, ctype: &str, cache: &str) -> Response {
    Response::new(code)
        .header("Content-Type", ctype)
        .header("Cache-Control", cache)
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .body(body)
}

fn text(code: u16, body: &str) -> Response {
    send(code, body.as_bytes().to_vec(), "text/plain; charset=utf-8", "no-store")
}

/// A font the page asks for: "fonts/NAME.woff2" with a plain file name (letters, digits, - _ .).
/// Anything else is refused: a drive or UNC path in NAME would make Path::join leave the fonts
/// folder (`fonts/C:\x.woff2`; `fonts/\\host\x.woff2` would even open an SMB connection).
fn font_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("fonts/")?;
    let plain = name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (plain && name.ends_with(".woff2") && !name.starts_with('.') && !name.contains("..")).then_some(name)
}

pub fn make_share_handler(source: Arc<dyn StatsSource>, key: String, paths: Arc<Paths>) -> http::Handler {
    let fails: Arc<Mutex<HashMap<IpAddr, (u32, f64)>>> = Arc::new(Mutex::new(HashMap::new())); // 20 wrong keys in 10 min = blocked
    Arc::new(move |req: &Request| {
        let host = req.header("Host").unwrap_or("");
        let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host).trim_matches(['[', ']']);
        if host.parse::<IpAddr>().is_err() {
            return text(403, "open termwall by the address termwall share shows\n"); // an IP, never a name
        }
        let path = req.path().trim_start_matches('/');
        if path == "ping" {
            return text(200, "termwall");
        }
        let font = font_name(path);
        if path == "logos.js" || font.is_some() {
            let file = match font {
                Some(name) => paths.app_dir.join("fonts").join(name),
                None => paths.app_dir.join("logos.js"),
            };
            return match fs::read(file) {
                Ok(body) => {
                    let kind = if font.is_some() { "font/woff2" } else { "text/javascript; charset=utf-8" };
                    send(200, body, kind, "public, max-age=3600")
                }
                Err(_) => text(404, ""),
            };
        }
        let ip = req.peer.unwrap_or(IpAddr::from([0, 0, 0, 0]));
        let now = util::now_secs();
        {
            let f = fails.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((n, t0)) = f.get(&ip)
                && *n >= 20
                && now - t0 < 600.0
            {
                return text(429, "too many wrong keys, try again in 10 minutes\n");
            }
        }
        let k = util::query_param(&req.target, "k");
        let sent = if k.is_empty() { util::query_param(&req.target, "t") } else { k };
        if !util::compare_digest(&sent, &key) {
            let mut f = fails.lock().unwrap_or_else(|e| e.into_inner());
            let e = f.entry(ip).or_insert((0, now));
            if now - e.1 >= 600.0 {
                *e = (0, now);
            }
            e.0 += 1;
            return text(403, "this link's key is wrong or old: termwall share shows the current one\n");
        }
        let mut st = style::read_style(&paths.style_file, &paths.theme_file, None);
        st.insert("private".into(), Value::Bool(true));
        let theme = theme::resolve_palette(&st, &paths.theme_file);
        match path {
            "" => match share_page(&key, &paths.page_file()) {
                Ok(body) => send(200, body, "text/html; charset=utf-8", "no-store"),
                Err(_) => text(404, ""),
            },
            "manifest.webmanifest" => send(200, share_manifest(&key, theme.as_ref()), "application/manifest+json", "no-store"),
            "icon-192.png" | "icon-512.png" => {
                let size = path[5..8].parse().unwrap_or(192);
                send(200, share_icon(size, theme.as_ref()), "image/png", "public, max-age=3600")
            }
            "logo-image" => match logos::logo_image_bytes(&paths) {
                Some((bytes, kind)) => send(200, bytes, kind, "no-store"),
                None => text(404, ""),
            },
            "stats" | "theme" => {
                let mut body = Map::new();
                if path == "stats" {
                    // nothing that names you or your network
                    body = source.get();
                    body.insert("user".into(), Value::from("user"));
                    body.insert("host".into(), Value::from("computer"));
                    body.insert("local_ip".into(), Value::from("hidden"));
                }
                body.insert("theme".into(), theme.clone().map(Value::Object).unwrap_or(Value::Null));
                body.insert("style".into(), Value::Object(st.clone()));
                body.insert("page".into(), page_version(&paths));
                if path == "stats" {
                    body.extend(logos::page_extras(&st, &paths));
                }
                send(200, serde_json::to_vec(&body).unwrap_or_default(), "application/json", "no-store")
            }
            _ => text(404, ""),
        }
    })
}

/// Watch share.json; start or stop the LAN server when it changes (termwall share on/off works
/// without restarting termwall).
pub fn run_share(source: Arc<dyn StatsSource>, paths: Arc<Paths>) {
    let mut running: Option<(u16, String)> = None;
    let mut stop: Option<Arc<AtomicBool>> = None;
    loop {
        let cfg = read_share(&paths.share_file);
        let want = match (&cfg.key, cfg.enabled) {
            (Some(k), true) => Some((cfg.port, k.clone())),
            _ => None,
        };
        if want != running {
            if let Some(s) = stop.take() {
                s.store(true, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(150)); // the old listener closes
            }
            if let Some((port, key)) = &want {
                match platform::bind_exclusive(std::net::SocketAddr::from(([0, 0, 0, 0], *port))) {
                    Ok(listener) => {
                        let flag = Arc::new(AtomicBool::new(false));
                        let handler = make_share_handler(source.clone(), key.clone(), paths.clone());
                        let f = flag.clone();
                        std::thread::spawn(move || http::serve(listener, handler, Some(f)));
                        stop = Some(flag);
                        println!("share: on, port {port}");
                    }
                    Err(e) => println!("share: port {port} can't be used ({e})"),
                }
            }
            running = want;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

pub fn cmd_share(args: &[String], paths: &Paths) -> i32 {
    let mut cfg = read_share(&paths.share_file);
    let sub = args.first().map(String::as_str).unwrap_or("on");
    if sub == "off" {
        cfg.enabled = false;
        if let Err(e) = write_share(&config_map(&cfg), &paths.share_file) {
            eprintln!("could not write {}: {e}", paths.share_file.display());
            return 1;
        }
        println!("share: off (the LAN server stops within a few seconds)");
        return 0;
    }
    if !["on", "new-key", "show"].contains(&sub) {
        println!("termwall share [on | off | new-key | show]");
        return 2;
    }
    if sub == "new-key" || cfg.key.is_none() {
        cfg.key = Some(util::token_urlsafe(12));
        if sub == "new-key" {
            println!("new key: links and home-screen icons made before stop working");
        }
    }
    if sub != "show" {
        cfg.enabled = true;
    }
    if let Err(e) = write_share(&config_map(&cfg), &paths.share_file) {
        eprintln!("could not write {}: {e}", paths.share_file.display());
        return 1;
    }
    if !cfg.enabled {
        println!("share is off: termwall share on");
        return 1;
    }
    let mut up = false;
    for _ in 0..10 {
        // the running server picks share.json up within 2 s
        match http::get(cfg.port, "/ping", "127.0.0.1", Duration::from_secs(1)) {
            Ok((_, body)) => {
                up = body == b"termwall";
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
    let ips = sampler::lan_addresses();
    let Some((first, _)) = ips.first() else {
        println!("no network address found");
        return 1;
    };
    match qr_matrix(&share_url(first, &cfg)) {
        Ok(m) => println!("{}", qr_text(&m)),
        Err(e) => println!("({e})"),
    }
    println!();
    for (ip, what) in &ips {
        println!("  {}   ({what})", share_url(ip, &cfg));
    }
    println!("\nScan it with the phone (same Wi-Fi), then the browser menu: Add to Home screen.");
    println!("Only stats and the look go out: no user name, computer name or IP.");
    if !up {
        println!("\nnote: termwall's server isn't answering on that port. Is it running (termwall)?");
    }
    if cfg!(windows) {
        println!(
            "If the phone can't open it: Windows Firewall has to allow termwall on private networks \
(it asks once; or allow it in Windows Security > Firewall > Allow an app)."
        );
    }
    0
}

#[cfg(test)]
mod tests {
    use super::font_name;

    #[test]
    fn fonts_only_by_plain_name() {
        assert_eq!(font_name("fonts/FiraCode-Regular.woff2"), Some("FiraCode-Regular.woff2"));
        for bad in [
            r"fonts/C:\Windows\x.woff2",
            r"fonts/\host\share\x.woff2",
            r"fonts/\Windows\x.woff2",
            "fonts/C:x.woff2",
            "fonts/../share.json",
            "fonts/..woff2",
            "fonts/sub/x.woff2",
            "fonts/x.woff2.json",
            "fonts/.woff2",
            "fonts/x%2e.woff2",
            "logos.js",
        ] {
            assert_eq!(font_name(bad), None, "{bad}");
        }
    }
}
