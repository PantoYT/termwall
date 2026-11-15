//! The self-test: every assertion of termwall_api.py's --selftest, in groups. `termwall
//! --selftest` runs them inside the shipped binary (no cargo needed); `cargo test` runs each
//! group as a test of its own.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use crate::gpu::{Busy, busy_percent, gpu_marketing_name, parse_fdinfo, parse_nvidia_smi, sysfs};
use crate::http;
use crate::integrate::{self, link_lively, lively_library, pick_installer, steam_libraries, unlink_lively, version_tuple};
use crate::logos::{self, logo_image, page_extras, read_logo};
use crate::paths::{self, Paths};
use crate::picture::{picture_logo, read_picture};
use crate::platform::bind_exclusive;
use crate::png::png;
use crate::qr::qr_matrix;
use crate::sampler::{self, Sampler, StatsSource, parse_cpuinfo, parse_os_release, read_text};
use crate::server::{prepare_page_dir, request_ok};
use crate::share::{self, SHARE_PORT, read_share, share_icon, write_share};
use crate::style::{
    self, check_config, clean_style, ensure_config, read_style, refresh_config, render_config, screen_of, set_style,
};
use crate::theme::{self, read_theme, resolve_palette, valid_theme};
use crate::util::{self, Map, TempDir, Value};

#[derive(Default)]
pub struct T {
    pub ok: usize,
    pub failures: Vec<String>,
}

impl T {
    fn check(&mut self, c: bool, m: &str) {
        if c {
            self.ok += 1;
        } else {
            self.failures.push(m.to_string());
        }
    }
}

type Group = fn(&mut T);

pub const GROUPS: [(&str, Group); 20] = [
    ("gpu", gpu),
    ("sampler", sampler_group),
    ("server", server),
    ("theme", theme_group),
    ("style", style_group),
    ("overrides", overrides),
    ("legacy", legacy),
    ("os", os),
    ("integrations", integrations),
    ("logo_txt", logo_txt),
    ("logo_image", logo_image_group),
    ("logo_from", logo_from),
    ("logo_settings", logo_settings),
    ("screens", screens),
    ("update", update),
    ("catalog", catalog),
    ("qr", qr),
    ("share_server", share_server),
    ("share_json", share_json),
    ("page_dir", page_dir),
];

/// All groups; prints the failures and the count like the Python selftest.
pub fn run_all() -> T {
    let mut t = T::default();
    for (_, g) in GROUPS {
        g(&mut t);
    }
    t
}

pub fn run() -> i32 {
    let t = run_all();
    for f in &t.failures {
        println!("  FAIL: {f}");
    }
    println!("selftest: {}/{} OK", t.ok, t.ok + t.failures.len());
    if t.failures.is_empty() { 0 } else { 1 }
}

fn good() -> Value {
    serde_json::json!({"bg": "#171717", "accent": "#b5f4e7", "secondary": "#7faca3", "text": "#939fa1", "dim": "#4b5252", "faint": "#242626"})
}

fn obj(v: Value) -> Map {
    v.as_object().cloned().unwrap_or_default()
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).expect("test file");
}

fn write_json(path: &Path, v: &Value) {
    write(path, &v.to_string());
}

/// Edit a file by hand, like a person in Notepad, with a later mtime even on a coarse clock.
fn edit(path: &Path, old: &str, new: &str) {
    use std::sync::atomic::AtomicU64;
    static BUMP: AtomicU64 = AtomicU64::new(1);
    let t = util::universal_newlines(&fs::read_to_string(path).expect("file to edit"));
    assert!(t.contains(old), "{old}");
    write(path, &t.replacen(old, new, 1));
    let f = fs::OpenOptions::new().write(true).open(path).expect("open");
    let when = SystemTime::now() + Duration::from_millis(BUMP.fetch_add(1, Ordering::Relaxed) * 10);
    let _ = f.set_modified(when);
}

fn read(path: &Path) -> String {
    util::universal_newlines(&fs::read_to_string(path).unwrap_or_default())
}

