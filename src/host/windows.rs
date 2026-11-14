//! Windows: one WebView2 per monitor, behind the desktop icons.
//!
//! Where a wallpaper goes, as Lively's source describes it (it quotes Microsoft): since Windows 11
//! 24H2 ("raised desktop") Progman is created with WS_EX_NOREDIRECTIONBITMAP, the icons
//! (SHELLDLL_DefView) are a layered child of Progman, and the wallpaper's WorkerW is a child of
//! Progman under them. A wallpaper is then its own layered child of Progman, right under the
//! icons. Before 24H2 the icons sit in a top-level WorkerW and the wallpaper goes into the next
//! WorkerW, which Progman spawns on message 0x052C.
//!
//! What Windows allows differs from that description, so the host does what Wallpaper Engine does
//! on the same machine: on build 26340 WE's windows are children of Progman's WorkerW child, and
//! Windows refuses to parent another process's window to Progman itself.

use std::mem::{size_of, zeroed};
use std::num::NonZeroIsize;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MapWindowPoints};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowExW, FindWindowW, GWL_EXSTYLE,
    GetClassNameW, GetMessageW, GetParent, GetWindowLongPtrW, HWND_TOP, KillTimer, LWA_ALPHA, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, SMTO_NORMAL, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SendMessageTimeoutW,
    SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow, TranslateMessage, WM_APP, WM_DESTROY, WM_TIMER, WNDCLASSW,
    WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows_sys::core::BOOL;
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use wry::{Rect, WebContext, WebViewBuilder};

use super::{Options, WE_API, audio, page_url};
use crate::paths::Paths;
use crate::style;

static LIVE: AtomicUsize = AtomicUsize::new(0);

const TIMER_REPORT: usize = 1;
const TIMER_QUIT: usize = 2;
/// The audio thread has a new frame in AUDIO for the pages.
const WM_APP_AUDIO: u32 = WM_APP + 1;
static AUDIO: Mutex<[f32; audio::BANDS * 2]> = Mutex::new([0.0; audio::BANDS * 2]);
static AUDIO_PENDING: AtomicBool = AtomicBool::new(false);
/// What each page shows in its status line (or why it shows nothing), and what its visualizer
/// draws, for --seconds runs.
const STATUS_JS: &str = r#"(() => {
  const s = [...document.querySelectorAll('[data-p=status]')].map(e => e.textContent.trim()).filter(Boolean);
  let out = 'layout ' + ([...document.body.classList].find(c => c.startsWith('L-')) || '?').slice(2) + ' | ' + (s[0] || '(no status line yet)');
  if (typeof VIZ !== 'undefined' && document.body.classList.contains('viz-bars')) {
    const bars = [...document.querySelectorAll('#fetch .eq s i')]
      .map(e => Math.round(parseFloat((e.style.height.match(/([\d.]+)%/) || [0, 0])[1])));
    out += ' | spectrum bars % ' + (bars.length ? bars.join(' ') : '(none drawn)');
  }
  return out;
})()"#;

thread_local! {
    /// The pages, by screen number: the window procedure asks them for their status line.
    static VIEWS: std::cell::RefCell<Vec<(usize, wry::WebView)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn class_of(hwnd: HWND) -> String {
    if hwnd.is_null() {
        return "(none)".into();
    }
    let mut buf = [0u16; 128];
    // SAFETY: the buffer's length is passed along.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// The monitors in physical pixels (the process is per-monitor DPI aware), left to right.
fn monitors() -> Vec<RECT> {
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: data is the Vec passed below, alive for the whole enumeration.
        let found = unsafe { &mut *(data as *mut Vec<RECT>) };
        let mut mi: MONITORINFO = unsafe { zeroed() };
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(m, &mut mi) } != 0 {
            found.push(mi.rcMonitor);
        }
        1
    }
    let mut found: Vec<RECT> = Vec::new();
    // SAFETY: the callback only touches `found` through the pointer it is given.
    unsafe { EnumDisplayMonitors(null_mut(), null(), Some(each), &mut found as *mut _ as LPARAM) };
    found.sort_by_key(|r| (r.left, r.top));
    found
}

