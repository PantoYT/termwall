//! The few operating-system calls termwall makes directly: the Windows registry, opening a file
//! with its associated app, and binding a port that no second server can share.

use std::io;
use std::net::{SocketAddr, TcpListener};

/// Windows' own name for itself, from the registry (what fastfetch shows).
#[derive(Default, Debug)]
pub struct WindowsVersion {
    pub product: String,
    pub display: String,
    pub version: String,
    pub ubr: String,
    pub cpu: String,
    pub board: String,
}

#[cfg(windows)]
pub fn windows_version() -> WindowsVersion {
    use reg::{HKLM, read};
    let cv = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let mut product = read(HKLM, cv, "ProductName").unwrap_or_else(|| "Windows".into());
    let build = read(HKLM, cv, "CurrentBuild").unwrap_or_default();
    // Windows 11 still reports "Windows 10" in ProductName; build 22000 and up is 11
    if product.starts_with("Windows 10") && build.parse::<u32>().is_ok_and(|b| b >= 22000) {
        product = product.replacen("Windows 10", "Windows 11", 1);
    }
    let major = read(HKLM, cv, "CurrentMajorVersionNumber").unwrap_or_else(|| "10".into());
    let minor = read(HKLM, cv, "CurrentMinorVersionNumber").unwrap_or_else(|| "0".into());
    WindowsVersion {
        product,
        display: read(HKLM, cv, "DisplayVersion").unwrap_or_default(),
        version: format!("{major}.{minor}.{build}"),
        ubr: read(HKLM, cv, "UBR").unwrap_or_default(),
        cpu: read(HKLM, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString").unwrap_or_default(),
        board: read(HKLM, r"HARDWARE\DESCRIPTION\System\BIOS", "BaseBoardProduct").unwrap_or_default(),
    }
}

#[cfg(not(windows))]
pub fn windows_version() -> WindowsVersion {
    WindowsVersion::default()
}

#[cfg(windows)]
pub mod reg {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, REG_EXPAND_SZ, REG_QWORD, REG_SZ,
        RRF_RT_ANY, RegCloseKey, RegDeleteKeyValueW, RegEnumValueW, RegGetValueW, RegOpenKeyExW, RegSetKeyValueW,
    };

    pub const HKLM: HKEY = HKEY_LOCAL_MACHINE;
    pub const HKCU: HKEY = HKEY_CURRENT_USER;
    pub const HKCR: HKEY = HKEY_CLASSES_ROOT;

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A value as text: strings as they are, numbers in decimal. None when missing. An empty
    /// name reads the key's default value.
    pub fn read(root: HKEY, subkey: &str, name: &str) -> Option<String> {
        let (k, n) = (wide(subkey), wide(name));
        let mut kind = 0u32;
        let mut size = 0u32;
        // SAFETY: the strings are NUL-terminated; a null buffer asks for the size first.
        let rc = unsafe { RegGetValueW(root, k.as_ptr(), n.as_ptr(), RRF_RT_ANY, &mut kind, std::ptr::null_mut(), &mut size) };
        if rc != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize + 2];
        // SAFETY: buf holds `size` bytes as the first call reported.
        let rc = unsafe { RegGetValueW(root, k.as_ptr(), n.as_ptr(), RRF_RT_ANY, &mut kind, buf.as_mut_ptr().cast(), &mut size) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let bytes = &buf[..size as usize];
        match kind {
            REG_SZ | REG_EXPAND_SZ => {
                let words: Vec<u16> =
                    bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|w| *w != 0).collect();
                Some(String::from_utf16_lossy(&words).trim().to_string())
            }
            REG_DWORD if bytes.len() >= 4 => Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).to_string()),
            REG_QWORD if bytes.len() >= 8 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&bytes[..8]);
                Some(u64::from_le_bytes(b).to_string())
            }
            _ => None,
        }
    }

    /// Write a string value (the key has to exist). False when Windows refuses.
    pub fn set_string(root: HKEY, subkey: &str, name: &str, value: &str) -> bool {
        let (k, n, v) = (wide(subkey), wide(name), wide(value));
        // SAFETY: NUL-terminated strings; the size is the value's bytes with its terminator.
        let rc = unsafe { RegSetKeyValueW(root, k.as_ptr(), n.as_ptr(), REG_SZ, v.as_ptr().cast(), (v.len() * 2) as u32) };
        rc == ERROR_SUCCESS
    }

    pub fn delete_value(root: HKEY, subkey: &str, name: &str) -> bool {
        let (k, n) = (wide(subkey), wide(name));
        // SAFETY: NUL-terminated strings.
        unsafe { RegDeleteKeyValueW(root, k.as_ptr(), n.as_ptr()) == ERROR_SUCCESS }
    }

    /// Every string value of a key, name and data (numbers left out).
    pub fn values(root: HKEY, subkey: &str) -> Vec<(String, String)> {
        let k = wide(subkey);
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: k is NUL-terminated and h receives the handle, closed at the end.
        if unsafe { RegOpenKeyExW(root, k.as_ptr(), 0, KEY_READ, &mut h) } != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut names = Vec::new();
        for i in 0.. {
            let mut name = vec![0u16; 16384];
            let mut len = name.len() as u32;
            // SAFETY: name holds len UTF-16 units; the type and data aren't asked for.
            let rc = unsafe {
                RegEnumValueW(
                    h,
                    i,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if rc != ERROR_SUCCESS {
                break;
            }
            names.push(String::from_utf16_lossy(&name[..len as usize]));
        }
        unsafe { RegCloseKey(h) };
        names
            .into_iter()
            .filter_map(|n| {
                let v = read(root, subkey, &n)?;
                Some((n, v))
            })
            .collect()
    }

    pub fn key_exists(root: HKEY, subkey: &str) -> bool {
        let k = wide(subkey);
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: k is NUL-terminated and h receives the handle, closed right after.
        let rc = unsafe { RegOpenKeyExW(root, k.as_ptr(), 0, KEY_READ, &mut h) };
        if rc == ERROR_SUCCESS {
            unsafe { RegCloseKey(h) };
            true
        } else {
            false
        }
    }
}

/// How full the paging files are, in percent: the same numbers Windows' "Paging File % Usage"
/// counter reads (and psutil with it). None without a paging file.
#[cfg(windows)]
pub fn pagefile_percent() -> Option<f64> {
    use windows_sys::Wdk::System::SystemInformation::NtQuerySystemInformation;
    const SYSTEM_PAGEFILE_INFORMATION: i32 = 18;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
    let mut buf = vec![0u64; 512]; // u64: the records are 8-byte aligned
    loop {
        let mut len = 0u32;
        let bytes = (buf.len() * 8) as u32;
        // SAFETY: buf is writable for `bytes` bytes; len receives the size needed.
        let st = unsafe { NtQuerySystemInformation(SYSTEM_PAGEFILE_INFORMATION, buf.as_mut_ptr().cast(), bytes, &mut len) };
        if st == STATUS_INFO_LENGTH_MISMATCH && buf.len() < 1 << 17 {
            buf.resize((len as usize / 8 + 1).max(buf.len() * 2), 0);
            continue;
        }
        if st < 0 {
            return None;
        }
        break;
    }
    // SYSTEM_PAGEFILE_INFORMATION: NextEntryOffset, TotalSize, TotalInUse, PeakUsage (u32 each,
    // sizes in pages), then the file name
    let raw: Vec<u8> = buf.iter().flat_map(|w| w.to_le_bytes()).collect();
    let (mut size, mut used, mut off) = (0u64, 0u64, 0usize);
    while off + 16 <= raw.len() {
        let rd = |i: usize| u64::from(u32::from_le_bytes([raw[off + i], raw[off + i + 1], raw[off + i + 2], raw[off + i + 3]]));
        size += rd(4);
        used += rd(8);
        match rd(0) as usize {
            0 => break,
            next => off += next,
        }
    }
    (size > 0).then(|| used as f64 / size as f64 * 100.0)
}

#[cfg(not(windows))]
pub fn pagefile_percent() -> Option<f64> {
    None
}

/// The process listening on 127.0.0.1:port (IPv4), if the system says. 0 or None when it
/// doesn't tell this process (other users' processes, a sandbox).
#[cfg(windows)]
pub fn listener_pid(port: u16) -> Option<u32> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER};
    let mut buf = vec![0u32; 4096];
    loop {
        let mut size = (buf.len() * 4) as u32;
        // SAFETY: buf is writable for size bytes; on ERROR_INSUFFICIENT_BUFFER size says how much.
        let rc = unsafe { GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut size, 0, 2, TCP_TABLE_OWNER_PID_LISTENER, 0) };
        if rc == 122 && buf.len() < 1 << 22 {
            buf.resize(size as usize / 4 + 64, 0);
            continue;
        }
        if rc != 0 {
            return None;
        }
        break;
    }
    // MIB_TCPTABLE_OWNER_PID: a count, then rows of six u32: state, local address, local port
    // (network order in the low 16 bits), remote address, remote port, pid
    let rows = buf[0] as usize;
    (0..rows).map(|i| &buf[1 + i * 6..1 + i * 6 + 6]).find_map(|r| {
        let lport = ((r[2] & 0xff) << 8 | (r[2] >> 8) & 0xff) as u16;
        let local_ok = r[1] == u32::from_ne_bytes([127, 0, 0, 1]) || r[1] == 0;
        (lport == port && local_ok).then_some(r[5])
    })
}

