//! The drawer's Settings view: theme, position, startup, refresh rate, and a
//! read-only table of the built-in prices. Every change is saved immediately.
//! The gear button in the drawer's header switches between Usage and Settings.

use super::*;

/// Something clickable in the drawer (logical panel coordinates).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Hit {
    Gear,
    Theme(&'static str),
    Dock(&'static str),
    Startup,
    Refresh(u32),
    OpenTranscripts,
    Quit,
}

pub(super) type Hits = Vec<([f32; 4], Hit)>;

/// Height of the settings body (kept within the usage view's minimum so the
/// drawer doesn't change size when switching views).
pub(super) const SETTINGS_BODY_H: f32 = GENERAL_H + 14.0 + PRICES_H + 14.0 + 24.0;
const ROW_H: f32 = 52.0;
const GENERAL_H: f32 = 40.0 + 4.0 * ROW_H;
/// Models in the price table: the current generation.
const PRICE_ROWS: [&str; 4] = ["fable-5-2", "opus-5-5", "sonnet-5-5", "haiku-5-5"];
const PRICE_ROW_H: f32 = 26.0;
const PRICES_H: f32 = 84.0 + PRICE_ROWS.len() as f32 * PRICE_ROW_H + 32.0;

/// "$4.00", "$0.20", "$0.125"
fn fmt_price(v: f64) -> String {
    if (v * 100.0 - (v * 100.0).round()).abs() < 1e-9 {
        format!("${v:.2}")
    } else {
        format!("${v:.3}")
    }
}

impl App {
    // ------------------------------------------------------------ actions --
    pub(super) fn set_theme(&mut self, t: &str) {
        self.cfg.cfg.theme = t.into();
        self.cfg.save();
        self.theme = resolve_theme(t);
        self.render_readout();
        self.redraw_panel();
    }

    pub(super) fn set_dock(&mut self, d: &str) {
        self.cfg.cfg.dock = d.into();
        self.cfg.save();
        self.locator.invalidate();
        self.render_readout();
        self.redraw_panel();
    }

    pub(super) fn set_refresh(&mut self, ms: u32) {
        self.cfg.cfg.refresh_ms = ms;
        self.cfg.save();
        unsafe {
            let _ = KillTimer(self.readout, T_TICK);
            SetTimer(self.readout, T_TICK, ms.clamp(500, 60_000), None);
        }
        self.redraw_panel();
    }

    /// Open the drawer on its Settings view.
    pub(super) fn open_settings(&mut self) {
        self.show_settings = true;
        if self.panel_open {
            self.redraw_panel();
            unsafe {
                let _ = SetForegroundWindow(self.panel);
            }
        } else {
            self.closed_at = None;
            self.open_panel();
        }
    }

    pub(super) fn set_view(&mut self, settings: bool) {
        if self.show_settings != settings {
            self.show_settings = settings;
            self.redraw_panel();
        }
    }

    // ------------------------------------------------------------- input --
    /// Hit test in panel-window pixels (the window includes the shadow margin).
    pub(super) fn panel_hit(&self, px: i32, py: i32) -> Option<Hit> {
        let k = self.scale();
        let (x, y) = (px as f32 / k - SHADOW_M, py as f32 / k - SHADOW_M);
        self.panel_hits
            .iter()
            .find(|(r, _)| x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3])
            .map(|(_, h)| *h)
    }

    pub(super) fn panel_click(&mut self, px: i32, py: i32) {
        match self.panel_hit(px, py) {
            Some(Hit::Gear) => self.set_view(!self.show_settings),
            Some(Hit::Theme(t)) => self.set_theme(t),
            Some(Hit::Dock(d)) => self.set_dock(d),
            Some(Hit::Startup) => {
                sys::set_startup(!sys::startup_enabled());
                self.redraw_panel();
            }
            Some(Hit::Refresh(ms)) => self.set_refresh(ms),
            Some(Hit::OpenTranscripts) => {
                sys::shell_open(&config::projects_dir().display().to_string(), None)
            }
            Some(Hit::Quit) => self.handle_menu(M_QUIT),
            None => {}
        }
    }
}

// ----------------------------------------------------------------- drawing --
/// Gear button in the drawer header, top right. `on` = Settings view showing.
pub(super) fn gear_button(c: &mut Canvas, th: &Theme, hits: &mut Hits, x: f32, y: f32, on: bool) {
    let s = 32.0;
    let ink = if on { [255, 255, 255, 255] } else { th.text2 };
    if on {
        c.rect(x, y, s, s, th.accent, 8.0);
    } else {
        c.card(x, y, s, s, th.card, th.card_border, 8.0);
    }
    let (cx, cy) = (x + s / 2.0, y + s / 2.0);
    for i in 0..8 {
        let a = (i as f32 * 45.0).to_radians();
        let (dx, dy) = (a.cos(), a.sin());
        c.line(
            cx + dx * 5.5,
            cy + dy * 5.5,
            cx + dx * 8.6,
            cy + dy * 8.6,
            ink,
            3.0,
        );
    }
    c.ring(cx, cy, 7.0, 3.4, 1.0, ink, ink);
    hits.push(([x, y, s, s], Hit::Gear));
}