struct Desktop {
    progman: HWND,
    defview: HWND,
    /// 24H2: Progman's WorkerW child (the system wallpaper); before: the top-level WorkerW to sit in.
    workerw: HWND,
    raised: bool,
}

fn find_desktop() -> Option<Desktop> {
    struct Found {
        defview: HWND,
        after: HWND,
    }
    unsafe extern "system" fn each(top: HWND, data: LPARAM) -> BOOL {
        // SAFETY: data is the Found below, alive for the whole enumeration.
        let f = unsafe { &mut *(data as *mut Found) };
        let dv = unsafe { FindWindowExW(top, null_mut(), wide("SHELLDLL_DefView").as_ptr(), null()) };
        if !dv.is_null() {
            f.defview = dv;
            f.after = unsafe { FindWindowExW(null_mut(), top, wide("WorkerW").as_ptr(), null()) };
        }
        1
    }
    // SAFETY: plain Win32 calls with NUL-terminated strings and pointers that outlive them.
    unsafe {
        let progman = FindWindowW(wide("Progman").as_ptr(), null());
        if progman.is_null() {
            return None;
        }
        let mut res = 0usize;
        SendMessageTimeoutW(progman, 0x052C, 0xD, 0x1, SMTO_NORMAL, 1000, &mut res);
        let raised = (GetWindowLongPtrW(progman, GWL_EXSTYLE) as u32) & WS_EX_NOREDIRECTIONBITMAP != 0;
        let mut f = Found { defview: null_mut(), after: null_mut() };
        EnumWindows(Some(each), &mut f as *mut _ as LPARAM);
        let workerw = if raised { FindWindowExW(progman, null_mut(), wide("WorkerW").as_ptr(), null()) } else { f.after };
        Some(Desktop { progman, defview: f.defview, workerw, raised })
    }
}

/// A window behind the icons over this monitor's rectangle (screen coordinates), created as a
/// child of the desktop window from the start. Moving a finished window there with SetParent is
/// refused ("Access is denied", build 26340); creating it there is what Wallpaper Engine does.
fn create_on_desktop(class: *const u16, title: *const u16, hinst: HINSTANCE, r: &RECT, d: &Desktop) -> Result<HWND, String> {
    // 24H2 and later: inside Progman's WorkerW, where Wallpaper Engine puts its windows too.
    // Progman directly, under the icons, only when there is no such WorkerW.
    let parent = if d.raised && d.workerw.is_null() { d.progman } else { d.workerw };
    if parent.is_null() {
        return Err("no desktop window to sit in (no WorkerW behind the icons)".into());
    }
    let layered = parent == d.progman;
    // SAFETY: a live desktop window found just before; our registered class; strings outlive the call.
    unsafe {
        let mut pts = [POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom }];
        MapWindowPoints(null_mut(), parent, pts.as_mut_ptr(), 2);
        let (w, h) = (pts[1].x - pts[0].x, pts[1].y - pts[0].y);
        let ex = WS_EX_NOACTIVATE | if layered { WS_EX_LAYERED } else { 0 };
        let hwnd =
            CreateWindowExW(ex, class, title, WS_CHILD | WS_VISIBLE, pts[0].x, pts[0].y, w, h, parent, null_mut(), hinst, null());
        if hwnd.is_null() {
            let err = std::io::Error::last_os_error();
            return Err(format!(
                "could not create a window in {} {:?} (raised desktop: {}): {err}",
                class_of(parent),
                parent,
                d.raised
            ));
        }
        if layered {
            SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
        }
        // above Wallpaper Engine's window on the same monitor; under the icons when in Progman
        let after = if layered && !d.defview.is_null() { d.defview } else { HWND_TOP };
        SetWindowPos(hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        Ok(hwnd)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if w == TIMER_REPORT => {
            // SAFETY: our own timer on our own window.
            unsafe { KillTimer(hwnd, TIMER_REPORT) };
            VIEWS.with(|v| {
                for (screen, view) in v.borrow().iter() {
                    let screen = *screen;
                    let _ = view.evaluate_script_with_callback(STATUS_JS, move |r| {
                        println!("screen {screen}: {}", serde_json::from_str::<String>(&r).unwrap_or(r));
                    });
                }
            });
            0
        }
        WM_APP_AUDIO => {
            AUDIO_PENDING.store(false, Ordering::SeqCst);
            let frame = *AUDIO.lock().unwrap_or_else(|e| e.into_inner());
            let js = audio::script(&frame);
            VIEWS.with(|v| {
                for (_, view) in v.borrow().iter() {
                    let _ = view.evaluate_script(&js);
                }
            });
            0
        }
        WM_TIMER => {
            // SAFETY: called on the window's own thread.
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_DESTROY => {
            // the last window gone (Explorer restarted and took its children along, say): quit
            if LIVE.fetch_sub(1, Ordering::SeqCst) == 1 {
                unsafe { PostQuitMessage(0) };
            }
            0
        }
        // SAFETY: the default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

struct Handle(HWND);

impl HasWindowHandle for Handle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = NonZeroIsize::new(self.0 as isize).ok_or(HandleError::Unavailable)?;
        // SAFETY: the window lives as long as the WebView built on it (both dropped at the end of run).
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(raw))) })
    }
}

