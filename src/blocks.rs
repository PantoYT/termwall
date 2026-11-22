//! Your own blocks for `layout = "custom"` (experimental): `[block.NAME]` tables in termwall.toml,
//! each a title and one of `text`, `file` (next to termwall.toml) or `command` (its output,
//! every `every` seconds). Commands run as you, so only with `experimental_commands = true` at
//! the top of termwall.toml; the switcher's theme.json can't define blocks or turn that on.
//!
//! The page gets the result as plain text (no HTML), through /theme on 127.0.0.1 only: termwall
//! share doesn't send it to the phone.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::paths::Paths;
use crate::style;
use crate::util::{self, Map, Value};

/// The top-level switch for commands.
pub const GATE: &str = "experimental_commands";
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_LINES: usize = 40;
const MAX_CHARS: usize = 6000;

#[derive(Clone, Debug, PartialEq)]
pub enum Src {
    Text(String),
    File(String),
    Command(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Def {
    pub name: String,
    pub title: String,
    pub src: Src,
    pub every: u64,
}

/// A name a block can have (built-in or your own): lowercase letters, digits and dashes.
pub fn name_ok(n: &str) -> bool {
    (1..=24).contains(&n.len()) && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether termwall.toml turns commands on.
pub fn commands_on(own: Option<&Map>) -> bool {
    own.and_then(|o| o.get(GATE)).and_then(Value::as_bool).unwrap_or(false)
}

/// The [block.NAME] tables of termwall.toml, and what is wrong with them (one line each).
pub fn defs(own: Option<&Map>) -> (Vec<Def>, Vec<String>) {
    let (mut out, mut problems) = (Vec::new(), Vec::new());
    let Some(tables) = own.and_then(|o| o.get("block")) else { return (out, problems) };
    let Some(tables) = tables.as_object() else {
        problems.push("block: has to be [block.NAME] tables".into());
        return (out, problems);
    };
    for (name, t) in tables {
        let bad = |what: &str| format!("block.{name}: {what}");
        if !name_ok(name) {
            problems.push(bad("the name takes lowercase letters, digits and dashes (up to 24)"));
            continue;
        }
        if style::BLOCKS.contains(&name.as_str()) {
            problems.push(bad("a built-in block has that name; pick another"));
            continue;
        }
        let Some(t) = t.as_object() else {
            problems.push(bad("has to be a table"));
            continue;
        };
        let text = |k: &str, max: usize| t.get(k).and_then(Value::as_str).filter(|s| s.chars().count() <= max).map(String::from);
        let sources: Vec<Src> =
            [text("text", 4000).map(Src::Text), text("file", 260).map(Src::File), text("command", 1000).map(Src::Command)]
                .into_iter()
                .flatten()
                .collect();
        if sources.len() != 1 {
            problems.push(bad("needs exactly one of text, file or command"));
            continue;
        }
        let title = match t.get("title") {
            None => name.to_uppercase(),
            Some(v) => match v.as_str().filter(|s| s.chars().count() <= 40 && util::is_printable(s)) {
                Some(s) => s.to_string(),
                None => {
                    problems.push(bad("title: text up to 40 characters"));
                    continue;
                }
            },
        };
        let src = sources.into_iter().next().expect("one source");
        let fallback = if matches!(src, Src::Command(_)) { 60 } else { 5 };
        let every = match t.get("every") {
            None => fallback,
            Some(v) => match v.as_i64().filter(|n| (2..=86_400).contains(n)) {
                Some(n) => n as u64,
                None => {
                    problems.push(bad("every: seconds, 2 to 86400"));
                    continue;
                }
            },
        };
        for k in t.keys() {
            if !["title", "text", "file", "command", "every"].contains(&k.as_str()) {
                problems.push(bad(&format!("{k}: unknown (title, text, file, command, every)")));
            }
        }
        out.push(Def { name: name.clone(), title, src, every });
    }
    (out, problems)
}

/// Output as plain text the page can show: terminal colors and controls removed, cut to size.
pub fn clean(raw: &str) -> String {
    let mut s = String::with_capacity(raw.len());
    let mut it = raw.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => {
                // CSI (ESC [ ... final byte) or a two-character escape
                if it.peek() == Some(&'[') {
                    it.next();
                    for d in it.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&d) {
                            break;
                        }
                    }
                } else {
                    it.next();
                }
            }
            '\t' => s.push_str("    "),
            '\n' => s.push('\n'),
            c if c.is_control() => {}
            c => s.push(c),
        }
    }
    let lines: Vec<String> = s.lines().take(MAX_LINES).map(|l| l.chars().take(200).collect()).collect();
    let mut out = lines.join("\n").trim_end().to_string();
    if out.chars().count() > MAX_CHARS {
        out = out.chars().take(MAX_CHARS).collect();
    }
    out
}