fn gpu(t: &mut T) {
    let g = parse_nvidia_smi("NVIDIA GeForce RTX 3060, 41, 47, 2059, 12288, 29.78, 1860");
    t.check(g.as_ref().is_some_and(|g| g["name"] == "NVIDIA GeForce RTX 3060" && g["util"] == 41.0), "nvidia-smi line");
    t.check(g.as_ref().is_some_and(|g| g["vram_total"] == 12288.0), "VRAM");
    let g2 = parse_nvidia_smi("GPU, 5, 40, 100, 200, [N/A], 900");
    t.check(g2.as_ref().is_some_and(|g| g["power"].is_null() && g["util"] == 5.0), "[N/A] -> None");
    t.check(parse_nvidia_smi("too,few").is_none(), "wrong field count");
    t.check(
        gpu_marketing_name(
            "00:02.0 VGA compatible controller: Intel Corporation Arrow Lake-P [Arc Pro 130T/140T] (rev 03)",
            "i915",
        ) == "Intel Arc Pro 130T/140T",
        "Intel GPU name from lspci",
    );
    t.check(
        gpu_marketing_name(
            "03:00.0 VGA compatible controller: Advanced Micro Devices, Inc. [AMD/ATI] Navi 23 [Radeon RX 6600] (rev c7)",
            "amdgpu",
        ) == "AMD Radeon RX 6600",
        "AMD GPU name from lspci",
    );
    let fi = "drm-driver:\ti915\ndrm-client-id:\t16\ndrm-pdev:\t0000:00:02.0\ndrm-engine-render:\t1000 ns\ndrm-engine-video:\t0 ns\ndrm-engine-capacity-video:\t2\n";
    let (dev, cl, eng) = parse_fdinfo(fi);
    let want: std::collections::HashMap<String, (u64, u64)> =
        [("render".to_string(), (1000, 1)), ("video".to_string(), (0, 2))].into();
    t.check(dev.as_deref() == Some("0000:00:02.0") && cl.as_deref() == Some("16") && eng == want, "DRM fdinfo");
    let key = |c: &str, e: &str| (c.to_string(), e.to_string());
    let prev: Busy = [(key("16", "render"), (0, 1)), (key("16", "video"), (0, 2))].into();
    let cur: Busy = [
        (key("16", "render"), (250_000_000, 1)),
        (key("16", "video"), (1_000_000_000, 2)),
        (key("17", "render"), (9_000_000_000, 1)),
    ]
    .into();
    t.check(busy_percent(&prev, &cur, 1.0) == 50.0, "GPU busy: busiest engine, capacity, new clients skipped");
    t.check(
        read_text("/nonexistent/termwall").is_empty() && sysfs("/nonexistent/termwall").is_none(),
        "missing files: read_text gives \"\" (static info), sysfs gives None (GPU)",
    );
}

fn sampler_group(t: &mut T) {
    let mut s = match Sampler::new() {
        Ok(s) => s,
        Err(e) => {
            t.check(false, &format!("the sampler starts ({e})"));
            return;
        }
    };
    s.sample();
    std::thread::sleep(Duration::from_millis(1100));
    s.sample();
    let snap = s.get();
    let f = |k: &str| snap.get(k).and_then(Value::as_f64).unwrap_or(-1.0);
    t.check((0.0..=100.0).contains(&f("cpu")), "cpu 0-100");
    t.check(snap["cores_pct"].as_array().map(Vec::len) == snap["cores"].as_u64().map(|n| n as usize), "per-core percentages");
    t.check(
        snap["ram"]["total"].as_u64().unwrap_or(0) > 0
            && (0.0..=100.0).contains(&snap["ram"]["percent"].as_f64().unwrap_or(-1.0)),
        "RAM",
    );
    t.check(
        snap["net"]["down"].as_f64().unwrap_or(-1.0) >= 0.0 && snap["net"]["up"].as_f64().unwrap_or(-1.0) >= 0.0,
        "network rates non-negative",
    );
    t.check(snap["history"]["cpu"].as_array().map(Vec::len) == Some(2), "history grows per sample");
    t.check(snap["disks"].as_array().is_some_and(|d| !d.is_empty() && d.iter().all(|x| x.get("mount").is_some())), "disks");
    t.check(snap["procs"].is_array(), "process list");
    t.check(
        snap["user"].as_str().is_some_and(|s| !s.is_empty()) && snap["host"].as_str().is_some_and(|s| !s.is_empty()),
        "static info",
    );
    t.check(snap["cpu_model"].as_str().is_some_and(|s| !s.is_empty()), "CPU model is a name, not overwritten by usage");
    t.check(snap["cpu"].is_f64(), "cpu is the usage percentage");
    let round: Option<Value> = serde_json::to_string(&snap).ok().and_then(|s| serde_json::from_str(&s).ok());
    t.check(round == Some(Value::Object(snap.clone())), "JSON-serializable");
    t.check(["windows", "linux", "darwin"].contains(&snap["os_family"].as_str().unwrap_or("")), "os_family for the logo");
}

fn server(t: &mut T) {
    let a = bind_exclusive("127.0.0.1:0".parse().expect("address"));
    let port = a.as_ref().ok().and_then(|l| l.local_addr().ok()).map(|a| a.port()).unwrap_or(0);
    let second = bind_exclusive(std::net::SocketAddr::from(([127, 0, 0, 1], port)));
    t.check(a.is_ok() && second.is_err(), "a second server can't bind a port in use");
    drop(a);
    let tok = "abc123";
    t.check(request_ok("/stats?t=abc123", Some("127.0.0.1:9002"), tok), "right token + loopback host");
    t.check(!request_ok("/stats", Some("127.0.0.1:9002"), tok), "no token -> refused");
    t.check(!request_ok("/stats?t=nope", Some("127.0.0.1:9002"), tok), "wrong token -> refused");
    t.check(!request_ok("/stats?t=abc123", Some("evil.example:9002"), tok), "foreign Host (DNS rebinding) -> refused");
    t.check(request_ok("/theme?t=abc123", Some("localhost:9002"), tok), "localhost Host accepted");
}

fn theme_group(t: &mut T) {
    let good = good();
    let with = |k: &str, v: Value| {
        let mut g = good.clone();
        g[k] = v;
        g
    };
    t.check(valid_theme(&good), "valid theme accepted");
    t.check(!valid_theme(&with("bg", "#12".into())), "short hex rejected");
    t.check(!valid_theme(&with("bg", "url(x)".into())), "non-color rejected (goes into CSS)");
    let mut missing = good.clone();
    missing.as_object_mut().map(|m| m.remove("dim"));
    t.check(!valid_theme(&missing), "missing key rejected");
    let td = TempDir::new().expect("temp dir");
    let p = td.join("theme.json");
    t.check(read_theme(&p).is_none(), "no file -> None");
    write_json(&p, &with("extra", 1.into()));
    t.check(read_theme(&p) == Some(obj(good.clone())), "theme read, extra keys dropped");
    write(&p, "{broken");
    t.check(read_theme(&p).is_none(), "broken JSON -> None");
}

