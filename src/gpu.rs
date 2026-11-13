//! The GPU: nvidia-smi first; on Linux the kernel's view when it is missing or finds no NVIDIA
//! card (nvidia-utils is often installed on machines without one).
//!
//! Linux without nvidia-smi: what the kernel exposes, no root needed. AMD reports utilisation,
//! temperature, power and VRAM in sysfs. Intel does not report utilisation anywhere a user can
//! read except per DRM client: each open /dev/dri file's fdinfo carries the nanoseconds every
//! engine spent on that client (what nvtop and gputop read). Summing the deltas over the user's
//! processes covers the compositor, browsers and games, which is what loads the GPU.

use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

use crate::util::{self, Map, Value};

pub const GPU_FIELDS: [&str; 7] =
    ["name", "utilization.gpu", "temperature.gpu", "memory.used", "memory.total", "power.draw", "clocks.gr"];

fn gpu_map(name: &str, vals: [Option<f64>; 6]) -> Map {
    let mut g = Map::new();
    g.insert("name".into(), Value::from(name));
    for (k, v) in ["util", "temp", "vram_used", "vram_total", "power", "clock"].iter().zip(vals) {
        g.insert(k.to_string(), v.map(util::num).unwrap_or(Value::Null));
    }
    g
}

/// One line of `nvidia-smi --query-gpu=... --format=csv,noheader,nounits`.
pub fn parse_nvidia_smi(line: &str) -> Option<Map> {
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    if parts.len() != GPU_FIELDS.len() {
        return None;
    }
    let num = |v: &str| v.parse::<f64>().ok(); // "[N/A]" on some cards and drivers
    Some(gpu_map(parts[0], [num(parts[1]), num(parts[2]), num(parts[3]), num(parts[4]), num(parts[5]), num(parts[6])]))
}

/// '00:02.0 VGA ...: Intel Corporation Arrow Lake-P [Arc Pro 130T/140T] (rev 03)' ->
/// 'Intel Arc Pro 130T/140T'. Without a bracket, the device part after the vendor.
pub fn gpu_marketing_name(lspci_line: &str, driver: &str) -> String {
    let vendor = if driver == "amdgpu" { "AMD" } else { "Intel" };
    let mut text = lspci_line.split_once(": ").map(|(_, t)| t).unwrap_or(lspci_line).to_string();
    // a trailing "(rev 03)"
    if text.ends_with(')')
        && let Some(open) = text.rfind("(rev ")
    {
        let hex = &text[open + 5..text.len() - 1];
        if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)) {
            text = text[..open].trim_end().to_string();
        }
    }
    let mut names = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '['
            && let Some(j) = chars[i + 1..].iter().position(|&c| c == ']').map(|p| p + i + 1)
            && j > i + 1
        {
            names.push(chars[i + 1..j].iter().collect::<String>());
            i = j + 1;
            continue;
        }
        i += 1;
    }
    let name = match names.last() {
        Some(n) if n != "AMD/ATI" && n != "AMD" => n.clone(),
        _ => {
            let mut t = text.as_str();
            for prefix in ["Intel Corporation", "Advanced Micro Devices, Inc. [AMD/ATI]"] {
                if let Some(rest) = t.strip_prefix(prefix) {
                    t = rest.trim_start();
                    break;
                }
            }
            t.to_string()
        }
    };
    if name.starts_with(vendor) { name } else { format!("{vendor} {name}") }
}

pub type Engines = HashMap<String, (u64, u64)>;

/// One DRM fdinfo -> (pdev, client id, {engine: (busy ns, capacity)}).
pub fn parse_fdinfo(text: &str) -> (Option<String>, Option<String>, Engines) {
    let (mut pdev, mut client) = (None, None);
    let (mut busy, mut cap): (Vec<(String, u64)>, HashMap<String, u64>) = (Vec::new(), HashMap::new());
    for line in text.lines() {
        let (key, val) = line.split_once(':').unwrap_or((line, ""));
        let val = val.trim();
        if key == "drm-pdev" {
            pdev = Some(val.to_string());
        } else if key == "drm-client-id" {
            client = Some(val.to_string());
        } else if let Some(e) = key.strip_prefix("drm-engine-capacity-") {
            cap.insert(e.to_string(), if val.is_empty() { 1 } else { val.parse().unwrap_or(1) });
        } else if let Some(e) = key.strip_prefix("drm-engine-")
            && let Some(ns) = val.strip_suffix(" ns").and_then(|n| n.trim().parse().ok())
        {
            busy.push((e.to_string(), ns));
        }
    }
    let engines = busy
        .into_iter()
        .map(|(e, ns)| {
            let c = *cap.get(&e).unwrap_or(&1);
            (e, (ns, c))
        })
        .collect();
    (pdev, client, engines)
}

