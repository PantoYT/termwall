//! The model behind `termwall settings`: its rows are the style spec sorted into groups, each
//! value with where it comes from, the next and previous values, and the writes, which go
//! through the same writer as `termwall --style`. No terminal code here (tui.rs draws it), so
//! all of this is tested without a terminal.

use crate::host::{self, Device, Options};
use crate::logos;
use crate::paths::Paths;
use crate::style::{self, Check, Layers, Source};
use crate::theme;
use crate::util::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Look,
    Logo,
    Colors,
    Sections,
    Music,
    Host,
}

impl Group {
    pub const ALL: [Group; 6] = [Group::Look, Group::Logo, Group::Colors, Group::Sections, Group::Music, Group::Host];

    pub fn title(self) -> &'static str {
        match self {
            Group::Look => "look",
            Group::Logo => "logo",
            Group::Colors => "colors",
            Group::Sections => "sections",
            Group::Music => "music",
            Group::Host => "host",
        }
    }
}

/// Where a setting of the spec is shown. None for a key nobody sorted in: a test fails then, so
/// a new setting can't go missing from `termwall settings`.
pub fn group_of(key: &str) -> Option<Group> {
    Some(match key {
        "layout" | "bars" | "rotate" | "font" | "scale" | "effects" | "clock" | "seconds" | "date" | "prompt"
        | "prompt_color" | "private" => Group::Look,
        "logo" | "logo_width" | "logo_chars" | "logo_colors" => Group::Logo,
        "theme" | "colors" => Group::Colors,
        "hide" => Group::Sections,
        "visualizer" | "sound_bars" => Group::Music,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Setting(&'static str),
    /// termwall host at logon (Windows)
    Autostart,
    /// which monitor the host covers
    Monitor,
    /// which playback device the host's visualizer hears
    AudioDevice,
}

impl Row {
    pub fn label(self) -> &'static str {
        match self {
            Row::Setting(k) => k,
            Row::Autostart => "autostart",
            Row::Monitor => "monitors",
            Row::AudioDevice => "audio device",
        }
    }
}

/// The order rows are shown in (the most used first); a key not listed comes after, in the
/// spec's order.
const SHOWN_FIRST: [&str; 13] = [
    "layout",
    "font",
    "scale",
    "effects",
    "bars",
    "clock",
    "seconds",
    "date",
    "logo",
    "logo_colors",
    "theme",
    "visualizer",
    "hide",
];

pub fn rows(g: Group) -> Vec<Row> {
    let mut keys: Vec<&'static str> = style::spec().iter().filter(|s| group_of(s.key) == Some(g)).map(|s| s.key).collect();
    keys.sort_by_key(|k| SHOWN_FIRST.iter().position(|x| x == k).unwrap_or(SHOWN_FIRST.len()));
    let mut out: Vec<Row> = keys.into_iter().map(Row::Setting).collect();
    match g {
        Group::Music => out.push(Row::AudioDevice),
        Group::Host => out.extend([Row::Autostart, Row::Monitor]),
        _ => {}
    }
    out
}

/// How a row is changed beyond left and right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edit {
    /// left/right (and Enter) step through the values
    Cycle,
    /// a list of boxes to tick
    Checklist(&'static [&'static str]),
    /// the logo picker with search and preview
    Logo,
    /// the theme picker with swatches
    Theme,
    /// a line of text
    Line,
}

pub fn edit_kind(row: Row) -> Edit {
    let Row::Setting(key) = row else { return Edit::Cycle };
    match style::setting(key).map(|s| &s.check) {
        Some(Check::Subset(list)) => Edit::Checklist(list),
        Some(Check::Logo) => Edit::Logo,
        Some(Check::Text | Check::Palette | Check::LogoColors) => Edit::Line,
        _ if key == "theme" => Edit::Theme,
        _ => Edit::Cycle,
    }
}

fn step(key: &str) -> f64 {
    match key {
        "rotate" => 5.0,
        "logo_width" => 2.0,
        "scale" => 0.05,
        _ => 1.0,
    }
}

