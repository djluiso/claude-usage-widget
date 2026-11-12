//! Windows: the taskbar readout, the slide-in drawer, and the Win32 plumbing
//! around them. Drawing lives in `crate::view`.

use crate::config::{self, ConfigFile};
use crate::draw::{blurred_backdrop, Fonts};
use crate::sys;
use crate::usage::{Stats, Store};
use chrono::Local;
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};
use tiny_skia::Pixmap;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

mod actions;

use crate::view::settings;
use crate::view::*;

/// Readout theme: always matches the taskbar (Windows mode) so it stays legible.
fn taskbar_theme() -> &'static Theme {
    if personalize_flag(w!("SystemUsesLightTheme")) {
        &LIGHT
    } else {
        &DARK
    }
}

pub(crate) fn personalize_flag(name: PCWSTR) -> bool {
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
const GAP_PX: i32 = 10; // readout <-> neighbour gap (physical)

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
    /// `--demo`: sample data instead of transcripts.
    demo: bool,
    /// Plan limits from Anthropic, fetched in the background.
    limits: crate::limits::Fetcher,
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
    /// The drawer is showing Settings instead of usage.
    show_settings: bool,
    /// Clickable regions of the drawer, in logical panel coordinates.
    panel_hits: settings::Hits,
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
pub fn run(opts: crate::Options) {
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
            demo: opts.demo,
            limits: crate::limits::Fetcher::start(cfg.cfg.plan_limits && !opts.demo),
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
            show_settings: false,
            panel_hits: Vec::new(),
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        with_app(|a| a.tick());
        let _ = ShowWindow(readout, SW_SHOWNOACTIVATE);
        SetTimer(readout, T_TICK, refresh, None);
        SetTimer(readout, T_GUARD, 700, None);
        if opts.settings {
            let _ = PostMessageW(readout, WM_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
        } else if opts.open {
            with_app(|a| a.open_panel());
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
        self.theme = theme;
        self.bar_theme = taskbar_theme();
        if self.demo {
            self.stats = Some(crate::usage::sample_stats());
        } else {
            self.store.scan();
            let mut stats = self.store.stats();
            stats.apply_limits(self.limits.snapshot());
            self.stats = Some(stats);
        }
        self.render_readout();
        self.redraw_panel();
    }

    /// Re-render the open drawer in place (not while it slides).
    fn redraw_panel(&mut self) {
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
        let Some(bar) = sys::rect_of(tray) else {
            return;
        };
        let hpx = bar.bottom - bar.top;
        let k = hpx as f32 / BAR_H;
        let c = draw_readout(
            &self.fonts,
            self.bar_theme,
            s,
            k,
            self.hover,
            self.panel_open,
        );
        let (pw, ph) = (c.pm.width() as i32, c.pm.height() as i32);
        let bgra = c.to_bgra(1); // alpha 1: invisible but clickable
        let x = if self.cfg.cfg.dock == "right" {
            let edge = sys::tray_notify(tray)
                .and_then(sys::rect_of)
                .map_or(bar.right - 300, |r| r.left);
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
        let startup = self.show_settings && crate::platform::autostart_enabled();
        let view = self.show_settings.then_some((&self.cfg.cfg, startup));
        let (full, hits) = draw_panel(
            &self.fonts,
            self.theme,
            s,
            self.scale(),
            self.backdrop.as_ref(),
            view,
        );
        self.panel_hits = hits;
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
        } else if self
            .closed_at
            .is_none_or(|t| t.elapsed() > Duration::from_millis(350))
        {
            self.show_settings = false;
            self.open_panel();
        }
    }

    fn open_panel(&mut self) {
        if self.stats.is_none() {
            return;
        }
        self.backdrop = None;
        self.limits.poke(); // fresh plan limits when the drawer opens
        self.render_panel(); // sizes the panel
        let (x_open, y, x_closed) = self.panel_target();
        self.capture_backdrop(x_open, y);
        self.render_panel();
        self.panel_open = true;
        self.panel_x = x_closed;
        self.panel_y = y;
        let ok = sys::layered_show(
            self.panel,
            &self.panel_bgra,
            self.panel_size.0,
            self.panel_size.1,
            x_closed,
            y,
            0,
        );
        trace(&format!(
            "panel ULW ok={ok} err={:?} bytes={}",
            unsafe { GetLastError() },
            self.panel_bgra.len()
        ));
        unsafe {
            let _ = ShowWindow(self.panel, SW_SHOW);
            let _ = SetForegroundWindow(self.panel);
        }
        trace(&format!(
            "open at {x_open},{y} size {:?} fg={}",
            self.panel_size,
            unsafe { GetForegroundWindow() } == self.panel
        ));
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
        self.start_anim(
            self.panel_x,
            x_closed,
            self.panel_y,
            255.0,
            0.0,
            0.18,
            false,
            true,
        );
        self.render_readout();
    }

    #[allow(clippy::too_many_arguments)]
    fn start_anim(
        &mut self,
        x0: i32,
        x1: i32,
        y: i32,
        a0: f32,
        a1: f32,
        dur: f32,
        ease_out: bool,
        hide: bool,
    ) {
        self.anim = Some(Anim {
            x0,
            x1,
            y,
            a0,
            a1,
            dur,
            start: Instant::now(),
            ease_out,
            hide_after: hide,
        });
        unsafe { SetTimer(self.panel, T_ANIM, 10, None) };
        self.anim_step();
    }

    fn anim_step(&mut self) {
        let Some(a) = &self.anim else { return };
        let p = (a.start.elapsed().as_secs_f32() / a.dur).min(1.0);
        let e = if a.ease_out {
            1.0 - (1.0 - p).powi(3)
        } else {
            p.powi(3)
        };
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
                crate::platform::set_autostart(!crate::platform::autostart_enabled());
                self.redraw_panel();
            }
            M_TRANSCRIPTS => crate::platform::open_folder(&config::projects_dir()),
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
    let Some((dock_right, panel_open, theme)) = with_app(|a| {
        (
            a.cfg.cfg.dock == "right",
            a.panel_open,
            a.cfg.cfg.theme.clone(),
        )
    }) else {
        return;
    };
    let startup = crate::platform::autostart_enabled();
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let chk = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(
            m,
            MF_STRING,
            M_SHOW,
            if panel_open {
                w!("Hide usage panel")
            } else {
                w!("Show usage panel")
            },
        );
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(
            m,
            MF_STRING | chk(!dock_right),
            M_DOCK_LEFT,
            w!("Dock next to Widgets button"),
        );
        let _ = AppendMenuW(
            m,
            MF_STRING | chk(dock_right),
            M_DOCK_RIGHT,
            w!("Dock next to tray icons"),
        );
        if let Ok(sub) = CreatePopupMenu() {
            let system = theme != "dark" && theme != "light";
            let _ = AppendMenuW(sub, MF_STRING | chk(system), M_THEME_SYSTEM, w!("System"));
            let _ = AppendMenuW(
                sub,
                MF_STRING | chk(theme == "dark"),
                M_THEME_DARK,
                w!("Dark"),
            );
            let _ = AppendMenuW(
                sub,
                MF_STRING | chk(theme == "light"),
                M_THEME_LIGHT,
                w!("Light"),
            );
            let _ = AppendMenuW(m, MF_POPUP, sub.0 as usize, w!("Theme"));
        }
        let _ = AppendMenuW(
            m,
            MF_STRING | chk(startup),
            M_STARTUP,
            w!("Start with Windows"),
        );
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING, M_TRANSCRIPTS, w!("Open transcripts folder"));
        let _ = AppendMenuW(m, MF_STRING, M_SETTINGS, w!("Settings"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING, M_QUIT, w!("Quit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        IN_MENU.with(|f| f.set(true));
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenu(
            m,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            owner,
            None,
        );
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
        // no menu on the taskbar readout: options live in the drawer
        WM_RBUTTONUP | WM_CONTEXTMENU => LRESULT(0),
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
            with_app(|a| {
                if a.show_settings {
                    a.set_view(false)
                } else {
                    a.close_panel()
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = (
                (lp.0 & 0xffff) as i16 as i32,
                ((lp.0 >> 16) & 0xffff) as i16 as i32,
            );
            with_app(|a| a.panel_click(x, y));
            LRESULT(0)
        }
        WM_SETCURSOR if (lp.0 & 0xffff) as u32 == HTCLIENT => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
            let hand = with_app(|a| a.panel_hit(pt.x, pt.y).is_some()).unwrap_or(false);
            let cur =
                LoadCursorW(None, if hand { IDC_HAND } else { IDC_ARROW }).unwrap_or_default();
            SetCursor(cur);
            LRESULT(1)
        }
        WM_RBUTTONUP => {
            show_menu(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