fn style_group(t: &mut T) {
    let td = TempDir::new().expect("temp dir");
    let (sp, tp) = (td.join("termwall.toml"), td.join("theme.json"));
    let rs = || read_style(&sp, &tp, None);
    t.check(rs() == style::style_defaults(), "no files -> default look");
    let _ = set_style("layout", "board", &sp);
    let _ = set_style("rotate", "15", &sp);
    t.check(rs()["layout"] == "board" && rs()["rotate"] == 15, "termwall.toml read");
    let _ = set_style("sound_bars", "8", &sp);
    let _ = set_style("logo_width", "48", &sp);
    t.check(
        rs()["sound_bars"] == 8 && rs()["logo_width"] == 48,
        "--style takes any number setting from the command line, not only rotate",
    );
    let _ = set_style("sound_bars", "default", &sp);
    let _ = set_style("logo_width", "default", &sp);
    let text = read(&sp);
    t.check(
        style::spec().iter().filter(|s| s.key != "colors").all(|s| text.contains(&format!("\n{} = ", s.key)))
            && text.contains("# colors = {"),
        "termwall.toml lists every setting",
    );
    t.check(text.contains("# choices: fetch | board | minimal | htop | portrait"), "its choices are in the comments");
    let parsed = style::parse_toml_text(&text).ok().map(Value::Object);
    t.check(parsed.is_some() && clean_style(parsed.as_ref()) == rs(), "the written file parses back the same");
    let mut themed = good();
    themed["style"] = serde_json::json!({"layout": "minimal", "clock": "segments", "bars": "comic-sans"});
    write_json(&tp, &themed);
    let st = rs();
    t.check(
        st["layout"] == "minimal" && st["bars"] == "blocks" && st["clock"] == "24h",
        "theme.json style wins, unknown values dropped",
    );
    let _ = fs::remove_file(&tp);
    let _ = set_style("hide", "gpu,procs,nonsense", &sp);
    let _ = set_style("private", "on", &sp);
    let _ = set_style("scale", "1.1", &sp);
    let _ = set_style("effects", "crt", &sp);
    let _ = set_style("prompt", "say \"hi\" \\ bye", &sp);
    let _ = set_style("theme", "nord", &sp);
    let st = rs();
    t.check(
        st["hide"] == serde_json::json!(["gpu", "procs"])
            && st["private"] == true
            && st["scale"] == 1.1
            && st["effects"] == serde_json::json!(["crt"])
            && st["prompt"] == "say \"hi\" \\ bye",
        "settings parsed from the command line",
    );
    t.check(resolve_palette(&st, &tp) == theme::builtin("nord"), "a built-in theme gives the palette");
    let accent = |sp: &Path| resolve_palette(&read_style(sp, &tp, None), &tp).and_then(|p| p.get("accent").cloned());
    edit(&sp, "# colors = {", "colors = {");
    edit(&sp, "accent = \"#b5f4e7\"", "accent = \"#ff0000\"");
    t.check(accent(&sp) == Some("#ff0000".into()), "own colors (edited in) win over a theme");
    edit(&sp, "layout = \"board\"", "layout = \"htop\"\nfrom_a_newer_termwall = 1");
    t.check(rs()["layout"] == "htop", "a hand edit is picked up");
    t.check(
        check_config(&sp) == vec!["from_a_newer_termwall: unknown setting (ignored)".to_string()],
        "--check names an unknown key",
    );
    write_json(&tp, &good());
    t.check(accent(&sp) == Some("#ff0000".into()), "own colors win over theme.json (livery)");
    edit(&sp, "colors = {", "# colors = {");
    t.check(resolve_palette(&rs(), &tp) == theme::builtin("nord"), "a picked theme wins over theme.json (livery)");
    edit(&sp, "theme = \"nord\"", "theme = \"auto\"");
    t.check(resolve_palette(&rs(), &tp) == Some(obj(good())), "theme auto follows theme.json (livery)");
    edit(&sp, "# colors = {", "colors = {");
    let _ = fs::remove_file(&tp);
    let _ = set_style("hide", "none", &sp);
    let raw = style::parse_toml_text(&read(&sp)).unwrap_or_default();
    t.check(
        raw.get("hide") == Some(&serde_json::json!([]))
            && raw.get("from_a_newer_termwall") == Some(&Value::from(1))
            && raw.get("colors").and_then(|c| c.get("accent")) == Some(&Value::from("#ff0000")),
        "unknown keys and own colors survive a --style write",
    );
    edit(&sp, "scale = 1.1", "scale = 7");
    t.check(
        rs()["scale"] == 1.0 && check_config(&sp).iter().any(|x| x.starts_with("scale = 7")),
        "an invalid value falls back to the default and --check names it",
    );
    edit(&sp, "scale = 7", "scale = 1.1");
    rs();
    edit(&sp, "font = \"cascadia\"", "font = \"jetbrains"); // a missing quote: not TOML any more
    t.check(rs()["scale"] == 1.1, "a broken file keeps the last good settings");
    let problems = check_config(&sp);
    t.check(problems.len() == 1 && problems[0].contains("termwall.toml"), "--check shows the parse error");
    t.check(set_style("layout", "board", &sp).is_err(), "--style refuses to overwrite a broken file");
    edit(&sp, "font = \"jetbrains", "font = \"jetbrains\"");
    let long = "x".repeat(61);
    for (bad_key, bad) in [("scale", "9"), ("layout", "nope"), ("private", "maybe"), ("prompt", long.as_str())] {
        t.check(set_style(bad_key, bad, &sp).is_err(), &format!("bad {bad_key} refused"));
    }
    let _ = set_style("theme", "default", &sp);
    t.check(rs()["theme"] == "auto", "default puts a setting back");
    let _ = set_style("reset", "", &sp);
    t.check(rs() == style::config_defaults(), "reset puts them all back");
    edit(&sp, "# choices: crt, glow, flicker", "# choices: crt, glow");
    edit(&sp, "layout = \"fetch\"", "layout = \"board\"");
    t.check(
        refresh_config(&sp) && read(&sp).contains("# choices: crt, glow, flicker") && rs()["layout"] == "board",
        "old comments are refreshed, values kept",
    );
    t.check(!refresh_config(&sp), "a current file is left alone");
}

