//! `termwall settings`: every setting in the terminal. Each change is written to termwall.toml
//! at once (settings.rs, the writer of `--style`); the wallpaper polls /theme four times a
//! second, so the wallpaper is the preview. Keyboard first, the mouse works too.

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::logos::{self, Catalog};
use crate::paths::Paths;
use crate::settings::{self, Edit, Group, Model, Row};
use crate::style;
#[cfg(test)]
use crate::style::Layers;
use crate::theme;
use crate::util::Value;

enum Mode {
    Browse,
    Line { key: &'static str, text: String, cursor: usize },
    Check { key: &'static str, list: &'static [&'static str], at: usize },
    Logo { query: String, found: Vec<String>, at: usize },
    Theme { at: usize, before: Option<Value> },
    Blocks(Grid),
}

/// What a pick from the list of blocks does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pick {
    Add,
    Replace,
    NewRow,
}

/// The blocks editor: rows of "name" or "name:N", a cursor, and the list it picks from.
#[derive(Clone, Debug, PartialEq)]
struct Grid {
    rows: Vec<Vec<String>>,
    r: usize,
    c: usize,
    choose: Option<(Pick, usize)>,
}

impl Grid {
    fn from_value(v: Option<&Value>) -> Grid {
        let rows: Vec<Vec<String>> = v
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| r.as_array().map(|r| r.iter().filter_map(|x| x.as_str().map(String::from)).collect()))
                    .filter(|r: &Vec<String>| !r.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let rows = if rows.is_empty() { vec![vec!["clock".to_string()]] } else { rows };
        Grid { rows, r: 0, c: 0, choose: None }
    }

    fn value(&self) -> Value {
        Value::from(self.rows.iter().map(|r| Value::from(r.clone())).collect::<Vec<_>>())
    }

    fn split(item: &str) -> (&str, u8) {
        match item.split_once(':') {
            Some((n, w)) => (n, w.parse().unwrap_or(1)),
            None => (item, 1),
        }
    }

    fn join(name: &str, w: u8) -> String {
        if w <= 1 { name.to_string() } else { format!("{name}:{w}") }
    }

    fn clamp(&mut self) {
        self.r = self.r.min(self.rows.len().saturating_sub(1));
        self.c = self.c.min(self.rows[self.r].len().saturating_sub(1));
    }

    /// One editing key. True when the grid changed (and has to be written).
    fn key(&mut self, code: KeyCode, shift: bool, names: &[String]) -> bool {
        if let Some((pick, at)) = self.choose {
            match code {
                KeyCode::Up | KeyCode::Char('k') => self.choose = Some((pick, at.saturating_sub(1))),
                KeyCode::Down | KeyCode::Char('j') => self.choose = Some((pick, (at + 1).min(names.len().saturating_sub(1)))),
                KeyCode::Esc => self.choose = None,
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.choose = None;
                    let Some(name) = names.get(at).cloned() else { return false };
                    match pick {
                        Pick::Add => {
                            let at = (self.c + 1).min(self.rows[self.r].len());
                            if self.rows[self.r].len() < 6 {
                                self.rows[self.r].insert(at, name);
                                self.c = at;
                            }
                        }
                        Pick::Replace => {
                            let w = Self::split(&self.rows[self.r][self.c]).1;
                            self.rows[self.r][self.c] = Self::join(&name, w);
                        }
                        Pick::NewRow => {
                            if self.rows.len() < 8 {
                                self.rows.insert(self.r + 1, vec![name]);
                                self.r += 1;
                                self.c = 0;
                            }
                        }
                    }
                    return true;
                }
                _ => {}
            }
            return false;
        }
        let open = |g: &mut Grid, pick: Pick| {
            let cur = Self::split(&g.rows[g.r][g.c]).0.to_string();
            g.choose = Some((pick, names.iter().position(|n| *n == cur).unwrap_or(0)));
        };
        let changed = match code {
            KeyCode::Left | KeyCode::Char('<') if shift || code == KeyCode::Char('<') => {
                if self.c > 0 {
                    self.rows[self.r].swap(self.c, self.c - 1);
                    self.c -= 1;
                    true
                } else {
                    false
                }
            }
            KeyCode::Right | KeyCode::Char('>') if shift || code == KeyCode::Char('>') => {
                if self.c + 1 < self.rows[self.r].len() {
                    self.rows[self.r].swap(self.c, self.c + 1);
                    self.c += 1;
                    true
                } else {
                    false
                }
            }
            KeyCode::Up | KeyCode::Down if shift => {
                let to = if code == KeyCode::Up { self.r.checked_sub(1) } else { Some(self.r + 1) };
                match to.filter(|t| *t < self.rows.len() && self.rows[*t].len() < 6) {
                    Some(t) => {
                        let item = self.rows[self.r].remove(self.c);
                        self.rows[t].push(item);
                        if self.rows[self.r].is_empty() {
                            self.rows.remove(self.r);
                        }
                        self.r = if self.rows.len() > t { t } else { t - 1 }.min(self.rows.len() - 1);
                        self.c = self.rows[self.r].len() - 1;
                        true
                    }
                    None => false,
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.c = self.c.saturating_sub(1);
                false
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.c += 1;
                false
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.r = self.r.saturating_sub(1);
                false
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.r += 1;
                false
            }
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('-') => {
                let (name, w) = Self::split(&self.rows[self.r][self.c]);
                let w = if code == KeyCode::Char('-') { w.saturating_sub(1).max(1) } else { (w + 1).min(6) };
                let next = Self::join(name, w);
                let changed = next != self.rows[self.r][self.c];
                self.rows[self.r][self.c] = next;
                changed
            }
            KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('x') => {
                if self.rows.iter().map(Vec::len).sum::<usize>() > 1 {
                    self.rows[self.r].remove(self.c);
                    if self.rows[self.r].is_empty() {
                        self.rows.remove(self.r);
                    }
                    true
                } else {
                    false
                }
            }
            KeyCode::Char('a') => {
                open(self, Pick::Add);
                false
            }
            KeyCode::Char('n') => {
                open(self, Pick::NewRow);
                false
            }
            KeyCode::Enter | KeyCode::Char('r') => {
                open(self, Pick::Replace);
                false
            }
            _ => false,
        };
        self.clamp();
        changed
    }
}

struct App {
    m: Model,
    group: usize,
    row: usize,
    offset: usize,
    mode: Mode,
    status: String,
    error: bool,
    cat: Arc<Catalog>,
    quit: bool,
    // where things were drawn, for the mouse
    groups_area: Rect,
    rows_area: Rect,
    list_area: Rect,
    list_offset: usize,
}

const SPECIAL_LOGOS: [(&str, &str); 6] = [
    ("auto", "your system's logo"),
    ("windows", "the Windows logo"),
    ("tux", "Tux"),
    ("none", "no logo"),
    ("custom", "logo.txt next to termwall.toml"),
    ("image", "logo.gif/png/jpg/webp next to termwall.toml"),
];

fn hex(c: &str) -> Option<Color> {
    if !theme::is_hex_color(c) {
        return None;
    }
    let h = |i: usize| u8::from_str_radix(&c[i..i + 2], 16).ok();
    Some(Color::Rgb(h(1)?, h(3)?, h(5)?))
}

fn swatches(colors: &[&str]) -> Vec<Span<'static>> {
    colors.iter().filter_map(|c| hex(c)).map(|c| Span::styled("██", Style::default().fg(c))).collect()
}

/// A logo's text ($1..$9 switch colors, $$ is a $) as lines in those colors.
fn logo_lines(text: &str, own: &[Option<String>]) -> Vec<Line<'static>> {
    let color = |n: usize| own.get(n.wrapping_sub(1)).and_then(|c| c.as_deref()).and_then(hex);
    let mut cur = 1;
    text.split('\n')
        .map(|line| {
            let chars: Vec<char> = line.chars().collect();
            let (mut spans, mut buf, mut i) = (Vec::new(), String::new(), 0);
            let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>, cur: usize| {
                if !buf.is_empty() {
                    let st = color(cur).map(|c| Style::default().fg(c)).unwrap_or_default();
                    spans.push(Span::styled(std::mem::take(buf), st));
                }
            };
            while i < chars.len() {
                if chars[i] == '$' && i + 1 < chars.len() && ('1'..='9').contains(&chars[i + 1]) {
                    flush(&mut buf, &mut spans, cur);
                    cur = chars[i + 1].to_digit(10).unwrap_or(1) as usize;
                    i += 2;
                    continue;
                }
                if chars[i] == '$' && chars.get(i + 1) == Some(&'$') {
                    i += 1;
                }
                buf.push(chars[i]);
                i += 1;
            }
            flush(&mut buf, &mut spans, cur);
            Line::from(spans)
        })
        .collect()
}