/// The value next to this one (dir 1) or before it (dir -1). Lists wrap, numbers stop at their
/// ends. None when the setting isn't stepped through (lists, text, the palette).
pub fn cycle(key: &str, cur: Option<&Value>, dir: i32) -> Option<Value> {
    let s = style::setting(key)?;
    let pick = |list: &[&str]| -> Value {
        let at = cur.and_then(Value::as_str).and_then(|c| list.iter().position(|x| *x == c));
        let n = list.len() as i32;
        let next = match at {
            Some(i) => (i as i32 + dir).rem_euclid(n),
            None if dir < 0 => n - 1,
            None => 0,
        };
        Value::from(list[next as usize])
    };
    match &s.check {
        Check::Choice(list) => Some(pick(list)),
        Check::Bool => Some(Value::from(!cur.and_then(Value::as_bool).unwrap_or(false))),
        Check::Int(lo, hi) => {
            let v = cur.and_then(Value::as_i64).unwrap_or(*lo);
            Some(Value::from((v + step(key) as i64 * i64::from(dir)).clamp(*lo, *hi)))
        }
        Check::Float(lo, hi) => {
            let v = cur.and_then(Value::as_f64).unwrap_or(*lo);
            let next = ((v + step(key) * f64::from(dir)) * 100.0).round() / 100.0;
            Some(crate::util::num(next.clamp(*lo, *hi)))
        }
        Check::LogoColors => Some(pick(&["auto", "own", "palette"])),
        Check::Logo => {
            let mut list = vec!["auto", "windows", "tux", "none", "custom", "image"];
            let cat = logos::catalog();
            list.extend(cat.featured.iter().map(String::as_str));
            Some(pick(&list))
        }
        _ => None,
    }
}

/// A line typed for a setting: Ok(None) puts the default back (an empty line, or "default").
pub fn parse_line(key: &str, text: &str) -> Result<Option<Value>, String> {
    let s = style::setting(key).ok_or_else(|| format!("unknown setting '{key}'"))?;
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("default") || (key == "colors" && t.eq_ignore_ascii_case("none")) {
        return Ok(None);
    }
    let v = if key == "colors" {
        let parts: Vec<&str> = t.split(|c: char| c == ',' || c.is_whitespace()).filter(|x| !x.is_empty()).collect();
        if parts.len() != 6 {
            return Err("colors: six #rrggbb (bg accent secondary text dim faint), or none".into());
        }
        Value::Object(theme::THEME_KEYS.iter().zip(&parts).map(|(k, c)| (k.to_string(), Value::from(c.to_lowercase()))).collect())
    } else {
        style::parse_value(key, t)
    };
    s.check.check(&v).map(Some).ok_or_else(|| format!("{key}: {}", s.check.help()))
}

/// What a row shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub value: String,
    /// where the value comes from: default, termwall.toml, livery (theme.json), [screen.N]
    pub source: String,
    /// the value the page gets
    pub effective: Option<Value>,
    /// what this scope's own layer holds (what a change starts from)
    pub written: Option<Value>,
    /// the trap on this row, if any: livery wins, a screen table differs...
    pub note: Option<String>,
    pub overridden: bool,
}

/// When termwall.toml and theme.json last changed.
type Stamps = (Option<(u128, u64)>, Option<(u128, u64)>);

pub struct Model {
    pub paths: Paths,
    pub layers: Layers,
    /// None: the main table (every screen); Some(n): [screen.n]
    pub scope: Option<String>,
    pub monitors: usize,
    pub host_entry: Option<String>,
    pub host_opts: Options,
    pub devices: Vec<Device>,
    stamp: Stamps,
}

pub fn monitor_count() -> usize {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};
        // SAFETY: a plain query.
        (unsafe { GetSystemMetrics(SM_CMONITORS) }).max(1) as usize
    }
    #[cfg(not(windows))]
    2
}

