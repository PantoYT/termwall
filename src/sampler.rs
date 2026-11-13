//! The stats: sampled once a second on a thread of its own, so a request never waits on the
//! operating system or nvidia-smi.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sysinfo::{Disks, Networks, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::gpu::GpuReader;
use crate::platform;
use crate::util::{self, Map, Value, round_to};

pub const HISTORY: usize = 60; // seconds of CPU/GPU/net history for sparklines
const PSEUDO_FS: [&str; 7] = ["squashfs", "overlay", "tmpfs", "devtmpfs", "efivarfs", "fuse.snapfuse", "nsfs"];

/// Where the latest snapshot lives; the HTTP servers read it.
pub trait StatsSource: Send + Sync {
    fn get(&self) -> Map;
}

#[derive(Clone, Default)]
pub struct Snapshot(Arc<Mutex<Map>>);

impl StatsSource for Snapshot {
    fn get(&self) -> Map {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// /etc/os-release -> {"NAME": ..., "PRETTY_NAME": ..., "ID": ...}
pub fn parse_os_release(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=')
            && !k.trim().is_empty()
            && !k.starts_with('#')
        {
            out.insert(k.trim().to_string(), v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    out
}

pub fn parse_cpuinfo(text: &str) -> String {
    for line in text.lines() {
        let (k, v) = line.split_once(':').unwrap_or((line, ""));
        if ["model name", "Hardware", "Processor"].contains(&k.trim()) && !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    String::new()
}

/// A text file, "" when it can't be read (static info only).
pub fn read_text(path: &str) -> String {
    std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

/// `getpass.getuser()`
pub fn user_name() -> String {
    for var in ["LOGNAME", "USER", "LNAME", "USERNAME"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return v;
        }
    }
    "user".into()
}

fn machine() -> String {
    match std::env::consts::ARCH {
        "x86_64" if cfg!(windows) => "AMD64".into(),
        "aarch64" if cfg!(windows) => "ARM64".into(),
        a => a.into(),
    }
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What doesn't change while termwall runs.
pub fn static_info(sys: &System) -> Map {
    let cores = sys.cpus().len();
    let physical = System::physical_core_count().ok();
    let host = System::host_name().ok().unwrap_or_default();
    let boot = System::boot_time().unwrap_or(0);
    let (os, family, distro, like, kernel, board, cpu, shell);
    if cfg!(target_os = "linux") {
        let rel = parse_os_release(&read_text("/etc/os-release"));
        let vendor = read_text("/sys/devices/virtual/dmi/id/board_vendor").trim().to_string();
        let mut b = read_text("/sys/devices/virtual/dmi/id/board_name").trim().to_string();
        if b.is_empty() {
            b = read_text("/sys/devices/virtual/dmi/id/product_name").trim().to_string();
        }
        let pretty = rel.get("PRETTY_NAME").filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "Linux".into());
        os = format!("{pretty} {}", machine()).trim().to_string();
        family = "linux".to_string();
        distro = rel.get("ID").cloned().unwrap_or_default();
        like = rel.get("ID_LIKE").cloned().unwrap_or_default();
        kernel = format!("Linux {}", System::kernel_version().unwrap_or_default());
        board = format!("{vendor} {b}").trim().to_string();
        let model = squash(&parse_cpuinfo(&read_text("/proc/cpuinfo")));
        cpu = if model.is_empty() { sys.cpus().first().map(|c| squash(c.brand())).unwrap_or_default() } else { model };
        let sh = std::env::var("SHELL").unwrap_or_default();
        let sh = Path::new(&sh).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        shell = if sh.is_empty() { "sh".to_string() } else { sh };
    } else if cfg!(windows) {
        let w = platform::windows_version();
        os = format!("{} {}", w.product, w.display).trim().to_string() + format!(" {}", machine()).trim_end();
        family = "windows".into();
        distro = String::new();
        like = String::new();
        kernel = format!("WIN32_NT {}", w.version) + &if w.ubr.is_empty() { String::new() } else { format!(".{}", w.ubr) };
        board = w.board;
        cpu =
            squash(&if w.cpu.is_empty() { sys.cpus().first().map(|c| c.brand().to_string()).unwrap_or_default() } else { w.cpu });
        shell = "PowerShell".into();
    } else {
        os = format!("{} {}", System::long_os_version().unwrap_or_default(), machine()).trim().to_string();
        family = std::env::consts::OS.to_string();
        distro = String::new();
        like = String::new();
        kernel = System::kernel_version().unwrap_or_default();
        board = String::new();
        cpu = sys.cpus().first().map(|c| squash(c.brand())).unwrap_or_default();
        shell = std::env::var("SHELL").unwrap_or_default();
    }
    let mut m = Map::new();
    m.insert("user".into(), Value::from(user_name()));
    m.insert("host".into(), Value::from(host));
    m.insert("os".into(), Value::from(os));
    m.insert("os_family".into(), Value::from(family));
    m.insert("distro".into(), Value::from(distro));
    m.insert("distro_like".into(), Value::from(like));
    m.insert("kernel".into(), Value::from(kernel));
    m.insert("board".into(), Value::from(board));
    m.insert("cpu_model".into(), Value::from(cpu));
    m.insert("cores".into(), Value::from(cores));
    m.insert("physical_cores".into(), physical.map(Value::from).unwrap_or(Value::Null));
    m.insert("ram_total".into(), Value::from(sys.total_memory()));
    m.insert("boot".into(), Value::from(boot));
    m.insert("shell".into(), Value::from(shell));
    m
}

fn excluded_ip(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 127 || (o[0] == 169 && o[1] == 254) || o[0] == 100
}

fn iface_ipv4s(nets: &Networks) -> Vec<(String, Ipv4Addr, bool)> {
    let mut out = Vec::new();
    for (name, data) in nets.iter() {
        let up = !matches!(data.operational_state(), sysinfo::InterfaceOperationalState::Down);
        for n in data.ip_networks() {
            if let IpAddr::V4(v4) = n.addr {
                out.push((name.to_string_lossy().into_owned(), v4, up));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The address of the interface with the default route, "a.b.c.d (name)"; "-" without one.
/// (Tailscale's 100.x and link-local addresses are not "the network".)
pub fn local_ip(nets: &Networks) -> String {
    let ifaces = iface_ipv4s(nets);
    let routed = UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s)) // no packet is sent
        .and_then(|s| s.local_addr())
        .ok()
        .and_then(|a| match a.ip() {
            IpAddr::V4(v4) if !excluded_ip(&v4) && !v4.is_unspecified() => Some(v4),
            _ => None,
        });
    if let Some(ip) = routed {
        let name = ifaces.iter().find(|(_, a, _)| *a == ip).map(|(n, _, _)| n.clone());
        return match name {
            Some(n) => format!("{ip} ({n})"),
            None => ip.to_string(),
        };
    }
    for (name, ip, up) in &ifaces {
        if *up && !excluded_ip(ip) {
            return format!("{ip} ({name})");
        }
    }
    "-".into()
}

/// (address, what it is) a phone could use: the main network address (the one with the default
/// route), then Tailscale's 100.x if it runs.
pub fn lan_addresses() -> Vec<(String, String)> {
    let nets = Networks::new_with_refreshed_list().unwrap_or_else(|_| Networks::new().expect("networks"));
    let mut out = Vec::new();
    let li = local_ip(&nets);
    let main = li.split(' ').next().unwrap_or("").to_string();
    if !main.is_empty() && main != "-" && !main.starts_with("127.") && !main.starts_with("169.254.") {
        out.push((main.clone(), "your network".to_string()));
    }
    for (_, ip, up) in iface_ipv4s(&nets) {
        let o = ip.octets();
        let tailscale = o[0] == 100 && (64..128).contains(&o[1]); // 100.64.0.0/10
        if up && tailscale && ip.to_string() != main {
            out.push((ip.to_string(), "Tailscale: away from home too, with Tailscale on the phone".into()));
        }
    }
    out
}

pub struct Sampler {
    pub static_info: Map,
    sys: System,
    disks: Disks,
    nets: Networks,
    snapshot: Snapshot,
    cpu_hist: VecDeque<f64>,
    gpu_hist: VecDeque<f64>,
    down_hist: VecDeque<f64>,
    up_hist: VecDeque<f64>,
    net_prev: (u64, u64),
    net_t: Instant,
    procs: Value,
    procs_total: usize,
    gpu: Option<Map>,
    gpu_reader: GpuReader,
    tick: u64,
}

fn proc_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing().with_cpu().with_memory()
}

fn net_totals(nets: &Networks) -> (u64, u64) {
    nets.iter().fold((0, 0), |(d, u), (_, n)| (d + n.total_received(), u + n.total_transmitted()))
}

impl Sampler {
    pub fn new() -> Result<Sampler, String> {
        let mut sys = System::new().map_err(|e| format!("system information: {e:?}"))?;
        sys.refresh_memory();
        sys.refresh_cpu_all(); // primes per-core usage: the first reading is always 0
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, proc_kind());
        let nets = Networks::new_with_refreshed_list().map_err(|e| format!("network counters: {e:?}"))?;
        let disks = Disks::new_with_refreshed_list().map_err(|e| format!("disks: {e:?}"))?;
        let static_info = static_info(&sys);
        let net_prev = net_totals(&nets);
        Ok(Sampler {
            static_info,
            sys,
            disks,
            nets,
            snapshot: Snapshot::default(),
            cpu_hist: VecDeque::with_capacity(HISTORY),
            gpu_hist: VecDeque::with_capacity(HISTORY),
            down_hist: VecDeque::with_capacity(HISTORY),
            up_hist: VecDeque::with_capacity(HISTORY),
            net_prev,
            net_t: Instant::now(),
            procs: Value::Array(vec![]),
            procs_total: 0,
            gpu: None,
            gpu_reader: GpuReader::default(),
            tick: 0,
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }

    pub fn get(&self) -> Map {
        self.snapshot.get()
    }

    /// The busiest programs, one row per name (Chrome's twenty processes are one Chrome).
    fn top_processes(&mut self, n: usize) -> Value {
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, proc_kind());
        let ncpu = self.static_info.get("cores").and_then(Value::as_u64).unwrap_or(1).max(1) as f64;
        let mut by_name: indexmap::IndexMap<String, (f64, u64, u64)> = indexmap::IndexMap::new();
        for p in self.sys.processes().values() {
            let name = p.name().to_string_lossy().into_owned();
            // the idle "process" (psutil and sysinfo name it differently)
            if name.is_empty() || ["System Idle Process", "Idle", "[System Process]"].contains(&name.as_str()) {
                continue;
            }
            let e = by_name.entry(name).or_default();
            e.0 += f64::from(p.cpu_usage()) / ncpu;
            e.1 += p.memory();
            e.2 += 1;
        }
        self.procs_total = self.sys.processes().len();
        let mut rows: Vec<(String, (f64, u64, u64))> = by_name.into_iter().collect();
        rows.sort_by(|a, b| (b.1.0, b.1.1).partial_cmp(&(a.1.0, a.1.1)).unwrap_or(std::cmp::Ordering::Equal));
        Value::Array(
            rows.into_iter()
                .take(n)
                .map(|(name, (cpu, mem, count))| {
                    let mut r = Map::new();
                    r.insert("name".into(), Value::from(name));
                    r.insert("cpu".into(), util::num(round_to(cpu, 1)));
                    r.insert("mem".into(), Value::from(mem));
                    r.insert("count".into(), Value::from(count));
                    Value::Object(r)
                })
                .collect(),
        )
    }

    fn disks(&mut self) -> Value {
        self.disks.refresh(true);
        let mut out = Vec::new();
        for d in self.disks.iter() {
            let fs = d.file_system().to_string_lossy().into_owned();
            let mount = d.mount_point().to_string_lossy().into_owned();
            let total = d.total_space();
            if total == 0 || fs.is_empty() || PSEUDO_FS.contains(&fs.as_str()) {
                continue; // an empty optical drive, a pseudo filesystem
            }
            if ["/snap", "/boot", "/var/lib/docker", "/run"].iter().any(|p| mount.starts_with(p)) || d.mount_point().is_file() {
                continue; // a bind-mounted file (containers' /etc/hosts)
            }
            if cfg!(target_os = "linux") && !d.name().to_string_lossy().starts_with("/dev/") {
                continue; // only real block devices, like psutil's disk_partitions(all=False)
            }
            let used = total.saturating_sub(d.available_space());
            let mut m = Map::new();
            m.insert("mount".into(), Value::from(mount.trim_end_matches('\\')));
            m.insert("fs".into(), Value::from(fs));
            m.insert("used".into(), Value::from(used));
            m.insert("total".into(), Value::from(total));
            m.insert("percent".into(), util::num(round_to(used as f64 / total as f64 * 100.0, 1)));
            out.push((mount, Value::Object(m)));
        }
        if cfg!(windows) {
            out.sort_by(|a, b| a.0.cmp(&b.0)); // C:, D:, E: like psutil (sysinfo's order is the volumes')
        }
        Value::Array(out.into_iter().map(|(_, v)| v).collect())
    }

    pub fn sample(&mut self) {
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        let cores: Vec<f64> = self.sys.cpus().iter().map(|c| round_to(f64::from(c.usage()), 1)).collect();
        let cpu = if cores.is_empty() { 0.0 } else { round_to(cores.iter().sum::<f64>() / cores.len() as f64, 1) };
        let freq = self.sys.cpus().first().map(|c| c.frequency()).filter(|f| *f > 0);

        self.nets.refresh(true);
        let now = Instant::now();
        let totals = net_totals(&self.nets);
        let dt = now.duration_since(self.net_t).as_secs_f64().max(1e-3);
        let down = totals.0.saturating_sub(self.net_prev.0) as f64 / dt;
        let up = totals.1.saturating_sub(self.net_prev.1) as f64 / dt;
        self.net_prev = totals;
        self.net_t = now;

        if self.tick.is_multiple_of(2) {
            self.gpu = self.gpu_reader.read();
        }
        if self.tick.is_multiple_of(3) {
            self.procs = self.top_processes(12);
        }
        self.tick += 1;
        let disks = self.disks();

        let push = |h: &mut VecDeque<f64>, v: f64| {
            if h.len() == HISTORY {
                h.pop_front();
            }
            h.push_back(v);
        };
        let gpu_util = self.gpu.as_ref().and_then(|g| g.get("util")).and_then(Value::as_f64).unwrap_or(0.0);
        push(&mut self.cpu_hist, cpu);
        push(&mut self.gpu_hist, gpu_util);
        push(&mut self.down_hist, down);
        push(&mut self.up_hist, up);

        let (total, avail) = (self.sys.total_memory(), self.sys.available_memory());
        let ram_used = total.saturating_sub(avail);
        let pct = |used: u64, total: u64| if total == 0 { 0.0 } else { round_to(used as f64 / total as f64 * 100.0, 1) };
        // Windows: the paging file's own use. sysinfo's used swap is the commit charge beyond
        // RAM, which reads several times higher than what psutil (and Task Manager) show.
        let sw_total = self.sys.total_swap();
        let (sw_used, sw_pct) = match platform::pagefile_percent().filter(|_| sw_total > 0) {
            Some(p) => ((0.01 * p * sw_total as f64) as u64, round_to(p, 1)),
            None => (self.sys.used_swap(), pct(self.sys.used_swap(), sw_total)),
        };
        let t = util::now_secs();
        let boot = self.static_info.get("boot").and_then(Value::as_f64).unwrap_or(0.0);
        let hist = |h: &VecDeque<f64>| Value::Array(h.iter().map(|v| util::num(*v)).collect());

        let mut snap = self.static_info.clone();
        snap.insert("time".into(), util::num(t));
        snap.insert("uptime".into(), Value::from((t - boot).max(0.0) as i64));
        snap.insert("local_ip".into(), Value::from(local_ip(&self.nets)));
        snap.insert("cpu".into(), util::num(cpu));
        snap.insert("cores_pct".into(), Value::Array(cores.iter().map(|c| util::num(*c)).collect()));
        snap.insert("cpu_freq".into(), freq.map(Value::from).unwrap_or(Value::Null));
        snap.insert("ram".into(), serde_json::json!({"used": ram_used, "total": total, "percent": pct(ram_used, total)}));
        snap.insert("swap".into(), serde_json::json!({"used": sw_used, "total": sw_total, "percent": sw_pct}));
        snap.insert("gpu".into(), self.gpu.clone().map(Value::Object).unwrap_or(Value::Null));
        snap.insert("disks".into(), disks);
        snap.insert("net".into(), serde_json::json!({"down": down, "up": up}));
        snap.insert("procs".into(), self.procs.clone());
        snap.insert("procs_total".into(), Value::from(self.procs_total));
        snap.insert(
            "history".into(),
            serde_json::json!({"cpu": hist(&self.cpu_hist), "gpu": hist(&self.gpu_hist), "down": hist(&self.down_hist), "up": hist(&self.up_hist)}),
        );
        *self.snapshot.0.lock().unwrap_or_else(|e| e.into_inner()) = snap;
    }

    /// Sample once a second, forever (on its own thread).
    pub fn run(mut self) {
        loop {
            let t0 = Instant::now();
            // the sampler must survive a bad reading
            if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.sample())) {
                eprintln!("sample error: {e:?}");
            }
            std::thread::sleep(Duration::from_secs(1).saturating_sub(t0.elapsed()));
        }
    }
}

/// Unused-on-this-platform helpers stay compiled everywhere so the tests cover them.
#[allow(dead_code)]
pub fn sysinfo_kind_for_exe() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet).with_cmd(UpdateKind::OnlyIfNotSet)
}