fn overrides(t: &mut T) {
    let td = TempDir::new().expect("temp dir");
    let (sp, tp) = (td.join("termwall.toml"), td.join("theme.json"));
    let _ = set_style("layout", "htop", &sp);
    let mut themed = good();
    themed["style"] = serde_json::json!({"layout": "fetch", "bars": "blocks"});
    write_json(&tp, &themed);
    t.check(
        style::theme_overrides(&sp, &tp) == obj(serde_json::json!({"layout": "fetch"})),
        "check names what theme.json overrides",
    );
    let env = |k: &str| (k == "TERMWALL_EDITOR").then(|| "myedit --wait".to_string());
    let want = vec!["myedit".to_string(), "--wait".to_string(), sp.to_string_lossy().into_owned()];
    t.check(integrate::editor_for(&sp, &env) == Some(want), "TERMWALL_EDITOR picks the editor");
}

/// The Rust program's own commands (not in the Python selftest): cargo test only.
#[cfg(test)]
#[test]
fn palette_command() {
    let td = TempDir::new().expect("temp dir");
    let p = Paths::in_dir(td.path());
    let a = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    write_json(&p.theme_file, &serde_json::json!({"style": {"layout": "board"}}));
    assert_eq!(crate::cmd_palette(&a(&["accent=#ff0000"]), &p), 2, "one color with no palette yet is refused");
    assert_eq!(crate::cmd_palette(&a(&["#111111", "#AAAAAA", "#333333", "#444444", "#555555", "#666666"]), &p), 0);
    assert_eq!(crate::cmd_palette(&a(&["accent=#ff0000"]), &p), 0);
    assert_eq!(crate::cmd_palette(&a(&["accent=red"]), &p), 2);
    let doc = util::read_json(&p.theme_file).expect("theme.json");
    assert_eq!(
        (doc["bg"].as_str(), doc["accent"].as_str(), doc["style"]["layout"].as_str()),
        (Some("#111111"), Some("#ff0000"), Some("board"))
    );
    assert_eq!(read_style(&p.style_file, &p.theme_file, None)["layout"], "board", "the switcher's look is kept");
    assert_eq!(crate::cmd_palette(&a(&["off"]), &p), 0);
    let doc = util::read_json(&p.theme_file).expect("theme.json");
    assert!(doc.get("accent").is_none() && doc["style"]["layout"] == "board", "off removes the six colors only");
}

fn legacy(t: &mut T) {
    let td = TempDir::new().expect("temp dir");
    let (sp, legacy) = (td.join("termwall.toml"), td.join("termwall.json"));
    write_json(&legacy, &serde_json::json!({"layout": "portrait", "effects": ["crt"]}));
    t.check(
        read_style(&sp, &td.join("theme.json"), None)["layout"] == "portrait",
        "termwall.json still read before it's carried over",
    );
    let _ = ensure_config(&sp);
    t.check(td.join("termwall.json.old").exists() && !legacy.exists(), "termwall.json carried over and renamed");
    t.check(
        read_style(&sp, &td.join("theme.json"), None)["effects"] == serde_json::json!(["crt"]),
        "termwall.toml has what termwall.json had",
    );
}

fn os(t: &mut T) {
    let rel = parse_os_release("NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n# comment\n");
    t.check(
        rel.get("PRETTY_NAME").map(String::as_str) == Some("Ubuntu 24.04.1 LTS")
            && rel.get("ID").map(String::as_str) == Some("ubuntu"),
        "os-release parsed",
    );
    t.check(
        parse_cpuinfo("processor\t: 0\nmodel name\t: Intel(R) Core(TM) i5-4590 CPU @ 3.30GHz\n")
            == "Intel(R) Core(TM) i5-4590 CPU @ 3.30GHz",
        "cpuinfo model name",
    );
}