fn line_text(key: &str, v: Option<&Value>) -> String {
    match v {
        Some(Value::Object(m)) => m.values().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        Some(Value::String(s)) => s.clone(),
        Some(v) => style::show_value(key, Some(v)),
        None => String::new(),
    }
}

impl App {
    fn new(m: Model) -> App {
        App {
            m,
            group: 0,
            row: 0,
            offset: 0,
            mode: Mode::Browse,
            status: "every change is written to termwall.toml at once; the wallpaper follows within a second".into(),
            error: false,
            cat: logos::catalog(),
            quit: false,
            groups_area: Rect::default(),
            rows_area: Rect::default(),
            list_area: Rect::default(),
            list_offset: 0,
        }
    }

    fn rows(&self) -> Vec<Row> {
        settings::rows(Group::ALL[self.group])
    }

    fn current(&self) -> Option<Row> {
        self.rows().get(self.row).copied()
    }

    fn accent(&self) -> Color {
        self.m.palette().and_then(|p| p.get("accent").and_then(Value::as_str).and_then(hex)).unwrap_or(Color::Cyan)
    }

    fn say(&mut self, r: Result<String, String>) {
        let (text, error) = match r {
            Ok(t) => (t, false),
            Err(e) => (e, true),
        };
        self.status = text;
        self.error = error;
    }

    fn set_group(&mut self, g: usize) {
        self.group = g % Group::ALL.len();
        self.row = 0;
        self.offset = 0;
    }

    fn move_row(&mut self, d: i32) {
        let n = self.rows().len() as i32;
        if n > 0 {
            self.row = (self.row as i32 + d).clamp(0, n - 1) as usize;
        }
    }