pub type Busy = HashMap<(String, String), (u64, u64)>;

/// Utilisation of the busiest engine class between two drm_busy() samples.
pub fn busy_percent(prev: &Busy, cur: &Busy, dt: f64) -> f64 {
    let mut per_engine: HashMap<&str, f64> = HashMap::new();
    for ((client, engine), (ns, cap)) in cur {
        let Some(before) = prev.get(&(client.clone(), engine.clone())) else { continue };
        if *ns < before.0 {
            continue; // a new client, or a restarted one
        }
        *per_engine.entry(engine.as_str()).or_default() += (ns - before.0) as f64 / (*cap).max(1) as f64;
    }
    if per_engine.is_empty() || dt <= 0.0 {
        return 0.0;
    }
    let busiest = per_engine.values().cloned().fold(f64::MIN, f64::max);
    util::round_to((busiest / (dt * 1e9) * 100.0).min(100.0), 1)
}

/// '1: 2100Mhz *' -> 2100.0 (AMD's pp_dpm_sclk)
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn num_text(line: &str) -> Option<f64> {
    let lower = line.to_lowercase();
    let i = lower.find("mhz")?;
    let digits: String = lower[..i].trim_end().chars().rev().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.chars().rev().collect::<String>().parse().ok()
}

/// nvidia-smi, then (Linux) the DRM interfaces. Keeps state between samples.
#[derive(Default)]
pub struct GpuReader {
    nvidia_failed: u32,
    #[allow(dead_code)]
    drm: Option<Option<(String, String, String)>>, // (card, driver, name), Some(None) = none found
    #[allow(dead_code)]
    drm_busy: Option<(Busy, f64)>,
}

impl GpuReader {
    pub fn read(&mut self) -> Option<Map> {
        let g = self.read_nvidia();
        #[cfg(target_os = "linux")]
        if g.is_none() {
            return self.read_drm();
        }
        g
    }

    fn read_nvidia(&mut self) -> Option<Map> {
        if self.nvidia_failed >= 3 {
            return None; // three failures in a row: no NVIDIA card here
        }
        let exe = util::which("nvidia-smi")?;
        let mut cmd = Command::new(exe);
        cmd.arg(format!("--query-gpu={}", GPU_FIELDS.join(","))).arg("--format=csv,noheader,nounits");
        let out = util::run_with_timeout(util::hide_window(&mut cmd), Duration::from_secs(3))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let g =
            if out.status.success() && !text.trim().is_empty() { text.lines().next().and_then(parse_nvidia_smi) } else { None };
        self.nvidia_failed = if g.is_some() { 0 } else { self.nvidia_failed + 1 };
        g
    }