/// Run one command through the shell in `dir`: its output (stdout, else stderr), and whether
/// it failed. Killed after `timeout`.
pub fn run_command(cmd: &str, dir: &Path, timeout: Duration) -> (String, bool) {
    let mut c = if cfg!(windows) { Command::new("cmd") } else { Command::new("sh") };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.arg("/C").raw_arg(cmd);
    }
    #[cfg(not(windows))]
    c.arg("-c").arg(cmd);
    util::hide_window(&mut c);
    c.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return (format!("could not start: {e}"), true),
    };
    // the pipes are read on threads of their own and waited for only briefly: a program the
    // command started can keep them open after the shell is gone
    let pipe = |r: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(r) = r {
                let _ = r.take(1 << 20).read_to_end(&mut buf);
            }
            let _ = tx.send(buf);
        });
        rx
    };
    let out = pipe(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = pipe(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                kill_tree(&mut child);
                break None;
            }
        }
    };
    let grace = Duration::from_millis(if status.is_some() { 2000 } else { 300 });
    let (out, err) = (out.recv_timeout(grace).unwrap_or_default(), err.recv_timeout(grace).unwrap_or_default());
    let text = if out.iter().any(|b| !b.is_ascii_whitespace()) { out } else { err };
    let text = clean(&String::from_utf8_lossy(&text));
    match status {
        None => (format!("{text}\n(stopped after {} s)", timeout.as_secs()).trim().to_string(), true),
        Some(st) if !st.success() => (if text.is_empty() { format!("exited with {st}") } else { text }, true),
        Some(_) => (text, false),
    }
}

