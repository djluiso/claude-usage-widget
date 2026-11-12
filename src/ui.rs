//! The taskbar readout, the slide-in panel, and the Win32 plumbing around them.

use crate::config::{self, ConfigFile};
use crate::draw::{blurred_backdrop, hex, Canvas, Fonts, Rgba, Weight};
use crate::sys;
use crate::usage::{fmt_ago, fmt_left, fmt_money, fmt_tokens, pretty_project, Stats, Store, BLOCK_HOURS};
use chrono::{Duration as CDuration, Local};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};
use tiny_skia::Pixmap;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use Weight::{Regular as R, Semibold as SB};

mod settings;

// ------------------------------------------------------------------ theme --
struct Theme {
    panel: Rgba,
    panel_border: Rgba,
    card: Rgba,
    card_border: Rgba,
    grid: Rgba,
    track: Rgba,
    hover: Rgba,
    active: Rgba,
    text1: Rgba,
    text2: Rgba,
    text3: Rgba,
    accent: Rgba,
    good: Rgba,
    cat: [Rgba; 4],
}

const DARK: Theme = Theme {
    panel: [28, 28, 30, 208],
    panel_border: [255, 255, 255, 30],
    card: [255, 255, 255, 12],
    card_border: [255, 255, 255, 18],
    grid: [255, 255, 255, 24],
    track: [255, 255, 255, 34],
    hover: [255, 255, 255, 18],
    active: [255, 255, 255, 12],
    text1: hex(0xf0efec),
    text2: hex(0xc8c7bd),
    text3: hex(0x9a9993),
    accent: hex(0xd97757),
    good: hex(0x0ca30c),
    cat: [hex(0x3987e5), hex(0xd95926), hex(0x199e70), hex(0xc98500)],
};

const LIGHT: Theme = Theme {
    panel: [243, 243, 243, 215],
    panel_border: [0, 0, 0, 26],
    card: [255, 255, 255, 150],
    card_border: [0, 0, 0, 16],
    grid: [0, 0, 0, 22],
    track: [0, 0, 0, 28],
    hover: [0, 0, 0, 14],
    active: [0, 0, 0, 9],
    text1: hex(0x1a1a19),
    text2: hex(0x4a4a46),
    text3: hex(0x6d6d66),
    accent: hex(0xc96442),
    good: hex(0x0ca30c),
    cat: [hex(0x2a78d6), hex(0xeb6834), hex(0x1baf7a), hex(0xeda100)],
};

/// Panel theme: "dark" / "light" force it; anything else follows Windows'
/// app mode (Settings > Personalization > Colors > default app mode).
fn resolve_theme(setting: &str) -> &'static Theme {
    match setting {
        "dark" => &DARK,
        "light" => &LIGHT,
        _ if personalize_flag(w!("AppsUseLightTheme")) => &LIGHT,
        _ => &DARK,
    }
}

/// Readout theme: always matches the taskbar (Windows mode) so it stays legible.
fn taskbar_theme() -> &'static Theme {
    if personalize_flag(w!("SystemUsesLightTheme")) {
        &LIGHT
    } else {
        &DARK
    }
}

fn personalize_flag(name: PCWSTR) -> bool {
    use windows::Win32::System::Registry::*;
    let mut v: u32 = 0;
    let mut sz = 4u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut sz),
        )
        .is_ok()
            && v == 1
    }
}

// ------------------------------------------------------------- constants --
const BAR_H: f32 = 48.0; // logical taskbar height
const GAP_PX: i32 = 10; // readout <-> neighbour gap (physical)
const PANEL_W: f32 = 760.0;
const SHADOW_M: f32 = 24.0;

const CLS_READOUT: PCWSTR = w!("ClaudeUsageReadout");
const CLS_PANEL: PCWSTR = w!("ClaudeUsagePanel");
const OWN_CLASSES: [&str; 2] = ["ClaudeUsageReadout", "ClaudeUsagePanel"];

const WM_MOUSELEAVE: u32 = 0x02A3;
/// Posted by a second launch of the exe: open the Settings window.
pub const WM_OPEN_SETTINGS: u32 = WM_APP + 1;

const T_TICK: usize = 1;
const T_GUARD: usize = 2;
const T_ANIM: usize = 3;

const M_SHOW: usize = 1;
const M_DOCK_LEFT: usize = 2;
const M_DOCK_RIGHT: usize = 3;
const M_STARTUP: usize = 4;
const M_TRANSCRIPTS: usize = 5;
const M_SETTINGS: usize = 6;
const M_QUIT: usize = 9;
const M_THEME_SYSTEM: usize = 10;
const M_THEME_DARK: usize = 11;
const M_THEME_LIGHT: usize = 12;

struct Anim {
    x0: i32,
    x1: i32,
    y: i32,
    a0: f32,
    a1: f32,
    dur: f32,
    start: Instant,
    ease_out: bool,
    hide_after: bool,
}