fn integrations(t: &mut T) {
    let td = TempDir::new().expect("temp dir");
    let here = paths::app_dir();
    t.check(lively_library(td.path()).is_none(), "no Lively -> nothing to link");
    let lw = td.join("Lively Wallpaper");
    fs::create_dir_all(&lw).expect("dir");
    write_json(&lw.join("Settings.json"), &serde_json::json!({"WallpaperDir": lw.join("Library").to_string_lossy()}));
    t.check(lively_library(td.path()) == Some(lw.join("Library").join("wallpapers")), "Lively's library");
    let (a, b, c) = (link_lively(td.path(), here), link_lively(td.path(), here), unlink_lively(td.path(), here));
    t.check(
        a.contains("added") && b.contains("added") && !b.contains("already") && c.contains("removed"),
        "Lively entry added (twice is fine) and removed",
    );
    let vdf = "\"libraryfolders\"\n{\n \"0\"\n {\n  \"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n }\n \"1\"\n {\n  \"path\"\t\t\"D:\\\\SteamLibrary\"\n }\n}";
    t.check(
        steam_libraries(vdf) == vec!["C:\\Program Files (x86)\\Steam".to_string(), "D:\\SteamLibrary".to_string()],
        "Steam libraries from libraryfolders.vdf",
    );
    let page = here.join("index.html");
    let other = if cfg!(windows) { "INDEX.HTML" } else { "index.html" };
    t.check(
        integrate::same_file(&here.join(".").join(other), &page) && !integrate::same_file(&here.join("no-such-file.html"), &page),
        "--stop knows its script by any spelling of the path, and no other script",
    );
}

fn logo_txt(t: &mut T) {
    let td = TempDir::new().expect("temp dir");
    let lp = td.join("logo.txt");
    t.check(read_logo(&lp).is_none(), "no logo.txt -> no custom logo");
    let mut text = "\u{feff}$1 /\\_/\\\n( o.o )\t$2x\n".to_string();
    text += &vec!["y".repeat(200); 60].join("\n");
    write(&lp, &text);
    let got = read_logo(&lp).unwrap_or_default();
    let lines: Vec<&str> = got.split('\n').collect();
    t.check(
        got.starts_with("$1 /\\_/\\\n( o.o ) $2x")
            && lines.len() == 40
            && lines.iter().map(|l| l.chars().count()).max() == Some(120),
        "logo.txt: BOM gone, tabs expanded, 40 x 120 at most",
    );
    let p = Paths::in_dir(td.path());
    let custom = page_extras(&obj(serde_json::json!({"logo": "custom"})), &p);
    t.check(
        custom.keys().map(String::as_str).collect::<Vec<_>>() == ["logo_text"]
            && page_extras(&obj(serde_json::json!({"logo": "arch"})), &p).is_empty(),
        "the custom logo goes out only when it's used",
    );
}

fn logo_image_group(t: &mut T) {
    let td = TempDir::new().expect("temp dir");
    t.check(logo_image(td.path()).is_none(), "no logo image -> none");
    fs::write(td.join("logo.png"), png(2, 2, &[vec![[255, 0, 0], [0, 0, 0]], vec![[0, 0, 0], [0, 0, 255]]])).expect("png");
    fs::write(td.join("logo.gif"), b"GIF89a").expect("gif");
    t.check(
        logo_image(td.path()).and_then(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())).as_deref()
            == Some("logo.gif"),
        "logo image: gif first, then png",
    );
}

