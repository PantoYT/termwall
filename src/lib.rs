//! termwall: a live system-stats wallpaper in the style of fastfetch.
//!
//! Read-only JSON on 127.0.0.1 only (never the LAN). A background thread samples once per
//! second, so a request never blocks on the operating system or nvidia-smi. The page
//! (index.html) is the same one Wallpaper Engine and Lively show.

mod gpu;
mod host;
mod http;
mod integrate;
mod logos;
mod paths;
mod picture;
mod platform;
mod png;
mod qr;
mod sampler;
mod selftest;
mod server;
mod share;
mod style;
mod theme;
mod util;

use std::path::Path;

use crate::integrate::VERSION;
use crate::paths::Paths;
use crate::util::Value;

const USAGE: &str = "termwall - a live system-stats wallpaper (github.com/PantoYT/termwall)

  termwall                       run the stats server here (the installer runs it at logon)
  termwall config                open termwall.toml in an editor: every setting, its choices in
                                 comments ($EDITOR, else the .toml app, else VS Code, else Notepad)
  termwall check                 which lines of termwall.toml are wrong, and what livery overrides
  termwall reset                 every setting back to its default
  termwall update [--check]      install the latest release (asks GitHub only when you run it)
  termwall share [off|new-key]   the wallpaper on a phone: a QR code to scan, a read-only server
                                 on your network (no user name, computer name or IP go out)
  (the other options work without the dashes too: termwall themes, termwall version)
  termwall --style               every setting, its value and its choices
  termwall --style KEY VALUE     change one (KEY default: remove it, --style reset: all)
  termwall --theme NAME          a built-in palette (termwall --themes lists them)
  termwall logos [WORD...]       the logos (fastfetch's), or those matching: termwall logos mac
  termwall logos ID              show one in its own colors; termwall --style logo ID uses it
  termwall logo-from PICTURE     a picture (PNG, GIF, JPEG, WebP, BMP) -> logo.txt with $1..$9,
                                 used as logo = \"custom\" with its colors in logo_colors. Options:
                                 --width N (40), --ascii, --colors N (1-9), --keep-bg,
                                 --out FILE (write only that file), --force (overwrite logo.txt)
  termwall host [--monitor N]    the page as the desktop background, behind the icons, without
                                 Wallpaper Engine (--window: in a window; --seconds N: quit after)
  termwall --once                print one sample
  termwall --selftest            run the self-test
  termwall --link-we | --unlink-we | --link-lively | --unlink-lively
  termwall --stop                stop this folder's server
  termwall --version
";

const OPTIONS: [&str; 18] = [
    "--host",
    "--selftest",
    "--version",
    "--link-we",
    "--unlink-we",
    "--link-lively",
    "--unlink-lively",
    "--stop",
    "--themes",
    "--theme",
    "--logos",
    "--logo-from",
    "--style",
    "--once",
    "--config",
    "--check",
    "--share",
    "--update",
];

/// termwall config / check / reset ... without the dashes.
fn word(arg: &str) -> Option<&'static str> {
    Some(match arg {
        "config" => "--config",
        "check" => "--check",
        "style" => "--style",
        "theme" => "--theme",
        "themes" => "--themes",
        "logos" => "--logos",
        "logo-from" => "--logo-from",
        "version" => "--version",
        "stop" => "--stop",
        "once" => "--once",
        "selftest" => "--selftest",
        "share" => "--share",
        "update" => "--update",
        "host" => "--host",
        _ => return None,
    })
}

fn after(args: &[String], flag: &str) -> Vec<String> {
    args.iter().position(|a| a == flag).map(|i| args[i + 1..].to_vec()).unwrap_or_default()
}

/// `termwall logos [WORD... | ID]`: list or find logos; an exact id shows it.
fn cmd_logos(args: &[String]) -> i32 {
    let cat = logos::catalog();
    if cat.is_empty() {
        println!("logos.js is missing next to the page ({})", paths::app_dir().display());
        return 1;
    }
    if args.len() == 1 && (cat.logos.contains_key(&args[0]) || cat.aliases.contains_key(&args[0])) {
        let key = cat.aliases.get(&args[0]).and_then(Value::as_str).unwrap_or(&args[0]).to_string();
        println!("{}", logos::logo_preview(&key, &cat));
        println!("\n{key}  ({})   use it: termwall --style logo {key}", cat.names_of(&key).join(", "));
        println!("its own colors: termwall --style logo_colors own");
        return 0;
    }
    let found = if args.is_empty() { logos::distro_ids(&cat) } else { logos::find_logos(args, &cat) };
    if found.is_empty() {
        println!("no logo matches '{}'; termwall logos lists them all", args.join(" "));
        return 1;
    }
    let width = found.iter().map(|k| k.chars().count()).max().unwrap_or(0) + 2;
    for k in &found {
        let names: String = cat.names_of(k).join(", ").chars().take(70).collect();
        println!("  {k:width$}{names}");
    }
    println!("{} logo(s). termwall logos ID shows one; termwall --style logo ID uses it", found.len());
    0
}

