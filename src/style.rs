//! The look: termwall.toml (a commented file listing every setting and its choices, meant to be
//! edited by hand), theme.json's "style" from a switcher, and [screen.N] tables per monitor.
//! Every key is optional; anything unknown or invalid is dropped, so an old or hand-edited file
//! can't break the page. termwall.json (before 1.2) is carried over once.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::logos;
use crate::theme::{self, THEMES, is_hex_color, palette_of};
use crate::util::{self, Map, Value, py_float, read_json, stamp, write_in_place};

pub const LAYOUTS: [&str; 7] = ["fetch", "board", "minimal", "htop", "portrait", "phone", "logo"];
pub const SECTIONS: [&str; 15] = [
    "prompt", "logo", "info", "clock", "cpu", "cores", "gpu", "memory", "disks", "network", "procs", "swatches", "music",
    "status", "sound",
];
pub const FONTS: [&str; 6] = ["cascadia", "jetbrains", "fira", "iosevka", "consolas", "system"];
pub const EFFECTS: [&str; 7] = ["crt", "glow", "flicker", "roll", "chroma", "curve", "noise"];

/// A value TOML can hold but JSON can't (a datetime, inf, nan), carried as its TOML text so a
/// rewrite of termwall.toml keeps it. No setting accepts it (it is not printable text).
const RAW: &str = "\u{e000}toml:";

pub enum Check {
    Choice(Vec<&'static str>),
    Int(i64, i64),
    Float(f64, f64),
    Bool,
    Subset(&'static [&'static str]),
    Text,
    Logo,
    LogoColors,
    Palette,
}

impl Check {
    /// The value this setting accepts, or None.
    pub fn check(&self, v: &Value) -> Option<Value> {
        match self {
            Check::Choice(allowed) => v.as_str().filter(|s| allowed.contains(s)).map(Value::from),
            Check::Int(lo, hi) => v.as_i64().filter(|n| lo <= n && n <= hi).map(Value::from),
            Check::Float(lo, hi) => v.as_f64().filter(|x| *lo <= *x && *x <= *hi).map(util::num),
            Check::Bool => v.as_bool().map(Value::from),
            Check::Subset(allowed) => v.as_array().map(|a| {
                let mut seen: Vec<&str> = Vec::new();
                for x in a.iter().filter_map(Value::as_str) {
                    if allowed.contains(&x) && !seen.contains(&x) {
                        seen.push(x);
                    }
                }
                Value::from(seen)
            }),
            Check::Text => v.as_str().filter(|s| s.chars().count() <= 60 && util::is_printable(s)).map(Value::from),
            Check::Logo => check_logo(v),
            Check::LogoColors => check_logo_colors(v),
            Check::Palette => palette_of(v).map(Value::Object),
        }
    }

    pub fn help(&self) -> String {
        match self {
            Check::Choice(allowed) => allowed.join(" | "),
            Check::Int(lo, hi) => format!("{lo}-{hi}"),
            Check::Float(lo, hi) => format!("{}-{}", py_float(*lo), py_float(*hi)),
            Check::Bool => "on | off".into(),
            Check::Subset(allowed) => format!("a list from: {} (comma-separated; none = empty)", allowed.join(", ")),
            Check::Text => "any text up to 60 characters".into(),
            Check::Logo => {
                "auto | windows | tux | none | custom (logo.txt) | image (logo.gif/png/jpg/webp) | any logo id (termwall logos)"
                    .into()
            }
            Check::LogoColors => "auto | own | palette | up to nine #rrggbb for $1, $2... (comma-separated)".into(),
            Check::Palette => "six #rrggbb colors (bg, accent, secondary, text, dim, faint): edit termwall.toml".into(),
        }
    }
}

fn check_logo(v: &Value) -> Option<Value> {
    let s = v.as_str()?;
    let ok = (1..=40).contains(&s.len()) && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return None;
    }
    if ["auto", "windows", "tux", "none", "custom", "image"].contains(&s) {
        return Some(v.clone());
    }
    let cat = logos::catalog();
    // without logos.js there is no telling, so it passes
    (cat.is_empty() || cat.logos.contains_key(s) || cat.aliases.contains_key(s)).then(|| v.clone())
}

