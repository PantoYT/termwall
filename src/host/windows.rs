//! Windows: one WebView2 per monitor, behind the desktop icons.
//!
//! Where a wallpaper goes, as Lively's source describes it (it quotes Microsoft): since Windows 11
//! 24H2 ("raised desktop") Progman is created with WS_EX_NOREDIRECTIONBITMAP, the icons
//! (SHELLDLL_DefView) are a layered child of Progman, and the wallpaper's WorkerW is a child of
//! Progman under them. Before 24H2 the icons sit in a top-level WorkerW and the wallpaper goes
//! into the next WorkerW, which Progman spawns on message 0x052C.
//!
//! What Windows allows differs from that description, so the host does what Wallpaper Engine does
//! on the same machine: on build 26340 WE's windows are children of Progman's WorkerW child, and
//! Windows refuses to move another process's window into Progman itself.
//!
//! Explorer owns that WorkerW. When Explorer restarts, the wallpaper windows go with it; a hidden
//! window of the host's own stays, and every two seconds it checks that the wallpaper windows
//! still exist and still match the monitors, and builds them again when they don't.

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateSolidBrush, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MapWindowPoints,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowExW, FindWindowW, GA_ROOT,
    GWL_EXSTYLE, GetAncestor, GetClassNameW, GetMessageW, GetParent, GetWindowLongPtrW, GetWindowRect, HWND_TOP, IsWindow,
    KillTimer, LWA_ALPHA, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SMTO_NORMAL, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SendMessageTimeoutW, SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, WM_APP, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE, WindowFromPoint,
};
use windows_sys::core::BOOL;
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use wry::{Rect, WebContext, WebViewBuilder};

use super::{Options, audio, init_script, media, page_url};
use crate::paths::Paths;
use crate::style;

const TIMER_REPORT: usize = 1;
const TIMER_QUIT: usize = 2;
/// Are the wallpaper windows still there, and still where the monitors are?
const TIMER_WATCH: usize = 3;
/// Debug builds, TERMWALL_TEST_REBUILD set: destroy the wallpaper windows as an Explorer restart would.
const TIMER_TEST_KILL: usize = 4;
/// Is a monitor's wallpaper covered by a fullscreen or maximized window?
const TIMER_COVER: usize = 5;
/// Every page is paused: no music is captured either.
static ALL_PAUSED: AtomicBool = AtomicBool::new(false);
/// The audio thread has a new frame in AUDIO for the pages.
const WM_APP_AUDIO: u32 = WM_APP + 1;
static AUDIO: Mutex<[f32; audio::BANDS * 2]> = Mutex::new([0.0; audio::BANDS * 2]);
static AUDIO_PENDING: AtomicBool = AtomicBool::new(false);
/// Other threads have scripts in SCRIPTS for the pages (now playing).
const WM_APP_SCRIPT: u32 = WM_APP + 2;
static SCRIPTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The host's own hidden window: timers and the other threads' messages go to it.
static CONTROL: AtomicIsize = AtomicIsize::new(0);
/// --window: closing the window ends the host.
static WINDOW_MODE: AtomicBool = AtomicBool::new(false);
/// What each page shows in its status line (or why it shows nothing), and what its visualizer
/// draws, for --seconds runs.
const STATUS_JS: &str = r#"(() => {
  const s = [...document.querySelectorAll('[data-p=status]')].map(e => e.textContent.trim()).filter(Boolean);
  let out = 'layout ' + ([...document.body.classList].find(c => c.startsWith('L-')) || '?').slice(2) + ' | ' + (s[0] || '(no status line yet)');
  const now = [...document.querySelectorAll('[data-p=now]')].map(e => e.textContent.trim()).filter(Boolean);
  out += ' | now playing: ' + (now[0] || '(nothing shown)');
  if (typeof VIZ !== 'undefined' && document.body.classList.contains('viz-bars')) {
    const bars = [...document.querySelectorAll('#fetch .eq s i')]
      .map(e => Math.round(parseFloat((e.style.height.match(/([\d.]+)%/) || [0, 0])[1])));
    out += ' | spectrum bars % ' + (bars.length ? bars.join(' ') : '(none drawn)');
  }
  return out;
})()"#;