fn logo_from(t: &mut T) {
    // a red and a blue square, a white dot inside the red one, on a white background
    let (w, r, b) = ([255u8, 255, 255], [230u8, 40, 30], [20u8, 90, 230]);
    let pic: Vec<Vec<[u8; 3]>> = (0..36)
        .map(|y| {
            (0..64)
                .map(|x| {
                    if (8..28).contains(&x) && (8..28).contains(&y) && !((14..18).contains(&x) && (14..18).contains(&y)) {
                        r
                    } else if (36..56).contains(&x) && (8..28).contains(&y) {
                        b
                    } else {
                        w
                    }
                })
                .collect()
        })
        .collect();
    let td = TempDir::new().expect("temp dir");
    let pp = td.join("pic.png");
    fs::write(&pp, png(64, 36, &pic)).expect("png");
    let px = |c: [u8; 3]| [i64::from(c[0]), i64::from(c[1]), i64::from(c[2]), 255];
    let ok = read_picture(&pp).is_ok_and(|(pw, ph, rows)| (pw, ph) == (64, 36) && rows[10][10] == px(r) && rows[0][0] == px(w));
    t.check(ok, "a PNG read");
    let (text, cols) = picture_logo(&pp, 32, "blocks", 9, false).unwrap_or_default();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut sorted = cols.clone();
    sorted.sort();
    t.check(
        sorted == ["#145ae6", "#e6281e", "#ffffff"],
        "logo-from: the two squares and the dot inside, the background and its blends gone",
    );
    t.check(
        cols.first().is_some_and(|c| c == "#e6281e" || c == "#145ae6")
            && text.contains("$1")
            && text.contains("$2")
            && text.contains("$3"),
        "logo-from: the most used color is $1",
    );
    t.check(
        lines.len() == 5
            && !lines[0].starts_with(' ')
            && lines.iter().all(|x| x.contains('█') || x.contains('▀') || x.contains('▄')),
        "logo-from: the white border trimmed, half blocks",
    );
    let (a_text, _) = picture_logo(&pp, 32, "ascii", 9, false).unwrap_or_default();
    let mut plain = String::new();
    let ac: Vec<char> = a_text.chars().collect();
    let mut i = 0;
    while i < ac.len() {
        if ac[i] == '$' && ac.get(i + 1).is_some_and(char::is_ascii_digit) {
            i += 2;
            continue;
        }
        plain.push(ac[i]);
        i += 1;
    }
    t.check(!a_text.is_empty() && plain.chars().all(|c| " .:-=+*#%@\n".contains(c)), "logo-from --ascii: the ramp");
    let (k_text, k_cols) = picture_logo(&pp, 32, "blocks", 9, true).unwrap_or_default();
    t.check(k_cols.iter().any(|c| c == "#ffffff") && k_text.split('\n').count() == 9, "logo-from --keep-bg keeps the background");
    let tall = td.join("tall.png");
    fs::write(&tall, png(10, 400, &vec![vec![r; 10]; 400])).expect("png");
    let (t_text, _) = picture_logo(&tall, 120, "blocks", 9, true).unwrap_or_default();
    t.check(!t_text.is_empty() && t_text.split('\n').count() <= 40, "logo-from: 40 lines at most");
    t.check(picture_logo(&pp, 32, "blocks", 1, false).is_ok_and(|(_, c)| c.len() == 1), "logo-from --colors 1: one color");
    let p = Paths::in_dir(td.path());
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let pps = pp.to_string_lossy().into_owned();
    let mut sink = std::io::sink();
    let rc1 = crate::cmd_logo_from(&args(&[&pps, "--width", "32"]), &p, &mut sink);
    let rc2 = crate::cmd_logo_from(&args(&[&pps]), &p, &mut sink);
    // not the real theme.json: a pack's own look there (livery) would win over the file
    let st = read_style(&p.style_file, &p.theme_file, None);
    t.check(
        rc1 == 0
            && rc2 == 1
            && read_logo(&p.logo_file()) == Some(text.clone())
            && st.get("logo") == Some(&Value::from("custom"))
            && st.get("logo_colors") == Some(&Value::from(cols.clone())),
        "logo-from writes logo.txt, sets logo custom and its colors, won't overwrite without --force",
    );
    let _ = std::io::stdout().flush();
}

fn logo_settings(t: &mut T) {
    let full = obj(serde_json::json!({"logo": "image", "logo_width": 48, "logo_chars": "ascii", "logo_colors": "palette"}));
    t.check(
        clean_style(Some(&Value::Object(full.clone()))) == full
            && clean_style(Some(&serde_json::json!({"logo_width": 500, "logo_chars": "x"}))).is_empty(),
        "logo image settings validated",
    );
}

fn screens(t: &mut T) {
    let td = TempDir::new().expect("temp dir");
    let (sp, tp) = (td.join("termwall.toml"), td.join("theme.json"));
    let _ = set_style("layout", "fetch", &sp);
    fs::OpenOptions::new()
        .append(true)
        .open(&sp)
        .and_then(|mut f| f.write_all(b"\n[screen.2]\nlayout = \"logo\"\neffects = [\"crt\"]\n\n[screen.3]\nlayout = \"nope\"\n"))
        .expect("append");
    let rs = |n: Option<&str>| read_style(&sp, &tp, n);
    t.check(
        rs(None)["layout"] == "fetch"
            && rs(Some("2"))["layout"] == "logo"
            && rs(Some("2"))["effects"] == serde_json::json!(["crt"])
            && rs(Some("3"))["layout"] == "fetch",
        "[screen.N] overrides only that monitor, bad values dropped",
    );
    let mut themed = good();
    themed["style"] = serde_json::json!({"layout": "board"});
    write_json(&tp, &themed);
    t.check(rs(None)["layout"] == "board" && rs(Some("2"))["layout"] == "logo", "[screen.N] wins over livery's per-pack look");
    let _ = set_style("bars", "dots", &sp);
    t.check(
        read(&sp).contains("[screen.2]\nlayout = \"logo\"") && rs(Some("2"))["layout"] == "logo",
        "[screen.N] tables survive a --style write",
    );
    t.check(
        check_config(&sp).iter().any(|x| x.contains("screen.3.layout") || x.contains("layout")),
        "check names a bad screen value",
    );
}

fn update(t: &mut T) {
    t.check(
        screen_of("/stats?t=x&screen=2").as_deref() == Some("2") && screen_of("/stats?screen=../x").is_none(),
        "?screen= read safely",
    );
    t.check(
        version_tuple("v1.10.0") > version_tuple("1.9.3") && version_tuple("1.5.0") == [1, 5, 0],
        "versions compare as numbers",
    );
    let rel = serde_json::json!({"assets": [{"name": "notes.txt"}, {"name": "termwall-setup-9.9.9.exe", "size": 5,
        "browser_download_url": "https://x/t.exe", "digest": format!("sha256:{}", "ab".repeat(32))}]});
    let a = pick_installer(&rel);
    t.check(
        a.as_ref().is_some_and(|a| a.name == "termwall-setup-9.9.9.exe" && a.sha256.as_deref() == Some("ab".repeat(32).as_str())),
        "update: the installer asset and its sha256",
    );
    t.check(pick_installer(&serde_json::json!({"assets": []})).is_none(), "update: no installer in a release -> none");
}