fn check_logo_colors(v: &Value) -> Option<Value> {
    let list: Option<Vec<Value>> = match v {
        Value::String(s) if s.contains('#') => {
            Some(s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(Value::from).collect())
        }
        Value::Array(a) => Some(a.clone()),
        _ => None,
    };
    if let Some(l) = list {
        let ok = !l.is_empty() && l.len() <= 9 && l.iter().all(|c| c.as_str().is_some_and(is_hex_color));
        return ok.then(|| Value::from(l));
    }
    v.as_str().filter(|s| ["auto", "own", "palette", "image"].contains(s)).map(Value::from)
}

pub struct Setting {
    pub key: &'static str,
    pub check: Check,
}

pub fn spec() -> &'static [Setting] {
    static SPEC: OnceLock<Vec<Setting>> = OnceLock::new();
    SPEC.get_or_init(|| {
        let mut themes = vec!["auto", "logo", "distro"];
        themes.extend(theme::theme_names());
        let s = |key, check| Setting { key, check };
        vec![
            s("layout", Check::Choice(LAYOUTS.to_vec())),
            s("bars", Check::Choice(vec!["blocks", "shade", "dots", "line"])),
            s("rotate", Check::Int(0, 1440)),
            s("theme", Check::Choice(themes)),
            s("colors", Check::Palette),
            s("clock", Check::Choice(vec!["24h", "12h"])),
            s("seconds", Check::Bool),
            s("date", Check::Choice(vec!["long", "short", "iso", "none"])),
            s("hide", Check::Subset(&SECTIONS)),
            s("prompt", Check::Text),
            s("prompt_color", Check::Choice(vec!["text", "accent", "secondary", "dim"])),
            s("logo_width", Check::Int(12, 120)),
            s("logo_chars", Check::Choice(vec!["blocks", "ascii"])),
            s("logo_colors", Check::LogoColors),
            s("private", Check::Bool),
            s("font", Check::Choice(FONTS.to_vec())),
            s("scale", Check::Float(0.8, 1.1)),
            s("effects", Check::Subset(&EFFECTS)),
            s("logo", Check::Logo),
            s("visualizer", Check::Choice(vec!["off", "bars", "pulse", "both"])),
            s("sound_bars", Check::Int(4, 32)),
        ]
    })
}

pub fn setting(key: &str) -> Option<&'static Setting> {
    spec().iter().find(|s| s.key == key)
}

pub fn style_defaults() -> Map {
    let v = serde_json::json!({
        "layout": "fetch", "bars": "blocks", "rotate": 0, "theme": "auto", "clock": "24h",
        "seconds": true, "date": "long", "hide": [], "private": false, "font": "cascadia",
        "scale": 1.0, "effects": [], "logo": "auto", "prompt_color": "text",
        "logo_width": 40, "logo_chars": "blocks", "logo_colors": "auto",
        "visualizer": "off", "sound_bars": 16
    });
    v.as_object().cloned().unwrap_or_default()
}

pub fn config_defaults() -> Map {
    let mut m = style_defaults();
    m.insert("prompt".into(), Value::from("fastfetch --live"));
    m
}

/// Only known keys with valid values.
pub fn clean_style(raw: Option<&Value>) -> Map {
    let mut out = Map::new();
    let Some(raw) = raw.and_then(Value::as_object) else { return out };
    for s in spec() {
        if let Some(v) = raw.get(s.key).and_then(|v| s.check.check(v)) {
            out.insert(s.key.into(), v);
        }
    }
    out
}

/// [screen.N] tables of termwall.toml, each cleaned like the main settings.
pub fn screen_tables(own: Option<&Map>) -> HashMap<String, Map> {
    let mut out = HashMap::new();
    if let Some(screens) = own.and_then(|o| o.get("screen")).and_then(Value::as_object) {
        for (n, t) in screens {
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && t.is_object() {
                out.insert(n.clone(), clean_style(Some(t)));
            }
        }
    }
    out
}

