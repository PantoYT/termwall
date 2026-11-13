//! Logos: fastfetch's catalog in logos.js, your own logo.txt, and a logo picture.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use unicode_normalization::UnicodeNormalization;

use crate::paths::{self, Paths};
use crate::util::{Map, Value, expandtabs, mtime_ns, splitlines, stamp};

/// What logos.js holds: id -> text ("logos"), id -> its own colors ("own"), id -> fastfetch's
/// names ("names"), name -> id ("aliases"), and the ids termwall.toml names ("featured").
#[derive(Default, Debug)]
pub struct Catalog {
    pub logos: Map,
    pub own: Map,
    pub names: Map,
    pub aliases: Map,
    pub featured: Vec<String>,
}

impl Catalog {
    pub fn is_empty(&self) -> bool {
        self.logos.is_empty()
    }
    pub fn names_of(&self, id: &str) -> Vec<String> {
        self.names
            .get(id)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }
    pub fn own_colors(&self, id: &str) -> Vec<Option<String>> {
        self.own
            .get(id)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }
}

fn parse_catalog(text: &str) -> Catalog {
    let value_of = |var: &str| -> Option<Value> {
        let marker = format!("window.{var} = ");
        let i = text.find(&marker)? + marker.len();
        serde_json::Deserializer::from_str(&text[i..]).into_iter::<Value>().next()?.ok()
    };
    let map = |var: &str| value_of(var).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    Catalog {
        logos: map("DISTRO_LOGOS"),
        own: map("LOGO_OWN_COLORS"),
        names: map("LOGO_NAMES"),
        aliases: map("LOGO_ALIASES"),
        featured: value_of("LOGO_FEATURED")
            .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()))
            .unwrap_or_default(),
    }
}

/// The logos termwall has, from logos.js next to the page. Empty when it can't be read.
pub fn catalog_at(path: &Path) -> Arc<Catalog> {
    type Cache = Option<((PathBuf, u128), Arc<Catalog>)>;
    static CACHE: Mutex<Cache> = Mutex::new(None);
    let Some(mtime) = fs::metadata(path).ok().as_ref().and_then(mtime_ns) else {
        return Arc::new(Catalog::default());
    };
    let key = (path.to_path_buf(), mtime);
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, c)) = guard.as_ref()
        && *k == key
    {
        return c.clone();
    }
    let cat = match fs::read_to_string(path) {
        Ok(text) => Arc::new(parse_catalog(&text)),
        Err(_) => return Arc::new(Catalog::default()),
    };
    *guard = Some((key, cat.clone()));
    cat
}

pub fn catalog() -> Arc<Catalog> {
    catalog_at(&paths::logos_js())
}

pub fn distro_ids(cat: &Catalog) -> Vec<String> {
    cat.logos.keys().cloned().collect()
}

/// "macOS Sequoia" -> "macos-sequoia": accents folded, anything else a dash.
pub fn logo_slug(name: &str) -> String {
    let plain: String = name.nfkd().filter(char::is_ascii).collect::<String>().to_lowercase();
    let mut out = String::new();
    let mut dash = false;
    for c in plain.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    out
}

