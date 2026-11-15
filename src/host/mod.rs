//! The wallpaper host: the page on the desktop itself, behind the icons, without Wallpaper Engine
//! or Lively. Windows for now; Linux keeps linux/termwall-layer.py until phase 3.

use std::path::Path;

use crate::paths::Paths;

#[cfg_attr(not(windows), allow(dead_code))] // the Linux host (phase 3) will use the spectrum too
mod audio;
#[cfg_attr(not(windows), allow(dead_code))] // and the now-playing events
mod media;
#[cfg(windows)]
mod windows;

/// The parts of Wallpaper Engine's web API the page uses, defined before the page's own scripts.
#[cfg_attr(not(windows), allow(dead_code))] // the Linux host (phase 3) will use it too
pub const WE_API: &str = include_str!("we_api.js");

pub const USAGE: &str = "termwall host [--monitor N] [--audio-device NAME] [--seconds N] [--window]
  the page as the desktop background on every monitor (or only monitor N, 1 = the main one),
  behind the icons; --window shows it in an ordinary window instead; --seconds quits after N.
  The visualizer hears the default playback device, or the one that plays when that is silent;
  --audio-device NAME fixes it to the device whose name contains NAME.
termwall host on [--monitor N] [--audio-device NAME]
  start it now and at every logon; termwall host off stops it and removes the autostart";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    pub monitor: Option<usize>,
    pub seconds: Option<u64>,
    pub window: bool,
    pub audio_device: Option<String>,
    /// `host on` / `host off`: the autostart, instead of running here.
    pub autostart: Option<bool>,
}

pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut number = |what: &str| -> Result<u64, String> {
            it.next()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("{what} takes a number above 0"))
        };
        match a.as_str() {
            "--monitor" => o.monitor = Some(number("--monitor")? as usize),
            "--seconds" => o.seconds = Some(number("--seconds")?),
            "--window" => o.window = true,
            "--audio-device" => {
                o.audio_device = Some(it.next().filter(|v| !v.is_empty()).ok_or("--audio-device takes a name")?.clone())
            }
            "on" | "off" if o.autostart.is_none() => o.autostart = Some(a == "on"),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(o)
}

/// file:///E:/x/index.html?screen=2 (spaces and the few characters a URL path can't carry escaped).
#[cfg_attr(not(windows), allow(dead_code))] // the Linux host (phase 3) will use it too
pub fn page_url(page: &Path, screen: usize) -> String {
    let p = page.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file:///");
    for c in p.trim_start_matches('/').chars() {
        match c {
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            '%' => out.push_str("%25"),
            c => out.push(c),
        }
    }
    out + &format!("?screen={screen}")
}

/// The stats server for the pages: the one already running, or this process becomes it.
#[cfg_attr(not(windows), allow(dead_code))]
fn ensure_server(paths: &Paths) -> Result<(), String> {
    use crate::server::{self, StartError};
    match server::start(paths) {
        Ok((listener, handler)) => {
            println!("no stats server was running: serving on http://{}:{} from here", server::HOST, server::port());
            std::thread::spawn(move || crate::http::serve(listener, handler, None));
            Ok(())
        }
        Err(StartError::PortInUse(port)) => {
            println!("using the stats server already on {}:{port}", server::HOST);
            Ok(())
        }
        Err(StartError::Other(e)) => Err(e),
    }
}

/// What the page has to know before its own scripts run: Wallpaper Engine's API, and where the
/// stats are when the server is not on its usual port.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn init_script() -> String {
    let port = crate::server::port();
    if port == crate::server::PORT {
        WE_API.to_string()
    } else {
        format!("window.TERMWALL_API = \"http://{}:{port}/stats\";\n{WE_API}", crate::server::HOST)
    }
}

pub fn cmd_host(args: &[String], paths: &Paths) -> i32 {
    let opts = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return 2;
        }
    };
    #[cfg(windows)]
    {
        if let Some(on) = opts.autostart {
            return windows::autostart(on, &opts);
        }
        if let Err(e) = ensure_server(paths) {
            eprintln!("{e}");
            return 1;
        }
        windows::run(&opts, paths)
    }
    #[cfg(not(windows))]
    {
        let _ = (opts, paths);
        eprintln!("termwall host runs on Windows for now; on Linux use linux/termwall-layer.py");
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_page_url() {
        let a = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            parse(&a(&["--monitor", "2", "--seconds", "15"])),
            Ok(Options { monitor: Some(2), seconds: Some(15), ..Default::default() })
        );
        assert_eq!(parse(&a(&["--window"])).map(|o| o.window), Ok(true));
        let on = parse(&a(&["on", "--monitor", "1", "--audio-device", "Head phones"])).expect("host on");
        assert_eq!((on.autostart, on.monitor, on.audio_device.as_deref()), (Some(true), Some(1), Some("Head phones")));
        assert_eq!(parse(&a(&["off"])).map(|o| o.autostart), Ok(Some(false)));
        assert!(parse(&a(&["on", "off"])).is_err() && parse(&a(&["--audio-device"])).is_err());
        assert!(parse(&a(&["--monitor", "0"])).is_err() && parse(&a(&["--nope"])).is_err() && parse(&a(&["--seconds"])).is_err());
        assert_eq!(
            page_url(Path::new(r"E:\My Files\termwall\index.html"), 2),
            "file:///E:/My%20Files/termwall/index.html?screen=2"
        );
        assert_eq!(page_url(Path::new("/usr/share/termwall/index.html"), 1), "file:///usr/share/termwall/index.html?screen=1");
        assert!(WE_API.contains("wallpaperRegisterAudioListener") && WE_API.contains("__termwallHost"));
    }
}