/// Where a value the page uses comes from, lowest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Default,
    File,
    Livery,
    Screen,
}

impl Source {
    pub fn label(self, screen: Option<&str>) -> String {
        match self {
            Source::Default => "default".into(),
            Source::File => "termwall.toml".into(),
            Source::Livery => "livery (theme.json)".into(),
            Source::Screen => format!("[screen.{}]", screen.unwrap_or("?")),
        }
    }
}

/// The layers the page's look is stacked from, kept apart so `termwall settings` can say where
/// each value comes from: defaults < termwall.toml < theme.json's "style" (livery, per pack) <
/// [screen.N] for the monitor asking.
pub struct Layers {
    pub file: Map,
    pub livery: Map,
    pub screens: HashMap<String, Map>,
}

impl Layers {
    pub fn read(style_path: &Path, theme_path: &Path) -> Layers {
        let themed = read_json(theme_path);
        let mut own = load_config(style_path).0;
        if own.is_none() && is_toml(style_path) && !style_path.exists() {
            own = read_json(&style_path.with_file_name("termwall.json")).and_then(|v| v.as_object().cloned());
        }
        Layers {
            file: clean_style(own.clone().map(Value::Object).as_ref()),
            livery: clean_style(themed.as_ref().and_then(|t| t.get("style"))),
            screens: screen_tables(own.as_ref()),
        }
    }

    /// One value as the page on this screen gets it, and where it comes from.
    pub fn value(&self, key: &str, screen: Option<&str>) -> (Option<Value>, Source) {
        if let Some(v) = screen.and_then(|n| self.screens.get(n)).and_then(|t| t.get(key)) {
            return (Some(v.clone()), Source::Screen);
        }
        if let Some(v) = self.livery.get(key) {
            return (Some(v.clone()), Source::Livery);
        }
        if let Some(v) = self.file.get(key) {
            return (Some(v.clone()), Source::File);
        }
        (style_defaults().get(key).cloned(), Source::Default)
    }

    /// Everything the page on this screen gets.
    pub fn resolve(&self, screen: Option<&str>) -> Map {
        let mut out = style_defaults();
        out.extend(self.file.clone());
        out.extend(self.livery.clone());
        if let Some(t) = screen.and_then(|n| self.screens.get(n)) {
            out.extend(t.clone());
        }
        out
    }
}

/// defaults < termwall.toml < theme.json's "style" (livery, per pack) < [screen.N] for the
/// monitor asking.
pub fn read_style(style_path: &Path, theme_path: &Path, screen: Option<&str>) -> Map {
    Layers::read(style_path, theme_path).resolve(screen)
}

/// `?screen=N` from the page (two digits at most).
pub fn screen_of(path_qs: &str) -> Option<String> {
    let v = util::query_param(path_qs, "screen");
    (!v.is_empty() && v.len() <= 2 && v.chars().all(|c| c.is_ascii_digit())).then_some(v)
}

fn is_toml(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "toml")
}

/// A command-line string as the type the setting takes.
pub fn parse_value(key: &str, value: &str) -> Value {
    let Some(s) = setting(key) else { return Value::from(value) };
    match s.check {
        Check::Bool => match value.to_lowercase().as_str() {
            "on" | "true" | "yes" | "1" => Value::from(true),
            "off" | "false" | "no" | "0" => Value::from(false),
            _ => Value::from(value),
        },
        Check::Subset(_) => {
            if value.is_empty() || value.eq_ignore_ascii_case("none") {
                Value::Array(vec![])
            } else {
                Value::from(value.split(',').map(str::trim).filter(|x| !x.is_empty()).collect::<Vec<_>>())
            }
        }
        Check::Int(..) => {
            if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
                value.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::from(value))
            } else {
                Value::from(value)
            }
        }
        Check::Float(..) => {
            value.trim().parse::<f64>().ok().map(util::num).filter(|v| !v.is_null()).unwrap_or_else(|| Value::from(value))
        }
        _ => Value::from(value),
    }
}

