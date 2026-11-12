//! Platform-neutral drawing: themes, the taskbar/menu-bar readout, the drawer
//! and its sections. Every frontend (Windows, macOS, Linux) renders through
//! these functions, and `--screenshots` uses them for the README images.

use crate::config;
use crate::draw::{hex, Canvas, Fonts, Rgba, Weight};
use crate::usage::{fmt_ago, fmt_left, fmt_money, fmt_tokens, pretty_project, Stats, BLOCK_HOURS};
use chrono::{Duration as CDuration, Local};
use tiny_skia::Pixmap;

use Weight::{Regular as R, Semibold as SB};

pub(crate) mod settings;
mod shots;

// ------------------------------------------------------------------ theme --
pub(crate) struct Theme {
    pub(crate) panel: Rgba,
    pub(crate) panel_border: Rgba,
    pub(crate) card: Rgba,
    pub(crate) card_border: Rgba,
    pub(crate) grid: Rgba,
    pub(crate) track: Rgba,
    pub(crate) hover: Rgba,
    pub(crate) active: Rgba,
    pub(crate) text1: Rgba,
    pub(crate) text2: Rgba,
    pub(crate) text3: Rgba,
    pub(crate) accent: Rgba,
    pub(crate) good: Rgba,
    pub(crate) cat: [Rgba; 4],
}

pub(crate) const DARK: Theme = Theme {
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

pub(crate) const LIGHT: Theme = Theme {
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

/// Drawer theme: "dark" / "light" force it; anything else follows the
/// system's light/dark setting.
pub(crate) fn resolve_theme(setting: &str) -> &'static Theme {
    match setting {
        "dark" => &DARK,
        "light" => &LIGHT,
        _ if crate::platform::system_dark() => &DARK,
        _ => &LIGHT,
    }
}

pub(crate) const BAR_H: f32 = 48.0; // logical taskbar height
pub(crate) const PANEL_W: f32 = 760.0;
pub(crate) const SHADOW_M: f32 = 24.0;

/// Square status icon for the macOS menu bar / Linux tray: the 5-hour ring
/// with the spark inside, plus (optionally) the last hour's sparkline.
#[cfg_attr(windows, allow(dead_code))] // menu bar / tray only
pub(crate) fn draw_status_icon<'a>(
    fonts: &'a Fonts,
    th: &Theme,
    s: &Stats,
    k: f32,
    size: f32,
    sparkline: bool,
) -> Canvas<'a> {
    let spark_w = if sparkline { size * 1.6 } else { 0.0 };
    let w = size
        + if sparkline {
            size * 0.25 + spark_w
        } else {
            0.0
        };
    let mut c = Canvas::new(w, size, k, fonts);
    let (cx, cy, r) = (size / 2.0, size / 2.0, size / 2.0 - 0.5);
    c.ring(cx, cy, r, size * 0.13, s.block_frac, th.accent, th.track);
    let live = if s.live() { th.accent } else { th.text2 };
    c.spark(cx, cy, size * 0.24, live);
    if sparkline {
        let sx = size * 1.25;
        let n = s.spark.len() as f32;
        let bw = (spark_w - (n - 1.0) * 0.5) / n;
        let peak = (*s.spark.iter().max().unwrap_or(&0)).max(1) as f32;
        let base = size * 0.88;
        let sh = size * 0.76;
        c.line(sx, base + 0.25, sx + spark_w, base + 0.25, th.track, 0.5);
        for (i, v) in s.spark.iter().enumerate() {
            if *v > 0 {
                let bh = (sh * *v as f32 / peak).max(1.5);
                c.bar_up(sx + i as f32 * (bw + 0.5), base, bw, bh, th.accent, 0.6);
            }
        }
    }
    c
}

