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
        RRF_RT_ANY, RegCloseKey, RegDeleteKeyValueW, RegGetValueW, RegOpenKeyExW, RegSetKeyValueW,
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