/// What each setting does, for the comments in termwall.toml (the choices come from the spec).
pub fn doc(key: &str) -> &'static str {
    match key {
        "layout" => {
            "How the screen is laid out.\n\
fetch = the fastfetch screen, board = boxes like btop, htop = meters and a big process\n\
table, minimal = the clock and one line, portrait = for a monitor turned on its side,\n\
phone = big gauges for a phone (termwall share uses it when the phone is upright),\n\
logo = the logo as big as the screen, with who you are under it (a second monitor)"
        }
        "bars" => "How meters are drawn: blocks ████, shade ▓▓░░, dots ■■··, line ━━──",
        "rotate" => "Minutes between layouts (fetch, board, minimal, htop, again); 0 = off",
        "theme" => {
            "A built-in palette. auto = follow livery (or another switcher) if it runs, else the\n\
logo's colors, else mint. logo = always the logo's colors: a picture's own (logo =\n\
\"image\") or the shown distro's (logo = \"arch\" on Windows gives Arch's); distro is\n\
the same. Any theme but auto wins over livery; \"colors\" below wins over this.\n\
termwall themes shows them"
        }
        "colors" => "Your own palette: six #rrggbb colors. Wins over \"theme\" and over livery.",
        "clock" => "24h or 12h (am/pm)",
        "seconds" => "Seconds under the clock",
        "date" => "long = monday 5 october, short = mon 5 oct, iso = 2026-10-05, none = no date",
        "hide" => "Sections to hide, e.g. [\"gpu\", \"procs\"]; [] shows everything",
        "prompt" => "The command shown in the prompt line, up to 60 characters",
        "prompt_color" => "The color of that command: one of the palette's colors",
        "private" => "true hides your user name, computer name and local IP (for screenshots and streams)",
        "scale" => "Text size, 0.8 (smaller, fits more) to 1.1 (bigger)",
        "effects" => {
            "Any mix, e.g. [\"crt\", \"glow\"]; [] = none.\n\
crt = scanlines and a dark vignette, glow = soft light around text, flicker = a faint\n\
unsteady brightness, roll = a slow band rolling down the screen, chroma = red/cyan\n\
color fringes, curve = rounded tube corners, noise = film grain"
        }
        "logo" => {
            "The logo. auto = your OS; or any of fastfetch's logos whatever the OS (macOS, ChromeOS,\n\
Windows 95 to 11, the BSDs, hundreds of distros: termwall logos lists them, termwall\n\
logos mac finds the macOS ones, termwall logos macos shows one); custom = your own,\n\
from logo.txt next to this file (up to 40 lines; $1 accent, $2 secondary, $3 text,\n\
$4 dim switch the color from there on; $$ is a $; termwall logo-from PICTURE makes one,\n\
its colors going to logo_colors); image = a picture or an animated\n\
GIF turned into text: logo.gif, .png, .jpg or .webp next to this file"
        }
        "logo_width" => "logo = image: how many characters wide",
        "logo_chars" => {
            "logo = image: blocks = half blocks, two pixels per character (best for pixel art);\n\
ascii = characters from light to dense"
        }
        "logo_colors" => {
            "The logo's colors, apart from the rest of the screen (theme, livery and \"colors\"\n\
stay as they are). own = the logo's own: fastfetch's (a rainbow apple for\n\
\"apple\"), a picture's; palette = the palette's (a picture's light and dark\n\
turned into it); a list = your own colors for $1, $2... of a text logo,\n\
e.g. [\"#e05252\", \"#5fb85f\"]; auto = a picture in its own, text logos in the palette"
        }
        "visualizer" => {
            "The music playing on the PC (Wallpaper Engine only; elsewhere nothing moves).\n\
bars = a spectrum: in layout logo under the logo, in fetch over the clock (hide it there\n\
with hide = [\"sound\"]), in the logo's colors (logo = \"apple\" with logo_colors = \"own\":\n\
a rainbow); pulse = the logo beats with the bass (layout logo); both = the two"
        }
        "sound_bars" => "How many bars the spectrum in fetch has, bass on the left",
        "font" => {
            "The font. cascadia comes with Windows 11; jetbrains, fira and iosevka come with termwall;\n\
consolas and system (the browser's monospace) are always there"
        }
        _ => "",
    }
}