pub(crate) fn draw_readout<'a>(
    fonts: &'a Fonts,
    th: &Theme,
    s: &Stats,
    k: f32,
    hover: bool,
    open: bool,
) -> Canvas<'a> {
    let line1 = fmt_tokens(if s.block_start.is_some() {
        s.block.all
    } else {
        s.today.all
    } as f64);
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

pub(crate) fn panel_layout(s: &Stats) -> (f32, usize, usize, [f32; 3], [f32; 4]) {
    let nm = s.models.len().clamp(1, 3);
    let ns = s.sessions.len().clamp(1, 4);
    let left = [160.0, 156.0, 2.0 * 82.0 + 12.0];
    let right = [
        164.0,
        124.0,
        46.0 + 30.0 * nm as f32,
        46.0 + 38.0 * ns as f32,
    ];
    let gap = 14.0;
    let lh: f32 = left.iter().sum::<f32>() + gap * 2.0;
    let rh: f32 = right.iter().sum::<f32>() + gap * 3.0;
    let body = lh.max(rh).max(settings::SETTINGS_BODY_H);
    ((72.0 + body + 44.0).floor(), nm, ns, left, right)
}

/// The whole panel window image: shadow margin + glass panel + sections.
/// `settings` = Some((config, start-with-Windows)) draws the Settings view.
/// Also returns the clickable regions.
pub(crate) fn draw_panel<'a>(
    fonts: &'a Fonts,
    th: &Theme,
    s: &Stats,
    k: f32,
    backdrop: Option<&Pixmap>,
    settings: Option<(&config::Config, bool)>,
) -> (Canvas<'a>, settings::Hits) {
    let (h, nm, ns, left_h, right_h) = panel_layout(s);
    let (w, pad, gap) = (PANEL_W, 20.0, 14.0);
    let cw = (w - 2.0 * pad - gap) / 2.0;

    let mut c = Canvas::new(w, h, k, fonts);
    match backdrop {
        Some(bd) => c.backdrop(bd, 12.0),
        None => c.rect(
            0.0,
            0.0,
            w,
            h,
            [th.panel[0], th.panel[1], th.panel[2], 255],
            12.0,
        ),
    }
    c.card(0.0, 0.0, w, h, th.panel, th.panel_border, 12.0);
    let mut hits = settings::Hits::new();
    header(&mut c, th, s, pad, w, settings.is_some(), &mut hits);
    let top = 72.0;
    if let Some((cfg, startup)) = settings {
        settings::draw_settings(&mut c, th, cfg, startup, &mut hits, pad, top, w - 2.0 * pad);
        c.text(
            pad,
            h - 22.0,
            "Changes are saved automatically  ·  Esc to go back",
            10.0,
            th.text3,
            R,
            "la",
        );
        return (frame(fonts, &c, w, h, k), hits);
    }
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
        "$ figures are API-list-price estimates, not your plan's billing  ·  Esc to close",
        10.0,
        th.text3,
        R,
        "la",
    );
    (frame(fonts, &c, w, h, k), hits)
}