thread_local! {
    /// The pages, by screen number.
    static VIEWS: RefCell<Vec<(usize, wry::WebView)>> = const { RefCell::new(Vec::new()) };
    /// Everything needed to build the wallpaper windows again.
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
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

fn same_rect(a: &RECT, b: &RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

/// The monitors in physical pixels (the process is per-monitor DPI aware), the main one first.
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
    // the main monitor (its corner is 0,0) is screen 1, the others follow from the left
    found.sort_by_key(|r| ((r.left, r.top) != (0, 0), r.left, r.top));
    found
}

/// One window covers this whole monitor, taskbar and all (a fullscreen game or video): nobody
/// sees the page, so it can stop drawing. A maximized window is not that: the taskbar is still
/// there, and a transparent one shows the wallpaper through it. Nor is a big window with the
/// desktop showing around it.
fn covered(r: &RECT) -> bool {
    let center = POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
    // SAFETY: plain queries about windows on screen.
    unsafe {
        let hwnd = WindowFromPoint(center);
        let root = GetAncestor(hwnd, GA_ROOT);
        let top = if root.is_null() { hwnd } else { root };
        let class = class_of(top);
        let desktop = ["(none)", "Progman", "WorkerW", "SHELLDLL_DefView", "SysListView32", "termwallHost"];
        if desktop.contains(&class.as_str()) || class.starts_with("WPE") {
            return false;
        }
        let mut w: RECT = zeroed();
        GetWindowRect(top, &mut w) != 0 && w.left <= r.left && w.top <= r.top && w.right >= r.right && w.bottom >= r.bottom
    }
}

/// The monitors to cover, with their screen numbers (1 = the main monitor).
fn chosen(opts: &Options) -> Vec<(usize, RECT)> {
    monitors().into_iter().enumerate().map(|(i, r)| (i + 1, r)).filter(|(n, _)| opts.monitor.is_none_or(|m| m == *n)).collect()
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
            return Err(format!("could not create a window in {} (raised desktop: {}): {err}", class_of(parent), d.raised));
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

struct Handle(HWND);

impl HasWindowHandle for Handle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = NonZeroIsize::new(self.0 as isize).ok_or(HandleError::Unavailable)?;
        // SAFETY: the window outlives the WebView built on it (teardown drops the views first).
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(raw))) })
    }
}

fn webview_data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("TERMWALL_WEBVIEW2_DATA").filter(|v| !v.is_empty()) {
        return d.into();
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    local.join("termwall").join("WebView2")
}

struct Host {
    opts: Options,
    page: PathBuf,
    class: Vec<u16>,
    hinst: HINSTANCE,
    context: WebContext,
    /// What stands now: screen number, the monitor's rectangle, the window.
    built: Vec<(usize, RECT, HWND)>,
    /// The last reason nothing could be built, so it is said once.
    said: String,
    /// The screens whose pages are paused because windows cover them.
    paused: Vec<usize>,
}

