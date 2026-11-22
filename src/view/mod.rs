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

/// Colour for a service status level (green / blue / amber / orange / red).
pub(crate) fn level_color(th: &Theme, l: crate::status::Level) -> Rgba {
    use crate::status::Level::*;
    match l {
        Operational => th.good,
        Maintenance => th.cat[0],
        Degraded => th.cat[3],
        Partial => th.cat[1],
        Major => hex(0xd93b3b),
    }
}

/// Colour of the weekly-limit ring inside the session ring: a softer accent,
/// so it reads as related to but distinct from the session ring.
fn weekly_color(th: &Theme) -> Rgba {
    [th.accent[0], th.accent[1], th.accent[2], 150]
}

/// The worst current status level, if something is wrong (for the small
/// warning dot on the taskbar readout / status icon).
fn status_alert(s: &Stats) -> Option<crate::status::Level> {
    let sv = s.service.service.as_ref()?;
    let l = sv.worst();
    (s.service.enabled && l != crate::status::Level::Operational).then_some(l)
}

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
    if let Some(l) = status_alert(s) {
        c.circle(size * 0.84, size * 0.16, size * 0.16, level_color(th, l));
    }
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
    let weekly = s.weekly_percent();
    let mut line2 = if s.block_start.is_some() {
        format!("{} left", fmt_left(s.block_left))
    } else {
        "idle · today".into()
    };
    if let Some(p) = weekly {
        line2 = format!("{line2} · wk {p:.0}%");
    }
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
    // outer ring: the 5-hour session; inner ring: the weekly limit
    c.ring(cx, cy, ring_d / 2.0, 3.2, s.block_frac, th.accent, th.track);
    let live = if s.live() { th.accent } else { th.text3 };
    match weekly {
        Some(p) => {
            let frac = (p / 100.0).clamp(0.0, 1.0);
            c.ring(cx, cy, 9.0, 2.6, frac, weekly_color(th), th.track);
            c.spark(cx, cy, 4.4, live);
        }
        None => c.spark(cx, cy, 6.2, live),
    }
    if let Some(l) = status_alert(s) {
        // something is wrong on status.claude.com
        c.circle(cx + 10.5, cy - 10.5, 3.6, level_color(th, l));
    }
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

