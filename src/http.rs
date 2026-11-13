//! A small HTTP/1.0 server: GET only, one thread per connection, the connection closed after
//! each answer. termwall's two servers answer a handful of paths, so this is all they need.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub struct Request {
    pub method: String,
    /// The path with its query, as sent.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub peer: Option<IpAddr>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn path(&self) -> &str {
        self.target.split(['?', '#']).next().unwrap_or("")
    }
}

pub struct Response {
    pub code: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(code: u16) -> Response {
        Response { code, headers: Vec::new(), body: Vec::new() }
    }
    pub fn header(mut self, k: &str, v: &str) -> Response {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Response {
        self.body = body.into();
        self
    }
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        400 => "Bad Request",
        501 => "Unsupported method",
        _ => "",
    }
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.take(16 * 1024));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            break;
        }
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Some(Request { method, target, headers, peer: stream.peer_addr().ok().map(|a| a.ip()) })
}

fn answer(mut stream: TcpStream, handler: &(dyn Fn(&Request) -> Response + Send + Sync)) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let resp = match read_request(&stream) {
        Some(req) if req.method == "GET" => handler(&req),
        Some(_) => Response::new(501),
        None => return,
    };
    let mut head = format!("HTTP/1.0 {} {}\r\nServer: termwall/{}\r\n", resp.code, reason(resp.code), env!("CARGO_PKG_VERSION"));
    for (k, v) in &resp.headers {
        head += &format!("{k}: {v}\r\n");
    }
    head += &format!("Content-Length: {}\r\nConnection: close\r\n\r\n", resp.body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&resp.body);
    let _ = stream.flush();
}

pub type Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

/// Answer requests until `stop` turns true (None: forever).
pub fn serve(listener: TcpListener, handler: Handler, stop: Option<Arc<AtomicBool>>) {
    if stop.is_some() {
        let _ = listener.set_nonblocking(true);
    }
    loop {
        if stop.as_ref().is_some_and(|s| s.load(Ordering::Relaxed)) {
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let h = handler.clone();
                std::thread::spawn(move || answer(stream, &*h));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// A GET for the tests and `termwall share`: (status, body).
pub fn get(port: u16, path: &str, host: &str, timeout: Duration) -> std::io::Result<(u16, Vec<u8>)> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n")?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)?;
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..split]);
    let code = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    Ok((code, buf[split..].to_vec()))
}