impl Host {
    /// A window and a page for every chosen monitor. Returns how many stand afterwards.
    fn build(&mut self) -> Result<usize, String> {
        let desktop =
            if self.opts.window { None } else { Some(find_desktop().ok_or("no desktop (Progman): is Explorer running?")?) };
        let title = wide("termwall");
        for (screen, r) in chosen(&self.opts) {
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let (vw, vh) = if desktop.is_some() { (w, h) } else { (w * 3 / 4, h * 3 / 4) };
            let hwnd = match &desktop {
                Some(d) => create_on_desktop(self.class.as_ptr(), title.as_ptr(), self.hinst, &r, d)?,
                // SAFETY: our registered class; the strings outlive the call.
                None => unsafe {
                    let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
                    let (x, y) = (r.left + w / 8, r.top + h / 8);
                    CreateWindowExW(
                        0,
                        self.class.as_ptr(),
                        title.as_ptr(),
                        style,
                        x,
                        y,
                        vw,
                        vh,
                        null_mut(),
                        null_mut(),
                        self.hinst,
                        null(),
                    )
                },
            };
            if hwnd.is_null() {
                return Err(format!("screen {screen}: could not create a window: {}", std::io::Error::last_os_error()));
            }
            if desktop.is_none() {
                unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            }
            let bounds =
                Rect { position: PhysicalPosition::new(0, 0).into(), size: PhysicalSize::new(vw as u32, vh as u32).into() };
            let built = WebViewBuilder::new_with_web_context(&mut self.context)
                .with_url(page_url(&self.page, screen))
                .with_initialization_script(init_script())
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
                    println!("screen {screen}: {w}x{h} at {},{}, {place}", r.left, r.top);
                    VIEWS.with(|views| views.borrow_mut().push((screen, v)));
                    self.built.push((screen, r, hwnd));
                }
                Err(e) => {
                    unsafe { DestroyWindow(hwnd) };
                    return Err(format!("screen {screen}: WebView2 failed: {e}"));
                }
            }
        }
        Ok(self.built.len())
    }

    /// The pages first, then their windows (the ones Explorer took along are gone already).
    fn teardown(&mut self) {
        self.paused.clear();
        ALL_PAUSED.store(false, Ordering::SeqCst);
        VIEWS.with(|v| v.borrow_mut().clear());
        for (_, _, hwnd) in self.built.drain(..) {
            unsafe { DestroyWindow(hwnd) };
        }
    }

    /// A window is gone (Explorer restarted), or the monitors are not the ones built for.
    fn stale(&self) -> bool {
        let want = chosen(&self.opts);
        want.len() != self.built.len()
            || self
                .built
                .iter()
                .zip(&want)
                .any(|((n, r, hwnd), (wn, wr))| n != wn || !same_rect(r, wr) || unsafe { IsWindow(*hwnd) } == 0)
    }

    /// Pause the pages nobody can see, wake the ones uncovered again. A hidden WebView2 stops
    /// drawing and its timers slow down; the window behind it keeps the page's background color.
    fn cover(&mut self) {
        for (screen, r, _) in &self.built {
            let (hide, was) = (covered(r), self.paused.contains(screen));
            if hide == was {
                continue;
            }
            VIEWS.with(|v| {
                if let Some((_, view)) = v.borrow().iter().find(|(n, _)| n == screen) {
                    let _ = view.set_visible(!hide);
                }
            });
            if hide {
                self.paused.push(*screen);
            } else {
                self.paused.retain(|n| n != screen);
            }
            println!("screen {screen}: {}", if hide { "covered by a fullscreen window, paused" } else { "uncovered, running" });
        }
        ALL_PAUSED.store(!self.built.is_empty() && self.paused.len() == self.built.len(), Ordering::SeqCst);
    }

    fn watch(&mut self) {
        if !self.stale() {
            return;
        }
        self.teardown();
        let said = match self.build() {
            Ok(n) => format!("the desktop changed: {n} window(s) built again"),
            Err(e) => {
                self.teardown();
                format!("waiting for the desktop ({e})")
            }
        };
        if said != self.said {
            println!("{said}");
        }
        self.said = if said.starts_with("waiting") { said } else { String::new() };
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if w == TIMER_WATCH => {
            // not while a build is under way: creating a WebView2 pumps messages, this timer's too
            HOST.with(|h| {
                if let Ok(mut h) = h.try_borrow_mut()
                    && let Some(host) = h.as_mut()
                {
                    host.watch();
                }
            });
            0
        }
        WM_TIMER if w == TIMER_COVER => {
            HOST.with(|h| {
                if let Ok(mut h) = h.try_borrow_mut()
                    && let Some(host) = h.as_mut()
                {
                    host.cover();
                }
            });
            0
        }
        WM_TIMER if w == TIMER_TEST_KILL => {
            // SAFETY: our own timer and windows.
            unsafe { KillTimer(hwnd, TIMER_TEST_KILL) };
            let windows: Vec<HWND> =
                HOST.with(|h| h.borrow().as_ref().map(|h| h.built.iter().map(|b| b.2).collect()).unwrap_or_default());
            println!("test: destroying {} wallpaper window(s), as an Explorer restart would", windows.len());
            for hwnd in windows {
                unsafe { DestroyWindow(hwnd) };
            }
            0
        }
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
        WM_TIMER => {
            // SAFETY: called on the window's own thread.
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_APP_SCRIPT => {
            let scripts = std::mem::take(&mut *SCRIPTS.lock().unwrap_or_else(|e| e.into_inner()));
            VIEWS.with(|v| {
                for js in &scripts {
                    for (_, view) in v.borrow().iter() {
                        let _ = view.evaluate_script(js);
                    }
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
        WM_DESTROY => {
            // --window: the window closed, the host ends. Behind the icons a destroyed window
            // (Explorer restarted) is built again by the watch timer.
            if WINDOW_MODE.load(Ordering::SeqCst) && hwnd as isize != CONTROL.load(Ordering::SeqCst) {
                unsafe { PostQuitMessage(0) };
            }
            0
        }
        // SAFETY: the default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

pub fn run(opts: &Options, paths: &Paths) -> i32 {
    // SAFETY: process-wide setting, before any window exists.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let page = paths.page_file();
    if !page.is_file() {
        eprintln!("no page at {}", page.display());
        return 1;
    }
    let screens: Vec<String> = chosen(opts).iter().map(|(n, _)| n.to_string()).collect();
    if screens.is_empty() {
        eprintln!("no monitor {}; there are {}", opts.monitor.unwrap_or(0), monitors().len());
        return 1;
    }
    // SAFETY: registering our class with a static window procedure, then the hidden control window.
    let hinst = unsafe { GetModuleHandleW(null()) };
    let class = wide("termwallHost");
    let mut wc: WNDCLASSW = unsafe { zeroed() };
    wc.lpfnWndProc = Some(wndproc);
    wc.hInstance = hinst;
    wc.lpszClassName = class.as_ptr();
    // the page's background color (0x00bbggrr): what shows for a moment before a page draws
    wc.hbrBackground = unsafe { CreateSolidBrush(0x0017_1717) };
    let control = unsafe {
        RegisterClassW(&wc);
        let title = wide("termwall host");
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        )
    };
    if control.is_null() {
        eprintln!("could not create the host's window: {}", std::io::Error::last_os_error());
        return 1;
    }
    CONTROL.store(control as isize, Ordering::SeqCst);
    WINDOW_MODE.store(opts.window, Ordering::SeqCst);

    let mut host = Host {
        opts: opts.clone(),
        page,
        class,
        hinst,
        context: WebContext::new(Some(webview_data_dir())),
        built: Vec::new(),
        said: String::new(),
        paused: Vec::new(),
    };
    if let Err(e) = host.build() {
        eprintln!("{e}");
        host.teardown();
        return 1;
    }
    HOST.with(|h| *h.borrow_mut() = Some(host));

    // the music for the visualizer, captured only while some screen here has it switched on
    let target = control as isize;
    let test_run = opts.seconds.is_some();
    let style_paths = paths.clone();
    let wanted = move || {
        !ALL_PAUSED.load(Ordering::SeqCst)
            && screens.iter().any(|n| {
                let st = style::read_style(&style_paths.style_file, &style_paths.theme_file, Some(n));
                st.get("visualizer").and_then(|v| v.as_str()).is_some_and(|v| v != "off")
            })
    };
    let frames = AtomicUsize::new(0);
    audio::capture::spawn(wanted, move |frame| {
        *AUDIO.lock().unwrap_or_else(|e| e.into_inner()) = *frame;
        // a --seconds run says once a second what it hears: the loudest band and how loud
        if test_run && frames.fetch_add(1, Ordering::Relaxed).is_multiple_of(30) {
            let top = (0..frame.len()).max_by(|a, b| frame[*a].total_cmp(&frame[*b])).unwrap_or(0);
            let above = frame.iter().filter(|v| **v > 0.05).count();
            println!("audio: loudest band {top} at {:.2}, {above} of 128 bands above 0.05", frame[top]);
        }
        // one message at a time: a busy page must not pile frames up in the queue
        if !AUDIO_PENDING.swap(true, Ordering::SeqCst) {
            // SAFETY: posting to our own window.
            unsafe { PostMessageW(target as HWND, WM_APP_AUDIO, 0, 0) };
        }
    });
    // now playing, from Windows' media session
    media::watch::spawn(move |now| {
        if test_run {
            println!("media: {:?} by {:?}, state {}", now.title, now.artist, now.state);
        }
        SCRIPTS.lock().unwrap_or_else(|e| e.into_inner()).push(media::script(now));
        // SAFETY: posting to our own window.
        unsafe { PostMessageW(target as HWND, WM_APP_SCRIPT, 0, 0) };
    });

    // SAFETY: timers on our own window (the window procedure handles every id), then the
    // standard message loop of this thread.
    unsafe {
        if !opts.window {
            SetTimer(control, TIMER_WATCH, 2000, None);
            SetTimer(control, TIMER_COVER, 500, None);
        }
        if cfg!(debug_assertions) && std::env::var_os("TERMWALL_TEST_REBUILD").is_some() {
            SetTimer(control, TIMER_TEST_KILL, 5000, None);
        }
        if let Some(s) = opts.seconds {
            let ms = (s * 1000).min(u32::MAX as u64) as u32;
            SetTimer(control, TIMER_REPORT, ms.saturating_sub(2000).max(ms / 2), None);
            SetTimer(control, TIMER_QUIT, ms, None);
        }
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    if let Some(mut host) = HOST.with(|h| h.borrow_mut().take()) {
        host.teardown();
    }
    unsafe { DestroyWindow(control) };
    0
}