struct App {
    fonts: Fonts,
    theme: &'static Theme,     // panel (user setting)
    bar_theme: &'static Theme, // readout (matches the taskbar)
    cfg: ConfigFile,
    store: Store,
    stats: Option<Stats>,
    readout: HWND,
    panel: HWND,
    owner: HWND,
    locator: sys::WidgetsLocator,
    hover: bool,
    hidden: bool,
    panel_open: bool,
    panel_x: i32,
    panel_y: i32,
    panel_size: (i32, i32),
    panel_bgra: Vec<u8>,
    backdrop: Option<Pixmap>,
    anim: Option<Anim>,
    closed_at: Option<Instant>,
    settings: Option<settings::SettingsUi>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static IN_MENU: Cell<bool> = const { Cell::new(false) };
}

pub fn trace(msg: &str) {
    if std::env::var_os("CLAUDE_WIDGET_TRACE").is_some() {
        let line = format!("[{}] {msg}\n", Local::now().format("%T%.3f"));
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(config::data_dir().join("trace.log"))
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
}

/// Run `f` on the app unless it is already borrowed (re-entrant message).
fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

// ------------------------------------------------------------------ setup --
pub fn run() {
    unsafe {
        let hinst = GetModuleHandleW(None).expect("module handle").into();
        let hand = LoadCursorW(None, IDC_HAND).unwrap_or_default();
        let arrow = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let procs: [(PCWSTR, WNDPROC, HCURSOR); 2] = [
            (CLS_READOUT, Some(readout_proc), hand),
            (CLS_PANEL, Some(panel_proc), arrow),
        ];
        for (cls, proc_, cur) in procs {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: proc_,
                hInstance: hinst,
                hCursor: cur,
                lpszClassName: cls,
                ..Default::default()
            };
            RegisterClassExW(&wc);
        }
        settings::register(hinst);
        let owner = sys::taskbar().unwrap_or_default();
        let readout = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            CLS_READOUT,
            w!("Claude Usage"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            owner,
            None,
            hinst,
            None,
        )
        .expect("create readout");
        let panel = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLS_PANEL,
            w!("Claude Code usage"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            hinst,
            None,
        )
        .expect("create panel");

        let cfg = ConfigFile::load();
        let refresh = cfg.cfg.refresh_ms.clamp(500, 60_000);
        let app = App {
            fonts: Fonts::load(),
            theme: resolve_theme(&cfg.cfg.theme),
            bar_theme: taskbar_theme(),
            cfg,
            store: Store::new(config::projects_dir()),
            stats: None,
            readout,
            panel,
            owner,
            locator: sys::WidgetsLocator::new(),
            hover: false,
            hidden: false,
            panel_open: false,
            panel_x: 0,
            panel_y: 0,
            panel_size: (0, 0),
            panel_bgra: Vec::new(),
            backdrop: None,
            anim: None,
            closed_at: None,
            settings: None,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        with_app(|a| a.tick());
        let _ = ShowWindow(readout, SW_SHOWNOACTIVATE);
        SetTimer(readout, T_TICK, refresh, None);
        SetTimer(readout, T_GUARD, 700, None);
        if std::env::args().any(|a| a == "--settings") {
            let _ = PostMessageW(readout, WM_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// --------------------------------------------------------------- behaviour --
impl App {
    fn tick(&mut self) {
        if self.cfg.reload_if_changed() {
            self.locator.invalidate();
        }
        // pick up Windows light/dark switches live
        let theme = resolve_theme(&self.cfg.cfg.theme);
        if !std::ptr::eq(theme, self.theme) {
            self.theme = theme;
            self.settings_theme_changed();
        }
        self.bar_theme = taskbar_theme();
        self.store.scan();
        self.stats = Some(self.store.stats(&self.cfg.cfg));
        self.render_readout();
        if self.panel_open && self.anim.is_none() {
            self.render_panel();
            sys::layered_show(
                self.panel,
                &self.panel_bgra,
                self.panel_size.0,
                self.panel_size.1,
                self.panel_x,
                self.panel_y,
                255,
            );
        }
    }

    fn scale(&self) -> f32 {
        sys::taskbar()
            .and_then(sys::rect_of)
            .map_or(1.0, |r| ((r.bottom - r.top) as f32 / BAR_H).max(0.5))
    }

    /// Owned windows stay above their owner, so the taskbar can't cover us.
    fn ensure_owner(&mut self, tray: HWND) {
        if self.owner != tray {
            unsafe { SetWindowLongPtrW(self.readout, GWLP_HWNDPARENT, tray.0 as isize) };
            self.owner = tray;
        }
    }

    fn guard(&mut self) {
        if sys::fullscreen_app_active(&OWN_CLASSES) {
            if !self.hidden {
                unsafe {
                    let _ = ShowWindow(self.readout, SW_HIDE);
                }
                self.hidden = true;
            }
        } else {
            if self.hidden {
                self.hidden = false;
                unsafe {
                    let _ = ShowWindow(self.readout, SW_SHOWNOACTIVATE);
                }
                self.render_readout();
            }
            unsafe {
                let _ = SetWindowPos(
                    self.readout,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
                );
            }
        }
    }

    // ------------------------------------------------------------ readout --
    fn render_readout(&mut self) {
        let Some(s) = self.stats.as_ref() else { return };
        let Some(tray) = sys::taskbar() else { return };
        let Some(bar) = sys::rect_of(tray) else { return };
        let hpx = bar.bottom - bar.top;
        let k = hpx as f32 / BAR_H;
        let c = draw_readout(&self.fonts, self.bar_theme, s, k, self.hover, self.panel_open);
        let (pw, ph) = (c.pm.width() as i32, c.pm.height() as i32);
        let bgra = c.to_bgra(1); // alpha 1: invisible but clickable
        let x = if self.cfg.cfg.dock == "right" {
            let edge = sys::tray_notify(tray).and_then(sys::rect_of).map_or(bar.right - 300, |r| r.left);
            edge - GAP_PX - pw
        } else {
            self.locator.right_edge(tray).unwrap_or(bar.left) + GAP_PX
        };
        let y = bar.top + (hpx - ph) / 2;
        sys::layered_show(self.readout, &bgra, pw, ph, x, y, 255);
        self.ensure_owner(tray);
    }

    // -------------------------------------------------------------- panel --
    fn panel_target(&self) -> (i32, i32, i32) {
        let k = self.scale();
        let (_, ph) = self.panel_size;
        let pw = self.panel_size.0;
        let bar = sys::taskbar().and_then(sys::rect_of);
        let (sw, sh) = sys::screen_size();
        let m = (SHADOW_M * k).round() as i32;
        let gap = (12.0 * k).round() as i32;
        let y = (bar.map_or(sh, |b| b.top) - gap - (ph - m)).max(-m);
        if self.cfg.cfg.dock == "right" {
            (bar.map_or(sw, |b| b.right) - gap - (pw - m), y, sw + 4)
        } else {
            (bar.map_or(0, |b| b.left) + gap - m, y, -pw - 4)
        }
    }

    fn render_panel(&mut self) {
        let Some(s) = self.stats.as_ref() else { return };
        let full = draw_panel(&self.fonts, self.theme, s, self.scale(), self.backdrop.as_ref());
        self.panel_size = (full.pm.width() as i32, full.pm.height() as i32);
        self.panel_bgra = full.to_bgra(0);
    }

    fn capture_backdrop(&mut self, x_open: i32, y: i32) {
        let k = self.scale();
        let m = (SHADOW_M * k).round() as i32;
        let (pw, ph) = self.panel_size;
        let (w, h) = (pw - 2 * m, ph - 2 * m);
        self.backdrop = sys::capture_screen(x_open + m, y + m, w, h)
            .and_then(|px| blurred_backdrop(&px, w as usize, h as usize, 4));
    }

    fn toggle_panel(&mut self) {
        trace(&format!("toggle open={}", self.panel_open));
        if self.panel_open {
            self.close_panel();
        } else if self.closed_at.map_or(true, |t| t.elapsed() > Duration::from_millis(350)) {
            self.open_panel();
        }
    }

    fn open_panel(&mut self) {
        if self.stats.is_none() {
            return;
        }
        self.backdrop = None;
        self.render_panel(); // sizes the panel
        let (x_open, y, x_closed) = self.panel_target();
        self.capture_backdrop(x_open, y);
        self.render_panel();
        self.panel_open = true;
        self.panel_x = x_closed;
        self.panel_y = y;
        let ok = sys::layered_show(self.panel, &self.panel_bgra, self.panel_size.0, self.panel_size.1, x_closed, y, 0);
        trace(&format!("panel ULW ok={ok} err={:?} bytes={}", unsafe { GetLastError() }, self.panel_bgra.len()));
        unsafe {
            let _ = ShowWindow(self.panel, SW_SHOW);
            let _ = SetForegroundWindow(self.panel);
        }
        trace(&format!("open at {x_open},{y} size {:?} fg={}", self.panel_size, unsafe { GetForegroundWindow() } == self.panel));
        self.start_anim(x_closed, x_open, y, 0.0, 255.0, 0.26, true, false);
        self.render_readout();
    }

    fn close_panel(&mut self) {
        trace("close_panel");
        if !self.panel_open {
            return;
        }
        self.panel_open = false;
        self.closed_at = Some(Instant::now());
        let (_, _, x_closed) = self.panel_target();
        self.start_anim(self.panel_x, x_closed, self.panel_y, 255.0, 0.0, 0.18, false, true);
        self.render_readout();
    }

    #[allow(clippy::too_many_arguments)]
    fn start_anim(&mut self, x0: i32, x1: i32, y: i32, a0: f32, a1: f32, dur: f32, ease_out: bool, hide: bool) {
        self.anim = Some(Anim { x0, x1, y, a0, a1, dur, start: Instant::now(), ease_out, hide_after: hide });
        unsafe { SetTimer(self.panel, T_ANIM, 10, None) };
        self.anim_step();
    }

    fn anim_step(&mut self) {
        let Some(a) = &self.anim else { return };
        let p = (a.start.elapsed().as_secs_f32() / a.dur).min(1.0);
        let e = if a.ease_out { 1.0 - (1.0 - p).powi(3) } else { p.powi(3) };
        let x = (a.x0 as f32 + (a.x1 - a.x0) as f32 * e).round() as i32;
        let alpha = a.a0 + (a.a1 - a.a0) * if a.ease_out { p } else { e };
        sys::layered_move(self.panel, x, a.y, alpha.clamp(0.0, 255.0) as u8);
        self.panel_x = x;
        if p >= 1.0 {
            let hide = a.hide_after;
            self.anim = None;
            unsafe {
                let _ = KillTimer(self.panel, T_ANIM);
                if hide {
                    let _ = ShowWindow(self.panel, SW_HIDE);
                }
            }
        }
    }

    fn handle_menu(&mut self, cmd: usize) {
        match cmd {
            M_SHOW => self.toggle_panel(),
            M_THEME_SYSTEM => self.set_theme("system"),
            M_THEME_DARK => self.set_theme("dark"),
            M_THEME_LIGHT => self.set_theme("light"),
            M_DOCK_LEFT => self.set_dock("left"),
            M_DOCK_RIGHT => self.set_dock("right"),
            M_STARTUP => {
                sys::set_startup(!sys::startup_enabled());
                self.settings_invalidate();
            }
            M_TRANSCRIPTS => sys::shell_open(&config::projects_dir().display().to_string(), None),
            M_SETTINGS => self.open_settings(),
            M_QUIT => unsafe {
                let _ = DestroyWindow(self.panel);
                let _ = DestroyWindow(self.readout);
                PostQuitMessage(0);
            },
            _ => {}
        }
    }
}

fn show_menu(owner: HWND) {
    let Some((dock_right, panel_open, theme)) =
        with_app(|a| (a.cfg.cfg.dock == "right", a.panel_open, a.cfg.cfg.theme.clone()))
    else {
        return;
    };
    let startup = sys::startup_enabled();
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let chk = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(
            m,
            MF_STRING,
            M_SHOW,
            if panel_open { w!("Hide usage panel") } else { w!("Show usage panel") },
        );
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING | chk(!dock_right), M_DOCK_LEFT, w!("Dock next to Widgets button"));
        let _ = AppendMenuW(m, MF_STRING | chk(dock_right), M_DOCK_RIGHT, w!("Dock next to tray icons"));
        if let Ok(sub) = CreatePopupMenu() {
            let system = theme != "dark" && theme != "light";
            let _ = AppendMenuW(sub, MF_STRING | chk(system), M_THEME_SYSTEM, w!("System"));
            let _ = AppendMenuW(sub, MF_STRING | chk(theme == "dark"), M_THEME_DARK, w!("Dark"));
            let _ = AppendMenuW(sub, MF_STRING | chk(theme == "light"), M_THEME_LIGHT, w!("Light"));
            let _ = AppendMenuW(m, MF_POPUP, sub.0 as usize, w!("Theme"));
        }
        let _ = AppendMenuW(m, MF_STRING | chk(startup), M_STARTUP, w!("Start with Windows"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING, M_TRANSCRIPTS, w!("Open transcripts folder"));
        let _ = AppendMenuW(m, MF_STRING, M_SETTINGS, w!("Settings…"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING, M_QUIT, w!("Quit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        IN_MENU.with(|f| f.set(true));
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, owner, None);
        let _ = PostMessageW(owner, WM_NULL, WPARAM(0), LPARAM(0));
        IN_MENU.with(|f| f.set(false));
        let _ = DestroyMenu(m);
        if cmd.0 != 0 {
            with_app(|a| a.handle_menu(cmd.0 as usize));
        }
    }
}

// ------------------------------------------------------------ window procs --
unsafe extern "system" fn readout_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp.0 == T_TICK => {
            with_app(|a| a.tick());
            LRESULT(0)
        }
        WM_TIMER if wp.0 == T_GUARD => {
            with_app(|a| a.guard());
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            with_app(|a| {
                if !a.hover {
                    a.hover = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                    a.render_readout();
                }
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_app(|a| {
                a.hover = false;
                a.render_readout();
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_app(|a| a.toggle_panel());
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            show_menu(hwnd);
            LRESULT(0)
        }
        WM_OPEN_SETTINGS => {
            with_app(|a| a.open_settings());
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp.0 == T_ANIM => {
            with_app(|a| a.anim_step());
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let inactive = (wp.0 & 0xffff) as u32 == WA_INACTIVE;
            trace(&format!("WM_ACTIVATE inactive={inactive}"));
            if inactive && !IN_MENU.with(|f| f.get()) {
                with_app(|a| a.close_panel());
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
            with_app(|a| a.close_panel());
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            show_menu(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ------------------------------------------------------- shared rendering --
fn draw_readout<'a>(fonts: &'a Fonts, th: &Theme, s: &Stats, k: f32, hover: bool, open: bool) -> Canvas<'a> {
    let line1 = fmt_tokens(if s.block_start.is_some() { s.block.all } else { s.today.all } as f64);
    let line2 = if s.block_start.is_some() {
        format!("{} left", fmt_left(s.block_left))
    } else {
        "idle · today".into()
    };
    let probe = Canvas::new(1.0, 1.0, k, fonts);
    let tw = probe.tw(&line1, 13.0, SB).max(probe.tw(&line2, 11.0, R));
    let (ring_d, spark_w) = (28.0, 62.0);
    let w = (8.0 + ring_d + 10.0 + tw + 12.0 + spark_w + 10.0).ceil();
    let mut c = Canvas::new(w, BAR_H, k, fonts);

    if open {
        c.rect(2.0, 4.0, w - 4.0, BAR_H - 8.0, th.active, 5.0);
        c.rect(w / 2.0 - 8.0, BAR_H - 7.0, 16.0, 3.0, th.accent, 1.5);
    } else if hover {
        c.rect(2.0, 4.0, w - 4.0, BAR_H - 8.0, th.hover, 5.0);
    }
    let (cx, cy) = (8.0 + ring_d / 2.0, BAR_H / 2.0);
    c.ring(cx, cy, ring_d / 2.0, 3.2, s.block_frac, th.accent, th.track);
    c.spark(cx, cy, 6.2, if s.live() { th.accent } else { th.text3 });
    let tx = 8.0 + ring_d + 10.0;
    c.text(tx, cy - 1.0, &line1, 13.0, th.text1, SB, "ls");
    c.text(tx, cy + 3.0, &line2, 11.0, th.text2, R, "lt");

    // sparkline: output tokens per 3 minutes over the last hour
    let (sx, base, sh) = (tx + tw + 12.0, cy + 10.0, 20.0);
    let n = s.spark.len() as f32;
    let bw = (spark_w - (n - 1.0)) / n;
    let peak = (*s.spark.iter().max().unwrap_or(&0)).max(1) as f32;
    c.line(sx, base + 0.5, sx + spark_w, base + 0.5, th.grid, 1.0);
    for (i, v) in s.spark.iter().enumerate() {
        if *v > 0 {
            let bh = (sh * *v as f32 / peak).max(2.0);
            c.bar_up(sx + i as f32 * (bw + 1.0), base, bw, bh, th.accent, 1.2);
        }
    }
    c
}

fn panel_layout(s: &Stats) -> (f32, usize, usize, [f32; 3], [f32; 4]) {
    let nm = s.models.len().clamp(1, 3);
    let ns = s.sessions.len().clamp(1, 4);
    let left = [160.0, 156.0, 2.0 * 82.0 + 12.0];
    let right = [164.0, 124.0, 46.0 + 30.0 * nm as f32, 46.0 + 38.0 * ns as f32];
    let gap = 14.0;
    let lh: f32 = left.iter().sum::<f32>() + gap * 2.0;
    let rh: f32 = right.iter().sum::<f32>() + gap * 3.0;
    ((72.0 + lh.max(rh) + 44.0).floor(), nm, ns, left, right)
}

/// The whole panel window image: shadow margin + glass panel + sections.
fn draw_panel<'a>(fonts: &'a Fonts, th: &Theme, s: &Stats, k: f32, backdrop: Option<&Pixmap>) -> Canvas<'a> {
    let (h, nm, ns, left_h, right_h) = panel_layout(s);
    let (w, pad, gap) = (PANEL_W, 20.0, 14.0);
    let cw = (w - 2.0 * pad - gap) / 2.0;

    let mut c = Canvas::new(w, h, k, fonts);
    match backdrop {
        Some(bd) => c.backdrop(bd, 12.0),
        None => c.rect(0.0, 0.0, w, h, [th.panel[0], th.panel[1], th.panel[2], 255], 12.0),
    }
    c.card(0.0, 0.0, w, h, th.panel, th.panel_border, 12.0);
    header(&mut c, th, s, pad, w);
    let top = 72.0;
    let mut y = top;
    hero(&mut c, th, s, pad, y, cw, left_h[0]);
    y += left_h[0] + gap;
    window_chart(&mut c, th, s, pad, y, cw, left_h[1]);
    y += left_h[1] + gap;
    tiles(&mut c, th, s, pad, y, cw);
    let x = pad + cw + gap;
    let mut y = top;
    week(&mut c, th, s, x, y, cw, right_h[0]);
    y += right_h[0] + gap;
    mix(&mut c, th, s, x, y, cw, right_h[1]);
    y += right_h[1] + gap;
    models(&mut c, th, s, x, y, cw, right_h[2], nm);
    y += right_h[2] + gap;
    sessions(&mut c, th, s, x, y, cw, right_h[3], ns);
    c.text(
        pad,
        h - 22.0,
        "$ figures are API-list-price estimates, not your plan's billing  ·  Esc to close  ·  right-click for options",
        10.0,
        th.text3,
        R,
        "la",
    );

    let m = SHADOW_M;
    let mut full = Canvas::new(w + 2.0 * m, h + 2.0 * m, k, fonts);
    for i in 0..16 {
        let i = i as f32;
        full.rect(m - i, m - i + 8.0, w + 2.0 * i, h + 2.0 * i, [0, 0, 0, 5], 12.0 + i);
    }
    let off = (m * k).round() as i32;
    full.draw_at(&c.pm, off, off);
    full
}

// ------------------------------------------------------ README screenshots --
/// `claude-usage-widget.exe --screenshots <dir>` renders the readout and panel
/// with made-up sample data (no real projects or usage) to PNG files.
pub fn render_screenshots(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let fonts = Fonts::load();
    let s = crate::usage::sample_stats();
    let k = 2.0; // crisp on high-DPI displays
    for (name, th, bar_bg, wall) in [
        ("dark", &DARK, [32u8, 32, 34, 255], [(58u8, 36u8, 92u8), (20, 70, 110)]),
        ("light", &LIGHT, [236, 236, 238, 255], [(196, 214, 240), (238, 214, 232)]),
    ] {
        // readout on a taskbar-coloured strip
        let ro = draw_readout(&fonts, th, &s, k, false, false);
        let strip_w = ro.pm.width() as f32 / k + 24.0;
        let mut strip = Canvas::new(strip_w, BAR_H, k, &fonts);
        strip.rect(0.0, 0.0, strip_w, BAR_H, bar_bg, 0.0);
        strip.draw_at(&ro.pm, (12.0 * k) as i32, 0);
        strip.pm.save_png(dir.join(format!("readout-{name}.png"))).map_err(std::io::Error::other)?;

        // panel over a soft "wallpaper" so the glass has something to blur
        let (bw, bh) = (96usize, 80usize);
        let mut wallpaper = vec![0u8; bw * bh * 4];
        for y in 0..bh {
            for x in 0..bw {
                let t = (x + y) as f32 / (bw + bh) as f32;
                let blob = (-(((x as f32 - 70.0).powi(2) + (y as f32 - 20.0).powi(2)) / 300.0)).exp();
                let mix = |a: u8, b: u8| a as f32 * (1.0 - t) + b as f32 * t;
                let i = (y * bw + x) * 4;
                wallpaper[i + 2] = (mix(wall[0].0, wall[1].0) + 120.0 * blob).min(255.0) as u8;
                wallpaper[i + 1] = (mix(wall[0].1, wall[1].1) + 60.0 * blob).min(255.0) as u8;
                wallpaper[i] = mix(wall[0].2, wall[1].2) as u8;
                wallpaper[i + 3] = 255;
            }
        }
        let backdrop = crate::draw::blurred_backdrop(&wallpaper, bw, bh, 1);
        let panel = draw_panel(&fonts, th, &s, k, backdrop.as_ref());
        panel.pm.save_png(dir.join(format!("panel-{name}.png"))).map_err(std::io::Error::other)?;
    }
    Ok(())
}

// --------------------------------------------------------- panel sections --
fn card(c: &mut Canvas, th: &Theme, x: f32, y: f32, w: f32, h: f32, title: &str, sub: Option<&str>) {
    c.card(x, y, w, h, th.card, th.card_border, 8.0);
    c.text(x + 16.0, y + 15.0, title, 12.0, th.text1, SB, "la");
    if let Some(sub) = sub {
        c.text(x + w - 16.0, y + 16.0, sub, 11.0, th.text3, R, "ra");
    }
}

fn header(c: &mut Canvas, th: &Theme, s: &Stats, pad: f32, w: f32) {
    c.spark(pad + 14.0, 36.0, 11.0, th.accent);
    c.text(pad + 36.0, 19.0, "Claude Code", 17.0, th.text1, SB, "la");
    let upd = format!("Usage  ·  updated {}", Local::now().format("%H:%M:%S"));
    c.text(pad + 36.0, 44.0, &upd, 11.0, th.text3, R, "la");
    let live = s.live();
    let label = if live {
        "Live"
    } else if s.active > 0 {
        "Session open"
    } else {
        "Idle"
    };
    let tw = c.tw(label, 11.0, SB);
    let cx = w - pad - tw - 34.0;
    c.card(cx, 22.0, tw + 34.0, 26.0, th.card, th.card_border, 13.0);
    c.circle(cx + 15.0, 35.0, 4.0, if live { th.good } else { th.text3 });
    c.text(cx + 25.0, 35.0, label, 11.0, th.text1, SB, "lm");
}

fn hero(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32) {
    c.card(x, y, w, h, th.card, th.card_border, 8.0);
    let (cx, cy) = (x + 76.0, y + h / 2.0);
    c.ring(cx, cy, 56.0, 9.0, s.block_frac, th.accent, th.track);
    if s.block_start.is_some() {
        c.text(cx, cy - 2.0, &fmt_left(s.block_left), 20.0, th.text1, SB, "ms");
        c.text(cx, cy + 4.0, "until reset", 10.0, th.text3, R, "ma");
    } else {
        c.text(cx, cy, "idle", 18.0, th.text3, SB, "mm");
    }
    let tx = x + 152.0;
    let b = &s.block;
    c.text(tx, y + 22.0, "5-HOUR WINDOW", 10.0, th.text3, SB, "la");
    let big = if s.block_start.is_some() { fmt_tokens(b.all as f64) } else { "—".into() };
    c.text(tx, y + 38.0, &big, 30.0, th.text1, SB, "la");
    c.text(tx, y + 80.0, &format!("tokens  ·  {} est.", fmt_money(b.cost)), 12.0, th.text2, R, "la");
    c.line(tx, y + 106.0, x + w - 16.0, y + 106.0, th.grid, 1.0);
    if let Some(st) = s.block_start {
        let st = st.with_timezone(&Local);
        let en = st + CDuration::hours(BLOCK_HOURS);
        let t = format!("{} → resets {}", st.format("%H:%M"), en.format("%H:%M"));
        c.text(tx, y + 114.0, &t, 11.0, th.text3, R, "la");
    }
    let t = format!(
        "{} messages  ·  {} active session{}",
        b.n,
        s.active,
        if s.active == 1 { "" } else { "s" }
    );
    c.text(tx, y + 132.0, &t, 11.0, th.text3, R, "la");
}

fn window_chart(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32) {
    let total: u64 = s.wbins.iter().sum();
    let sub = s.block_start.map(|_| format!("{} out", fmt_tokens(total as f64)));
    card(c, th, x, y, w, h, "Output tokens this window", sub.as_deref());
    let (px0, px1, base, top) = (x + 16.0, x + w - 16.0, y + h - 30.0, y + 52.0);
    let n = s.wbins.len() as f32;
    let g = 2.0;
    let bw = (px1 - px0 - g * (n - 1.0)) / n;
    let peak = (*s.wbins.iter().max().unwrap_or(&0)).max(1) as f32;
    c.line(px0, base + 0.5, px1, base + 0.5, th.grid, 1.0);
    for (i, v) in s.wbins.iter().enumerate() {
        let bx = px0 + i as f32 * (bw + g);
        if s.block_start.is_some() && i as i64 > s.cur_bin {
            c.rect(bx, base - 2.0, bw, 2.0, th.track, 1.0);
        } else if *v > 0 {
            c.bar_up(bx, base, bw, ((base - top) * *v as f32 / peak).max(2.0), th.accent, 3.0);
        }
    }
    match s.block_start {
        Some(st) => {
            let maxv = *s.wbins.iter().max().unwrap_or(&0);
            if maxv > 0 {
                let i = s.wbins.iter().position(|v| *v == maxv).unwrap_or(0);
                let bx = (px0 + i as f32 * (bw + g) + bw / 2.0).clamp(px0 + 20.0, px1 - 20.0);
                c.text(bx, top - 6.0, &fmt_tokens(maxv as f64), 10.0, th.text2, R, "ms");
            }
            let st = st.with_timezone(&Local);
            let mid = st + CDuration::minutes(BLOCK_HOURS * 30);
            let en = st + CDuration::hours(BLOCK_HOURS);
            c.text(px0, base + 8.0, &st.format("%H:%M").to_string(), 10.0, th.text3, R, "la");
            c.text((px0 + px1) / 2.0, base + 8.0, &mid.format("%H:%M").to_string(), 10.0, th.text3, R, "ma");
            c.text(px1, base + 8.0, &en.format("%H:%M").to_string(), 10.0, th.text3, R, "ra");
        }
        None => c.text((px0 + px1) / 2.0, (top + base) / 2.0, "No active window", 11.0, th.text3, R, "mm"),
    }
}

fn tiles(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32) {
    let (tw, tht, g) = ((w - 12.0) / 2.0, 82.0, 12.0);
    let t = &s.today;
    let denom = (t.cr + t.cw + t.inp) as f64;
    let hit = if denom > 0.0 { t.cr as f64 / denom } else { 0.0 };
    let items: [(&str, String, String, Option<f64>); 4] = [
        ("Burn rate", format!("{}/min", fmt_tokens(s.burn)), "output · last 10 min".into(), None),
        ("Last hour", fmt_tokens(s.hour.all as f64), format!("{} est.", fmt_money(s.hour.cost)), None),
        ("Today", fmt_tokens(t.all as f64), format!("{} est. · {} msgs", fmt_money(t.cost), t.n), None),
        ("Cache hit", format!("{:.0}%", hit * 100.0), "of input read from cache".into(), Some(hit)),
    ];
    for (i, (lab, val, sub, frac)) in items.iter().enumerate() {
        let bx = x + (i % 2) as f32 * (tw + g);
        let by = y + (i / 2) as f32 * (tht + g);
        c.card(bx, by, tw, tht, th.card, th.card_border, 8.0);
        c.text(bx + 14.0, by + 12.0, lab, 11.0, th.text3, R, "la");
        c.text(bx + 14.0, by + 28.0, val, 20.0, th.text1, SB, "la");
        let sub = c.fit(sub, 10.0, R, tw - 28.0);
        c.text(bx + 14.0, by + 60.0, &sub, 10.0, th.text3, R, "la");
        if let Some(f) = frac {
            c.rect(bx + tw - 64.0, by + 40.0, 50.0, 5.0, th.track, 2.5);
            c.rect(bx + tw - 64.0, by + 40.0, (50.0 * *f as f32).max(5.0), 5.0, th.accent, 2.5);
        }
    }
}

fn week(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32) {
    let total: f64 = s.days.iter().map(|d| d.cost).sum();
    card(c, th, x, y, w, h, "Last 7 days", Some(&format!("{} est.", fmt_money(total))));
    let (px0, px1, base, top) = (x + 16.0, x + w - 16.0, y + h - 30.0, y + 58.0);
    let n = s.days.len();
    let slot = (px1 - px0) / n as f32;
    let bw = (slot - 12.0).min(26.0);
    let peak = s.days.iter().map(|d| d.cost).fold(0.0, f64::max).max(1e-9);
    let ipeak = s
        .days
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.cost.partial_cmp(&b.1.cost).unwrap_or(std::cmp::Ordering::Equal))
        .map_or(0, |(i, _)| i);
    c.line(px0, base + 0.5, px1, base + 0.5, th.grid, 1.0);
    for (i, d) in s.days.iter().enumerate() {
        let cx = px0 + slot * (i as f32 + 0.5);
        if d.cost > 0.0 {
            let hh = ((base - top) * (d.cost / peak) as f32).max(2.0);
            c.bar_up(cx - bw / 2.0, base, bw, hh, th.accent, 4.0);
            if i == ipeak || i == n - 1 {
                c.text(cx, base - hh - 5.0, &fmt_money(d.cost), 10.0, th.text2, R, "ms");
            }
        }
        let today = i == n - 1;
        c.text(
            cx,
            base + 8.0,
            &d.label,
            10.0,
            if today { th.text1 } else { th.text3 },
            if today { SB } else { R },
            "ma",
        );
    }
}

fn mix(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32) {
    let t = &s.today;
    card(c, th, x, y, w, h, "Today's token mix", Some(&format!("{} total", fmt_tokens(t.all as f64))));
    let parts = [("Cache read", t.cr), ("Cache write", t.cw), ("Input", t.inp), ("Output", t.out)];
    let (bx0, bx1, by) = (x + 16.0, x + w - 16.0, y + 46.0);
    let total: u64 = parts.iter().map(|p| p.1).sum();
    if total > 0 {
        let nz: Vec<(usize, u64)> = parts.iter().enumerate().filter(|p| p.1 .1 > 0).map(|(i, p)| (i, p.1)).collect();
        let avail = bx1 - bx0 - 2.0 * (nz.len() as f32 - 1.0);
        let widths: Vec<f32> = nz.iter().map(|(_, v)| (avail * *v as f32 / total as f32).max(4.0)).collect();
        let kf = avail / widths.iter().sum::<f32>();
        let mut cx = bx0;
        for ((i, _), wd) in nz.iter().zip(widths) {
            let wd = wd * kf;
            c.rect(cx, by, wd, 12.0, th.cat[*i], 3.0);
            cx += wd + 2.0;
        }
    } else {
        c.rect(bx0, by, bx1 - bx0, 12.0, th.track, 3.0);
    }
    let colw = (bx1 - bx0 - 16.0) / 2.0;
    for (i, (name, v)) in parts.iter().enumerate() {
        let lx = bx0 + (i % 2) as f32 * (colw + 16.0);
        let ly = by + 26.0 + (i / 2) as f32 * 24.0;
        c.rect(lx, ly + 3.0, 9.0, 9.0, th.cat[i], 2.0);
        c.text(lx + 16.0, ly, name, 11.0, th.text2, R, "la");
        let pct = if total > 0 { format!("  {:.0}%", *v as f64 / total as f64 * 100.0) } else { String::new() };
        c.text(lx + colw, ly, &format!("{}{}", fmt_tokens(*v as f64), pct), 11.0, th.text1, SB, "ra");
    }
}

#[allow(clippy::too_many_arguments)]
fn models(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32, nm: usize) {
    card(c, th, x, y, w, h, "Models today", None);
    if s.models.is_empty() {
        c.text(x + 16.0, y + 46.0, "No usage yet today", 11.0, th.text3, R, "la");
        return;
    }
    let tot = s.models.iter().map(|m| m.cost).sum::<f64>().max(1e-9);
    for (i, m) in s.models.iter().take(nm).enumerate() {
        let ry = y + 44.0 + i as f32 * 30.0;
        let share = m.cost / tot;
        let name = c.fit(&m.name, 12.0, R, 120.0);
        c.text(x + 16.0, ry, &name, 12.0, th.text1, R, "la");
        let (b0, b1) = (x + 150.0, x + w - 120.0);
        c.rect(b0, ry + 5.0, b1 - b0, 6.0, th.track, 3.0);
        c.rect(b0, ry + 5.0, ((b1 - b0) * share as f32).max(6.0), 6.0, th.accent, 3.0);
        let t = format!("{} · {:.0}%", fmt_money(m.cost), share * 100.0);
        c.text(x + w - 16.0, ry, &t, 11.0, th.text2, R, "ra");
    }
}

#[allow(clippy::too_many_arguments)]
fn sessions(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32, ns: usize) {
    card(c, th, x, y, w, h, "Recent sessions", Some("last 7 days"));
    if s.sessions.is_empty() {
        c.text(x + 16.0, y + 46.0, "No sessions yet", 11.0, th.text3, R, "la");
        return;
    }
    for (i, ss) in s.sessions.iter().take(ns).enumerate() {
        let ry = y + 44.0 + i as f32 * 38.0;
        if i > 0 {
            c.line(x + 16.0, ry - 4.0, x + w - 16.0, ry - 4.0, th.grid, 1.0);
        }
        let name = c.fit(&pretty_project(&ss.proj), 12.0, SB, w - 140.0);
        c.text(x + 16.0, ry, &name, 12.0, th.text1, SB, "la");
        let (when, wx) = if ss.active {
            c.circle(x + 20.0, ry + 23.0, 3.5, th.good);
            ("active now".to_string(), x + 28.0)
        } else {
            (fmt_ago((s.now - ss.last).num_seconds()), x + 16.0)
        };
        let t = format!("{}  ·  {} tokens", when, fmt_tokens(ss.all as f64));
        c.text(wx, ry + 17.0, &t, 10.0, th.text3, R, "la");
        c.text(x + w - 16.0, ry + 6.0, &fmt_money(ss.cost), 12.0, th.text2, R, "ra");
    }
}