/// Add the drop-shadow margin around the drawn panel.
pub(crate) fn frame<'a>(fonts: &'a Fonts, c: &Canvas, w: f32, h: f32, k: f32) -> Canvas<'a> {
    let m = SHADOW_M;
    let mut full = Canvas::new(w + 2.0 * m, h + 2.0 * m, k, fonts);
    for i in 0..16 {
        let i = i as f32;
        full.rect(
            m - i,
            m - i + 8.0,
            w + 2.0 * i,
            h + 2.0 * i,
            [0, 0, 0, 5],
            12.0 + i,
        );
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
        (
            "dark",
            &DARK,
            [32u8, 32, 34, 255],
            [(58u8, 36u8, 92u8), (20, 70, 110)],
        ),
        (
            "light",
            &LIGHT,
            [236, 236, 238, 255],
            [(196, 214, 240), (238, 214, 232)],
        ),
    ] {
        // readout on a taskbar-coloured strip
        let ro = draw_readout(&fonts, th, &s, k, false, false);
        let strip_w = ro.pm.width() as f32 / k + 24.0;
        let mut strip = Canvas::new(strip_w, BAR_H, k, &fonts);
        strip.rect(0.0, 0.0, strip_w, BAR_H, bar_bg, 0.0);
        strip.draw_at(&ro.pm, (12.0 * k) as i32, 0);
        strip
            .pm
            .save_png(dir.join(format!("readout-{name}.png")))
            .map_err(std::io::Error::other)?;

        // panel over a soft "wallpaper" so the glass has something to blur
        let (bw, bh) = (96usize, 80usize);
        let mut wallpaper = vec![0u8; bw * bh * 4];
        for y in 0..bh {
            for x in 0..bw {
                let t = (x + y) as f32 / (bw + bh) as f32;
                let blob =
                    (-(((x as f32 - 70.0).powi(2) + (y as f32 - 20.0).powi(2)) / 300.0)).exp();
                let mix = |a: u8, b: u8| a as f32 * (1.0 - t) + b as f32 * t;
                let i = (y * bw + x) * 4;
                wallpaper[i + 2] = (mix(wall[0].0, wall[1].0) + 120.0 * blob).min(255.0) as u8;
                wallpaper[i + 1] = (mix(wall[0].1, wall[1].1) + 60.0 * blob).min(255.0) as u8;
                wallpaper[i] = mix(wall[0].2, wall[1].2) as u8;
                wallpaper[i + 3] = 255;
            }
        }
        let backdrop = crate::draw::blurred_backdrop(&wallpaper, bw, bh, 1);
        let (panel, _) = draw_panel(&fonts, th, &s, k, backdrop.as_ref(), None);
        panel
            .pm
            .save_png(dir.join(format!("panel-{name}.png")))
            .map_err(std::io::Error::other)?;
    }
    shots::render_scenes(dir, &fonts, &s)
}

// --------------------------------------------------------- panel sections --
fn card(
    c: &mut Canvas,
    th: &Theme,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    title: &str,
    sub: Option<&str>,
) {
    c.card(x, y, w, h, th.card, th.card_border, 8.0);
    c.text(x + 16.0, y + 15.0, title, 12.0, th.text1, SB, "la");
    if let Some(sub) = sub {
        c.text(x + w - 16.0, y + 16.0, sub, 11.0, th.text3, R, "ra");
    }
}

fn header(
    c: &mut Canvas,
    th: &Theme,
    s: &Stats,
    pad: f32,
    w: f32,
    in_settings: bool,
    hits: &mut settings::Hits,
) {
    c.spark(pad + 14.0, 36.0, 11.0, th.accent);
    let gear_x = w - pad - 32.0;
    settings::gear_button(c, th, hits, gear_x, 19.0, in_settings);
    if in_settings {
        c.text(pad + 36.0, 19.0, "Settings", 17.0, th.text1, SB, "la");
        let sub = "Claude Code usage widget";
        c.text(pad + 36.0, 44.0, sub, 11.0, th.text3, R, "la");
        return;
    }
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
    let cx = gear_x - 10.0 - tw - 34.0;
    c.card(cx, 22.0, tw + 34.0, 26.0, th.card, th.card_border, 13.0);
    c.circle(cx + 15.0, 35.0, 4.0, if live { th.good } else { th.text3 });
    c.text(cx + 25.0, 35.0, label, 11.0, th.text1, SB, "lm");
}