impl Model {
    pub fn new(paths: &Paths, with_host: bool) -> Model {
        let _ = style::ensure_config(&paths.style_file);
        let (host_entry, host_opts) = if with_host {
            host::autostart_state().map(|(c, o)| (Some(c), o)).unwrap_or_default()
        } else {
            (None, Options::default())
        };
        let mut m = Model {
            paths: paths.clone(),
            layers: Layers::read(&paths.style_file, &paths.theme_file),
            scope: None,
            monitors: if with_host { monitor_count() } else { 2 },
            host_entry,
            host_opts,
            devices: if with_host { host::playback_devices(300) } else { Vec::new() },
            stamp: (None, None),
        };
        m.stamp = m.stamps();
        m
    }

    fn stamps(&self) -> Stamps {
        (crate::util::stamp(&self.paths.style_file), crate::util::stamp(&self.paths.theme_file))
    }

    /// Re-read the files when they changed (by hand, by livery switching a pack). True if so.
    pub fn refresh(&mut self) -> bool {
        let now = self.stamps();
        if now == self.stamp {
            return false;
        }
        self.stamp = now;
        self.layers = Layers::read(&self.paths.style_file, &self.paths.theme_file);
        true
    }

    fn reload(&mut self) {
        self.layers = Layers::read(&self.paths.style_file, &self.paths.theme_file);
        self.stamp = self.stamps();
    }

    /// The screens a scope can be: those with a table, and one per monitor.
    pub fn screens(&self) -> Vec<String> {
        let mut out: Vec<usize> = self.layers.screens.keys().filter_map(|k| k.parse().ok()).collect();
        out.extend(1..=self.monitors.max(1));
        out.sort_unstable();
        out.dedup();
        out.into_iter().map(|n| n.to_string()).collect()
    }

    /// All screens, then screen 1, 2...
    pub fn next_scope(&mut self) {
        let screens = self.screens();
        self.scope = match &self.scope {
            None => screens.first().cloned(),
            Some(n) => screens.iter().position(|s| s == n).and_then(|i| screens.get(i + 1).cloned()),
        };
    }

    pub fn scope_label(&self) -> String {
        match &self.scope {
            None => "every screen (termwall.toml)".into(),
            Some(n) => format!("screen {n} ([screen.{n}])"),
        }
    }

    pub fn view(&self, row: Row) -> View {
        match row {
            Row::Setting(key) => self.setting_view(key),
            Row::Autostart => View {
                value: if self.host_entry.is_some() { "on".into() } else { "off".into() },
                source: if self.host_entry.is_some() { "autostart entry".into() } else { "-".into() },
                note: self.host_entry.clone(),
                ..View::default()
            },
            Row::Monitor => View {
                value: self.host_opts.monitor.map(|n| format!("screen {n} only")).unwrap_or_else(|| "all".into()),
                source: self.host_source(),
                ..View::default()
            },
            Row::AudioDevice => View {
                value: self.host_opts.audio_device.clone().unwrap_or_else(|| "follow the sound".into()),
                source: self.host_source(),
                note: Some(self.audio_note()),
                ..View::default()
            },
        }
    }

    fn host_source(&self) -> String {
        if self.host_entry.is_some() { "autostart entry".into() } else { "applies when the host is on".into() }
    }

    fn audio_note(&self) -> String {
        let mut parts = vec![
            "termwall host only; Wallpaper Engine hears Windows' default output or the device picked in its own settings"
                .to_string(),
        ];
        for d in &self.devices {
            let mut m = Vec::new();
            if d.default {
                m.push("default");
            }
            if d.plays() {
                m.push("playing");
            }
            parts.push(if m.is_empty() { d.name.clone() } else { format!("{} ({})", d.name, m.join(", ")) });
        }
        parts.join("\n")
    }