#[cfg(not(windows))]
pub fn listener_pid(port: u16) -> Option<u32> {
    // /proc/net/tcp: "0100007F:232A" is 127.0.0.1:9002, state 0A is LISTEN; then the process
    // holding that socket's inode among its open files
    let text = std::fs::read_to_string("/proc/net/tcp").ok()?;
    let want = format!(":{port:04X}");
    let inode = text.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        let ok = f.len() > 9
            && f[3] == "0A"
            && f[1].ends_with(&want)
            && (f[1].starts_with("0100007F") || f[1].starts_with("00000000"));
        ok.then(|| f[9].to_string())
    })?;
    let target = format!("socket:[{inode}]");
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else { continue };
        for fd in fds.flatten() {
            if std::fs::read_link(fd.path()).is_ok_and(|l| l.to_string_lossy() == target) {
                return Some(pid);
            }
        }
    }
    None
}

/// Open a file with the app Windows associates with it (`os.startfile`).
#[cfg(windows)]
pub fn shell_open(path: &std::path::Path) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let op = reg::wide("open");
    let file = reg::wide(&path.to_string_lossy());
    // SAFETY: NUL-terminated strings; no window owner.
    let r = unsafe {
        ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
    };
    if r as isize > 32 { Ok(()) } else { Err(io::Error::other(format!("ShellExecute failed ({})", r as isize))) }
}

#[cfg(not(windows))]
pub fn shell_open(path: &std::path::Path) -> io::Result<()> {
    std::process::Command::new("xdg-open").arg(path).spawn().map(|_| ())
}

/// A listening socket that nothing else can share. Windows lets a second socket bind a port in
/// use when that one asks for SO_REUSEADDR (two servers, and the second one's token.js locks
/// the wallpaper out of the first), so the port is taken exclusively there. Elsewhere
/// SO_REUSEADDR is on: it never lets two listeners share a port, and without it a restarted
/// server can't bind for about a minute while old connections sit in TIME_WAIT.
pub fn bind_exclusive(addr: SocketAddr) -> io::Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{SO_EXCLUSIVEADDRUSE, SOCKET, SOL_SOCKET, setsockopt};
        let one: i32 = 1;
        // SAFETY: a valid socket handle and a pointer to an int of the given size.
        let rc = unsafe {
            setsockopt(sock.as_raw_socket() as SOCKET, SOL_SOCKET, SO_EXCLUSIVEADDRUSE, (&one as *const i32).cast(), 4)
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(not(windows))]
    sock.set_reuse_address(true)?;
    sock.bind(&addr.into())?;
    sock.listen(128)?;
    Ok(sock.into())
}