    fn logo_matches(&self, query: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let q = query.trim().to_lowercase();
        for (id, _) in SPECIAL_LOGOS {
            if q.is_empty() || id.contains(&q) {
                out.push(id.to_string());
            }
        }
        let mut found = if q.is_empty() {
            logos::distro_ids(&self.cat)
        } else {
            logos::find_logos(&q.split_whitespace().map(String::from).collect::<Vec<_>>(), &self.cat)
        };
        found.sort();
        out.extend(found);
        out
    }

    /// Enter on a row: the next value, or the row's editor.
    fn open(&mut self) {
        let Some(row) = self.current() else { return };
        let Row::Setting(key) = row else {
            let r = self.m.step(row, 1);
            self.say(r);
            return;
        };
        let v = self.m.view(row);
        match settings::edit_kind(row) {
            Edit::Cycle => {
                let r = self.m.step(row, 1);
                self.say(r);
            }
            Edit::Checklist(list) => self.mode = Mode::Check { key, list, at: 0 },
            Edit::Line => {
                let text = line_text(key, v.written.as_ref().or(v.effective.as_ref()));
                self.mode = Mode::Line { key, cursor: text.chars().count(), text };
            }
            Edit::Logo => {
                let found = self.logo_matches("");
                let cur = v.effective.as_ref().and_then(Value::as_str).unwrap_or("auto").to_string();
                let at = found.iter().position(|x| *x == cur).unwrap_or(0);
                self.mode = Mode::Logo { query: String::new(), found, at };
            }
            Edit::Blocks => {
                let from = v.written.as_ref().or(if v.overridden { None } else { v.effective.as_ref() });
                self.mode = Mode::Blocks(Grid::from_value(from));
            }
            Edit::Theme => {
                let list = theme_list();
                let cur = v.effective.as_ref().and_then(Value::as_str).unwrap_or("auto");
                let at = list.iter().position(|x| *x == cur).unwrap_or(0);
                self.mode = Mode::Theme { at, before: v.written.clone() };
            }
        }
    }