/// A TOML value for the types settings take (JSON strings are valid TOML basic strings).
/// None for what TOML can't write (null).
pub fn toml_value(v: &Value) -> Option<String> {
    Some(match v {
        Value::Bool(b) => {
            if *b {
                "true".into()
            } else {
                "false".into()
            }
        }
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => i.to_string(),
            (_, Some(u)) => u.to_string(),
            _ => py_float(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => match s.strip_prefix(RAW) {
            Some(raw) => raw.to_string(),
            None => serde_json::to_string(s).ok()?,
        },
        Value::Array(a) => {
            let items: Option<Vec<String>> = a.iter().map(toml_value).collect();
            format!("[{}]", items?.join(", "))
        }
        Value::Object(m) => {
            let items: Option<Vec<String>> = m.iter().map(|(k, x)| toml_value(x).map(|x| format!("{k} = {x}"))).collect();
            format!("{{ {} }}", items?.join(", "))
        }
        Value::Null => return None,
    })
}

fn wrapped(prefix: &str, words: &[String], width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = prefix.to_string();
    for w in words {
        if cur.chars().count() + w.chars().count() + 2 > width && cur.trim() != prefix.trim() {
            lines.push(cur.trim_end_matches([',', ' ']).to_string() + ",");
            cur = "#   ".into();
        }
        cur += w;
        cur += ", ";
    }
    lines.push(cur.trim_end_matches([',', ' ']).to_string());
    lines
}

/// termwall.toml with every setting, its current value, what it does and what it accepts.
/// Keys this version doesn't know (from a newer termwall) are kept at the end.
pub fn render_config(cur: &Map) -> String {
    let mut out: Vec<String> = [
        "# termwall settings. Edit a value and save: the wallpaper follows within a second.",
        "# A value termwall doesn't accept is ignored (the default is used); termwall --check",
        "# tells you which line is wrong. Delete this file to get every default back.",
        "#",
        "# With theme = \"auto\", a switcher like livery (theme.json) gives the colors; its per-pack",
        "# look wins over this file. This file is rewritten by termwall --style, so keep notes",
        "# elsewhere.",
        "#",
        "# Several monitors (Wallpaper Engine): give termwall on each monitor a number with its",
        "# \"termwall screen\" property in WE, then a [screen.N] table at the end of this file",
        "# changes any setting for that monitor only, e.g.",
        "#   [screen.2]",
        "#   layout = \"logo\"",
        "",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let defaults = config_defaults();
    for s in spec() {
        let k = s.key;
        for line in doc(k).split('\n') {
            out.push(format!("# {line}"));
        }
        if k == "colors" {
            match cur.get("colors").filter(|c| c.is_object()).and_then(toml_value) {
                Some(v) => out.push(format!("colors = {v}")),
                None => {
                    let example = theme::builtin("mint").map(Value::Object).and_then(|v| toml_value(&v)).unwrap_or_default();
                    out.push(format!("# colors = {example}"));
                }
            }
        } else {
            match k {
                "hide" => out.push(format!("# choices: {}", SECTIONS.join(", "))),
                "effects" => out.push(format!("# choices: {}", EFFECTS.join(", "))),
                "theme" => {
                    let names: Vec<&str> = theme::theme_names().collect();
                    out.push(format!("# choices: auto, logo, distro, {},", names[..5].join(", ")));
                    out.push(format!("#          {}   default: \"auto\"", names[5..].join(", ")));
                }
                "logo" => {
                    out.push("# choices: auto | windows | tux | none | custom | image | a logo id:   default: \"auto\"".into());
                    let cat = logos::catalog();
                    let mut words = cat.featured.clone();
                    words.push(format!("... {} in all: termwall logos", cat.logos.len()));
                    out.extend(wrapped("#   ", &words, 92));
                }
                "prompt" => {}
                _ => {
                    let choices = if matches!(s.check, Check::Bool) { "true | false".to_string() } else { s.check.help() };
                    let def = defaults.get(k).and_then(toml_value).unwrap_or_default();
                    out.push(format!("# choices: {choices}   default: {def}"));
                }
            }
            let v = cur.get(k).and_then(toml_value).or_else(|| defaults.get(k).and_then(toml_value)).unwrap_or_default();
            out.push(format!("{k} = {v}"));
        }
        out.push(String::new());
    }
    let mut tables: Vec<(&String, &Value)> =
        cur.get("screen").and_then(Value::as_object).map(|m| m.iter().collect()).unwrap_or_default();
    tables.sort_by(|a, b| a.0.cmp(b.0));
    for (n, t) in tables {
        if let Some(t) = t.as_object() {
            out.push(format!("[screen.{n}]"));
            for (k, v) in t {
                if let Some(v) = toml_value(v) {
                    out.push(format!("{k} = {v}"));
                }
            }
            out.push(String::new());
        }
    }
    let extra: Vec<(&String, &Value)> = cur.iter().filter(|(k, _)| setting(k).is_none() && k.as_str() != "screen").collect();
    if !extra.is_empty() {
        out.push("# from a newer termwall (kept as they were)".into());
        for (k, v) in extra {
            if let Some(v) = toml_value(v) {
                out.push(format!("{k} = {v}"));
            }
        }
    }
    out.join("\n").trim_end_matches('\n').to_string() + "\n"
}

pub fn write_style(cur: &Map, path: &Path) -> std::io::Result<()> {
    let text = if is_toml(path) { render_config(cur) } else { serde_json::to_string_pretty(cur).unwrap_or_default() + "\n" };
    write_in_place(path, &util::native_newlines(&text))?;
    config_cache().lock().unwrap_or_else(|e| e.into_inner()).remove(path); // two writes in one tick
    Ok(())
}

type ConfigCache = HashMap<PathBuf, ((u128, u64), Option<Map>, Option<String>)>;

fn config_cache() -> &'static Mutex<ConfigCache> {
    static C: OnceLock<Mutex<ConfigCache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A TOML value as the JSON value termwall works with.
fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::from(s.as_str()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) if f.is_finite() => util::num(*f),
        toml::Value::Float(f) => Value::from(format!("{RAW}{}", py_float(*f))),
        toml::Value::Boolean(b) => Value::from(*b),
        toml::Value::Datetime(d) => Value::from(format!("{RAW}{d}")),
        toml::Value::Array(a) => Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Value::Object(t.iter().map(|(k, v)| (k.clone(), toml_to_json(v))).collect()),
    }
}