    fn setting_view(&self, key: &str) -> View {
        let scope = self.scope.as_deref();
        let (effective, src) = self.layers.value(key, scope);
        let written = match scope {
            None => self.layers.file.get(key).cloned(),
            Some(n) => self.layers.screens.get(n).and_then(|t| t.get(key)).cloned(),
        };
        let mut notes = Vec::new();
        let mut overridden = false;
        if src == Source::Livery && scope.is_none() {
            overridden = true;
            notes.push(match &written {
                Some(w) if Some(w) != effective.as_ref() => format!(
                    "termwall.toml says {}, but livery's pack (theme.json \"style\") sets {} and wins; a change here shows only \
without that pack (livery termwall look PACK none)",
                    style::show_value(key, Some(w)),
                    style::show_value(key, effective.as_ref())
                ),
                _ => "set by livery's pack (theme.json \"style\"), which wins over termwall.toml; a change here shows only \
without that pack (livery termwall look PACK none)"
                    .into(),
            });
        }
        if scope.is_none() {
            let mut own: Vec<&String> = self.layers.screens.iter().filter(|(_, t)| t.contains_key(key)).map(|(n, _)| n).collect();
            own.sort();
            if !own.is_empty() {
                let list: Vec<String> = own.iter().map(|n| format!("[screen.{n}]")).collect();
                notes.push(format!("{} set their own, so those screens keep it", list.join(", ")));
            }
        }
        if key == "theme" || key == "colors" {
            let st = self.layers.resolve(scope);
            let has_colors = st.get("colors").and_then(Value::as_object).is_some_and(|c| !c.is_empty());
            let auto = st.get("theme").and_then(Value::as_str).unwrap_or("auto") == "auto";
            if has_colors && key == "theme" {
                notes.push("\"colors\" is set and wins over the theme".into());
            } else if auto && !has_colors && theme::read_theme(&self.paths.theme_file).is_some() {
                notes.push("theme \"auto\": the colors come from theme.json (livery or termwall palette)".into());
            }
        }
        // termwall.toml spells out every default, so a value equal to it reads as the default
        let is_default = src == Source::File && effective.as_ref() == style::config_defaults().get(key);
        View {
            value: style::show_value(key, effective.as_ref()),
            source: if is_default { "default".into() } else { src.label(scope) },
            effective,
            written,
            note: (!notes.is_empty()).then(|| notes.join("\n")),
            overridden,
        }
    }

    /// The palette the page shows on this scope's screen, if termwall knows it (the swatches).
    pub fn palette(&self) -> Option<Map> {
        theme::resolve_palette(&self.layers.resolve(self.scope.as_deref()), &self.paths.theme_file)
    }

    /// Write one setting in this scope (None: back to what's below), then read the files again.
    /// Says what happened, including when the change won't show.
    pub fn set(&mut self, key: &str, value: Option<Value>) -> Result<String, String> {
        let shown = style::show_value(key, value.as_ref());
        style::write_value(&self.paths.style_file, self.scope.as_deref(), key, value.clone())?;
        self.reload();
        let place = match &self.scope {
            None => "termwall.toml".to_string(),
            Some(n) => format!("[screen.{n}]"),
        };
        let what = if value.is_some() { format!("{key} = {shown} in {place}") } else { format!("{key}: removed from {place}") };
        let v = self.setting_view(key);
        Ok(if v.overridden { format!("{what}; livery's pack still wins, so the wallpaper keeps {}", v.value) } else { what })
    }

    /// Left/right on a row.
    pub fn step(&mut self, row: Row, dir: i32) -> Result<String, String> {
        match row {
            Row::Setting(key) => {
                let v = self.view(row);
                // start from what this scope holds; livery's value is not what is written here
                let from = v.written.clone().or(if v.overridden { None } else { v.effective.clone() });
                let from = from.or_else(|| style::style_defaults().get(key).cloned());
                match cycle(key, from.as_ref(), dir) {
                    Some(next) if Some(&next) != v.written.as_ref() || v.written.is_none() => self.set(key, Some(next)),
                    Some(_) => Ok(format!("{key}: at its end")),
                    None => Err(format!("{key}: Enter to change it")),
                }
            }
            Row::Autostart => self.set_host(Some(self.host_entry.is_none()), self.host_opts.clone()),
            Row::Monitor => {
                let n = self.monitors.max(1) as i64;
                let cur = self.host_opts.monitor.map(|m| m as i64).unwrap_or(0);
                let next = (cur + i64::from(dir)).rem_euclid(n + 1);
                let mut o = self.host_opts.clone();
                o.monitor = (next > 0).then_some(next as usize);
                self.set_host(None, o)
            }
            Row::AudioDevice => {
                let mut names: Vec<Option<String>> = vec![None];
                names.extend(self.devices.iter().map(|d| Some(d.name.clone())));
                let at = names.iter().position(|n| *n == self.host_opts.audio_device).unwrap_or(0) as i32;
                let next = (at + dir).rem_euclid(names.len() as i32) as usize;
                let mut o = self.host_opts.clone();
                o.audio_device = names[next].clone();
                self.set_host(None, o)
            }
        }
    }