fn catalog(t: &mut T) {
    let cat = logos::catalog();
    let ids = logos::distro_ids(&cat);
    t.check(
        ids.iter().any(|i| i == "arch") && ids.iter().any(|i| i == "nixos") && ids.len() >= 500,
        "the logo list comes from logos.js",
    );
    let cfg = render_config(&Map::new());
    t.check(
        ["arch", "mx"].iter().all(|x| cfg.contains(&format!("#   {x}")) || cfg.contains(&format!(", {x}")))
            && !cfg.contains("macos2")
            && cfg.contains("termwall logos"),
        "termwall.toml names the featured logos, not all of them",
    );
    let mac = cat.own_colors("macos");
    t.check(mac.len() == 6 && mac.iter().all(|c| c.as_deref().is_some_and(|c| c.starts_with('#'))), "macOS keeps its six colors");
    t.check(
        ["chromeos", "windows-7", "windows-95", "windows-11", "freebsd", "haiku"].iter().all(|k| cat.logos.contains_key(*k)),
        "logos beyond Linux: ChromeOS, Windows, BSD, Haiku",
    );
    t.check(
        cat.aliases.get("sunos").and_then(Value::as_str) == Some("solaris") && !cat.logos.contains_key("windows"),
        "fastfetch's other names are aliases; termwall's own windows logo keeps its id",
    );
    let logo = style::setting("logo").expect("logo setting");
    t.check(
        logo.check.check(&"macos".into()) == Some("macos".into())
            && logo.check.check(&"sunos".into()) == Some("sunos".into())
            && logo.check.check(&"no-such-os".into()).is_none(),
        "logo: a known id or alias, an unknown one refused",
    );
    let mac_found = logos::find_logos(&["mac".to_string()], &cat);
    t.check(
        mac_found.first().map(String::as_str) == Some("macos") || mac_found.iter().any(|k| k == "macos"),
        "termwall logos mac finds macOS",
    );
    t.check(logos::find_logos(&["windows".to_string(), "95".to_string()], &cat) == ["windows-95"], "every word has to match");
    let preview = logos::logo_preview("apple", &cat);
    t.check(preview.contains("\x1b[38;2;") && !preview.contains("$1"), "a preview in the logo's own colors, without the $N");
    let lc = style::setting("logo_colors").expect("logo_colors setting");
    let chk = |v: Value| lc.check.check(&v);
    t.check(
        chk("own".into()) == Some("own".into())
            && chk("image".into()) == Some("image".into())
            && chk("#e05252, #5fb85f".into()) == Some(serde_json::json!(["#e05252", "#5fb85f"]))
            && chk(serde_json::json!(["#e05252"])) == Some(serde_json::json!(["#e05252"]))
            && chk(serde_json::json!(["red"])).is_none()
            && chk(serde_json::json!(vec!["#000000"; 10])).is_none(),
        "logo_colors: a mode or up to nine colors",
    );
    let nowhere = std::env::temp_dir().join("no-such-theme.json");
    t.check(
        resolve_palette(&obj(serde_json::json!({"theme": "logo"})), &nowhere).is_none(),
        "theme logo: the page colors itself",
    );
    t.check(
        resolve_palette(&obj(serde_json::json!({"theme": "distro"})), &nowhere).is_none(),
        "theme distro: the page colors itself",
    );
    let both = serde_json::json!({"prompt_color": "accent", "logo": "custom"});
    t.check(
        clean_style(Some(&both)) == obj(both.clone())
            && clean_style(Some(&serde_json::json!({"prompt_color": "pink"}))).is_empty(),
        "prompt_color and logo custom validated",
    );
}

fn qr(t: &mut T) {
    let m = qr_matrix(&format!("http://192.168.1.20:9012/?k={}", "A".repeat(16))).unwrap_or_default();
    t.check(m.len() == 33 && m.iter().all(|r| r.len() == 33), "a share link fits a version 4 QR code");
    t.check(
        m.len() == 33 && m[0][..7] == [true; 7] && m[3][..7] == [true, false, true, true, true, false, true],
        "QR finder pattern",
    );
    t.check(m.len() == 33 && (8..25).all(|x| m[6][x] == (x % 2 == 0)), "QR timing pattern");
    t.check(m.len() == 33 && m[4 * 4 + 9][8], "QR dark module");
    t.check(qr_matrix(&"x".repeat(200)).map(|m| m.len()) == Ok(57), "a long text takes version 10");
    t.check(qr_matrix(&"x".repeat(400)).is_err(), "too long for a QR code is refused");
    t.check(share_icon(192, None).starts_with(b"\x89PNG\r\n\x1a\n"), "the home-screen icon is a PNG");
}

struct FakeSource;

impl StatsSource for FakeSource {
    fn get(&self) -> Map {
        obj(serde_json::json!({"user": "panto", "host": "KOMPU", "local_ip": "192.168.1.20 (Ethernet)", "cpu": 12.0, "time": 1}))
    }
}