/// termwall.toml's text as a map, or the parse error with its line and column.
pub fn parse_toml_text(text: &str) -> Result<Map, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match text.parse::<toml::Table>() {
        Ok(t) => Ok(t.iter().map(|(k, v)| (k.clone(), toml_to_json(v))).collect()),
        Err(e) => {
            let msg = e.message().trim().to_string();
            match e.span() {
                Some(span) => {
                    let before = &text[..span.start.min(text.len())];
                    let line = before.matches('\n').count() + 1;
                    let col = before.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0) + 1;
                    Err(format!("{msg} (at line {line}, column {col})"))
                }
                None => Err(msg),
            }
        }
    }
}

/// The settings file as a map, and the parse error if any. A TOML file is re-read only when it
/// changes, and a broken edit keeps the last good settings (the wallpaper doesn't jump to
/// defaults while you're typing).
pub fn load_config(path: &Path) -> (Option<Map>, Option<String>) {
    if !is_toml(path) {
        return (read_json(path).and_then(|v| v.as_object().cloned()), None);
    }
    let Some(st) = stamp(path) else { return (None, None) };
    let mut cache = config_cache().lock().unwrap_or_else(|e| e.into_inner());
    let last_good = match cache.get(path) {
        Some((s, data, err)) if *s == st => return (data.clone(), err.clone()),
        Some((_, data, _)) => data.clone(),
        None => None,
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (data, err) = match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => match parse_toml_text(&text) {
                Ok(m) => (Some(m), None),
                Err(e) => (last_good, Some(format!("{name}: {e}"))),
            },
            Err(e) => (last_good, Some(format!("{name}: {e}"))),
        },
        Err(e) => (last_good, Some(format!("{name}: {e}"))),
    };
    cache.insert(path.to_path_buf(), (st, data.clone(), err.clone()));
    (data, err)
}