    fn key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match std::mem::replace(&mut self.mode, Mode::Browse) {
            Mode::Browse => self.browse_key(k),
            Mode::Line { key, mut text, mut cursor } => {
                let at = |t: &str, c: usize| t.char_indices().nth(c).map(|(i, _)| i).unwrap_or(t.len());
                match k.code {
                    KeyCode::Esc => {
                        self.say(Ok(format!("{key}: unchanged")));
                        return;
                    }
                    KeyCode::Enter => match settings::parse_line(key, &text) {
                        Ok(v) => {
                            let r = self.m.set(key, v);
                            self.say(r);
                            return;
                        }
                        Err(e) => self.say(Err(e)),
                    },
                    KeyCode::Backspace if cursor > 0 => {
                        let i = at(&text, cursor - 1);
                        text.remove(i);
                        cursor -= 1;
                    }
                    KeyCode::Delete if cursor < text.chars().count() => {
                        let i = at(&text, cursor);
                        text.remove(i);
                    }
                    KeyCode::Left => cursor = cursor.saturating_sub(1),
                    KeyCode::Right => cursor = (cursor + 1).min(text.chars().count()),
                    KeyCode::Home => cursor = 0,
                    KeyCode::End => cursor = text.chars().count(),
                    KeyCode::Char(c) if !ctrl => {
                        let i = at(&text, cursor);
                        text.insert(i, c);
                        cursor += 1;
                    }
                    _ => {}
                }
                self.mode = Mode::Line { key, text, cursor };
            }
            Mode::Check { key, list, mut at } => {
                match k.code {
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => return,
                    KeyCode::Up | KeyCode::Char('k') => at = at.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => at = (at + 1).min(list.len() - 1),
                    KeyCode::Char(' ') | KeyCode::Char('x') => self.toggle(key, list[at]),
                    _ => {}
                }
                self.mode = Mode::Check { key, list, at };
            }
            Mode::Logo { mut query, mut found, mut at } => {
                match k.code {
                    KeyCode::Esc => {
                        self.say(Ok("logo: unchanged".into()));
                        return;
                    }
                    KeyCode::Enter => {
                        if let Some(id) = found.get(at) {
                            let r = self.m.set("logo", Some(Value::from(id.as_str())));
                            self.say(r);
                        }
                        return;
                    }
                    KeyCode::Up => at = at.saturating_sub(1),
                    KeyCode::Down => at = (at + 1).min(found.len().saturating_sub(1)),
                    KeyCode::PageUp => at = at.saturating_sub(10),
                    KeyCode::PageDown => at = (at + 10).min(found.len().saturating_sub(1)),
                    KeyCode::Backspace => {
                        query.pop();
                        found = self.logo_matches(&query);
                        at = 0;
                    }
                    KeyCode::Char(c) if !ctrl => {
                        query.push(c);
                        found = self.logo_matches(&query);
                        at = 0;
                    }
                    _ => {}
                }
                self.mode = Mode::Logo { query, found, at };
            }
            Mode::Theme { mut at, before } => {
                let list = theme_list();
                match k.code {
                    KeyCode::Esc => {
                        let r = self.m.set("theme", before).map(|_| "theme: put back".to_string());
                        self.say(r);
                        return;
                    }
                    KeyCode::Enter => {
                        self.say(Ok(format!("theme = {} kept", list[at])));
                        return;
                    }
                    KeyCode::Up | KeyCode::Char('k') => at = at.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => at = (at + 1).min(list.len() - 1),
                    _ => {
                        self.mode = Mode::Theme { at, before };
                        return;
                    }
                }
                // the wallpaper is the preview: each theme is written as it is reached
                let r = self.m.set("theme", Some(Value::from(list[at])));
                self.say(r);
                self.mode = Mode::Theme { at, before };
            }
            Mode::Blocks(mut g) => {
                if g.choose.is_none() && matches!(k.code, KeyCode::Esc | KeyCode::Char('q')) {
                    return;
                }
                if g.choose.is_none() && k.code == KeyCode::Char('L') {
                    let r = self.m.set("layout", Some(Value::from("custom")));
                    self.say(r);
                } else if g.key(k.code, k.modifiers.contains(KeyModifiers::SHIFT), &self.block_names()) {
                    let r = self.m.set("blocks", Some(g.value()));
                    self.say(r);
                }
                self.mode = Mode::Blocks(g);
            }
        }
    }

    /// The blocks to pick from: the built-in ones, then your own [block.NAME].
    fn block_names(&self) -> Vec<String> {
        let mut out: Vec<String> = style::BLOCKS.iter().map(|s| s.to_string()).collect();
        let own = style::load_config(&self.m.paths.style_file).0;
        out.extend(crate::blocks::defs(own.as_ref()).0.into_iter().map(|d| d.name));
        out
    }

    fn toggle(&mut self, key: &'static str, item: &str) {
        let v = self.m.view(Row::Setting(key));
        let mut list: Vec<String> = v
            .written
            .or(if v.overridden { None } else { v.effective })
            .and_then(|x| x.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        match list.iter().position(|x| x == item) {
            Some(i) => {
                list.remove(i);
            }
            None => list.push(item.to_string()),
        }
        let r = self.m.set(key, Some(Value::from(list)));
        self.say(r);
    }

    fn browse_key(&mut self, k: KeyEvent) {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.move_row(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_row(1),
            KeyCode::Home => self.row = 0,
            KeyCode::End => self.row = self.rows().len().saturating_sub(1),
            KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
                let dir = if matches!(k.code, KeyCode::Left | KeyCode::Char('h')) { -1 } else { 1 };
                if let Some(row) = self.current() {
                    let r = self.m.step(row, dir);
                    self.say(r);
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.open(),
            KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('d') => {
                if let Some(Row::Setting(key)) = self.current() {
                    let r = self.m.set(key, None);
                    self.say(r);
                }
            }
            KeyCode::BackTab => self.set_group(self.group + Group::ALL.len() - 1),
            KeyCode::Tab if shift => self.set_group(self.group + Group::ALL.len() - 1),
            KeyCode::Tab | KeyCode::Char(']') | KeyCode::PageDown => self.set_group(self.group + 1),
            KeyCode::Char('[') | KeyCode::PageUp => self.set_group(self.group + Group::ALL.len() - 1),
            KeyCode::Char(c @ '1'..='6') => self.set_group(c as usize - '1' as usize),
            KeyCode::Char('s') => {
                self.m.next_scope();
                self.status = format!("now changing: {}", self.m.scope_label());
                self.error = false;
            }
            KeyCode::Char('r') => {
                self.m.refresh();
                self.status = "read the files again".into();
                self.error = false;
            }
            _ => {}
        }
    }

    fn mouse(&mut self, e: MouseEvent) {
        let inside = |r: Rect| e.column >= r.x && e.column < r.x + r.width && e.row >= r.y && e.row < r.y + r.height;
        match e.kind {
            MouseEventKind::ScrollUp => self.scroll(-1),
            MouseEventKind::ScrollDown => self.scroll(1),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Mode::Browse = self.mode {
                    if inside(self.groups_area) {
                        let i = e.row.saturating_sub(self.groups_area.y + 1) as usize;
                        if i < Group::ALL.len() {
                            self.set_group(i);
                        }
                    } else if inside(self.rows_area) {
                        let i = self.offset + e.row.saturating_sub(self.rows_area.y + 1) as usize;
                        if i < self.rows().len() {
                            if i == self.row {
                                self.open();
                            } else {
                                self.row = i;
                            }
                        }
                    }
                } else if inside(self.list_area) {
                    let i = self.list_offset + e.row.saturating_sub(self.list_area.y + 1) as usize;
                    match &mut self.mode {
                        Mode::Check { key, list, at } if i < list.len() => {
                            *at = i;
                            let (key, item) = (*key, list[i]);
                            self.toggle(key, item);
                        }
                        Mode::Logo { found, at, .. } if i < found.len() => *at = i,
                        Mode::Theme { at, .. } if i < theme_list().len() => {
                            *at = i;
                            let r = self.m.set("theme", Some(Value::from(theme_list()[i])));
                            self.say(r);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn scroll(&mut self, d: i32) {
        match &mut self.mode {
            Mode::Browse => self.move_row(d),
            Mode::Logo { found, at, .. } => *at = (*at as i32 + d * 3).clamp(0, found.len().saturating_sub(1) as i32) as usize,
            Mode::Check { list, at, .. } => *at = (*at as i32 + d).clamp(0, list.len() as i32 - 1) as usize,
            _ => {}
        }
    }

    // ── drawing

    fn draw(&mut self, f: &mut Frame) {
        let accent = self.accent();
        // what the row does, its choices, its trap
        let mut text = Text::default();
        if let Some(row) = self.current() {
            let v = self.m.view(row);
            if let Some(note) = &v.note {
                for l in note.lines() {
                    let st = if v.overridden { Style::default().fg(Color::Yellow) } else { Style::default().fg(accent) };
                    text.lines.push(Line::from(Span::styled(l.to_string(), st)));
                }
            }
            match row {
                Row::Setting(key) => {
                    for l in style::doc(key).lines() {
                        text.lines.push(Line::from(l.to_string()));
                    }
                    if let Some(s) = style::setting(key) {
                        text.lines.push(Line::from(Span::styled(
                            format!("choices: {}", s.check.help()),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
                Row::Autostart => text.lines.push(Line::from(
                    "termwall host shows the page as the desktop background without Wallpaper Engine; on = now and at every logon",
                )),
                Row::Monitor => text.lines.push(Line::from("which monitors termwall host covers (screen 1 is the main one)")),
                Row::AudioDevice => {}
            }
        }
        let detail_h = (text.lines.len() as u16 + 2).clamp(4, 12);
        let [head, body, detail, status, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(detail_h),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(f.area());
        let theme_file = if self.m.paths.theme_file.exists() { "  livery: theme.json present" } else { "" };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" termwall settings ", Style::default().fg(Color::Black).bg(accent)),
                Span::raw(format!("  {}", self.m.paths.style_file.display())),
                Span::styled(theme_file, Style::default().fg(Color::DarkGray)),
            ])),
            head,
        );
        let [groups, rows] = Layout::horizontal([Constraint::Length(14), Constraint::Min(30)]).areas(body);
        self.groups_area = groups;
        self.rows_area = rows;

        let glines: Vec<Line> = Group::ALL
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let st = if i == self.group { Style::default().fg(Color::Black).bg(accent) } else { Style::default() };
                Line::from(Span::styled(format!(" {} {:9}", i + 1, g.title()), st))
            })
            .collect();
        f.render_widget(Paragraph::new(glines).block(Block::default().borders(Borders::ALL).title(" group ")), groups);

        let scope = format!(" changing: {}   s: switch ", self.m.scope_label());
        let list = self.rows();
        let visible = rows.height.saturating_sub(2) as usize;
        if self.row < self.offset {
            self.offset = self.row;
        } else if visible > 0 && self.row >= self.offset + visible {
            self.offset = self.row + 1 - visible;
        }
        let width = rows.width.saturating_sub(2) as usize;
        let longest = list.iter().map(|r| self.m.view(*r).value.chars().count()).max().unwrap_or(8);
        let value_w = (longest + 2).clamp(10, 40).min(width.saturating_sub(16 + 22).max(8));
        let mut lines = Vec::new();
        for (i, row) in list.iter().enumerate().skip(self.offset).take(visible) {
            let v = self.m.view(*row);
            let mut value: String = v.value.chars().take(value_w).collect();
            if v.value.chars().count() > value_w {
                value.pop();
                value.push('~');
            }
            let selected = i == self.row;
            let base = if selected { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
            let mut spans = vec![
                Span::styled(format!(" {:14}", row.label()), base),
                Span::styled(format!(" {value:value_w$} "), base.add_modifier(Modifier::BOLD)),
            ];
            if (*row == Row::Setting("theme") || *row == Row::Setting("colors"))
                && let Some(p) = self.m.palette()
            {
                let cols: Vec<&str> = p.values().filter_map(Value::as_str).collect();
                spans.extend(swatches(&cols));
                spans.push(Span::raw(" "));
            }
            let src_style = if v.overridden { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
            spans.push(Span::styled(v.source.clone(), src_style));
            if v.note.is_some() && v.overridden {
                spans.push(Span::styled(" !", Style::default().fg(Color::Yellow)));
            }
            lines.push(Line::from(spans));
        }
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", Group::ALL[self.group].title()))
                    .title_bottom(Line::from(Span::styled(scope, Style::default().fg(accent)))),
            ),
            rows,
        );

        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(Block::default().borders(Borders::ALL)), detail);

        let st = if self.error { Style::default().fg(Color::Red) } else { Style::default().fg(accent) };
        f.render_widget(Paragraph::new(Span::styled(format!(" {}", self.status), st)), status);
        let help = match self.mode {
            Mode::Browse => " up/down row  left/right change  Enter edit  Del default  Tab group  s screen  q quit",
            Mode::Line { .. } => " type  Enter save  Esc cancel  (empty = default)",
            Mode::Check { .. } => " up/down  Space tick (written at once)  Enter/Esc close",
            Mode::Logo { .. } => " type to search  up/down  Enter use it  Esc cancel",
            Mode::Theme { .. } => " up/down (the wallpaper follows)  Enter keep  Esc put back",
            Mode::Blocks(_) => {
                " arrows move  shift+arrows carry  +/- width  a add  n new row  Enter swap  Del remove  L layout custom  Esc close"
            }
        };
        f.render_widget(Paragraph::new(Span::styled(help, Style::default().fg(Color::DarkGray))), keys);

        self.draw_overlay(f, accent);
    }

    fn draw_overlay(&mut self, f: &mut Frame, accent: Color) {
        let area = f.area();
        let centered = |w: u16, h: u16| {
            let (w, h) = (w.min(area.width), h.min(area.height));
            Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
        };
        let sel = Style::default().fg(Color::Black).bg(accent);
        match &self.mode {
            Mode::Browse => {}
            Mode::Line { key, text, cursor } => {
                let r = centered(70, 5);
                f.render_widget(Clear, r);
                let help = style::setting(key).map(|s| s.check.help()).unwrap_or_default();
                let shown = format!("{text} ");
                let (a, b) = shown.split_at(shown.char_indices().nth(*cursor).map(|(i, _)| i).unwrap_or(shown.len()));
                let mut cs = b.chars();
                let c = cs.next().unwrap_or(' ');
                let lines = vec![
                    Line::from(vec![
                        Span::raw(a.to_string()),
                        Span::styled(c.to_string(), Style::default().add_modifier(Modifier::REVERSED)),
                        Span::raw(cs.collect::<String>()),
                    ]),
                    Line::from(Span::styled(help, Style::default().fg(Color::DarkGray))),
                ];
                f.render_widget(
                    Paragraph::new(lines)
                        .wrap(Wrap { trim: false })
                        .block(Block::default().borders(Borders::ALL).title(format!(" {key} "))),
                    r,
                );
            }
            Mode::Check { key, list, at } => {
                let r = centered(40, list.len() as u16 + 2);
                self.list_area = r;
                self.list_offset = 0;
                f.render_widget(Clear, r);
                let v = self.m.view(Row::Setting(key));
                let on: Vec<String> = v
                    .effective
                    .and_then(|x| x.as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect();
                let lines: Vec<Line> = list
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let mark = if on.iter().any(|x| x == item) { "[x]" } else { "[ ]" };
                        let st = if i == *at { sel } else { Style::default() };
                        Line::from(Span::styled(format!(" {mark} {item:30}"), st))
                    })
                    .collect();
                let title = if *key == "hide" { " sections to hide " } else { " effects " };
                f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)), r);
            }
            Mode::Blocks(g) => {
                let names = self.block_names();
                let r =
                    centered(area.width.saturating_sub(8).min(100), (g.rows.len() as u16 * 2 + 6).max(names.len() as u16 + 4));
                f.render_widget(Clear, r);
                let [left, right] = Layout::horizontal([Constraint::Min(30), Constraint::Length(24)]).areas(r);
                let layout = self
                    .m
                    .layers
                    .resolve(self.m.scope.as_deref())
                    .get("layout")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let mut lines = vec![if layout == "custom" {
                    Line::from(Span::styled("the wallpaper shows these as you change them", Style::default().fg(Color::DarkGray)))
                } else {
                    Line::from(Span::styled(
                        format!("layout is {layout}: L switches to custom to see these"),
                        Style::default().fg(Color::Yellow),
                    ))
                }];
                lines.push(Line::from(""));
                for (ri, row) in g.rows.iter().enumerate() {
                    let mut spans = Vec::new();
                    for (ci, item) in row.iter().enumerate() {
                        let (name, w) = Grid::split(item);
                        let label = if w > 1 { format!(" {name} x{w} ") } else { format!(" {name} ") };
                        let st = if ri == g.r && ci == g.c { sel } else { Style::default().add_modifier(Modifier::REVERSED) };
                        spans.push(Span::styled(label, st));
                        spans.push(Span::raw(" "));
                    }
                    lines.push(Line::from(spans));
                    lines.push(Line::from(""));
                }
                f.render_widget(
                    Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" blocks (layout custom) ")),
                    left,
                );
                let title = match g.choose {
                    Some((Pick::Add, _)) => " add which ",
                    Some((Pick::NewRow, _)) => " new row with ",
                    Some((Pick::Replace, _)) => " swap for ",
                    None => " blocks ",
                };
                let builtin = style::BLOCKS.len();
                let items: Vec<Line> = names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        let st = if g.choose.is_some_and(|(_, at)| at == i) {
                            sel
                        } else if i >= builtin {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default()
                        };
                        let tag = if i >= builtin { " (yours)" } else { "" };
                        Line::from(Span::styled(format!(" {n}{tag}"), st))
                    })
                    .collect();
                f.render_widget(Paragraph::new(items).block(Block::default().borders(Borders::ALL).title(title)), right);
            }
            Mode::Theme { at, .. } => {
                let list = theme_list();
                let r = centered(44, list.len() as u16 + 2);
                self.list_area = r;
                self.list_offset = 0;
                f.render_widget(Clear, r);
                let lines: Vec<Line> = list
                    .iter()
                    .enumerate()
                    .map(|(i, name)| {
                        let st = if i == *at { sel } else { Style::default() };
                        let mut spans = vec![Span::styled(format!(" {name:12} "), st)];
                        match theme::builtin(name) {
                            Some(p) => spans.extend(swatches(&p.values().filter_map(Value::as_str).collect::<Vec<_>>())),
                            None => spans.push(Span::styled(
                                match *name {
                                    "auto" => "livery's palette, else the logo's",
                                    _ => "the logo's colors",
                                },
                                Style::default().fg(Color::DarkGray),
                            )),
                        }
                        Line::from(spans)
                    })
                    .collect();
                f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" theme ")), r);
            }
            Mode::Logo { query, found, at } => {
                let r = centered(area.width.saturating_sub(6), area.height.saturating_sub(4));
                f.render_widget(Clear, r);
                let [left, right] = Layout::horizontal([Constraint::Length(42), Constraint::Min(20)]).areas(r);
                let [search, listr] = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(left);
                f.render_widget(
                    Paragraph::new(format!("{query}_"))
                        .block(Block::default().borders(Borders::ALL).title(format!(" search: {} logos ", found.len()))),
                    search,
                );
                let visible = listr.height.saturating_sub(2) as usize;
                let offset =
                    if visible == 0 { 0 } else { at.saturating_sub(visible / 2).min(found.len().saturating_sub(visible)) };
                self.list_area = listr;
                self.list_offset = offset;
                let lines: Vec<Line> = found
                    .iter()
                    .enumerate()
                    .skip(offset)
                    .take(visible)
                    .map(|(i, id)| {
                        let name = SPECIAL_LOGOS
                            .iter()
                            .find(|(s, _)| s == id)
                            .map(|(_, d)| d.to_string())
                            .unwrap_or_else(|| self.cat.names_of(id).join(", "));
                        let st = if i == *at { sel } else { Style::default() };
                        let text: String = format!(" {id:18} {name}").chars().take(40).collect();
                        Line::from(Span::styled(format!("{text:40}"), st))
                    })
                    .collect();
                f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL)), listr);
                let id = found.get(*at).cloned().unwrap_or_default();
                let preview = match self.cat.logos.get(&id).and_then(Value::as_str) {
                    Some(t) => logo_lines(t, &self.cat.own_colors(&id)),
                    None => vec![Line::from(Span::styled(
                        SPECIAL_LOGOS.iter().find(|(s, _)| *s == id).map(|(_, d)| *d).unwrap_or(""),
                        Style::default().fg(Color::DarkGray),
                    ))],
                };
                f.render_widget(
                    Paragraph::new(preview)
                        .block(Block::default().borders(Borders::ALL).title(format!(" {id} (its own colors) "))),
                    right,
                );
            }
        }
    }
}