    #[cfg(target_os = "linux")]
    fn read_drm(&mut self) -> Option<Map> {
        use linux::*;
        if self.drm.is_none() {
            self.drm = Some(drm_card().map(|(card, driver)| {
                let name = drm_name(&card, &driver);
                (card, driver, name)
            }));
        }
        let (card, driver, name) = self.drm.clone().flatten()?;
        let dev = format!("{card}/device");
        let mut v: [Option<f64>; 6] = [None; 6];
        if driver == "amdgpu" {
            v[0] = num_file(&format!("{dev}/gpu_busy_percent"), 1.0);
            if let Some(hw) = glob_first(&format!("{dev}/hwmon"), "hwmon") {
                v[1] = num_file(&format!("{hw}/temp1_input"), 1000.0);
                v[4] = num_file(&format!("{hw}/power1_average"), 1e6)
                    .filter(|p| *p != 0.0)
                    .or_else(|| num_file(&format!("{hw}/power1_input"), 1e6));
            }
            v[2] = num_file(&format!("{dev}/mem_info_vram_used"), 1024.0 * 1024.0);
            v[3] = num_file(&format!("{dev}/mem_info_vram_total"), 1024.0 * 1024.0);
            for line in sysfs(&format!("{dev}/pp_dpm_sclk")).unwrap_or_default().lines() {
                if line.ends_with('*') {
                    v[5] = num_text(line);
                }
            }
            return Some(gpu_map(&name, v));
        }
        // Intel: utilisation from fdinfo, clock from sysfs (i915 on the card, xe per GT)
        let pdev = std::fs::canonicalize(&dev).ok()?.file_name()?.to_string_lossy().into_owned();
        let now = util::now_secs();
        let cur = drm_busy(&pdev);
        if let Some((prev, t)) = &self.drm_busy {
            v[0] = Some(busy_percent(prev, &cur, now - t));
        }
        self.drm_busy = Some((cur, now));
        // the actual clock is 0 while the GPU sleeps (RC6); the requested one then
        for f in [
            format!("{card}/gt_act_freq_mhz"),
            format!("{dev}/tile0/gt0/freq0/act_freq"),
            format!("{card}/gt_cur_freq_mhz"),
            format!("{dev}/tile0/gt0/freq0/cur_freq"),
        ] {
            if let Some(x) = num_file(&f, 1.0).filter(|x| *x != 0.0) {
                v[5] = Some(x);
                break;
            }
        }
        Some(gpu_map(&name, v))
    }
}

/// A sysfs file's text, or None (a missing file is no GPU field, not an error).
pub fn sysfs(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs;

    pub const DRM_DRIVERS: [&str; 3] = ["i915", "xe", "amdgpu"];

    pub fn num_file(path: &str, scale: f64) -> Option<f64> {
        sysfs(path)?.parse::<f64>().ok().map(|v| v / scale)
    }

    pub fn glob_first(dir: &str, prefix: &str) -> Option<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(prefix))
            .collect();
        names.sort();
        names.first().map(|n| format!("{dir}/{n}"))
    }

    /// The first DRM card with a driver read here: (sysfs card path, driver name).
    pub fn drm_card() -> Option<(String, String)> {
        let mut cards: Vec<String> = fs::read_dir("/sys/class/drm")
            .ok()?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.len() == 5 && n.starts_with("card") && n.as_bytes()[4].is_ascii_digit())
            .collect();
        cards.sort();
        for c in cards {
            let card = format!("/sys/class/drm/{c}");
            let Ok(link) = fs::read_link(format!("{card}/device/driver")) else { continue };
            let driver = link.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if DRM_DRIVERS.contains(&driver.as_str()) {
                return Some((card, driver));
            }
        }
        None
    }

    pub fn drm_name(card: &str, driver: &str) -> String {
        let slot =
            fs::canonicalize(format!("{card}/device")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let lspci = slot.zip(util::which("lspci")).and_then(|(slot, exe)| {
            let out = util::run_with_timeout(Command::new(exe).arg("-s").arg(slot), Duration::from_secs(3))?;
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.trim().lines().next().map(|line| gpu_marketing_name(line, driver))
        });
        lspci.unwrap_or_else(|| if driver == "amdgpu" { "AMD GPU".into() } else { "Intel GPU".into() })
    }

    /// {(client id, engine): (busy ns, capacity)} over every DRM client this user can read.
    pub fn drm_busy(pdev: &str) -> Busy {
        let mut out = Busy::new();
        let Ok(procs) = fs::read_dir("/proc") else { return out };
        for p in procs.flatten() {
            let pid = p.file_name().to_string_lossy().into_owned();
            if !pid.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
            for fd in fds.flatten() {
                let fd = fd.file_name().to_string_lossy().into_owned();
                match fs::read_link(format!("/proc/{pid}/fd/{fd}")) {
                    Ok(t) if t.to_string_lossy().starts_with("/dev/dri/") => {}
                    _ => continue,
                }
                let Some(text) = sysfs(&format!("/proc/{pid}/fdinfo/{fd}")) else { continue };
                let (dev, client, engines) = parse_fdinfo(&text);
                if dev.as_deref() != Some(pdev) {
                    continue;
                }
                let Some(client) = client else { continue };
                for (engine, v) in engines {
                    out.insert((client.clone(), engine), v); // several fds of one client report the same
                }
            }
        }
        out
    }
}