/// The Settings view body, drawn inside the drawer at (x, y) with width w.
pub(super) fn draw_settings(
    c: &mut Canvas,
    th: &Theme,
    cfg: &crate::config::Config,
    startup: bool,
    hits: &mut Hits,
    x: f32,
    y: f32,
    w: f32,
) {
    // ---- General
    c.card(x, y, w, GENERAL_H, th.card, th.card_border, 8.0);
    c.text(x + 16.0, y + 15.0, "General", 12.0, th.text1, SB, "la");
    let rows: [(&str, &str); 4] = [
        ("Theme", "Drawer colours. System follows Windows' app mode."),
        ("Position", "Where the readout sits on the taskbar"),
        (
            "Start with Windows",
            "Launch automatically when you sign in",
        ),
        ("Refresh every", "How often transcripts are rescanned"),
    ];
    for (i, (label, desc)) in rows.iter().enumerate() {
        let ry = y + 40.0 + i as f32 * ROW_H;
        if i > 0 {
            c.line(x + 16.0, ry, x + w - 16.0, ry, th.grid, 1.0);
        }
        c.text(x + 16.0, ry + 10.0, label, 12.0, th.text1, R, "la");
        c.text(x + 16.0, ry + 29.0, desc, 10.0, th.text3, R, "la");
        let (right, cy) = (x + w - 16.0, ry + ROW_H / 2.0);
        match i {
            0 => {
                let sel = match cfg.theme.as_str() {
                    "dark" => 1,
                    "light" => 2,
                    _ => 0,
                };
                segmented(
                    c,
                    th,
                    hits,
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
                    c,
                    th,
                    hits,
                    right,
                    cy,
                    &[
                        ("Widgets button", Hit::Dock("left")),
                        ("Tray icons", Hit::Dock("right")),
                    ],
                    sel,
                );
            }
            2 => toggle(c, th, hits, right, cy, startup),
            _ => {
                let opts = [1000u32, 3000, 5000, 10000];
                let sel = opts
                    .iter()
                    .position(|v| *v == cfg.refresh_ms)
                    .unwrap_or(usize::MAX);
                segmented(
                    c,
                    th,
                    hits,
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

    // ---- Prices (read-only)
    let y = y + GENERAL_H + 14.0;
    c.card(x, y, w, PRICES_H, th.card, th.card_border, 8.0);
    c.text(x + 16.0, y + 15.0, "Prices", 12.0, th.text1, SB, "la");
    c.text(
        x + w - 16.0,
        y + 16.0,
        "USD per million tokens",
        11.0,
        th.text3,
        R,
        "ra",
    );
    c.text(
        x + 16.0,
        y + 36.0,
        "Anthropic's published API prices, built in and used for the $ estimates.",
        10.0,
        th.text3,
        R,
        "la",
    );
    let cols = [
        "Input",
        "Output",
        "Cache write 5m",
        "Cache write 1h",
        "Cache read",
    ];
    let col_w = 104.0;
    let col_r = |j: usize| x + w - 16.0 - (cols.len() - 1 - j) as f32 * col_w;
    let hy = y + 60.0;
    c.text(x + 16.0, hy, "Model", 10.0, th.text3, SB, "la");
    for (j, name) in cols.iter().enumerate() {
        c.text(col_r(j), hy, name, 10.0, th.text3, SB, "ra");
    }
    for (i, key) in PRICE_ROWS.iter().enumerate() {
        let Some((_, name, p)) = crate::pricing::TABLE.iter().find(|(k, ..)| k == key) else {
            continue;
        };
        let ry = y + 84.0 + i as f32 * PRICE_ROW_H;
        c.line(x + 16.0, ry - 4.0, x + w - 16.0, ry - 4.0, th.grid, 1.0);
        c.text(x + 16.0, ry + 9.0, name, 12.0, th.text1, R, "lm");
        for (j, v) in [p.input, p.output, p.write_5m, p.write_1h, p.read]
            .iter()
            .enumerate()
        {
            c.text(col_r(j), ry + 9.0, &fmt_price(*v), 12.0, th.text2, R, "rm");
        }
    }
    c.text(
        x + 16.0,
        y + PRICES_H - 18.0,
        "Older models use their own published prices. Claude Code writes 1-hour cache entries.",
        10.0,
        th.text3,
        R,
        "lm",
    );

    // ---- links
    let ly = y + PRICES_H + 14.0 + 12.0;
    let label = "Open transcripts folder";
    let tw = c.tw(label, 11.0, R);
    c.text(x + 4.0, ly, label, 11.0, th.accent, R, "lm");
    hits.push(([x, ly - 11.0, tw + 8.0, 22.0], Hit::OpenTranscripts));
    let label = "Quit widget";
    let tw = c.tw(label, 11.0, R);
    c.text(x + w - 4.0, ly, label, 11.0, th.text2, R, "rm");
    hits.push(([x + w - 8.0 - tw, ly - 11.0, tw + 8.0, 22.0], Hit::Quit));
}

// ------------------------------------------------------------ controls -----
fn segmented(
    c: &mut Canvas,
    th: &Theme,
    hits: &mut Hits,
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
        hits.push(([x, cy - h / 2.0, w, h], *hit));
        x += w;
    }
}

fn toggle(c: &mut Canvas, th: &Theme, hits: &mut Hits, right: f32, cy: f32, on: bool) {
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