fn webview_data_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("TERMWALL_WEBVIEW2_DATA").filter(|v| !v.is_empty()) {
        return d.into();
    }
    let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    local.join("termwall").join("WebView2")
}

pub fn run(opts: &Options, paths: &Paths) -> i32 {
    // SAFETY: process-wide setting, before any window exists.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let page = paths.page_file();
    if !page.is_file() {
        eprintln!("no page at {}", page.display());
        return 1;
    }
    let all = monitors();
    let chosen: Vec<(usize, RECT)> =
        all.iter().enumerate().map(|(i, r)| (i + 1, *r)).filter(|(n, _)| opts.monitor.is_none_or(|m| m == *n)).collect();
    if chosen.is_empty() {
        eprintln!("no monitor {}; there are {}", opts.monitor.unwrap_or(0), all.len());
        return 1;
    }
    let desktop = if opts.window { None } else { find_desktop() };
    if !opts.window && desktop.is_none() {
        eprintln!("no desktop (Progman) to sit in: is Explorer running?");
        return 1;
    }
    // SAFETY: registering our class with a static window procedure.
    let hinst = unsafe { GetModuleHandleW(null()) };
    let class = wide("termwallHost");
    let mut wc: WNDCLASSW = unsafe { zeroed() };
    wc.lpfnWndProc = Some(wndproc);
    wc.hInstance = hinst;
    wc.lpszClassName = class.as_ptr();
    unsafe { RegisterClassW(&wc) };

    let mut context = WebContext::new(Some(webview_data_dir()));
    let mut windows = Vec::new();
    for (screen, r) in &chosen {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let title = wide("termwall");
        // SAFETY: our registered class; the strings outlive the call.
        let hwnd = unsafe {
            if opts.window {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    r.left + w / 8,
                    r.top + h / 8,
                    w * 3 / 4,
                    h * 3 / 4,
                    null_mut(),
                    null_mut(),
                    hinst,
                    null(),
                )
            } else {
                match create_on_desktop(class.as_ptr(), title.as_ptr(), hinst, r, desktop.as_ref().expect("checked above")) {
                    Ok(h) => h,
                    Err(e) => {
                        eprintln!("screen {screen}: {e}");
                        continue;
                    }
                }
            }
        };
        if hwnd.is_null() {
            eprintln!("screen {screen}: could not create a window: {}", std::io::Error::last_os_error());
            continue;
        }
        LIVE.fetch_add(1, Ordering::SeqCst);
        let (vw, vh) = if opts.window { (w * 3 / 4, h * 3 / 4) } else { (w, h) };
        if desktop.is_none() {
            unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        }
        let bounds = Rect { position: PhysicalPosition::new(0, 0).into(), size: PhysicalSize::new(vw as u32, vh as u32).into() };
        let built = WebViewBuilder::new_with_web_context(&mut context)
            .with_url(page_url(&page, *screen))
            .with_initialization_script(WE_API)
            .with_bounds(bounds)
            .with_background_color((0x17, 0x17, 0x17, 0xff))
            .with_focused(false)
            .with_devtools(cfg!(debug_assertions))
            .build_as_child(&Handle(hwnd));
        match built {
            Ok(v) => {
                let place = match &desktop {
                    // what happened rather than what was meant: our parent, and its parent
                    Some(_) => {
                        let parent = unsafe { GetParent(hwnd) };
                        format!("behind the icons, in {} of {}", class_of(parent), class_of(unsafe { GetParent(parent) }))
                    }
                    None => "in a window".to_string(),
                };
                println!("screen {screen}: {}x{} at {},{}, {place}", w, h, r.left, r.top);
                VIEWS.with(|views| views.borrow_mut().push((*screen, v)));
                windows.push(hwnd);
            }
            Err(e) => {
                eprintln!("screen {screen}: WebView2 failed: {e}");
                unsafe { DestroyWindow(hwnd) };
            }
        }
    }
    if windows.is_empty() {
        return 1;
    }
    // the music for the visualizer, captured only while some screen here has it switched on
    let target = windows[0] as isize;
    let (style_paths, screens) = (paths.clone(), chosen.iter().map(|(n, _)| n.to_string()).collect::<Vec<_>>());
    let wanted = move || {
        screens.iter().any(|n| {
            let st = style::read_style(&style_paths.style_file, &style_paths.theme_file, Some(n));
            st.get("visualizer").and_then(|v| v.as_str()).is_some_and(|v| v != "off")
        })
    };
    let test_run = opts.seconds.is_some();
    let frames = AtomicUsize::new(0);
    audio::capture::spawn(wanted, move |frame| {
        *AUDIO.lock().unwrap_or_else(|e| e.into_inner()) = *frame;
        // a --seconds run says once a second what it hears: the loudest band and how loud
        if test_run && frames.fetch_add(1, Ordering::Relaxed).is_multiple_of(30) {
            let top = (0..frame.len()).max_by(|a, b| frame[*a].total_cmp(&frame[*b])).unwrap_or(0);
            println!(
                "audio: loudest band {top} at {:.2}, {} of 128 bands above 0.05",
                frame[top],
                frame.iter().filter(|v| **v > 0.05).count()
            );
        }
        // one message at a time: a busy page must not pile frames up in the queue
        if !AUDIO_PENDING.swap(true, Ordering::SeqCst) {
            // SAFETY: posting to our own window; a destroyed one just makes the call fail.
            unsafe { PostMessageW(target as HWND, WM_APP_AUDIO, 0, 0) };
        }
    });
    if let (Some(s), Some(first)) = (opts.seconds, windows.first()) {
        let ms = (s * 1000).min(u32::MAX as u64) as u32;
        // SAFETY: timers on our own window; the window procedure handles both ids.
        unsafe {
            SetTimer(*first, TIMER_REPORT, ms.saturating_sub(2000).max(ms / 2), None);
            SetTimer(*first, TIMER_QUIT, ms, None);
        }
    }
    // SAFETY: the standard message loop of this thread.
    unsafe {
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    VIEWS.with(|v| v.borrow_mut().clear());
    for hwnd in windows {
        unsafe { DestroyWindow(hwnd) };
    }
    0
}