/// termwall.toml exists after this. An older termwall.json next to it is carried over once and
/// renamed to termwall.json.old.
pub fn ensure_config(path: &Path) -> std::io::Result<PathBuf> {
    if !is_toml(path) || path.exists() {
        return Ok(path.to_path_buf());
    }
    let legacy = path.with_file_name("termwall.json");
    let raw = read_json(&legacy).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    write_style(&raw, path)?;
    if legacy.exists() {
        let mut old = legacy.clone().into_os_string();
        old.push(".old");
        let _ = fs::rename(&legacy, PathBuf::from(old));
    }
    Ok(path.to_path_buf())
}

/// Rewrite termwall.toml with this version's comments (new choices, new settings) when its values
/// parse; the values stay. True if it was rewritten. A broken file is left alone.
pub fn refresh_config(path: &Path) -> bool {
    if !is_toml(path) {
        return false;
    }
    let (raw, err) = load_config(path);
    let (Some(raw), None) = (raw, err) else { return false };
    let Ok(text) = fs::read_to_string(path) else { return false };
    if render_config(&raw) == util::universal_newlines(&text) {
        return false;
    }
    write_style(&raw, path).is_ok()
}

/// Problems in the settings file, one line each (empty = fine).
pub fn check_config(path: &Path) -> Vec<String> {
    let (raw, err) = load_config(path);
    if let Some(e) = err {
        return vec![e];
    }
    let raw = raw.unwrap_or_default();
    let mut problems = Vec::new();
    let mut items: Vec<(String, Value)> = raw.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if let Some(screens) = raw.get("screen").and_then(Value::as_object) {
        items.retain(|(k, _)| k != "screen");
        for (n, t) in screens {
            match t.as_object() {
                Some(t) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
                    items.extend(t.iter().map(|(k, v)| (format!("screen.{n}.{k}"), v.clone())));
                }
                _ => problems.push(format!("screen.{n}: has to be [screen.NUMBER] with settings under it")),
            }
        }
    }
    for (k, v) in items {
        let k = if k.starts_with("screen.") { k.rsplit('.').next().unwrap_or("").to_string() } else { k };
        match setting(&k) {
            None => problems.push(format!("{k}: unknown setting (ignored)")),
            Some(s) => match s.check.check(&v) {
                None => {
                    let shown = if v.is_object() { "table".into() } else { toml_value(&v).unwrap_or_default() };
                    problems.push(format!("{k} = {shown}: not accepted, the default is used ({})", s.check.help()));
                }
                Some(ok) => {
                    if let (Some(a), Some(b)) = (v.as_array(), ok.as_array())
                        && a.len() != b.len()
                    {
                        problems.push(format!("{k}: some entries ignored ({})", s.check.help()));
                    }
                }
            },
        }
    }
    problems
}