/// println! into any writer, errors ignored (a closed pipe must not kill the command).
macro_rules! w {
    ($out:expr, $($arg:tt)*) => {{ let _ = writeln!($out, $($arg)*); }};
}

const LOGO_FROM_USAGE: &str = "termwall logo-from PICTURE [--width N] [--ascii] [--colors N] [--keep-bg] [--out FILE] [--force]";

/// `termwall logo-from PICTURE [--width N] [--ascii] [--colors N] [--keep-bg] [--out FILE] [--force]`
pub fn cmd_logo_from(args: &[String], paths: &Paths, out: &mut dyn std::io::Write) -> i32 {
    let (mut width, mut colors, mut out_file) = ("40".to_string(), "9".to_string(), None::<String>);
    let (mut flags, mut files) = (Vec::<&str>::new(), Vec::<&String>::new());
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if ["--width", "--colors", "--out"].contains(&a) && i + 1 < args.len() {
            let v = args[i + 1].clone();
            match a {
                "--width" => width = v,
                "--colors" => colors = v,
                _ => out_file = Some(v),
            }
            i += 2;
        } else if ["--ascii", "--keep-bg", "--force"].contains(&a) {
            flags.push(a);
            i += 1;
        } else if a.starts_with("--") {
            w!(out, "unknown option {a}\n\n{USAGE}");
            return 2;
        } else {
            files.push(&args[i]);
            i += 1;
        }
    }
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if files.len() != 1 || !digits(&width) || !digits(&colors) {
        w!(out, "{LOGO_FROM_USAGE}");
        return 2;
    }
    let chars = if flags.contains(&"--ascii") { "ascii" } else { "blocks" };
    let result = picture::picture_logo(
        Path::new(files[0]),
        width.parse().unwrap_or(40),
        chars,
        colors.parse().unwrap_or(9),
        flags.contains(&"--keep-bg"),
    );
    let (text, cols) = match result {
        Ok(r) => r,
        Err(e) => {
            w!(out, "error: {}: {e}", files[0]);
            return 1;
        }
    };
    let own: Vec<Option<String>> = cols.iter().cloned().map(Some).collect();
    let target = out_file.clone().map(std::path::PathBuf::from).unwrap_or_else(|| paths.logo_file());
    if target.exists() && out_file.is_none() && !flags.contains(&"--force") {
        w!(out, "{}", logos::ansi_logo(&text, &own));
        w!(out, "\n{} already exists: --force replaces it, --out FILE writes somewhere else", target.display());
        return 1;
    }
    if let Some(dir) = std::path::absolute(&target).ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&target, format!("{text}\n")) {
        w!(out, "error: {}: {e}", target.display());
        return 1;
    }
    w!(out, "{}", logos::ansi_logo(&text, &own));
    w!(out, "\nwrote {}: {} lines, {} colors", target.display(), text.lines().count(), cols.len());
    let colors_toml = format!("logo_colors = [{}]", cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", "));
    if out_file.is_some() {
        w!(out, "its colors: {colors_toml}");
        return 0;
    }
    let set = style::set_style("logo", "custom", &paths.style_file)
        .and_then(|_| style::set_style("logo_colors", &cols.join(","), &paths.style_file));
    if let Err(e) = set {
        w!(out, "could not change termwall.toml ({e}); set by hand: logo = \"custom\", {colors_toml}");
        return 1;
    }
    w!(out, "termwall.toml: logo = \"custom\", {colors_toml}");
    0
}

fn cmd_check(paths: &Paths) -> i32 {
    let path = style::ensure_config(&paths.style_file).unwrap_or_else(|_| paths.style_file.clone());
    let problems = style::check_config(&path);
    for line in &problems {
        println!("  {line}");
    }
    let verdict = if problems.is_empty() { "fine".to_string() } else { format!("{} problem(s)", problems.len()) };
    println!("{}: {verdict}", paths.style_file.display());
    let own = style::read_style(&paths.style_file, &paths.theme_file, None);
    let theme_auto = own.get("theme").and_then(Value::as_str).unwrap_or("auto") == "auto";
    if theme::read_theme(&paths.theme_file).is_some() && theme_auto && own.get("colors").is_none() {
        println!("note: theme \"auto\": the colors come from theme.json (livery); pick a theme here to override");
    }
    let over = style::theme_overrides(&paths.style_file, &paths.theme_file);
    if !over.is_empty() {
        let list: Vec<String> = over.iter().map(|(k, v)| format!("{k} = {}", style::toml_value(v).unwrap_or_default())).collect();
        println!(
            "note: theme.json also sets {} for the wallpaper shown now, and that wins over this file \
(livery: that pack's own termwall look; livery termwall look PACK none drops it)",
            list.join(", ")
        );
    }
    if problems.is_empty() { 0 } else { 1 }
}