fn hero(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32) {
    c.card(x, y, w, h, th.card, th.card_border, 8.0);
    let (cx, cy) = (x + 76.0, y + h / 2.0);
    c.ring(cx, cy, 56.0, 9.0, s.block_frac, th.accent, th.track);
    if s.block_start.is_some() {
        c.text(
            cx,
            cy - 2.0,
            &fmt_left(s.block_left),
            20.0,
            th.text1,
            SB,
            "ms",
        );
        c.text(cx, cy + 4.0, "until reset", 10.0, th.text3, R, "ma");
    } else {
        c.text(cx, cy, "idle", 18.0, th.text3, SB, "mm");
    }
    let tx = x + 152.0;
    let b = &s.block;
    c.text(tx, y + 22.0, "5-HOUR WINDOW", 10.0, th.text3, SB, "la");
    let big = if s.block_start.is_some() {
        fmt_tokens(b.all as f64)
    } else {
        "—".into()
    };
    c.text(tx, y + 38.0, &big, 30.0, th.text1, SB, "la");
    c.text(
        tx,
        y + 80.0,
        &format!("tokens  ·  {} est.", fmt_money(b.cost)),
        12.0,
        th.text2,
        R,
        "la",
    );
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
    let sub = s
        .block_start
        .map(|_| format!("{} out", fmt_tokens(total as f64)));
    card(
        c,
        th,
        x,
        y,
        w,
        h,
        "Output tokens this window",
        sub.as_deref(),
    );
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
            c.bar_up(
                bx,
                base,
                bw,
                ((base - top) * *v as f32 / peak).max(2.0),
                th.accent,
                3.0,
            );
        }
    }
    match s.block_start {
        Some(st) => {
            let maxv = *s.wbins.iter().max().unwrap_or(&0);
            if maxv > 0 {
                let i = s.wbins.iter().position(|v| *v == maxv).unwrap_or(0);
                let bx = (px0 + i as f32 * (bw + g) + bw / 2.0).clamp(px0 + 20.0, px1 - 20.0);
                c.text(
                    bx,
                    top - 6.0,
                    &fmt_tokens(maxv as f64),
                    10.0,
                    th.text2,
                    R,
                    "ms",
                );
            }
            let st = st.with_timezone(&Local);
            let mid = st + CDuration::minutes(BLOCK_HOURS * 30);
            let en = st + CDuration::hours(BLOCK_HOURS);
            c.text(
                px0,
                base + 8.0,
                &st.format("%H:%M").to_string(),
                10.0,
                th.text3,
                R,
                "la",
            );
            c.text(
                (px0 + px1) / 2.0,
                base + 8.0,
                &mid.format("%H:%M").to_string(),
                10.0,
                th.text3,
                R,
                "ma",
            );
            c.text(
                px1,
                base + 8.0,
                &en.format("%H:%M").to_string(),
                10.0,
                th.text3,
                R,
                "ra",
            );
        }
        None => c.text(
            (px0 + px1) / 2.0,
            (top + base) / 2.0,
            "No active window",
            11.0,
            th.text3,
            R,
            "mm",
        ),
    }
}

fn tiles(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32) {
    let (tw, tht, g) = ((w - 12.0) / 2.0, 82.0, 12.0);
    let t = &s.today;
    let denom = (t.cr + t.cw + t.inp) as f64;
    let hit = if denom > 0.0 {
        t.cr as f64 / denom
    } else {
        0.0
    };
    let items: [(&str, String, String, Option<f64>); 4] = [
        (
            "Burn rate",
            format!("{}/min", fmt_tokens(s.burn)),
            "output · last 10 min".into(),
            None,
        ),
        (
            "Last hour",
            fmt_tokens(s.hour.all as f64),
            format!("{} est.", fmt_money(s.hour.cost)),
            None,
        ),
        (
            "Today",
            fmt_tokens(t.all as f64),
            format!("{} est. · {} msgs", fmt_money(t.cost), t.n),
            None,
        ),
        (
            "Cache hit",
            format!("{:.0}%", hit * 100.0),
            "of input read from cache".into(),
            Some(hit),
        ),
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
            c.rect(
                bx + tw - 64.0,
                by + 40.0,
                (50.0 * *f as f32).max(5.0),
                5.0,
                th.accent,
                2.5,
            );
        }
    }
}