/// The command and what it started (cmd.exe leaves its children running when it is killed).
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let mut t = Command::new("taskkill");
        t.args(["/PID", &child.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null());
        util::hide_window(&mut t);
        let _ = t.status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[derive(Clone, Debug, Default)]
struct Out {
    title: String,
    text: String,
    error: bool,
    /// the definition it came from: a changed block is run again at once
    from: String,
    at: Option<Instant>,
    running: bool,
}

fn state() -> &'static Mutex<HashMap<String, Out>> {
    static S: OnceLock<Mutex<HashMap<String, Out>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What the page shows in each block: name -> {title, text, error}.
pub fn outputs() -> Map {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    let mut names: Vec<&String> = st.keys().collect();
    names.sort();
    names
        .into_iter()
        .map(|n| {
            let o = &st[n];
            (n.clone(), serde_json::json!({"title": o.title, "text": o.text, "error": o.error}))
        })
        .collect()
}

/// One pass: every block whose time has come is brought up to date.
fn tick(style_file: &Path) {
    let own = style::load_config(style_file).0;
    let (defs, _) = defs(own.as_ref());
    let allowed = commands_on(own.as_ref());
    let dir: PathBuf = style_file.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.retain(|n, _| defs.iter().any(|d| d.name == *n));
    for d in defs {
        let from = format!("{:?}{}{allowed}", d.src, d.every);
        let o = st.entry(d.name.clone()).or_default();
        o.title = d.title.clone();
        let fresh = o.from == from && o.at.is_some_and(|t| t.elapsed() < Duration::from_secs(d.every));
        if fresh || o.running {
            continue;
        }
        o.from = from;
        o.at = Some(Instant::now());
        match &d.src {
            Src::Text(t) => (o.text, o.error) = (clean(t), false),
            Src::File(f) => {
                let p = dir.join(f);
                (o.text, o.error) = match std::fs::read(&p) {
                    Ok(b) => (clean(&String::from_utf8_lossy(&b[..b.len().min(64 * 1024)])), false),
                    Err(e) => (format!("{}: {e}", p.display()), true),
                };
            }
            Src::Command(_) if !allowed => {
                (o.text, o.error) =
                    (format!("commands are off: {GATE} = true in termwall.toml runs them (at your own risk)"), true)
            }
            Src::Command(cmd) => {
                o.running = true;
                if o.text.is_empty() || o.text.starts_with("commands are off") {
                    (o.text, o.error) = ("running...".into(), false);
                }
                let (cmd, dir, name) = (cmd.clone(), dir.clone(), d.name.clone());
                std::thread::spawn(move || {
                    let (text, error) = run_command(&cmd, &dir, TIMEOUT);
                    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(o) = st.get_mut(&name) {
                        (o.text, o.error, o.running) = (text, error, false);
                    }
                });
            }
        }
    }
}

/// The blocks, kept up to date on a thread of their own for as long as the server runs.
pub fn start(paths: Arc<Paths>) {
    let _ = std::thread::Builder::new().name("termwall-blocks".into()).spawn(move || {
        loop {
            tick(&paths.style_file);
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own(toml: &str) -> Map {
        style::parse_toml_text(toml).expect("toml")
    }

    #[test]
    fn block_tables() {
        let o = own(
            "experimental_commands = true\n[block.notes]\nfile = \"notes.txt\"\n[block.up]\ntitle = \"UPTIME\"\ncommand = \"uptime\"\nevery = 30\n\
[block.bad]\ntext = \"a\"\ncommand = \"b\"\n[block.cpu]\ntext = \"x\"\n[block.Odd]\ntext = \"x\"\n[block.slow]\ncommand = \"x\"\nevery = 1\n",
        );
        let (d, problems) = defs(Some(&o));
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0], Def { name: "notes".into(), title: "NOTES".into(), src: Src::File("notes.txt".into()), every: 5 });
        assert_eq!(d[1].every, 30);
        assert_eq!(d[1].title, "UPTIME");
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(commands_on(Some(&o)));
        assert!(!commands_on(Some(&own("[block.x]\ntext = \"y\"\n"))), "off unless switched on");
    }

    #[test]
    fn the_blocks_setting_and_the_file() {
        let check = |v: serde_json::Value| style::setting("blocks").unwrap().check.check(&v);
        assert_eq!(check(serde_json::json!([["cpu:1", "notes:2"]])), Some(serde_json::json!([["cpu", "notes:2"]])));
        assert!(check(serde_json::json!([["cpu:7"]])).is_none() && check(serde_json::json!([[]])).is_none());
        assert!(check(serde_json::json!([["Bad Name"]])).is_none() && check(serde_json::json!(["cpu"])).is_none());
        assert_eq!(
            style::parse_value("blocks", "logo, info / cpu:2,memory"),
            serde_json::json!([["logo", "info"], ["cpu:2", "memory"]])
        );
        assert_eq!(style::show_value("blocks", Some(&serde_json::json!([["logo", "info"], ["cpu:2"]]))), "logo,info / cpu:2");

        let td = util::TempDir::new().expect("temp dir");
        let (sf, tf) = (td.join("termwall.toml"), td.join("theme.json"));
        std::fs::write(
            &sf,
            "layout = \"custom\"
from_later = 1
[screen.2]
layout = \"logo\"
[block.notes]
text = \"hi\"
",
        )
        .unwrap();
        style::set_style("font", "fira", &sf).expect("write");
        let text = std::fs::read_to_string(&sf).unwrap();
        let own = style::parse_toml_text(&text).expect("still TOML");
        assert_eq!(own["from_later"], 1, "a key from a newer termwall stays at the top level, not under a table");
        assert_eq!(own["block"]["notes"]["text"], "hi");
        assert_eq!(own["screen"]["2"]["layout"], "logo");
        assert_eq!(own[GATE], false, "the switch is written out, off");
        assert!(style::check_config(&sf).iter().all(|p| p.contains("from_later")), "{:?}", style::check_config(&sf));
        // a switcher can't bring blocks or switch commands on
        std::fs::write(
            &tf,
            r#"{"style": {"block": {"x": {"command": "evil"}}, "experimental_commands": true, "layout": "board"}}"#,
        )
        .unwrap();
        let st = style::read_style(&sf, &tf, None);
        assert!(st.get("block").is_none() && st.get(GATE).is_none() && st["layout"] == "board");
        assert!(!commands_on(style::load_config(&sf).0.as_ref()));
        // reset keeps your own blocks
        style::set_style("reset", "", &sf).unwrap();
        let own = style::load_config(&sf).0.unwrap();
        assert_eq!(own["block"]["notes"]["text"], "hi");
        assert!(own.get("screen").is_none() && own["layout"] == "fetch");
    }

    #[test]
    fn output_is_plain_text() {
        assert_eq!(clean("\u{1b}[1;32mgreen\u{1b}[0m\r\n\tx\u{7}"), "green\n    x");
        let long: String = (0..100).map(|i| format!("line {i}\n")).collect();
        assert_eq!(clean(&long).lines().count(), MAX_LINES);
    }

    #[test]
    fn commands_run_and_time_out() {
        let dir = std::env::temp_dir();
        let (text, err) = run_command("echo hello", &dir, TIMEOUT);
        assert_eq!((text.as_str(), err), ("hello", false));
        let (_, err) = run_command("exit 3", &dir, TIMEOUT);
        assert!(err, "a failing command is shown as failed");
        let slow = if cfg!(windows) { "ping -n 6 127.0.0.1 >nul" } else { "sleep 5" };
        let t = Instant::now();
        let (text, err) = run_command(slow, &dir, Duration::from_secs(1));
        assert!(err && text.contains("stopped after 1 s") && t.elapsed() < Duration::from_secs(4), "{text}");
    }

    #[test]
    fn the_runner_follows_the_file() {
        let td = util::TempDir::new().expect("temp dir");
        let f = td.join("termwall.toml");
        std::fs::write(td.join("notes.txt"), "buy milk").unwrap();
        std::fs::write(&f, "[block.notes]\nfile = \"notes.txt\"\n[block.hi]\ncommand = \"echo hi\"\n").unwrap();
        tick(&f);
        let o = outputs();
        assert_eq!(o["notes"]["text"], "buy milk");
        assert_eq!(o["notes"]["title"], "NOTES");
        assert!(o["hi"]["text"].as_str().unwrap().contains("commands are off"), "no command without the switch");
        std::fs::write(&f, "experimental_commands = true\n[block.hi]\ncommand = \"echo hi\"\n").unwrap();
        tick(&f);
        for _ in 0..100 {
            if outputs().get("hi").is_some_and(|h| h["text"] == "hi") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let o = outputs();
        assert_eq!(o["hi"]["text"], "hi");
        assert!(o.get("notes").is_none(), "a removed block goes");
    }
}