pub(crate) fn panel_layout(s: &Stats) -> (f32, usize, usize, [f32; 4], [f32; 5]) {
    let nm = s.models.len().clamp(1, 3);
    let status_h = status_height(s);
    // one session row fewer when the status card is shown, to keep the
    // drawer about the same height
    let ns = s.sessions.len().clamp(1, if status_h > 0.0 { 3 } else { 4 });
    let left = [160.0, 156.0, 2.0 * 82.0 + 12.0, LIMITS_H];
    let right = [
        164.0,
        124.0,
        46.0 + 30.0 * nm as f32,
        46.0 + 38.0 * ns as f32,
        status_h,
    ];
    let gap = 14.0;
    let lh: f32 = left.iter().sum::<f32>() + gap * 3.0;
    let rgaps = if status_h > 0.0 { 4.0 } else { 3.0 };
    let rh: f32 = right.iter().sum::<f32>() + gap * rgaps;
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
    y += left_h[2] + gap;
    plan_limits(&mut c, th, s, pad, y, cw, left_h[3]);
    let x = pad + cw + gap;
    let mut y = top;
    week(&mut c, th, s, x, y, cw, right_h[0]);
    y += right_h[0] + gap;
    mix(&mut c, th, s, x, y, cw, right_h[1]);
    y += right_h[1] + gap;
    models(&mut c, th, s, x, y, cw, right_h[2], nm);
    y += right_h[2] + gap;
    sessions(&mut c, th, s, x, y, cw, right_h[3], ns);
    if right_h[4] > 0.0 {
        y += right_h[3] + gap;
        service_status(&mut c, th, s, x, y, cw, right_h[4]);
        hits.push(([x, y, cw, right_h[4]], settings::Hit::Tip(crate::status::PAGE)));
    }
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
        let t = match s.session_percent() {
            Some(p) => format!("{p:.0}% of session used  ·  resets {}", en.format("%H:%M")),
            None => format!("{} → resets {}", st.format("%H:%M"), en.format("%H:%M")),
        };
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

const LIMITS_H: f32 = 150.0;

/// "Plan limits": the session and weekly numbers Claude Code's `/usage`
/// shows, fetched from Anthropic (they include the phone, web and other
/// computers, which the transcripts can't see).
fn plan_limits(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32) {
    use crate::limits::Status;
    let snap = &s.limits;
    let stale = matches!(
        snap.status,
        Status::Failed(_) | Status::Expired | Status::NoLogin
    );
    let sub = match &snap.status {
        _ if stale && snap.limits.is_some() => {
            let at = snap.limits.as_ref().map_or(s.now, |l| l.fetched_at);
            format!("last known · {}", fmt_ago((s.now - at).num_seconds()))
        }
        Status::Ok | Status::Loading if snap.limits.is_some() => {
            let at = snap.limits.as_ref().map(|l| l.fetched_at);
            match at {
                Some(t) => format!("from Anthropic · {}", fmt_ago((s.now - t).num_seconds())),
                None => "from Anthropic".into(),
            }
        }
        Status::Loading => "checking…".into(),
        Status::Off => "off".into(),
        Status::NoLogin => "no Claude login".into(),
        Status::Expired => "login expired".into(),
        Status::Failed(e) => format!("couldn't update ({e})"),
        Status::Ok => String::new(),
    };
    card(c, th, x, y, w, h, "Plan limits", Some(&sub));
    let Some(l) = &snap.limits else {
        let msg: &[&str] = match &snap.status {
            Status::Off => &[
                "Turn on \"Plan limits\" in Settings to see the session",
                "and weekly limits Claude Code's /usage shows.",
            ],
            Status::NoLogin => &[
                "Sign in to Claude Code on this computer (Pro or Max)",
                "to see your session and weekly limits here.",
            ],
            Status::Expired => &[
                "Claude Code's saved login has expired and couldn't",
                "be renewed. Sign in to Claude Code again.",
            ],
            Status::Loading => &["Asking Anthropic for your plan limits…"],
            _ => &[
                "Couldn't reach Anthropic. The 5-hour window above",
                "is estimated from this computer's transcripts.",
            ],
        };
        for (i, line) in msg.iter().enumerate() {
            c.text(
                x + 16.0,
                y + 52.0 + i as f32 * 18.0,
                line,
                11.0,
                th.text3,
                R,
                "la",
            );
        }
        return;
    };
    let mut rows: Vec<(String, &crate::limits::Window, &str)> = Vec::new();
    if let Some(w) = &l.session {
        rows.push(("Session".into(), w, "%H:%M"));
    }
    if let Some(w) = &l.weekly {
        rows.push(("Weekly".into(), w, "%a %H:%M"));
    }
    if let Some((name, w)) = l.scoped.first() {
        rows.push((format!("{name} weekly"), w, "%a %H:%M"));
    }
    let (lx, bx, rx) = (x + 16.0, x + 104.0, x + w - 16.0);
    let bw = (rx - 112.0) - bx;
    for (i, (label, win, fmt)) in rows.iter().take(3).enumerate() {
        let ry = y + 50.0 + i as f32 * 26.0;
        c.text(lx, ry, label, 11.0, th.text2, R, "lm");
        let frac = (win.percent / 100.0).clamp(0.0, 1.0) as f32;
        c.rect(bx, ry - 3.0, bw, 6.0, th.track, 3.0);
        if frac > 0.0 {
            c.rect(bx, ry - 3.0, (bw * frac).max(6.0), 6.0, th.accent, 3.0);
        }
        let when = win.resets_at.with_timezone(&Local).format(fmt).to_string();
        c.text(
            rx,
            ry,
            &format!("{:.0}%  ·  {when}", win.percent),
            11.0,
            th.text1,
            R,
            "rm",
        );
    }
    if !l.breakdown.is_empty() {
        let parts: Vec<String> = l
            .breakdown
            .iter()
            .map(|(n, p)| format!("{n} {p}%"))
            .collect();
        let t = format!("This week: {}", parts.join("  ·  "));
        let t = c.fit(&t, 10.0, R, w - 32.0);
        c.text(lx, y + h - 20.0, &t, 10.0, th.text3, R, "lm");
    }
}

/// Columns of the service row in the status card.
const STATUS_COLS: usize = 5;

/// Height of the "Claude status" card (0 when it's turned off).
fn status_height(s: &Stats) -> f32 {
    if !s.service.enabled {
        return 0.0;
    }
    let (rows, inc) = match &s.service.service {
        Some(sv) => (
            sv.components.len().div_ceil(STATUS_COLS).max(1),
            sv.incidents.len().min(2),
        ),
        None => (1, 0),
    };
    60.0 + 20.0 * rows as f32 + 18.0 * inc as f32 + 14.0
}

/// "Claude status": the overall state from status.claude.com, the main
/// services, and open incidents. Clicking it opens the status page. When the
/// live check fails it shows the last known reading, with its age.
fn service_status(c: &mut Canvas, th: &Theme, s: &Stats, x: f32, y: f32, w: f32, h: f32) {
    let snap = &s.service;
    let ago = |sv: &crate::status::Service| fmt_ago((s.now - sv.fetched_at).num_seconds());
    let sub = match (&snap.service, &snap.error) {
        (Some(sv), None) => format!("status.claude.com · {}", ago(sv)),
        (Some(sv), Some(_)) => format!("last known · {}", ago(sv)),
        (None, Some(e)) => format!("couldn't update ({e})"),
        (None, None) => "checking…".into(),
    };
    card(c, th, x, y, w, h, "Claude status", Some(&sub));
    let Some(sv) = &snap.service else {
        let msg = if snap.error.is_some() {
            "Couldn't reach status.claude.com. Click to open it."
        } else {
            "Checking status.claude.com…"
        };
        c.text(x + 16.0, y + 47.0, msg, 11.0, th.text3, R, "lm");
        return;
    };
    let worst = sv.worst();
    let desc = if sv.description.is_empty() {
        worst.label().to_string()
    } else {
        sv.description.clone()
    };
    c.circle(x + 21.0, y + 47.0, 4.5, level_color(th, worst));
    let desc = c.fit(&desc, 12.0, SB, w - 48.0);
    c.text(x + 32.0, y + 47.0, &desc, 12.0, th.text1, SB, "lm");
    let colw = (w - 32.0) / STATUS_COLS as f32;
    for (i, comp) in sv.components.iter().enumerate() {
        let cx = x + 16.0 + (i % STATUS_COLS) as f32 * colw;
        let cy = y + 71.0 + (i / STATUS_COLS) as f32 * 20.0;
        let ok = comp.level == crate::status::Level::Operational;
        c.circle(cx + 4.0, cy, 3.0, level_color(th, comp.level));
        let (ink, wt) = if ok { (th.text2, R) } else { (th.text1, SB) };
        let name = c.fit(&comp.name, 10.5, wt, colw - 16.0);
        c.text(cx + 12.0, cy, &name, 10.5, ink, wt, "lm");
    }
    let rows = sv.components.len().div_ceil(STATUS_COLS).max(1);
    for (i, inc) in sv.incidents.iter().take(2).enumerate() {
        let iy = y + 71.0 + rows as f32 * 20.0 + i as f32 * 18.0;
        let stage = if inc.stage.is_empty() {
            String::new()
        } else {
            format!("{}: ", inc.stage)
        };
        let t = c.fit(&format!("{stage}{}", inc.name), 10.5, R, w - 44.0);
        c.rect(x + 16.0, iy - 5.0, 3.0, 10.0, level_color(th, inc.impact), 1.5);
        c.text(x + 26.0, iy, &t, 10.5, th.text2, R, "lm");
    }
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