fn share_server(t: &mut T) {
    let td = TempDir::new().expect("temp dir");
    let p = Arc::new(Paths::in_dir(td.path()));
    let key = "k".repeat(16);
    let Ok(listener) = bind_exclusive("127.0.0.1:0".parse().expect("address")) else {
        t.check(false, "share: a test port");
        return;
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let stop = Arc::new(AtomicBool::new(false));
    let handler = share::make_share_handler(Arc::new(FakeSource), key.clone(), p);
    let s = stop.clone();
    let srv = std::thread::spawn(move || http::serve(listener, handler, Some(s)));
    let host = format!("127.0.0.1:{port}");
    let get = |path: &str, h: &str| http::get(port, path, h, Duration::from_secs(5)).unwrap_or((0, vec![]));
    let (code, body) = get(&format!("/stats?t={key}"), &host);
    let st: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    t.check(code == 200 && st["cpu"] == 12.0 && st["style"]["private"] == true, "share serves stats, private on");
    let text = String::from_utf8_lossy(&body);
    t.check(
        !text.contains("panto") && !text.contains("KOMPU") && !text.contains("192.168.1.20"),
        "share never sends the user, computer name or IP",
    );
    t.check(get("/stats?t=wrong", &host).0 == 403, "share: a wrong key gets 403");
    t.check(
        get(&format!("/stats?t={key}"), &format!("evil.example:{port}")).0 == 403,
        "share: a host name (DNS rebinding) gets 403",
    );
    let (code, body) = get(&format!("/?k={key}"), &host);
    let page = String::from_utf8_lossy(&body);
    t.check(
        code == 200
            && page.contains(&key)
            && !page.contains("<script src=\"token.js\">")
            && page.contains("manifest.webmanifest"),
        "share serves the page with its key and a manifest",
    );
    t.check(
        get("/logos.js", &host).0 == 200
            && [403, 404].contains(&get("/termwall_api.py", &host).0)
            && [403, 404].contains(&get("/fonts/../share.json", &host).0),
        "share: static files only from the list, no source or settings",
    );
    let outside_dir = TempDir::new().expect("temp dir"); // a font-named file outside the page folder
    let outside = outside_dir.join("secret.woff2");
    write(&outside, "secret");
    let (code, body) = get(&format!("/fonts/{}", outside.to_string_lossy().replace(' ', "%20")), &host);
    t.check(code == 403 && body != b"secret", r"share: a font path into another folder (C:\..., \\host\...) is refused");
    for _ in 0..25 {
        get("/stats?t=nope", &host);
    }
    t.check(get(&format!("/stats?t={key}"), &host).0 == 429, "share: 20 wrong keys block that address for a while");
    stop.store(true, Ordering::Relaxed);
    let _ = srv.join();
}

fn share_json(t: &mut T) {
    let td = TempDir::new().expect("temp dir");
    let sf = td.join("share.json");
    let off = share::ShareConfig { enabled: false, key: None, port: SHARE_PORT };
    t.check(read_share(&sf) == off, "share is off without share.json");
    let _ = write_share(&obj(serde_json::json!({"enabled": true, "key": "short", "port": 80})), &sf);
    t.check(
        read_share(&sf) == share::ShareConfig { enabled: true, key: None, port: SHARE_PORT },
        "share.json: a short key and a bad port are refused",
    );
}

fn page_dir(t: &mut T) {
    let app = paths::app_dir();
    t.check(prepare_page_dir(app, app).is_ok_and(|m| m.is_empty()), "a clone or the installer keeps the page where it is");
    if cfg!(unix) {
        // symlinks need extra rights on Windows, and packages are Linux only
        let td = TempDir::new().expect("temp dir");
        let (app, data) = (td.join("share"), td.join("data"));
        fs::create_dir_all(app.join("fonts")).expect("dir");
        for name in ["index.html", "logos.js"] {
            write(&app.join(name), "");
        }
        fs::create_dir_all(data.join("fonts")).expect("dir"); // a stale real folder is replaced by a link
        let made = prepare_page_dir(&app, &data).unwrap_or_default();
        let real = |p: &Path| fs::canonicalize(p).ok();
        t.check(
            made.len() == 3 && real(&data.join("index.html")) == real(&app.join("index.html")),
            "system package: page linked into the data dir",
        );
        t.check(prepare_page_dir(&app, &data).is_ok_and(|m| m.is_empty()), "system package: correct links are left alone");
    }
    let _ = sampler::HISTORY;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(name: &str) {
        let g = GROUPS.iter().find(|(n, _)| *n == name).expect("a group").1;
        let mut t = T::default();
        g(&mut t);
        assert!(t.failures.is_empty(), "{name}: {} passed, failed: {:#?}", t.ok, t.failures);
    }

    macro_rules! groups {
        ($($name:ident),*) => { $( #[test] fn $name() { group(stringify!($name)); } )* };
    }

    groups!(
        gpu,
        server,
        theme,
        style,
        overrides,
        legacy,
        os,
        integrations,
        logo_txt,
        logo_image,
        logo_from,
        logo_settings,
        screens,
        update,
        catalog,
        qr,
        share_server,
        share_json,
        page_dir
    );

    #[test]
    fn sampler() {
        group("sampler");
    }

    /// Every assertion of the Python selftest is here: 128 on Windows, 130 on Linux.
    #[test]
    fn all_assertions_counted() {
        let t = run_all();
        assert!(t.failures.is_empty(), "{:#?}", t.failures);
        assert_eq!(t.ok, if cfg!(unix) { 130 } else { 128 });
    }
}