/// termwall logos WORD...: the ids whose id or fastfetch names contain every word.
pub fn find_logos(words: &[String], cat: &Catalog) -> Vec<String> {
    let words: Vec<String> = words.iter().map(|w| logo_slug(w)).filter(|w| !w.is_empty()).collect();
    cat.logos
        .keys()
        .filter(|k| {
            let mut hay = vec![k.to_string()];
            hay.extend(cat.names_of(k).iter().map(|n| logo_slug(n)));
            let hay = hay.join(" ");
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .cloned()
        .collect()
}

/// Logo text ($1..$9, $$) as 24-bit ANSI with these colors for $1, $2... (None: the terminal's).
pub fn ansi_logo(text: &str, own: &[Option<String>]) -> String {
    let sgr = |n: usize| -> String {
        match own.get(n.wrapping_sub(1)).and_then(|c| c.as_deref()).filter(|_| n > 0) {
            Some(c) if c.len() == 7 => {
                let h = |i: usize| u8::from_str_radix(&c[i..i + 2], 16).unwrap_or(0);
                format!("\x1b[38;2;{};{};{}m", h(1), h(3), h(5))
            }
            _ => "\x1b[39m".to_string(),
        }
    };
    let mut out = Vec::new();
    let mut cur = 1;
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        let mut s = sgr(cur);
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '$' && i + 1 < chars.len() && ('1'..='9').contains(&chars[i + 1]) {
                cur = chars[i + 1].to_digit(10).unwrap_or(1) as usize;
                s += &sgr(cur);
                i += 2;
                continue;
            }
            if chars[i] == '$' && chars.get(i + 1) == Some(&'$') {
                i += 1;
            }
            s.push(chars[i]);
            i += 1;
        }
        out.push(s + "\x1b[0m");
    }
    out.join("\n")
}

/// One logo in this terminal, in its own colors.
pub fn logo_preview(key: &str, cat: &Catalog) -> String {
    let text = cat.logos.get(key).and_then(Value::as_str).unwrap_or("");
    ansi_logo(text, &cat.own_colors(key))
}

/// logo.txt for logo = "custom": up to 40 lines of 120 characters, tabs as spaces. Re-read
/// only when it changes.
pub fn read_logo(path: &Path) -> Option<String> {
    type Cache = HashMap<PathBuf, ((u128, u64), Option<String>)>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let st = stamp(path)?;
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some((s, t)) = cache.get(path)
        && *s == st
    {
        return t.clone();
    }
    let text = fs::read(path).ok().and_then(|b| {
        let s = String::from_utf8_lossy(&b).into_owned();
        let s = s.strip_prefix('\u{feff}').unwrap_or(&s).to_string();
        let lines: Vec<String> =
            splitlines(&expandtabs(&s, 4)).into_iter().take(40).map(|l| l.trim_end().chars().take(120).collect()).collect();
        let joined = lines.join("\n");
        let joined = joined.trim_matches('\n');
        (!joined.is_empty()).then(|| joined.to_string())
    });
    cache.insert(path.to_path_buf(), (st, text.clone()));
    text
}

pub const IMAGE_TYPES: [(&str, &str); 5] =
    [("gif", "image/gif"), ("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("webp", "image/webp")];

/// logo.gif / .png / .jpg / .jpeg / .webp in this folder (the first found), up to 8 MB.
pub fn logo_image(folder: &Path) -> Option<PathBuf> {
    IMAGE_TYPES
        .iter()
        .map(|(ext, _)| folder.join(format!("logo.{ext}")))
        .find(|f| fs::metadata(f).map(|m| m.is_file() && m.len() <= 8 * 1024 * 1024).unwrap_or(false))
}

pub fn logo_image_bytes(paths: &Paths) -> Option<(Vec<u8>, &'static str)> {
    let f = logo_image(&paths.style_dir())?;
    let ext = f.extension()?.to_string_lossy().to_lowercase();
    let kind = IMAGE_TYPES.iter().find(|(e, _)| *e == ext)?.1;
    Some((fs::read(&f).ok()?, kind))
}

/// What the page needs besides stats, style and palette: the custom logo when it's used, or
/// which logo image to load (its mtime, so an edited file is fetched again).
pub fn page_extras(st: &Map, paths: &Paths) -> Map {
    let mut out = Map::new();
    match st.get("logo").and_then(Value::as_str) {
        Some("custom") => {
            out.insert("logo_text".into(), read_logo(&paths.logo_file()).map(Value::from).unwrap_or(Value::Null));
        }
        Some("image") => {
            let m = logo_image(&paths.style_dir()).and_then(|f| fs::metadata(f).ok()).and_then(|m| mtime_ns(&m));
            out.insert("logo_image".into(), m.map(|n| Value::from(n as u64)).unwrap_or(Value::Null));
        }
        _ => {}
    }
    out
}