fn cmd_style(rest: &[String], paths: &Paths) -> i32 {
    let r = if rest.first().is_some_and(|r| r == "reset") {
        style::set_style("reset", "", &paths.style_file).map(|_| ())
    } else if rest.len() >= 2 {
        style::set_style(&rest[0], &rest[1..].join(" "), &paths.style_file).map(|_| ())
    } else if let Some(k) = rest.first() {
        Err(match style::setting(k) {
            Some(s) => format!("{k}: give a value ({})", s.check.help()),
            None => format!("unknown setting '{k}'"),
        })
    } else {
        Ok(())
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        return 1;
    }
    let st = style::read_style(&paths.style_file, &paths.theme_file, None);
    for s in style::spec() {
        let v = match (s.key, st.get(s.key)) {
            ("colors", Some(c)) if c.as_object().is_some_and(|c| !c.is_empty()) => "set".to_string(),
            ("colors", _) => "-".to_string(),
            (_, None) => "-".to_string(),
            (_, Some(Value::Array(a))) => {
                a.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(",")
            }
            (_, Some(Value::Bool(b))) => {
                if *b {
                    "on".into()
                } else {
                    "off".into()
                }
            }
            (_, Some(Value::String(s))) => s.clone(),
            (_, Some(Value::Number(n))) if n.is_f64() => util::py_float(n.as_f64().unwrap_or(0.0)),
            (_, Some(v)) => v.to_string(),
        };
        let v = if v.is_empty() { "-".to_string() } else { v };
        println!("  {:8} {:22} {}", s.key, v, s.check.help());
    }
    println!("  termwall --style KEY VALUE | KEY default | reset    or edit it: termwall --config");
    0
}

fn cmd_once() -> i32 {
    let mut s = match sampler::Sampler::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    s.sample();
    std::thread::sleep(std::time::Duration::from_secs(1));
    s.sample();
    let text = serde_json::to_string_pretty(&s.get()).unwrap_or_default();
    println!("{}", text.chars().take(4000).collect::<String>());
    0
}

/// The whole program: both binaries (termwall, and termwallw without a console) call this.
pub fn run() {
    let mut args: Vec<String> = std::env::args().collect();
    if let Some(w) = args.get(1).and_then(|a| word(a)) {
        args[1] = w.to_string();
    } else if args.get(1).is_some_and(|a| a == "reset") {
        args.splice(1..2, ["--style".to_string(), "reset".to_string()]);
    }
    let has = |f: &str| args.iter().any(|a| a == f);
    if ["--help", "-h", "/?", "help"].iter().any(|h| has(h)) {
        println!("{USAGE}");
        return;
    }
    if let Some(first) = args.get(1).filter(|a| !OPTIONS.contains(&a.as_str())) {
        eprintln!("unknown option {first}\n\n{USAGE}");
        std::process::exit(1);
    }
    let p = paths::get();
    let here = p.app_dir.clone();
    let code = if has("--selftest") {
        selftest::run()
    } else if has("--version") {
        println!("termwall {VERSION}");
        // asked for, so a quick look is fine; offline is not an error
        if let Ok(Some(rel)) = integrate::newer_release(3) {
            println!(
                "{} is out: termwall update",
                rel.get("tag_name").and_then(Value::as_str).unwrap_or("").trim_start_matches('v')
            );
        }
        0
    } else if has("--update") {
        integrate::cmd_update(&after(&args, "--update"), p)
    } else if has("--link-we") {
        println!("{}", integrate::link_we(&here));
        0
    } else if has("--link-lively") {
        println!("{}", integrate::link_lively(&integrate::local_appdata(), &here));
        0
    } else if has("--unlink-lively") {
        println!("{}", integrate::unlink_lively(&integrate::local_appdata(), &here));
        0
    } else if has("--unlink-we") {
        println!("{}", integrate::unlink_we(&here));
        0
    } else if has("--stop") {
        println!("stopped {}", integrate::stop_running(&here));
        0
    } else if has("--share") {
        share::cmd_share(&after(&args, "--share"), p)
    } else if has("--config") {
        println!("opening {}", integrate::open_config(p));
        println!("save it and the wallpaper follows within a second; termwall check if something doesn't change");
        0
    } else if has("--check") {
        cmd_check(p)
    } else if has("--themes") {
        for line in style::themes_listing() {
            println!("{line}");
        }
        println!("  termwall --theme NAME   (auto = the distro's colors on Linux, mint on Windows)");
        0
    } else if has("--logo-from") {
        cmd_logo_from(&after(&args, "--logo-from"), p, &mut std::io::stdout())
    } else if has("--logos") {
        cmd_logos(&after(&args, "--logos"))
    } else if has("--theme") {
        let rest = after(&args, "--theme");
        let name = rest.first().cloned().unwrap_or_default();
        match style::set_style("theme", &name, &p.style_file) {
            Ok(_) => {
                let note =
                    if theme::read_theme(&p.theme_file).is_some() { "  (theme.json exists: its palette wins)" } else { "" };
                println!("theme: {name}{note}");
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        }
    } else if has("--style") {
        cmd_style(&after(&args, "--style"), p)
    } else if has("--host") {
        host::cmd_host(&after(&args, "--host"), p)
    } else if has("--once") {
        cmd_once()
    } else {
        server::serve(p)
    };
    std::process::exit(code);
}