/// One setting written to termwall.toml at once, in the main table or in [screen.N]; None
/// removes it there (a table left empty goes too). The value has to pass the setting's check.
/// Everything else in the file (unknown keys, the other tables) stays. Returns the file's map.
pub fn write_value(path: &Path, screen: Option<&str>, key: &str, value: Option<Value>) -> Result<Map, String> {
    let s = setting(key).ok_or_else(|| format!("unknown setting '{key}'"))?;
    let value = match value {
        Some(v) => Some(s.check.check(&v).ok_or_else(|| format!("{key}: {}", s.check.help()))?),
        None => None,
    };
    if let Some(n) = screen
        && (n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(format!("screen {n}: has to be a number"));
    }
    let path = ensure_config(path).map_err(|e| e.to_string())?;
    let (raw, err) = load_config(&path);
    if let Some(e) = err {
        return Err(format!("{e}: fix that line by hand first"));
    }
    let mut cur = raw.unwrap_or_default();
    let put = |table: &mut Map| match value {
        Some(v) => {
            table.insert(key.into(), v);
        }
        None => {
            table.shift_remove(key);
        }
    };
    match screen {
        None => put(&mut cur),
        Some(n) => {
            let mut screens = cur.get("screen").and_then(Value::as_object).cloned().unwrap_or_default();
            let mut table = screens.get(n).and_then(Value::as_object).cloned().unwrap_or_default();
            put(&mut table);
            if table.is_empty() {
                screens.shift_remove(n);
            } else {
                screens.insert(n.into(), Value::Object(table));
            }
            if screens.is_empty() {
                cur.shift_remove("screen");
            } else {
                cur.insert("screen".into(), Value::Object(screens));
            }
        }
    }
    write_style(&cur, &path).map_err(|e| e.to_string())?;
    Ok(cur)
}

/// A value as `termwall --style` and `termwall settings` show it: on/off, lists comma-separated.
pub fn show_value(key: &str, v: Option<&Value>) -> String {
    let text = match (key, v) {
        ("colors", Some(Value::Object(c))) if !c.is_empty() => c.values().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
        (_, None) | ("colors", _) => String::new(),
        (_, Some(Value::Array(a))) => {
            a.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(",")
        }
        (_, Some(Value::Bool(b))) => (if *b { "on" } else { "off" }).into(),
        (_, Some(Value::String(s))) => s.clone(),
        (_, Some(Value::Number(n))) if n.is_f64() => py_float(n.as_f64().unwrap_or(0.0)),
        (_, Some(v)) => v.to_string(),
    };
    if text.is_empty() { "-".into() } else { text }
}

/// One setting from the command line; "default" removes it, key "reset" clears them all. Keys
/// this version doesn't know (written by a newer termwall or by hand) are kept.
pub fn set_style(key: &str, value: &str, path: &Path) -> Result<Map, String> {
    let path = ensure_config(path).map_err(|e| e.to_string())?;
    let (raw, err) = load_config(&path);
    if let Some(e) = err {
        return Err(format!("{e}\nfix that line by hand (or delete the file to start from the defaults)"));
    }
    let mut cur = raw.unwrap_or_default();
    if key == "reset" {
        cur = Map::new();
    } else if setting(key).is_none() {
        let names: Vec<&str> = spec().iter().map(|s| s.key).collect();
        return Err(format!("unknown setting '{key}' ({})", names.join(", ")));
    } else if value.eq_ignore_ascii_case("default") {
        cur.shift_remove(key);
    } else {
        if key == "colors" {
            return Err("colors: put six #rrggbb colors into termwall.toml by hand, or use --theme".into());
        }
        let s = setting(key).expect("checked above");
        match s.check.check(&parse_value(key, value)) {
            Some(v) => {
                cur.insert(key.into(), v);
            }
            None => return Err(format!("{key}: {}", s.check.help())),
        }
    }
    write_style(&cur, &path).map_err(|e| e.to_string())?;
    Ok(cur)
}

/// Settings a switcher's theme.json "style" sets differently from termwall.toml: those win.
pub fn theme_overrides(style_path: &Path, theme_path: &Path) -> Map {
    let themed = read_json(theme_path);
    let sw = clean_style(themed.as_ref().and_then(|t| t.get("style")));
    let mut own = style_defaults();
    own.extend(clean_style(load_config(style_path).0.map(Value::Object).as_ref()));
    sw.into_iter().filter(|(k, v)| own.get(k) != Some(v)).collect()
}

/// The order THEMES are listed in, for `termwall themes`.
pub fn themes_listing() -> Vec<String> {
    THEMES.iter().map(|(n, c)| format!("  {n:12} {} {} {}", c[0], c[1], c[2])).collect()
}