fn week(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32) {
    let total: f64 = s.days.iter().map(|d| d.cost).sum();
    card(
        c,
        th,
        x,
        y,
        w,
        h,
        "Last 7 days",
        Some(&format!("{} est.", fmt_money(total))),
    );
    let (px0, px1, base, top) = (x + 16.0, x + w - 16.0, y + h - 30.0, y + 58.0);
    let n = s.days.len();
    let slot = (px1 - px0) / n as f32;
    let bw = (slot - 12.0).min(26.0);
    let peak = s.days.iter().map(|d| d.cost).fold(0.0, f64::max).max(1e-9);
    let ipeak = s
        .days
        .iter()
        .enumerate()
        .max_by(|a, b| {
            a.1.cost
                .partial_cmp(&b.1.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map_or(0, |(i, _)| i);
    c.line(px0, base + 0.5, px1, base + 0.5, th.grid, 1.0);
    for (i, d) in s.days.iter().enumerate() {
        let cx = px0 + slot * (i as f32 + 0.5);
        if d.cost > 0.0 {
            let hh = ((base - top) * (d.cost / peak) as f32).max(2.0);
            c.bar_up(cx - bw / 2.0, base, bw, hh, th.accent, 4.0);
            if i == ipeak || i == n - 1 {
                c.text(
                    cx,
                    base - hh - 5.0,
                    &fmt_money(d.cost),
                    10.0,
                    th.text2,
                    R,
                    "ms",
                );
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
    card(
        c,
        th,
        x,
        y,
        w,
        h,
        "Today's token mix",
        Some(&format!("{} total", fmt_tokens(t.all as f64))),
    );
    let parts = [
        ("Cache read", t.cr),
        ("Cache write", t.cw),
        ("Input", t.inp),
        ("Output", t.out),
    ];
    let (bx0, bx1, by) = (x + 16.0, x + w - 16.0, y + 46.0);
    let total: u64 = parts.iter().map(|p| p.1).sum();
    if total > 0 {
        let nz: Vec<(usize, u64)> = parts
            .iter()
            .enumerate()
            .filter(|p| p.1 .1 > 0)
            .map(|(i, p)| (i, p.1))
            .collect();
        let avail = bx1 - bx0 - 2.0 * (nz.len() as f32 - 1.0);
        let widths: Vec<f32> = nz
            .iter()
            .map(|(_, v)| (avail * *v as f32 / total as f32).max(4.0))
            .collect();
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
        let pct = if total > 0 {
            format!("  {:.0}%", *v as f64 / total as f64 * 100.0)
        } else {
            String::new()
        };
        c.text(
            lx + colw,
            ly,
            &format!("{}{}", fmt_tokens(*v as f64), pct),
            11.0,
            th.text1,
            SB,
            "ra",
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn models(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32, nm: usize) {
    card(c, th, x, y, w, h, "Models today", None);
    if s.models.is_empty() {
        c.text(
            x + 16.0,
            y + 46.0,
            "No usage yet today",
            11.0,
            th.text3,
            R,
            "la",
        );
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
        c.rect(
            b0,
            ry + 5.0,
            ((b1 - b0) * share as f32).max(6.0),
            6.0,
            th.accent,
            3.0,
        );
        let t = format!("{} · {:.0}%", fmt_money(m.cost), share * 100.0);
        c.text(x + w - 16.0, ry, &t, 11.0, th.text2, R, "ra");
    }
}

#[allow(clippy::too_many_arguments)]
fn sessions(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32, ns: usize) {
    card(c, th, x, y, w, h, "Recent sessions", Some("last 7 days"));
    if s.sessions.is_empty() {
        c.text(
            x + 16.0,
            y + 46.0,
            "No sessions yet",
            11.0,
            th.text3,
            R,
            "la",
        );
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
        c.text(
            x + w - 16.0,
            ry + 6.0,
            &fmt_money(ss.cost),
            12.0,
            th.text2,
            R,
            "ra",
        );
    }
}
