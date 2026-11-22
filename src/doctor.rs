//! `termwall doctor`: what runs, what starts at logon, what the wallpaper hears, and whether
//! those fit together. Facts are gathered first (`gather`), then judged in `judge`, which the
//! tests feed with made-up facts. Plain text; exit 1 when something is really wrong.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::host::{self, Device};
use crate::integrate::{self, VERSION};
use crate::paths::Paths;
use crate::style::{self, Layers};
use crate::util::{Map, Value};

/// The installer's id (installer/termwall.iss AppId), under which Windows keeps its uninstall entry.
#[cfg_attr(not(windows), allow(dead_code))]
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{6F1D2C0B-8E54-4C47-9B6A-3C2E7D9F4A11}_is1";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub exe: Option<PathBuf>,
    pub cmd: Vec<String>,
}

impl Proc {
    fn stem(&self) -> String {
        self.exe.as_ref().and_then(|e| e.file_stem()).map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
    }

    /// termwall_api.py on its command line (the Python server, 2.x).
    pub fn script(&self) -> Option<&str> {
        self.cmd.iter().skip(1).map(String::as_str).find(|a| a.to_lowercase().ends_with("termwall_api.py"))
    }

    pub fn is_server(&self) -> bool {
        let rust = (self.stem() == "termwall" || self.stem() == "termwallw") && self.cmd.len() <= 1;
        rust || (self.script().is_some() && self.cmd.iter().skip(2).all(|a| !a.starts_with('-') || a == "--serve"))
    }

    pub fn is_host(&self) -> bool {
        (self.stem() == "termwall" || self.stem() == "termwallw")
            && self.cmd.get(1).is_some_and(|a| a == "host" || a == "--host")
            && !self.cmd.iter().any(|a| a == "on" || a == "off")
    }

    /// The folder it serves from: the exe's, or the script's.
    pub fn home(&self) -> Option<PathBuf> {
        match self.script() {
            Some(s) => Path::new(s).parent().map(Path::to_path_buf),
            None => self.exe.as_ref().and_then(|e| e.parent().map(Path::to_path_buf)),
        }
    }

