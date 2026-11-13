//! Palettes: theme.json written by a switcher (livery), the built-in themes, and which one wins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::util::{Map, Value, read_json, stamp};

pub const THEME_KEYS: [&str; 6] = ["bg", "accent", "secondary", "text", "dim", "faint"];

/// Built-in palettes for when nothing writes theme.json: the six roles termwall uses, taken
/// from each project's published palette (dark variants).
pub const THEMES: [(&str, [&str; 6]); 14] = [
    ("mint", ["#171717", "#b5f4e7", "#7faca3", "#939fa1", "#4b5252", "#242626"]),
    ("catppuccin", ["#1e1e2e", "#cba6f7", "#89b4fa", "#cdd6f4", "#6c7086", "#313244"]),
    ("gruvbox", ["#282828", "#fabd2f", "#8ec07c", "#ebdbb2", "#928374", "#3c3836"]),
    ("nord", ["#2e3440", "#88c0d0", "#81a1c1", "#d8dee9", "#4c566a", "#3b4252"]),
    ("dracula", ["#282a36", "#bd93f9", "#ff79c6", "#f8f8f2", "#6272a4", "#44475a"]),
    ("tokyo-night", ["#1a1b26", "#7aa2f7", "#bb9af7", "#c0caf5", "#565f89", "#24283b"]),
    ("rose-pine", ["#191724", "#ebbcba", "#c4a7e7", "#e0def4", "#6e6a86", "#26233a"]),
    ("everforest", ["#2d353b", "#a7c080", "#83c092", "#d3c6aa", "#859289", "#343f44"]),
    ("kanagawa", ["#1f1f28", "#7e9cd8", "#957fb8", "#dcd7ba", "#727169", "#2a2a37"]),
    ("solarized", ["#002b36", "#268bd2", "#2aa198", "#93a1a1", "#586e75", "#073642"]),
    ("one-dark", ["#282c34", "#61afef", "#c678dd", "#abb2bf", "#5c6370", "#2c313a"]),
    ("monokai", ["#272822", "#a6e22e", "#f92672", "#f8f8f2", "#75715e", "#3e3d32"]),
    ("amber", ["#120d02", "#ffb000", "#cc8c00", "#e0a020", "#6b4a00", "#241a04"]),
    ("phosphor", ["#050f07", "#33ff66", "#1fbf4a", "#2fe05a", "#145c27", "#0a1f0e"]),
];

pub fn theme_names() -> impl Iterator<Item = &'static str> {
    THEMES.iter().map(|(n, _)| *n)
}

/// A built-in palette as the page receives it.
pub fn builtin(name: &str) -> Option<Map> {
    let (_, colors) = THEMES.iter().find(|(n, _)| *n == name)?;
    Some(THEME_KEYS.iter().zip(colors).map(|(k, c)| (k.to_string(), Value::from(*c))).collect())
}

pub fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Six #rrggbb colors under the six role names (they go into CSS, so nothing else passes).
pub fn valid_theme(t: &Value) -> bool {
    let Some(m) = t.as_object() else { return false };
    THEME_KEYS.iter().all(|k| m.get(*k).and_then(Value::as_str).is_some_and(is_hex_color))
}

/// Only the six keys, in their order.
pub fn palette_of(t: &Value) -> Option<Map> {
    if !valid_theme(t) {
        return None;
    }
    Some(THEME_KEYS.iter().map(|k| (k.to_string(), t[*k].clone())).collect())
}

type ThemeCache = HashMap<PathBuf, ((u128, u64), Option<Map>)>;

/// The palette in theme.json, re-read only when the file changes. Invalid gives None (the page
/// keeps its defaults).
pub fn read_theme(path: &Path) -> Option<Map> {
    static CACHE: Mutex<Option<ThemeCache>> = Mutex::new(None);
    let st = stamp(path)?;
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some((s, t)) = cache.get(path)
        && *s == st
    {
        return t.clone();
    }
    let t = read_json(path).and_then(|v| palette_of(&v));
    cache.insert(path.to_path_buf(), (st, t.clone()));
    t
}

/// Which palette wins: your own "colors", then a built-in "theme" you picked, then theme.json
/// (livery or another switcher), then None (the page uses the distro's colors on Linux, its
/// defaults on Windows). theme "auto" is what lets a switcher color termwall.
pub fn resolve_palette(style: &Map, theme_path: &Path) -> Option<Map> {
    if let Some(c) = style.get("colors").and_then(Value::as_object).filter(|c| !c.is_empty()) {
        return Some(c.clone());
    }
    let theme = style.get("theme").and_then(Value::as_str).unwrap_or("auto");
    match theme {
        "logo" | "distro" => None, // the page colors itself from the logo
        "auto" => read_theme(theme_path),
        name => builtin(name),
    }
}