fn theme_list() -> Vec<&'static str> {
    let mut out = vec!["auto", "logo", "distro"];
    out.extend(theme::theme_names());
    out
}

fn run(term: &mut DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    while !app.quit {
        term.draw(|f| app.draw(f))?;
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                // Windows reports the release too
                Event::Key(k) if k.kind == KeyEventKind::Press => app.key(k),
                Event::Mouse(m) => app.mouse(m),
                _ => {}
            }
        } else if app.m.refresh() {
            app.status = "the files changed outside (livery, an editor): read again".into();
            app.error = false;
        }
    }
    Ok(())
}

pub fn cmd_settings(paths: &Paths) -> i32 {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        eprintln!("termwall settings needs a terminal; termwall --style lists and changes settings in scripts");
        return 2;
    }
    let mut app = App::new(Model::new(paths, true));
    let mut term = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let r = run(&mut term, &mut app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    match r {
        Ok(()) => {
            println!("{}", paths.style_file.display());
            0
        }
        Err(e) => {
            eprintln!("termwall settings: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_text_in_its_colors() {
        let l = logo_lines("$1ab$2c$$d\nx", &[Some("#ff0000".into()), Some("#00ff00".into())]);
        assert_eq!(l.len(), 2);
        let spans: Vec<(String, Option<Color>)> = l[0].spans.iter().map(|s| (s.content.to_string(), s.style.fg)).collect();
        assert_eq!(
            spans,
            [("ab".into(), Some(Color::Rgb(255, 0, 0))), ("c$d".into(), Some(Color::Rgb(0, 255, 0)))],
            "$$ is a dollar"
        );
        assert_eq!(l[1].spans[0].style.fg, Some(Color::Rgb(0, 255, 0)), "the color carries over to the next line");
    }

    fn screen(app: &mut App) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut t = Terminal::new(TestBackend::new(110, 34)).expect("test terminal");
        t.draw(|f| app.draw(f)).expect("draw");
        let buf = t.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    }

    fn press(app: &mut App, keys: &[KeyCode]) {
        for k in keys {
            app.key(KeyEvent::new(*k, KeyModifiers::NONE));
        }
    }

    #[test]
    fn keys_change_the_file_and_the_screen_shows_it() {
        let td = crate::util::TempDir::new().expect("temp dir");
        let paths = Paths::in_dir(td.path());
        let mut app = App::new(Model::new(&paths, false));
        let s = screen(&mut app);
        assert!(s.contains("termwall settings") && s.contains("layout") && s.contains("fetch"), "{s}");
        assert!(s.contains("How the screen is laid out."), "the row's description from the spec");
        // right on layout: board, written at once
        press(&mut app, &[KeyCode::Right]);
        assert!(std::fs::read_to_string(&paths.style_file).unwrap().contains("layout = \"board\""));
        assert!(screen(&mut app).contains("termwall.toml"), "the source column says where it comes from");
        // Del puts the default back
        press(&mut app, &[KeyCode::Delete]);
        assert!(std::fs::read_to_string(&paths.style_file).unwrap().contains("layout = \"fetch\""));
        // the logo picker: search, Enter
        press(&mut app, &[KeyCode::Char('2'), KeyCode::Enter]);
        assert!(matches!(app.mode, Mode::Logo { .. }));
        if !app.cat.is_empty() {
            for c in "arch".chars() {
                press(&mut app, &[KeyCode::Char(c)]);
            }
            let s = screen(&mut app);
            assert!(s.contains("search:") && s.contains("arch"), "{s}");
            press(&mut app, &[KeyCode::Enter]);
            let logo = Layers::read(&paths.style_file, &paths.theme_file).file.get("logo").cloned();
            assert!(logo.and_then(|v| v.as_str().map(String::from)).is_some_and(|l| l.contains("arch")));
        } else {
            press(&mut app, &[KeyCode::Esc]);
        }
        // sections: tick gpu in the checklist
        press(&mut app, &[KeyCode::Char('4'), KeyCode::Enter]);
        let gpu = style::SECTIONS.iter().position(|x| *x == "gpu").unwrap();
        for _ in 0..gpu {
            press(&mut app, &[KeyCode::Down]);
        }
        press(&mut app, &[KeyCode::Char(' ')]);
        assert!(screen(&mut app).contains("[x] gpu"));
        press(&mut app, &[KeyCode::Esc]);
        assert_eq!(Layers::read(&paths.style_file, &paths.theme_file).file["hide"], serde_json::json!(["gpu"]));
        // s: screen 1, then a change lands in [screen.1]
        press(&mut app, &[KeyCode::Char('1'), KeyCode::Char('s'), KeyCode::Right]);
        assert!(screen(&mut app).contains("[screen.1]"));
        assert_eq!(Layers::read(&paths.style_file, &paths.theme_file).screens["1"]["layout"], "board");
        // a typed prompt
        press(&mut app, &[KeyCode::Char('s'), KeyCode::Char('s')]);
        assert!(app.m.scope.is_none() || app.m.scope.as_deref() == Some("2"));
        while app.m.scope.is_some() {
            press(&mut app, &[KeyCode::Char('s')]);
        }
        let prompt = settings::rows(Group::Look).iter().position(|r| *r == Row::Setting("prompt")).unwrap();
        app.row = prompt;
        press(&mut app, &[KeyCode::Enter, KeyCode::End]);
        for _ in 0..20 {
            press(&mut app, &[KeyCode::Backspace]);
        }
        for c in "btop".chars() {
            press(&mut app, &[KeyCode::Char(c)]);
        }
        press(&mut app, &[KeyCode::Enter]);
        assert_eq!(Layers::read(&paths.style_file, &paths.theme_file).file["prompt"], "btop");
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(app.quit);
    }

    #[test]
    fn the_blocks_editor() {
        let names: Vec<String> = style::BLOCKS.iter().map(|s| s.to_string()).chain(["notes".to_string()]).collect();
        let mut g = Grid::from_value(Some(&serde_json::json!([["logo", "info"], ["cpu"]])));
        let k = |g: &mut Grid, code: KeyCode, shift: bool| g.key(code, shift, &names);
        assert!(!k(&mut g, KeyCode::Right, false) && g.c == 1, "arrows move");
        assert!(k(&mut g, KeyCode::Left, true), "shift carries");
        assert_eq!(g.rows[0], ["info", "logo"]);
        assert!(k(&mut g, KeyCode::Char('+'), false));
        assert_eq!(g.rows[0][0], "info:2");
        assert!(k(&mut g, KeyCode::Down, true), "down to the next row");
        assert_eq!(g.value(), serde_json::json!([["logo"], ["cpu", "info:2"]]));
        // a: add, pick "notes" (your own) from the list
        assert!(!k(&mut g, KeyCode::Char('a'), false) && g.choose.is_some());
        for _ in 0..names.len() {
            k(&mut g, KeyCode::Down, false);
        }
        assert!(k(&mut g, KeyCode::Enter, false));
        assert_eq!(g.rows[1], ["cpu", "info:2", "notes"]);
        // n: a new row; Del empties it again
        k(&mut g, KeyCode::Char('n'), false);
        assert!(k(&mut g, KeyCode::Enter, false));
        assert_eq!(g.rows.len(), 3);
        assert!(k(&mut g, KeyCode::Delete, false));
        assert_eq!(g.rows.len(), 2, "an emptied row goes");
        assert!(style::setting("blocks").unwrap().check.check(&g.value()).is_some(), "what it writes passes the spec");
        // the last block stays
        let mut one = Grid::from_value(Some(&serde_json::json!([["clock"]])));
        assert!(!one.key(KeyCode::Delete, false, &names));
    }

    #[test]
    fn the_line_editor_starts_from_the_value() {
        let c = theme::builtin("mint").map(Value::Object);
        assert_eq!(line_text("colors", c.as_ref()).split(' ').count(), 6);
        assert_eq!(line_text("logo_colors", Some(&Value::from(vec!["#111111", "#222222"]))), "#111111, #222222");
        assert_eq!(line_text("prompt", None), "");
    }
}