    fn shown(&self) -> String {
        match (self.script(), &self.exe) {
            (Some(s), _) => format!("pid {} {} (Python, termwall 2.x)", self.pid, s),
            (None, Some(e)) => format!("pid {} {}", self.pid, e.display()),
            (None, None) => format!("pid {} {}", self.pid, self.name),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Health {
    pub version: String,
    pub forbidden: u64,
    /// the same counter a moment later: rising means the wallpaper asks with another token
    pub forbidden_later: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub this_exe: Option<PathBuf>,
    pub port: u16,
    pub listener: Option<u32>,
    pub procs: Vec<Proc>,
    pub health: Option<Health>,
    /// the installer's copy: its version and folder
    pub installed: Option<(String, PathBuf)>,
    /// HKCU Run "termwall"
    pub run_entry: Option<String>,
    /// the host's autostart command
    pub host_entry: Option<String>,
    /// logon entries that start the Python server: (where, what)
    pub python_logon: Vec<(String, String)>,
    /// None: not looked at (not Windows)
    pub webview2: Option<Result<String, String>>,
    pub we_page: Option<PathBuf>,
    pub we_audio: Option<String>,
    pub devices: Vec<Device>,
    pub host_audio: Option<String>,
    pub visualizer_on: bool,
    pub config_problems: Vec<String>,
    pub livery_overrides: Map,
    /// your own command blocks, when experimental_commands is on
    pub command_blocks: Option<Vec<String>>,
    pub windows: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Note,
    Warn,
    Fail,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub part: &'static str,
    pub level: Level,
    pub text: String,
    pub fix: Option<String>,
}

fn f(part: &'static str, level: Level, text: impl Into<String>, fix: Option<String>) -> Finding {
    Finding { part, level, text: text.into(), fix }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    integrate::same_file(a, b) || a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

/// What the facts mean, in the order they are printed.
pub fn judge(x: &Facts) -> Vec<Finding> {
    let mut out = Vec::new();
    let installed_dir = x.installed.as_ref().map(|(_, d)| d.clone());
    let holder = x.listener.filter(|p| *p != 0).and_then(|pid| x.procs.iter().find(|p| p.pid == pid));
    let servers: Vec<&Proc> = x.procs.iter().filter(|p| p.is_server()).collect();
    // the listener when the system names it, else the only server running
    let serving = holder.or(if servers.len() == 1 { Some(servers[0]) } else { None });

    // ── the server
    match (&x.health, serving) {
        (None, _) => out.push(f(
            "server",
            Level::Fail,
            format!("nothing answers on 127.0.0.1:{}: the wallpaper shows \"api offline\"", x.port),
            Some(match &installed_dir {
                Some(d) => format!("start it: \"{}\" (it also starts at every logon)", d.join("termwallw.exe").display()),
                None => "start it: termwall".into(),
            }),
        )),
        (Some(h), p) => {
            let who = p.map(Proc::shown).unwrap_or_else(|| "a process this one can't see".into());
            out.push(f("server", Level::Ok, format!("127.0.0.1:{} answers: termwall {}, {who}", x.port, h.version), None));
            if let Some((iv, idir)) = &x.installed {
                if let Some(p) = p
                    && p.script().is_some()
                {
                    out.push(f(
                        "server",
                        Level::Fail,
                        format!("the Python server holds the port, not the installed termwall {iv}"),
                        Some(format!(
                            "stop pid {} and start \"{}\"; see below for what starts it at logon",
                            p.pid,
                            idir.join("termwallw.exe").display()
                        )),
                    ));
                } else if &h.version != iv {
                    out.push(f(
                        "server",
                        Level::Fail,
                        format!("running termwall {}, installed {iv}", h.version),
                        Some(format!("stop the old one and start \"{}\"", idir.join("termwallw.exe").display())),
                    ));
                } else if let Some(home) = p.and_then(Proc::home)
                    && !same_dir(&home, idir)
                {
                    out.push(f(
                        "server",
                        Level::Warn,
                        format!("the server runs from {}, not from the installed copy {}", home.display(), idir.display()),
                        Some("each copy has its own token.js: the wallpaper only works with the one it was linked to".into()),
                    ));
                }
            }
            if h.forbidden_later > h.forbidden {
                out.push(f(
                    "server",
                    Level::Fail,
                    format!(
                        "requests are being refused ({} in the last moment): a wallpaper asks with another copy's token.js and shows \"connecting\"",
                        h.forbidden_later - h.forbidden
                    ),
                    Some("run termwall --link-we (or --link-lively) from the copy that runs, or start the copy the wallpaper was linked to".into()),
                ));
            }
        }
    }
    if servers.len() > 1 {
        let list: Vec<String> = servers.iter().map(|p| p.shown()).collect();
        out.push(f(
            "server",
            Level::Warn,
            format!("{} servers are running: {}", servers.len(), list.join("; ")),
            Some("only one gets the port; stop the others".into()),
        ));
    }
    if let (Some(page), Some(home)) = (&x.we_page, serving.and_then(Proc::home))
        && !same_dir(page, &home)
    {
        out.push(f(
            "server",
            Level::Fail,
            format!("Wallpaper Engine shows the page from {}, the server runs from {}", page.display(), home.display()),
            Some(format!("termwall --link-we from {} (the tokens differ)", home.display())),
        ));
    }

    // ── what starts at logon
    if x.windows {
        match &x.run_entry {
            Some(cmd) => {
                let exe = host::split_command_line(cmd).into_iter().next().unwrap_or_default();
                if Path::new(&exe).is_file() {
                    out.push(f("logon", Level::Ok, format!("the server starts at logon: {cmd}"), None));
                } else {
                    out.push(f(
                        "logon",
                        Level::Fail,
                        format!("the logon entry points at a file that isn't there: {cmd}"),
                        Some("reinstall termwall, or remove the \"termwall\" value under HKCU\\...\\CurrentVersion\\Run".into()),
                    ));
                }
            }
            None if x.installed.is_some() => out.push(f(
                "logon",
                Level::Warn,
                "the installed termwall does not start at logon (no \"termwall\" Run entry)",
                Some("reinstall, or add it: termwallw.exe in HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run".into()),
            )),
            None => {}
        }
        for (place, what) in &x.python_logon {
            let level = if x.installed.is_some() { Level::Fail } else { Level::Note };
            let fix = if place.starts_with("task ") {
                format!("schtasks /Change /TN \"{}\" /DISABLE", place.trim_start_matches("task "))
            } else {
                format!("remove {place}")
            };
            out.push(f(
                "logon",
                level,
                format!("{place} starts the Python server ({what}); after a reboot it takes the port first"),
                (level == Level::Fail).then_some(fix),
            ));
        }
    }

    // ── the host
    if x.windows {
        let hosts: Vec<&Proc> = x.procs.iter().filter(|p| p.is_host()).collect();
        match &x.host_entry {
            Some(cmd) => {
                out.push(f("host", Level::Ok, format!("termwall host starts at logon: {cmd}"), None));
                if hosts.is_empty() {
                    out.push(f(
                        "host",
                        Level::Warn,
                        "termwall host is set to start at logon but isn't running",
                        Some("termwall host on starts it now".into()),
                    ));
                }
            }
            None => out.push(f(
                "host",
                Level::Note,
                if hosts.is_empty() {
                    "termwall host is off (Wallpaper Engine or Lively shows the page)".to_string()
                } else {
                    format!("termwall host runs ({}) but doesn't start at logon", hosts[0].shown())
                },
                None,
            )),
        }
        match &x.webview2 {
            None => {}
            Some(Ok(v)) => out.push(f("host", Level::Ok, format!("WebView2 runtime {v}"), None)),
            Some(Err(e)) => out.push(f(
                "host",
                if x.host_entry.is_some() { Level::Fail } else { Level::Note },
                format!("no WebView2 runtime ({e}): termwall host can't show the page"),
                Some("install it from https://developer.microsoft.com/microsoft-edge/webview2/".into()),
            )),
        }
    }

    // ── settings
    if x.config_problems.is_empty() {
        out.push(f("settings", Level::Ok, "termwall.toml is fine", None));
    } else {
        for p in &x.config_problems {
            out.push(f(
                "settings",
                Level::Fail,
                format!("termwall.toml: {p}"),
                Some("termwall settings, or edit it: termwall config".into()),
            ));
        }
    }
    if !x.livery_overrides.is_empty() {
        let list: Vec<String> =
            x.livery_overrides.iter().map(|(k, v)| format!("{k} = {}", style::toml_value(v).unwrap_or_default())).collect();
        out.push(f(
            "settings",
            Level::Note,
            format!("livery's pack sets {} and wins over termwall.toml", list.join(", ")),
            Some("livery termwall look PACK none drops it".into()),
        ));
    }

    if let Some(names) = &x.command_blocks {
        out.push(f(
            "settings",
            Level::Note,
            if names.is_empty() {
                "experimental_commands is on, no command blocks yet".to_string()
            } else {
                format!("experimental_commands is on: {} run as you ({})", names.len(), names.join(", "))
            },
            Some("experimental_commands = false in termwall.toml stops them".into()),
        ));
    }

    // ── the visualizer
    if x.windows && !x.devices.is_empty() {
        let default = x.devices.iter().find(|d| d.default);
        let playing: Vec<&Device> = x.devices.iter().filter(|d| d.plays()).collect();
        let mut others = 0;
        for d in &x.devices {
            let marks = [(d.default, "Windows' default"), (d.plays(), "playing now")];
            let marks: Vec<&str> = marks.iter().filter(|(on, _)| *on).map(|(_, m)| *m).collect();
            if marks.is_empty() {
                others += 1;
            } else {
                out.push(f("music", Level::Note, format!("output: {} ({})", d.name, marks.join(", ")), None));
            }
        }
        if others > 0 {
            out.push(f("music", Level::Note, format!("{others} other output(s), silent"), None));
        }
        // with the visualizer off nothing is broken, but turning it on would show stillness
        let bad = if x.visualizer_on { Level::Fail } else { Level::Note };
        if !x.visualizer_on {
            out.push(f("music", Level::Ok, "visualizer is off on every screen: nothing needs the music", None));
        }
        let we_runs = x.procs.iter().any(|p| {
            let n = p.name.to_lowercase();
            n.starts_with("wallpaper64") || n.starts_with("wallpaper32")
        });
        let host_runs = x.procs.iter().any(Proc::is_host);
        if playing.is_empty() {
            out.push(f("music", Level::Note, "nothing plays right now; play some music and run termwall doctor again", None));
        } else {
            let loudest =
                playing.iter().max_by(|a, b| a.peak.total_cmp(&b.peak)).map(|d| d.name.as_str()).unwrap_or("another device");
            if we_runs {
                let we = x.we_audio.as_deref().unwrap_or("default");
                let hears = if we == "default" { default.map(|d| d.name.clone()) } else { Some(we.to_string()) };
                let hears_playing = hears
                    .as_ref()
                    .is_some_and(|h| playing.iter().any(|d| d.name.eq_ignore_ascii_case(h) || d.name.contains(h.as_str())));
                if hears_playing {
                    out.push(f(
                        "music",
                        Level::Ok,
                        format!("Wallpaper Engine hears {}, where the music plays", hears.unwrap_or_default()),
                        None,
                    ));
                } else {
                    out.push(f(
                        "music",
                        bad,
                        format!(
                            "the music plays on {loudest}, Wallpaper Engine listens to {} (its audio device: {we}): its visualizer stays still",
                            hears.as_deref().unwrap_or("nothing")
                        ),
                        Some(
                            "make that device Windows' default output, or pick it in Wallpaper Engine's settings, or use termwall host, which follows the sound"
                                .into(),
                        ),
                    ));
                }
            }
            if host_runs || x.host_entry.is_some() {
                match &x.host_audio {
                    None => out.push(f("music", Level::Ok, "termwall host follows the sound (no --audio-device)", None)),
                    Some(want) => {
                        let w = want.to_lowercase();
                        if playing.iter().any(|d| d.name.to_lowercase().contains(&w)) {
                            out.push(f(
                                "music",
                                Level::Ok,
                                format!("termwall host listens to \"{want}\", where the music plays"),
                                None,
                            ));
                        } else {
                            out.push(f(
                                "music",
                                bad,
                                format!("termwall host is fixed to \"{want}\" but the music plays on {loudest}"),
                                Some(
                                    "termwall host on without --audio-device follows the sound (or termwall settings, host)"
                                        .into(),
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }
    out
}

/// Plain text, grouped by part; the fix under each finding that has one.
pub fn render(findings: &[Finding], this: &str) -> String {
    let mut out = format!("termwall doctor ({this})\n");
    let mut part = "";
    for x in findings {
        if x.part != part {
            part = x.part;
            out += &format!("\n{part}\n");
        }
        let tag = match x.level {
            Level::Ok => "ok  ",
            Level::Note => "    ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        out += &format!("  {tag}  {}\n", x.text);
        if let Some(fix) = &x.fix {
            out += &format!("        fix: {fix}\n");
        }
    }
    let fails = findings.iter().filter(|x| x.level == Level::Fail).count();
    let warns = findings.iter().filter(|x| x.level == Level::Warn).count();
    out += &match (fails, warns) {
        (0, 0) => "\nall fine\n".to_string(),
        (0, w) => format!("\n{w} warning(s), nothing broken\n"),
        (n, _) => format!("\n{n} problem(s)\n"),
    };
    out
}

// ─── gathering ───────────────────────────────────────────────────────────────

fn processes() -> Vec<Proc> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let Ok(mut sys) = System::new() else { return Vec::new() };
    let kind = ProcessRefreshKind::nothing().with_exe(UpdateKind::Always).with_cmd(UpdateKind::Always);
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    let me = std::process::id();
    let mut out: Vec<Proc> = sys
        .processes()
        .values()
        .filter(|p| p.pid().as_u32() != me && p.thread_kind().is_none())
        .map(|p| Proc {
            pid: p.pid().as_u32(),
            name: p.name().to_string_lossy().into_owned(),
            exe: p.exe().map(Path::to_path_buf),
            cmd: p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect(),
        })
        .collect();
    out.sort_by_key(|p| p.pid);
    out
}

/// GET on the server, the body as JSON.
fn get_json(port: u16, path: &str) -> Option<Value> {
    use std::io::{Read, Write};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").ok()?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    serde_json::from_str(text.split_once("\r\n\r\n")?.1.trim()).ok()
}

fn health_of(v: &Value) -> Option<(String, u64)> {
    Some((v.get("termwall")?.as_str()?.to_string(), v.get("forbidden").and_then(Value::as_u64).unwrap_or(0)))
}

/// Where a logon entry mentions the Python server.
#[cfg_attr(not(windows), allow(dead_code))]
fn mentions_python_server(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("termwall_api.py") || t.contains("termwall-api.vbs")
}

#[cfg(windows)]
fn python_logon_entries() -> Vec<(String, String)> {
    use crate::platform::reg;
    let mut out = Vec::new();
    for (name, data) in reg::values(reg::HKCU, r"Software\Microsoft\Windows\CurrentVersion\Run") {
        if mentions_python_server(&data) {
            out.push((format!("the Run entry \"{name}\""), data));
        }
    }
    // shortcuts keep their target as UTF-16, scripts as text
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let dir = Path::new(&appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let bytes = std::fs::read(e.path()).unwrap_or_default();
            let ascii: String = bytes.iter().filter(|b| **b != 0).map(|b| *b as char).collect();
            if mentions_python_server(&ascii) {
                out.push((format!("the Startup item {}", e.path().display()), "a shortcut or script".into()));
            }
        }
    }
    out.extend(scheduled_tasks());
    out
}

/// Enabled scheduled tasks that start the Python server, from `schtasks /query /xml`.
#[cfg(windows)]
fn scheduled_tasks() -> Vec<(String, String)> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(o) = std::process::Command::new("schtasks").args(["/query", "/xml"]).creation_flags(CREATE_NO_WINDOW).output() else {
        return Vec::new();
    };
    tasks_from_xml(&String::from_utf8_lossy(&o.stdout))
}

/// The tasks in `schtasks /query /xml` output (each after a `<!-- \path\name -->` line) that are
/// enabled and start the Python server: (where, its command).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn tasks_from_xml(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for chunk in xml.split("<!-- ").skip(1) {
        let Some((name, body)) = chunk.split_once(" -->") else { continue };
        let between = |tag: &str| -> Vec<String> {
            let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
            body.split(&open).skip(1).filter_map(|s| s.split_once(&close).map(|(v, _)| v.trim().to_string())).collect()
        };
        let run: String = [between("Command"), between("Arguments")].concat().join(" ");
        let settings = body.split_once("<Settings>").map(|(_, s)| s).unwrap_or("");
        let enabled = !settings.split("</Settings>").next().unwrap_or("").contains("<Enabled>false</Enabled>");
        if enabled && mentions_python_server(&run) {
            out.push((format!("task {}", name.trim()), run.replace("&quot;", "\"")));
        }
    }
    out
}

/// Wallpaper Engine's audio device setting ("default" = Windows' default output), from its
/// config.json.
fn we_audio_device(we_projects: &Path) -> Option<String> {
    let config = we_projects.parent()?.parent()?.join("config.json");
    fn find(v: &Value) -> Option<String> {
        match v {
            Value::Object(m) => {
                m.get("audioinputdevice").and_then(Value::as_str).map(String::from).or_else(|| m.values().find_map(find))
            }
            Value::Array(a) => a.iter().find_map(find),
            _ => None,
        }
    }
    find(&crate::util::read_json(&config)?)
}

pub fn gather(paths: &Paths) -> Facts {
    let port = crate::server::port();
    // the devices need a second of listening; the server's counters a moment apart: together
    let devices = std::thread::spawn(|| host::playback_devices(1200));
    let first = get_json(port, "/health").as_ref().and_then(health_of);
    std::thread::sleep(Duration::from_millis(1500));
    let later = get_json(port, "/health").as_ref().and_then(health_of);
    let health =
        first.map(|(version, forbidden)| Health { forbidden_later: later.map(|l| l.1).unwrap_or(forbidden), version, forbidden });
    let layers = Layers::read(&paths.style_file, &paths.theme_file);
    let mut screens: Vec<Option<&str>> = vec![None];
    screens.extend(layers.screens.keys().map(|k| Some(k.as_str())));
    let visualizer_on =
        screens.iter().any(|s| layers.resolve(*s).get("visualizer").and_then(Value::as_str).is_some_and(|v| v != "off"));
    let (host_entry, host_audio) = match host::autostart_state() {
        Some((cmd, o)) => (Some(cmd), o.audio_device),
        None => (None, None),
    };
    let we = integrate::we_projects();
    let mut x = Facts {
        this_exe: std::env::current_exe().ok(),
        port,
        listener: crate::platform::listener_pid(port),
        procs: processes(),
        health,
        host_entry,
        host_audio,
        webview2: cfg!(windows).then(host::webview2_version),
        we_page: we.as_ref().map(|w| w.join("termwall")).filter(|p| p.exists()).map(|p| std::fs::canonicalize(&p).unwrap_or(p)),
        we_audio: we.as_deref().and_then(we_audio_device),
        visualizer_on,
        config_problems: style::check_config(&paths.style_file),
        command_blocks: {
            let own = style::load_config(&paths.style_file).0;
            crate::blocks::commands_on(own.as_ref()).then(|| {
                let defs = crate::blocks::defs(own.as_ref()).0;
                defs.into_iter().filter(|d| matches!(d.src, crate::blocks::Src::Command(_))).map(|d| d.name).collect()
            })
        },
        livery_overrides: style::theme_overrides(&paths.style_file, &paths.theme_file),
        windows: cfg!(windows),
        ..Facts::default()
    };
    #[cfg(windows)]
    {
        use crate::platform::reg;
        let read = |root, name: &str| reg::read(root, UNINSTALL_KEY, name);
        x.installed = [reg::HKCU, reg::HKLM].into_iter().find_map(|root| {
            let v = read(root, "DisplayVersion")?;
            let dir = read(root, "InstallLocation")?;
            Some((v, PathBuf::from(dir.trim_end_matches('\\'))))
        });
        x.run_entry =
            reg::read(reg::HKCU, r"Software\Microsoft\Windows\CurrentVersion\Run", "termwall").filter(|s| !s.is_empty());
        x.python_logon = python_logon_entries();
    }
    x.devices = devices.join().unwrap_or_default();
    x
}

pub fn cmd_doctor(paths: &Paths) -> i32 {
    let facts = gather(paths);
    let findings = judge(&facts);
    let this = format!(
        "termwall {VERSION}, {}",
        facts.this_exe.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "this binary".into())
    );
    print!("{}", render(&findings, &this));
    if findings.iter().any(|x| x.level == Level::Fail) { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, exe: &str, args: &[&str]) -> Proc {
        let mut cmd = vec![exe.to_string()];
        cmd.extend(args.iter().map(|a| a.to_string()));
        Proc { pid, name: Path::new(exe).file_name().unwrap().to_string_lossy().into(), exe: Some(exe.into()), cmd }
    }

    fn fails(x: &Facts) -> Vec<String> {
        judge(x).into_iter().filter(|f| f.level == Level::Fail).map(|f| f.text).collect()
    }

    fn healthy() -> Facts {
        let inst = r"C:\Users\u\AppData\Local\Programs\termwall";
        Facts {
            port: 9002,
            listener: Some(10),
            procs: vec![proc(10, &format!(r"{inst}\termwallw.exe"), &[])],
            health: Some(Health { version: "3.0.2".into(), forbidden: 3, forbidden_later: 3 }),
            installed: Some(("3.0.2".into(), inst.into())),
            run_entry: Some(format!(r#""{inst}\termwallw.exe""#)),
            webview2: Some(Ok("140.0".into())),
            windows: true,
            ..Facts::default()
        }
    }

    #[test]
    fn a_healthy_machine_has_no_failures() {
        let x = healthy();
        // the Run entry's file doesn't exist here, so only that one may fail
        let f: Vec<String> = fails(&x).into_iter().filter(|t| !t.contains("isn't there")).collect();
        assert!(f.is_empty(), "{f:?}");
        assert!(judge(&x).iter().any(|f| f.level == Level::Ok && f.text.contains("termwall 3.0.2")));
    }

    #[test]
    fn the_python_server_ahead_of_the_installed_one() {
        // the case from 2026-11-16: an old rice logon task started termwall_api.py first
        let mut x = healthy();
        x.procs = vec![proc(20, r"C:\Python\pythonw.exe", &[r"E:\Projects\termwall\termwall_api.py"])];
        x.listener = Some(20);
        x.health = Some(Health { version: "2.4.1".into(), forbidden: 1197, forbidden_later: 1210 });
        x.python_logon = vec![("task \\rice\\termwall API".into(), "wscript.exe termwall-api.vbs".into())];
        let f = fails(&x);
        assert!(f.iter().any(|t| t.contains("Python server holds the port")), "{f:?}");
        assert!(f.iter().any(|t| t.contains("refused")), "the rising counter is the 'connecting' symptom: {f:?}");
        let task = judge(&x).into_iter().find(|f| f.text.contains("rice")).expect("the task is named");
        assert_eq!(task.level, Level::Fail);
        assert!(task.fix.unwrap().contains(r#"schtasks /Change /TN "\rice\termwall API" /DISABLE"#));
    }

    #[test]
    fn nothing_serving_and_old_versions() {
        let mut x = healthy();
        x.health = None;
        x.procs.clear();
        assert!(fails(&x).iter().any(|t| t.contains("nothing answers")));
        let mut y = healthy();
        y.health = Some(Health { version: "3.0.1".into(), ..Health::default() });
        assert!(fails(&y).iter().any(|t| t.contains("running termwall 3.0.1, installed 3.0.2")));
    }

    #[test]
    fn music_on_another_device_than_wallpaper_engine_hears() {
        // Panto's: music on "Sonar - Media", Windows' default "Sonar - Gaming"
        let mut x = healthy();
        x.procs.push(proc(30, r"D:\Steam\wallpaper_engine\wallpaper64.exe", &[]));
        x.visualizer_on = true;
        x.we_audio = Some("default".into());
        x.devices = vec![
            Device { name: "SteelSeries Sonar - Gaming".into(), default: true, peak: 0.0 },
            Device { name: "SteelSeries Sonar - Media".into(), default: false, peak: 0.4 },
        ];
        let f = fails(&x);
        assert!(
            f.iter()
                .any(|t| t.contains("plays on SteelSeries Sonar - Media") && t.contains("listens to SteelSeries Sonar - Gaming")),
            "{f:?}"
        );
        x.devices[0].peak = 0.3; // the default plays too: fine
        assert!(fails(&x).iter().all(|t| !t.contains("visualizer stays still")));
        x.visualizer_on = false;
        x.devices[0].peak = 0.0;
        assert!(fails(&x).is_empty() || fails(&x).iter().all(|t| !t.contains("visualizer")), "visualizer off: no audio problem");
        // the host fixed to a device that doesn't play
        let mut h = healthy();
        h.visualizer_on = true;
        h.host_entry = Some("termwallw.exe host --audio-device Gaming".into());
        h.host_audio = Some("Gaming".into());
        h.devices = vec![
            Device { name: "Sonar - Gaming".into(), default: true, peak: 0.0 },
            Device { name: "Sonar - Media".into(), default: false, peak: 0.5 },
        ];
        assert!(fails(&h).iter().any(|t| t.contains("fixed to \"Gaming\"")));
    }

    #[test]
    fn tasks_in_schtasks_xml() {
        let xml = r#"
<!-- \rice\termwall API -->
<?xml version="1.0" encoding="UTF-16"?>
<Task><Settings><Enabled>true</Enabled></Settings>
<Actions><Exec><Command>wscript.exe</Command><Arguments>"E:\P\termwall\termwall-api.vbs"</Arguments></Exec></Actions></Task>
<!-- \off one -->
<Task><Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers><Settings><Enabled>false</Enabled></Settings>
<Actions><Exec><Command>pythonw</Command><Arguments>termwall_api.py</Arguments></Exec></Actions></Task>
<!-- \other -->
<Task><Actions><Exec><Command>notepad.exe</Command></Exec></Actions></Task>
"#;
        let t = tasks_from_xml(xml);
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!(t[0].0, r"task \rice\termwall API");
        assert!(t[0].1.contains("termwall-api.vbs"));
    }

    #[test]
    fn the_report_is_plain_text() {
        let text = render(&judge(&healthy()), "termwall 3.0.2");
        assert!(text.starts_with("termwall doctor (termwall 3.0.2)\n\nserver\n  ok  "));
        assert!(text.is_ascii() || text.chars().all(|c| (c as u32) < 0x2000), "no emoji");
    }
}