    /// The host's options and autostart: rewritten (and the host restarted) when it is on.
    fn set_host(&mut self, on: Option<bool>, opts: Options) -> Result<String, String> {
        let turn = on.unwrap_or(self.host_entry.is_some());
        self.host_opts = Options { autostart: None, seconds: None, window: false, ..opts };
        if on.is_none() && self.host_entry.is_none() {
            return Ok("kept for when the host is turned on (autostart)".into());
        }
        let msg = host::set_autostart(turn, &self.host_opts)?;
        self.host_entry = host::autostart_state().map(|(c, _)| c);
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TempDir;

    #[test]
    fn every_setting_has_a_group() {
        for s in style::spec() {
            assert!(group_of(s.key).is_some(), "{} is in no group of termwall settings", s.key);
        }
        let all: usize = Group::ALL.iter().map(|g| rows(*g).iter().filter(|r| matches!(r, Row::Setting(_))).count()).sum();
        assert_eq!(all, style::spec().len(), "each setting shown once");
        assert_eq!(edit_kind(Row::Setting("hide")), Edit::Checklist(&style::SECTIONS));
        assert_eq!(edit_kind(Row::Setting("effects")), Edit::Checklist(&style::EFFECTS));
        assert_eq!(edit_kind(Row::Setting("logo")), Edit::Logo);
        assert_eq!(edit_kind(Row::Setting("theme")), Edit::Theme);
        assert_eq!(edit_kind(Row::Setting("prompt")), Edit::Line);
        assert_eq!(edit_kind(Row::Setting("layout")), Edit::Cycle);
    }

    #[test]
    fn values_step_through_their_choices() {
        let s = |v: &str| Value::from(v);
        assert_eq!(cycle("layout", Some(&s("fetch")), 1), Some(s("board")));
        assert_eq!(cycle("layout", Some(&s("fetch")), -1), Some(s("logo")), "lists wrap");
        assert_eq!(cycle("layout", Some(&s("nonsense")), 1), Some(s("fetch")));
        assert_eq!(cycle("seconds", Some(&Value::from(true)), 1), Some(Value::from(false)));
        assert_eq!(cycle("sound_bars", Some(&Value::from(16)), 1), Some(Value::from(17)));
        assert_eq!(cycle("sound_bars", Some(&Value::from(32)), 1), Some(Value::from(32)), "numbers stop at the end");
        assert_eq!(cycle("rotate", Some(&Value::from(0)), -1), Some(Value::from(0)));
        assert_eq!(cycle("rotate", Some(&Value::from(0)), 1), Some(Value::from(5)));
        assert_eq!(cycle("scale", Some(&Value::from(1.0)), 1), Some(Value::from(1.05)));
        assert_eq!(cycle("scale", Some(&Value::from(0.8)), -1), Some(Value::from(0.8)));
        assert_eq!(cycle("logo_colors", Some(&Value::from(vec!["#112233"])), 1), Some(s("auto")));
        assert_eq!(cycle("hide", None, 1), None, "lists are ticked, not stepped");
        assert_eq!(cycle("prompt", None, 1), None);
    }

    #[test]
    fn typed_lines() {
        assert_eq!(parse_line("prompt", "neofetch"), Ok(Some(Value::from("neofetch"))));
        assert_eq!(parse_line("prompt", "  "), Ok(None), "empty is the default");
        let c = parse_line("colors", "#000000 #111111, #222222 #333333 #444444 #555555").unwrap().unwrap();
        assert_eq!(c["bg"], "#000000");
        assert_eq!(c["faint"], "#555555");
        assert!(parse_line("colors", "#000000").is_err());
        assert_eq!(parse_line("colors", "none"), Ok(None));
        assert_eq!(parse_line("logo_colors", "#ff0000, #00ff00").unwrap(), Some(Value::from(vec!["#ff0000", "#00ff00"])));
        assert!(parse_line("logo_colors", "red").is_err());
    }

    #[test]
    fn sources_and_write_through() {
        let td = TempDir::new().expect("temp dir");
        let paths = Paths::in_dir(td.path());
        let mut m = Model::new(&paths, false);
        assert_eq!(m.view(Row::Setting("layout")).source, "default");
        m.step(Row::Setting("layout"), 1).expect("write");
        let v = m.view(Row::Setting("layout"));
        assert_eq!((v.value.as_str(), v.source.as_str()), ("board", "termwall.toml"));
        let text = std::fs::read_to_string(&paths.style_file).unwrap();
        assert!(text.contains("layout = \"board\"") && text.contains("# How the screen is laid out."), "the writer of --style");

        // livery's pack sets the layout: it wins, and the row says so
        std::fs::write(&paths.theme_file, r#"{"style": {"layout": "logo", "font": "fira"}}"#).unwrap();
        assert!(m.refresh(), "a changed theme.json is noticed");
        let v = m.view(Row::Setting("layout"));
        assert_eq!((v.value.as_str(), v.source.as_str(), v.overridden), ("logo", "livery (theme.json)", true));
        assert!(v.note.unwrap().contains("termwall.toml says board"));
        let msg = m.step(Row::Setting("layout"), 1).expect("written anyway");
        assert!(msg.contains("livery's pack still wins"), "{msg}");
        assert_eq!(m.layers.file["layout"], "minimal", "stepped from the file's value, not livery's");

        // [screen.2] wins over livery for that screen only
        m.scope = Some("2".into());
        m.set("layout", Some(Value::from("htop"))).expect("screen table");
        let v = m.view(Row::Setting("layout"));
        assert_eq!((v.value.as_str(), v.source.as_str()), ("htop", "[screen.2]"));
        m.scope = None;
        assert!(m.view(Row::Setting("layout")).note.unwrap().contains("[screen.2] set their own"));
        let text = std::fs::read_to_string(&paths.style_file).unwrap();
        assert!(text.contains("[screen.2]\nlayout = \"htop\"") || text.contains("[screen.2]\r\nlayout = \"htop\""), "{text}");

        // removing the last key of a table removes the table
        m.scope = Some("2".into());
        m.set("layout", None).unwrap();
        assert!(m.layers.screens.is_empty());
        assert!(!std::fs::read_to_string(&paths.style_file).unwrap().lines().any(|l| l.starts_with("[screen.")));

        // a value the spec doesn't accept is refused and nothing is written
        m.scope = None;
        let before = std::fs::read_to_string(&paths.style_file).unwrap();
        assert!(m.set("sound_bars", Some(Value::from(99))).is_err());
        assert_eq!(std::fs::read_to_string(&paths.style_file).unwrap(), before);
    }

    #[test]
    fn scopes_cover_the_monitors_and_tables() {
        let td = TempDir::new().expect("temp dir");
        let paths = Paths::in_dir(td.path());
        std::fs::write(&paths.style_file, "[screen.4]\nlayout = \"logo\"\n").unwrap();
        let mut m = Model::new(&paths, false);
        assert_eq!(m.screens(), ["1", "2", "4"]);
        let mut seen = vec![m.scope.clone()];
        for _ in 0..4 {
            m.next_scope();
            seen.push(m.scope.clone());
        }
        assert_eq!(seen, [None, Some("1".into()), Some("2".into()), Some("4".into()), None]);
    }
}
