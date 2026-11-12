//! Settings window: theme, position, startup, refresh rate and price table.
//! Custom-drawn to match the panel; every change is saved immediately.

use super::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};

const SW: f32 = 560.0; // logical width
const CLS_SETTINGS: PCWSTR = w!("ClaudeUsageSettings");
const T_CARET: usize = 4;
const STYLE: WINDOW_STYLE =
    WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

#[derive(Clone, PartialEq)]
enum Hit {
    Theme(&'static str),
    Dock(&'static str),
    Startup,
    Refresh(u32),
    Price(String, usize),
    Reset,
    OpenConfig,
    OpenTranscripts,
}

pub(super) struct SettingsUi {
    hwnd: HWND,
    k: f32,
    hits: Vec<([f32; 4], Hit)>,
    focus: Option<(String, usize)>,
    buf: String,
    fresh: bool, // value selected: next keystroke replaces it
    caret: bool,
    size: (i32, i32),
}

pub(super) fn register(hinst: windows::Win32::Foundation::HINSTANCE) {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(settings_proc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            // MAKEINTRESOURCE(1): the app icon embedded by build.rs
            hIcon: LoadIconW(hinst, PCWSTR(std::ptr::without_provenance(1))).unwrap_or_default(),
            lpszClassName: CLS_SETTINGS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

fn fmt_price(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn is_light(th: &Theme) -> bool {
    std::ptr::eq(th, &LIGHT)
}

impl App {
    // ------------------------------------------------------------ actions --
    pub(super) fn set_theme(&mut self, t: &str) {
        self.cfg.cfg.theme = t.into();
        self.cfg.save();
        self.theme = resolve_theme(t);
        self.render_readout();
        if self.panel_open && self.anim.is_none() {
            self.render_panel();
            let (w, h) = self.panel_size;
            sys::layered_show(
                self.panel,
                &self.panel_bgra,
                w,
                h,
                self.panel_x,
                self.panel_y,
                255,
            );
        }
        self.settings_theme_changed();
    }

    pub(super) fn set_dock(&mut self, d: &str) {
        self.cfg.cfg.dock = d.into();
        self.cfg.save();
        self.locator.invalidate();
        self.render_readout();
    }

    pub(super) fn set_refresh(&mut self, ms: u32) {
        self.cfg.cfg.refresh_ms = ms;
        self.cfg.save();
        unsafe {
            let _ = KillTimer(self.readout, T_TICK);
            SetTimer(self.readout, T_TICK, ms.clamp(500, 60_000), None);
        }
    }

    // --------------------------------------------------------- lifecycle --
    pub(super) fn open_settings(&mut self) {
        if let Some(s) = &self.settings {
            unsafe {
                let _ = ShowWindow(s.hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(s.hwnd);
            }
            return;
        }
        unsafe {
            let hinst: windows::Win32::Foundation::HINSTANCE =
                GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_APPWINDOW,
                CLS_SETTINGS,
                w!("Claude Usage Widget — Settings"),
                STYLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                100,
                100,
                None,
                None,
                hinst,
                None,
            ) else {
                return;
            };
            let k = GetDpiForWindow(hwnd) as f32 / 96.0;
            self.settings = Some(SettingsUi {
                hwnd,
                k,
                hits: Vec::new(),
                focus: None,
                buf: String::new(),
                fresh: false,
                caret: true,
                size: (0, 0),
            });
            self.settings_theme_changed();
            self.fit_settings_window(true);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }

    fn settings_height(&self) -> f32 {
        let rows = self.cfg.cfg.prices.len() as f32;
        84.0 + 288.0 + 16.0 + (88.0 + rows * 38.0 + 52.0) + 16.0 + 40.0
    }

    /// Size the window so its client area matches the content (and centre it).
    fn fit_settings_window(&mut self, center: bool) {
        let h = self.settings_height();
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        let (cw, ch) = ((SW * s.k).round() as i32, (h * s.k).round() as i32);
        if s.size == (cw, ch) && !center {
            return;
        }
        s.size = (cw, ch);
        unsafe {
            let mut r = RECT {
                left: 0,
                top: 0,
                right: cw,
                bottom: ch,
            };
            let dpi = GetDpiForWindow(s.hwnd);
            let _ = AdjustWindowRectExForDpi(&mut r, STYLE, false, WS_EX_APPWINDOW, dpi);
            let (ww, wh) = (r.right - r.left, r.bottom - r.top);
            let mut flags = SWP_NOZORDER | SWP_NOACTIVATE;
            let (mut x, mut y) = (0, 0);
            if center {
                let (sw, sh) = sys::screen_size();
                x = (sw - ww) / 2;
                y = ((sh - wh) / 2).max(0);
            } else {
                flags |= SWP_NOMOVE;
            }
            let _ = SetWindowPos(s.hwnd, None, x, y, ww, wh, flags);
            let _ = InvalidateRect(s.hwnd, None, false);
        }
    }

    /// Title bar colours follow the theme.
    pub(super) fn settings_theme_changed(&mut self) {
        let light = is_light(self.theme);
        let Some(s) = &self.settings else { return };
        unsafe {
            let dark = BOOL::from(!light);
            let _ = DwmSetWindowAttribute(
                s.hwnd,
                DWMWINDOWATTRIBUTE(20), // DWMWA_USE_IMMERSIVE_DARK_MODE
                &dark as *const _ as *const _,
                4,
            );
            let bg: u32 = if light { 0x00f3f3f3 } else { 0x00202020 };
            let _ = DwmSetWindowAttribute(
                s.hwnd,
                DWMWINDOWATTRIBUTE(35), // DWMWA_CAPTION_COLOR
                &bg as *const _ as *const _,
                4,
            );
            let _ = InvalidateRect(s.hwnd, None, false);
        }
    }

    pub(super) fn settings_invalidate(&self) {
        if let Some(s) = &self.settings {
            unsafe {
                let _ = InvalidateRect(s.hwnd, None, false);
            }
        }
    }

    // ----------------------------------------------------------- drawing --
    fn paint_settings(&mut self, hwnd: HWND) {
        self.fit_settings_window(false);
        let h = self.settings_height();
        let th = self.theme;
        let startup = sys::startup_enabled();
        let cfg = self.cfg.cfg.clone();
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        let k = s.k;
        let mut c = Canvas::new(SW, h, k, &self.fonts);
        let mut hits: Vec<([f32; 4], Hit)> = Vec::new();
        let light = is_light(th);
        let bg = if light {
            [243, 243, 243, 255]
        } else {
            [32, 32, 32, 255]
        };
        let field_bg = if light {
            [255, 255, 255, 255]
        } else {
            [255, 255, 255, 14]
        };
        c.rect(0.0, 0.0, SW, h, bg, 0.0);

        // header
        c.spark(32.0, 40.0, 11.0, th.accent);
        c.text(54.0, 22.0, "Settings", 20.0, th.text1, SB, "la");
        c.text(
            54.0,
            52.0,
            "Changes are saved automatically",
            11.0,
            th.text3,
            R,
            "la",
        );

        // ---- General
        let (x, w) = (20.0, SW - 40.0);
        let mut y = 84.0;
        c.card(x, y, w, 288.0, th.card, th.card_border, 8.0);
        c.text(x + 16.0, y + 14.0, "General", 12.0, th.text1, SB, "la");
        let rows: [(&str, &str); 4] = [
            ("Theme", "Panel colours. System follows Windows' app mode."),
            ("Position", "Where the readout sits on the taskbar"),
            (
                "Start with Windows",
                "Launch automatically when you sign in",
            ),
            ("Refresh every", "How often transcripts are rescanned"),
        ];
        for (i, (label, desc)) in rows.iter().enumerate() {
            let ry = y + 40.0 + i as f32 * 60.0;
            if i > 0 {
                c.line(x + 16.0, ry, x + w - 16.0, ry, th.grid, 1.0);
            }
            c.text(x + 16.0, ry + 13.0, label, 12.0, th.text1, R, "la");
            c.text(x + 16.0, ry + 32.0, desc, 10.0, th.text3, R, "la");
            let (right, cy) = (x + w - 16.0, ry + 30.0);
            match i {
                0 => {
                    let sel = match cfg.theme.as_str() {
                        "dark" => 1,
                        "light" => 2,
                        _ => 0,
                    };
                    segmented(
                        &mut c,
                        th,
                        &mut hits,
                        right,
                        cy,
                        &[
                            ("System", Hit::Theme("system")),
                            ("Dark", Hit::Theme("dark")),
                            ("Light", Hit::Theme("light")),
                        ],
                        sel,
                    );
                }
                1 => {
                    let sel = usize::from(cfg.dock == "right");
                    segmented(
                        &mut c,
                        th,
                        &mut hits,
                        right,
                        cy,
                        &[
                            ("Widgets button", Hit::Dock("left")),
                            ("Tray icons", Hit::Dock("right")),
                        ],
                        sel,
                    );
                }
                2 => toggle(&mut c, th, &mut hits, right, cy, startup),
                _ => {
                    let opts = [1000u32, 3000, 5000, 10000];
                    let sel = opts
                        .iter()
                        .position(|v| *v == cfg.refresh_ms)
                        .unwrap_or(usize::MAX);
                    segmented(
                        &mut c,
                        th,
                        &mut hits,
                        right,
                        cy,
                        &[
                            ("1 s", Hit::Refresh(1000)),
                            ("3 s", Hit::Refresh(3000)),
                            ("5 s", Hit::Refresh(5000)),
                            ("10 s", Hit::Refresh(10000)),
                        ],
                        sel,
                    );
                }
            }
        }

        // ---- Prices
        y += 288.0 + 16.0;
        let n = cfg.prices.len() as f32;
        let ph = 88.0 + n * 38.0 + 52.0;
        c.card(x, y, w, ph, th.card, th.card_border, 8.0);
        c.text(x + 16.0, y + 14.0, "Prices", 12.0, th.text1, SB, "la");
        c.text(
            x + w - 16.0,
            y + 15.0,
            "USD per million tokens",
            11.0,
            th.text3,
            R,
            "ra",
        );
        c.text(
            x + 16.0,
            y + 36.0,
            "Used for the $ estimates. Matched against the model name.",
            10.0,
            th.text3,
            R,
            "la",
        );
        let (fw, fg) = (82.0, 10.0);
        let col = |j: usize| x + w - 16.0 - fw - (3 - j) as f32 * (fw + fg);
        let hy = y + 64.0;
        c.text(x + 16.0, hy, "Model", 10.0, th.text3, SB, "la");
        for (j, name) in ["Input", "Output", "Cache write", "Cache read"]
            .iter()
            .enumerate()
        {
            c.text(col(j) + fw - 8.0, hy, name, 10.0, th.text3, SB, "ra");
        }
        for (i, (model, p)) in cfg.prices.iter().enumerate() {
            let ry = y + 86.0 + i as f32 * 38.0;
            c.text(x + 16.0, ry + 15.0, model, 12.0, th.text1, R, "lm");
            #[allow(clippy::needless_range_loop)]
            // j picks the column, the value and the hit target
            for j in 0..4 {
                let fx = col(j);
                let focused = s
                    .focus
                    .as_ref()
                    .is_some_and(|(m, jj)| m == model && *jj == j);
                c.rect(fx, ry, fw, 30.0, field_bg, 4.0);
                let border = if focused { th.accent } else { th.card_border };
                c.card(fx, ry, fw, 30.0, [0, 0, 0, 0], border, 4.0);
                if focused {
                    c.rect(fx + 1.0, ry + 28.0, fw - 2.0, 2.0, th.accent, 1.0);
                }
                let text = if focused {
                    s.buf.clone()
                } else {
                    fmt_price(p[j])
                };
                if focused && s.fresh && !text.is_empty() {
                    let tw = c.tw(&text, 12.0, R);
                    let sel = [th.accent[0], th.accent[1], th.accent[2], 90];
                    c.rect(fx + fw - 12.0 - tw, ry + 6.0, tw + 4.0, 18.0, sel, 2.0);
                }
                c.text(fx + fw - 10.0, ry + 15.0, &text, 12.0, th.text1, R, "rm");
                if focused && s.caret && !s.fresh {
                    c.line(
                        fx + fw - 9.0,
                        ry + 8.0,
                        fx + fw - 9.0,
                        ry + 22.0,
                        th.accent,
                        1.0,
                    );
                }
                hits.push(([fx, ry, fw, 30.0], Hit::Price(model.clone(), j)));
            }
        }
        let by = y + ph - 46.0;
        let label = "Reset to defaults";
        let bw = c.tw(label, 11.0, R) + 28.0;
        let bx = x + w - 16.0 - bw;
        c.card(bx, by, bw, 30.0, field_bg, th.card_border, 4.0);
        c.text(bx + bw / 2.0, by + 15.0, label, 11.0, th.text1, R, "mm");
        hits.push(([bx, by, bw, 30.0], Hit::Reset));
        c.text(
            x + 16.0,
            by + 15.0,
            "Tab: next field · Enter: save · Esc: cancel",
            10.0,
            th.text3,
            R,
            "lm",
        );

        // ---- links
        let ly = y + ph + 20.0;
        let mut lx = x + 4.0;
        for (label, hit) in [
            ("Open config file", Hit::OpenConfig),
            ("Open transcripts folder", Hit::OpenTranscripts),
        ] {
            let tw = c.tw(label, 11.0, R);
            c.text(lx, ly, label, 11.0, th.accent, R, "la");
            hits.push(([lx - 4.0, ly - 4.0, tw + 8.0, 22.0], hit));
            lx += tw + 24.0;
        }

        s.hits = hits;
        // blit
        let (pw, phh) = (c.pm.width() as i32, c.pm.height() as i32);
        let bgra = c.to_bgra(0);
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: pw,
                    biHeight: -phh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            SetDIBitsToDevice(
                hdc,
                0,
                0,
                pw as u32,
                phh as u32,
                0,
                0,
                0,
                phh as u32,
                bgra.as_ptr() as *const _,
                &bmi,
                DIB_RGB_COLORS,
            );
            let _ = EndPaint(hwnd, &ps);
        }
    }

    // ------------------------------------------------------------- input --
    fn settings_hit(&self, px: i32, py: i32) -> Option<Hit> {
        let s = self.settings.as_ref()?;
        let (x, y) = (px as f32 / s.k, py as f32 / s.k);
        s.hits
            .iter()
            .find(|(r, _)| x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3])
            .map(|(_, h)| h.clone())
    }

    fn commit_price(&mut self) {
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        if let Some((model, j)) = s.focus.take() {
            if let Ok(v) = s.buf.trim().parse::<f64>() {
                if v.is_finite() && v >= 0.0 {
                    if let Some(p) = self.cfg.cfg.prices.get_mut(&model) {
                        p[j] = v;
                        self.cfg.save();
                    }
                }
            }
            s.buf.clear();
            unsafe {
                let _ = KillTimer(s.hwnd, T_CARET);
            }
        }
    }

    fn focus_price(&mut self, model: String, j: usize) {
        let value = self
            .cfg
            .cfg
            .prices
            .get(&model)
            .map(|p| fmt_price(p[j]))
            .unwrap_or_default();
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        s.focus = Some((model, j));
        s.buf = value;
        s.fresh = true;
        s.caret = true;
        unsafe { SetTimer(s.hwnd, T_CARET, 530, None) };
    }

    fn settings_click(&mut self, px: i32, py: i32) {
        let hit = self.settings_hit(px, py);
        if !matches!(hit, Some(Hit::Price(..))) {
            self.commit_price();
        }
        match hit {
            Some(Hit::Theme(t)) => self.set_theme(t),
            Some(Hit::Dock(d)) => self.set_dock(d),
            Some(Hit::Startup) => sys::set_startup(!sys::startup_enabled()),
            Some(Hit::Refresh(ms)) => self.set_refresh(ms),
            Some(Hit::Price(m, j)) => {
                self.commit_price();
                self.focus_price(m, j);
            }
            Some(Hit::Reset) => {
                self.cfg.cfg.prices = crate::config::Config::default().prices;
                self.cfg.save();
            }
            Some(Hit::OpenConfig) => {
                let p = format!("\"{}\"", config::config_path().display());
                sys::shell_open("notepad.exe", Some(&p));
            }
            Some(Hit::OpenTranscripts) => {
                sys::shell_open(&config::projects_dir().display().to_string(), None)
            }
            None => {}
        }
        self.settings_invalidate();
    }

    fn settings_char(&mut self, ch: char) {
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        if s.focus.is_none() {
            return;
        }
        // Tab/Enter/Esc also arrive here as characters (TranslateMessage): ignore them
        if !matches!(ch, '0'..='9' | '.' | ',' | '\u{8}') {
            return;
        }
        if s.fresh && ch != '\u{8}' {
            s.buf.clear();
        }
        s.fresh = false;
        match ch {
            '0'..='9' if s.buf.len() < 10 => s.buf.push(ch),
            '.' | ',' if !s.buf.contains('.') => s.buf.push('.'),
            '\u{8}' => {
                s.buf.pop();
            }
            _ => return,
        }
        s.caret = true;
        self.settings_invalidate();
    }

    /// Returns true if the key was handled.
    fn settings_key(&mut self, vk: u16) -> bool {
        let focus = self.settings.as_ref().and_then(|s| s.focus.clone());
        match (vk, focus) {
            (k, Some((model, j))) if k == VK_TAB.0 => {
                self.commit_price();
                let back = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
                let models: Vec<String> = self.cfg.cfg.prices.keys().cloned().collect();
                let mi = models.iter().position(|m| *m == model).unwrap_or(0);
                let idx = (mi * 4 + j) as isize + if back { -1 } else { 1 };
                let total = (models.len() * 4) as isize;
                let idx = idx.rem_euclid(total.max(1)) as usize;
                self.focus_price(models[idx / 4].clone(), idx % 4);
            }
            (k, Some(_)) if k == VK_RETURN.0 => self.commit_price(),
            (k, Some(_)) if k == VK_ESCAPE.0 => {
                if let Some(s) = self.settings.as_mut() {
                    s.focus = None;
                    s.buf.clear();
                }
            }
            (k, None) if k == VK_ESCAPE.0 => unsafe {
                if let Some(s) = &self.settings {
                    let _ = PostMessageW(s.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            },
            _ => return false,
        }
        self.settings_invalidate();
        true
    }
}

// ------------------------------------------------------------ controls -----
fn segmented(
    c: &mut Canvas,
    th: &Theme,
    hits: &mut Vec<([f32; 4], Hit)>,
    right: f32,
    cy: f32,
    items: &[(&str, Hit)],
    sel: usize,
) {
    let widths: Vec<f32> = items
        .iter()
        .map(|(t, _)| c.tw(t, 11.0, SB) + 26.0)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let (x0, h) = (right - total, 30.0);
    c.card(x0, cy - h / 2.0, total, h, th.card, th.card_border, 6.0);
    let mut x = x0 + 2.0;
    for (i, ((label, hit), w)) in items.iter().zip(widths).enumerate() {
        if i == sel {
            c.rect(x, cy - h / 2.0 + 2.0, w, h - 4.0, th.accent, 5.0);
            c.text(x + w / 2.0, cy, label, 11.0, [255, 255, 255, 255], SB, "mm");
        } else {
            c.text(x + w / 2.0, cy, label, 11.0, th.text2, R, "mm");
        }
        hits.push(([x, cy - h / 2.0, w, h], hit.clone()));
        x += w;
    }
}

fn toggle(
    c: &mut Canvas,
    th: &Theme,
    hits: &mut Vec<([f32; 4], Hit)>,
    right: f32,
    cy: f32,
    on: bool,
) {
    let (w, h) = (44.0, 22.0);
    let x = right - w;
    if on {
        c.rect(x, cy - h / 2.0, w, h, th.accent, h / 2.0);
        c.circle(x + w - h / 2.0, cy, 7.0, [255, 255, 255, 255]);
    } else {
        c.card(x, cy - h / 2.0, w, h, [0, 0, 0, 0], th.text3, h / 2.0);
        c.circle(x + h / 2.0, cy, 6.0, th.text2);
    }
    c.text(
        x - 10.0,
        cy,
        if on { "On" } else { "Off" },
        11.0,
        th.text2,
        R,
        "rm",
    );
    hits.push(([x - 40.0, cy - h / 2.0, w + 40.0, h], Hit::Startup));
}

// --------------------------------------------------------- window proc -----
unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let xy = || {
        (
            (lp.0 & 0xffff) as i16 as i32,
            ((lp.0 >> 16) & 0xffff) as i16 as i32,
        )
    };
    match msg {
        WM_PAINT => {
            if with_app(|a| a.paint_settings(hwnd)).is_none() {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_LBUTTONDOWN => {
            let (x, y) = xy();
            with_app(|a| a.settings_click(x, y));
            LRESULT(0)
        }
        WM_SETCURSOR if (lp.0 & 0xffff) as u32 == HTCLIENT => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = ScreenToClient(hwnd, &mut pt);
            let hand = with_app(|a| a.settings_hit(pt.x, pt.y).is_some()).unwrap_or(false);
            let cur =
                LoadCursorW(None, if hand { IDC_HAND } else { IDC_ARROW }).unwrap_or_default();
            SetCursor(cur);
            LRESULT(1)
        }
        WM_CHAR => {
            if let Some(ch) = char::from_u32(wp.0 as u32) {
                with_app(|a| a.settings_char(ch));
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if with_app(|a| a.settings_key(wp.0 as u16)).unwrap_or(false) {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_TIMER if wp.0 == T_CARET => {
            with_app(|a| {
                if let Some(s) = a.settings.as_mut() {
                    s.caret = !s.caret;
                }
                a.settings_invalidate();
            });
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let r = &*(lp.0 as *const RECT);
            with_app(|a| {
                if let Some(s) = a.settings.as_mut() {
                    s.k = (wp.0 & 0xffff) as f32 / 96.0;
                    s.size = (0, 0);
                }
            });
            let _ = SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            with_app(|a| a.fit_settings_window(false));
            LRESULT(0)
        }
        WM_CLOSE => {
            with_app(|a| a.commit_price());
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            with_app(|a| a.settings = None);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
