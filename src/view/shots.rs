//! README scenes for `--screenshots`: the widget on Windows 11-, macOS- and
//! Ubuntu-style desktops. The readout, status icon and drawer are drawn by
//! the app's own code, placed where each frontend puts them; the desktop
//! around them (bars, dock, generic app icons) is a mock-up. Everything here is drawn from sample data with
//! generic placeholder icons, so no real desktop, apps or usage appear.

use super::*;
use crate::config::Config;
use crate::draw::blurred_backdrop;
use std::path::Path;

const SCENE_W: f32 = 1280.0;
const SCENE_H: f32 = 900.0;
const SCENE_K: f32 = 1.5; // 1920 x 1350 px

type Io = std::io::Result<()>;
/// Wallpaper glow: centre x, centre y, spread, colour.
type Blob = (f32, f32, f32, (f32, f32, f32));

fn save(c: &Canvas, path: &Path) -> Io {
    c.pm.save_png(path).map_err(std::io::Error::other)
}

/// Soft abstract wallpaper, as low-res BGRA (scaled up smoothly when drawn).
fn wallpaper(light: bool) -> (Vec<u8>, usize, usize) {
    let (w, h) = (160usize, 100usize);
    let (a, b): ((f32, f32, f32), (f32, f32, f32)) = if light {
        ((205.0, 222.0, 247.0), (245.0, 220.0, 236.0))
    } else {
        ((14.0, 24.0, 58.0), (46.0, 20.0, 72.0))
    };
    let blobs: [Blob; 3] = if light {
        [
            (118.0, 30.0, 520.0, (255.0, 196.0, 160.0)),
            (30.0, 80.0, 700.0, (170.0, 214.0, 240.0)),
            (80.0, 55.0, 380.0, (230.0, 200.0, 245.0)),
        ]
    } else {
        [
            (120.0, 28.0, 520.0, (190.0, 84.0, 48.0)),
            (28.0, 82.0, 760.0, (22.0, 110.0, 130.0)),
            (78.0, 58.0, 420.0, (90.0, 52.0, 150.0)),
        ]
    };
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let t = (x as f32 / w as f32) * 0.6 + (y as f32 / h as f32) * 0.4;
            let mut c = [
                a.0 + (b.0 - a.0) * t,
                a.1 + (b.1 - a.1) * t,
                a.2 + (b.2 - a.2) * t,
            ];
            for (bx, by, r2, col) in blobs {
                let d2 = (x as f32 - bx).powi(2) + (y as f32 - by).powi(2);
                let f = (-d2 / r2).exp() * 0.75;
                c[0] += (col.0 - c[0]) * f;
                c[1] += (col.1 - c[1]) * f;
                c[2] += (col.2 - c[2]) * f;
            }
            let i = (y * w + x) * 4;
            px[i] = c[2] as u8;
            px[i + 1] = c[1] as u8;
            px[i + 2] = c[0] as u8;
            px[i + 3] = 255;
        }
    }
    (px, w, h)
}

fn bgra_to_pixmap(bgra: &[u8], w: usize, h: usize) -> Pixmap {
    let mut pm = Pixmap::new(w as u32, h as u32).expect("pixmap");
    for (d, s) in pm.data_mut().chunks_mut(4).zip(bgra.chunks(4)) {
        d.copy_from_slice(&[s[2], s[1], s[0], 255]);
    }
    pm
}

/// Crop a logical-px rectangle of the scene out of the low-res wallpaper.
fn crop(
    bgra: &[u8],
    w: usize,
    h: usize,
    x: f32,
    y: f32,
    cw: f32,
    ch: f32,
) -> (Vec<u8>, usize, usize) {
    let sx = |v: f32| ((v / SCENE_W) * w as f32).clamp(0.0, w as f32 - 1.0) as usize;
    let sy = |v: f32| ((v / SCENE_H) * h as f32).clamp(0.0, h as f32 - 1.0) as usize;
    let (x0, y0, x1, y1) = (
        sx(x),
        sy(y),
        sx(x + cw).max(sx(x) + 1),
        sy(y + ch).max(sy(y) + 1),
    );
    let (ow, oh) = (x1 - x0, y1 - y0);
    let mut out = Vec::with_capacity(ow * oh * 4);
    for yy in y0..y1 {
        let i = (yy * w + x0) * 4;
        out.extend_from_slice(&bgra[i..i + ow * 4]);
    }
    (out, ow, oh)
}

#[derive(Clone, Copy, PartialEq)]
enum Drawer {
    Closed,
    Usage,
    Settings,
}

/// Windows 11-style taskbar with generic icons, the readout docked next to
/// the weather button, and (optionally) the drawer open above it.
fn desktop(
    fonts: &Fonts,
    th: &'static Theme,
    s: &Stats,
    light: bool,
    drawer: Drawer,
    path: &Path,
) -> Io {
    let panel = drawer != Drawer::Closed;
    let k = SCENE_K;
    let mut c = Canvas::new(SCENE_W, SCENE_H, k, fonts);
    let (wp, ww, wh) = wallpaper(light);
    c.backdrop(&bgra_to_pixmap(&wp, ww, wh), 0.0);

    // taskbar
    let top = SCENE_H - BAR_H;
    let cy = top + BAR_H / 2.0;
    let bar = if light {
        [238, 238, 240, 228]
    } else {
        [28, 28, 30, 228]
    };
    c.rect(0.0, top, SCENE_W, BAR_H, bar, 0.0);
    c.line(0.0, top + 0.5, SCENE_W, top + 0.5, th.grid, 1.0);

    // weather button
    c.circle(26.0, cy - 2.0, 7.5, hex(0xf6c443));
    c.circle(
        31.0,
        cy + 4.0,
        6.0,
        if light { hex(0xffffff) } else { hex(0xdfe6f0) },
    );
    c.circle(
        24.0,
        cy + 5.0,
        5.0,
        if light { hex(0xffffff) } else { hex(0xdfe6f0) },
    );
    c.text(44.0, cy - 1.0, "24°C", 12.0, th.text1, SB, "ls");
    c.text(44.0, cy + 3.0, "Sunny", 11.0, th.text2, R, "lt");
    let weather_right = 104.0;

    // readout, exactly as the app draws it
    let ro = draw_readout(fonts, th, s, k, false, panel);
    c.draw_at(
        &ro.pm,
        ((weather_right + 10.0) * k) as i32,
        (top * k) as i32,
    );

    // centred app icons (generic placeholders)
    let colors = [0x4f8ef7, 0xf0a030, 0x2fb67c, 0xe2556f, 0x8b6cf0, 0x3ab0c8];
    let n = colors.len() as f32;
    let x0 = SCENE_W / 2.0 - n * 44.0 / 2.0;
    for (i, col) in colors.iter().enumerate() {
        let bx = x0 + i as f32 * 44.0;
        if i == 1 {
            c.rect(bx + 2.0, top + 4.0, 40.0, 40.0, th.active, 5.0);
            c.rect(bx + 15.0, top + 41.0, 14.0, 3.0, th.text3, 1.5);
        }
        c.rect(bx + 10.0, cy - 12.0, 24.0, 24.0, hex(*col), 6.0);
        c.rect(bx + 10.0, cy - 12.0, 24.0, 10.0, [255, 255, 255, 40], 6.0);
    }

    // tray + clock
    let right = SCENE_W - 12.0;
    c.text(right - 8.0, cy - 1.0, "14:32", 12.0, th.text1, R, "rs");
    c.text(right - 8.0, cy + 3.0, "12/11/2026", 11.0, th.text1, R, "rt");
    // battery, language and overflow chevron (generic tray items)
    let bx = right - 104.0;
    c.card(bx, cy - 6.0, 20.0, 12.0, [0, 0, 0, 0], th.text2, 2.5);
    c.rect(bx + 20.5, cy - 2.5, 2.0, 5.0, th.text2, 1.0);
    c.rect(bx + 2.0, cy - 4.0, 12.0, 8.0, th.text2, 1.5);
    c.text(bx - 16.0, cy - 1.0, "ENG", 10.0, th.text1, R, "rs");
    c.text(bx - 16.0, cy + 3.0, "US", 10.0, th.text1, R, "rt");
    c.text(bx - 58.0, cy, "^", 12.0, th.text2, R, "rm");

    if panel {
        let (h, ..) = panel_layout(s);
        let (px, py) = (12.0, top - 12.0 - h);
        let (bd, bw, bh) = crop(&wp, ww, wh, px, py, PANEL_W, h);
        let backdrop = blurred_backdrop(&bd, bw, bh, 1);
        let cfg = Config::default();
        let view = (drawer == Drawer::Settings).then_some((&cfg, true));
        let (p, _) = draw_panel(fonts, th, s, k, backdrop.as_ref(), view);
        c.draw_at(
            &p.pm,
            ((px - SHADOW_M) * k) as i32,
            ((py - SHADOW_M) * k) as i32,
        );
        return save(&c, path);
    }
    // without the panel, keep just a strip of desktop + taskbar
    let strip_h = (110.0 * k) as u32;
    let rect =
        tiny_skia::IntRect::from_xywh(0, (c.pm.height() - strip_h) as i32, c.pm.width(), strip_h)
            .expect("strip");
    let strip = c.pm.clone_rect(rect).expect("crop");
    strip.save_png(path).map_err(std::io::Error::other)
}

/// Generic coloured app icons for docks and taskbars.
const APP_COLORS: [u32; 7] = [
    0x4f8ef7, 0xf0a030, 0x2fb67c, 0xe2556f, 0x8b6cf0, 0x3ab0c8, 0x9aa0a6,
];

fn app_icon(c: &mut Canvas, x: f32, y: f32, size: f32, col: u32) {
    c.rect(x, y, size, size, hex(col), size * 0.24);
    c.rect(x, y, size, size * 0.42, [255, 255, 255, 40], size * 0.24);
}

fn new_scene<'a>(fonts: &'a Fonts, light: bool) -> Canvas<'a> {
    let mut c = Canvas::new(SCENE_W, SCENE_H, SCENE_K, fonts);
    let (wp, ww, wh) = wallpaper(light);
    c.backdrop(&bgra_to_pixmap(&wp, ww, wh), 0.0);
    c
}

/// The drawer as macOS and Linux show it (no blur behind it there).
fn place_drawer(c: &mut Canvas, fonts: &Fonts, th: &Theme, s: &Stats, x: f32, y: f32) {
    let k = c.k;
    let (p, _) = draw_panel(fonts, th, s, k, None, None);
    c.draw_at(
        &p.pm,
        ((x - SHADOW_M) * k).round() as i32,
        ((y - SHADOW_M) * k).round() as i32,
    );
}

/// macOS: menu bar with the status item (ring + sparkline + text), the
/// drawer dropped down right-aligned below it, and a Dock.
fn macos(fonts: &Fonts, th: &'static Theme, s: &Stats, light: bool, path: &Path) -> Io {
    let k = SCENE_K;
    let mut c = new_scene(fonts, light);
    let mb = 26.0;
    let cy = mb / 2.0;
    let (bar, ink) = if light {
        ([246, 246, 246, 214], hex(0x1d1d1f))
    } else {
        ([22, 22, 24, 200], hex(0xf2f2f2))
    };
    c.rect(0.0, 0.0, SCENE_W, mb, bar, 0.0);
    let mut x = 20.0;
    for (i, m) in ["Finder", "File", "Edit", "View", "Go", "Window", "Help"]
        .iter()
        .enumerate()
    {
        let wt = if i == 0 { SB } else { R };
        c.text(x, cy, m, 13.0, ink, wt, "lm");
        x += c.tw(m, 13.0, wt) + 18.0;
    }
    // right side: clock, generic status icons, then our item
    let right = SCENE_W - 14.0;
    let clock = "Thu 12 Nov  14:32";
    c.text(right, cy, clock, 13.0, ink, R, "rm");
    let mut rx = right - c.tw(clock, 13.0, R) - 16.0;
    c.card(rx - 24.0, cy - 5.5, 22.0, 11.0, [0, 0, 0, 0], ink, 3.0); // battery
    c.rect(rx - 22.0, cy - 3.5, 14.0, 7.0, ink, 1.5);
    c.rect(rx - 1.5, cy - 2.0, 1.5, 4.0, ink, 0.75);
    rx -= 24.0 + 16.0;
    c.card(rx - 16.0, cy - 6.0, 16.0, 5.0, [0, 0, 0, 0], ink, 2.5); // toggles
    c.card(rx - 16.0, cy + 1.0, 16.0, 5.0, [0, 0, 0, 0], ink, 2.5);
    rx -= 16.0 + 12.0;

    let bar_th: &Theme = if light { &LIGHT } else { &DARK };
    let icon = draw_status_icon(fonts, bar_th, s, k, 16.0, true);
    let title = format!(
        "{} · {} left",
        fmt_tokens(s.block.all as f64),
        fmt_left(s.block_left)
    );
    let tw = c.tw(&title, 13.0, R);
    let iw = icon.pm.width() as f32 / k;
    let item_w = 8.0 + iw + 6.0 + tw + 8.0;
    let ix = rx - item_w;
    let hl = if light {
        [0, 0, 0, 28]
    } else {
        [255, 255, 255, 46]
    };
    c.rect(ix, 3.0, item_w, mb - 6.0, hl, 5.0); // open: highlighted
    c.draw_at(
        &icon.pm,
        ((ix + 8.0) * k).round() as i32,
        ((cy - 8.0) * k).round() as i32,
    );
    c.text(ix + 8.0 + iw + 6.0, cy, &title, 13.0, ink, R, "lm");

    // Dock
    let n = APP_COLORS.len() as f32;
    let dock_w = n * 54.0 + 14.0;
    let (dx, dy) = ((SCENE_W - dock_w) / 2.0, SCENE_H - 72.0);
    let (dock, dock_b) = if light {
        ([255, 255, 255, 120], [0, 0, 0, 24])
    } else {
        ([40, 40, 44, 150], [255, 255, 255, 30])
    };
    c.card(dx, dy, dock_w, 64.0, dock, dock_b, 18.0);
    for (i, col) in APP_COLORS.iter().enumerate() {
        app_icon(&mut c, dx + 12.0 + i as f32 * 54.0, dy + 9.0, 46.0, *col);
    }

    // drawer: right edge lined up with the item, just below the menu bar
    let px = (ix + item_w - PANEL_W).max(8.0);
    place_drawer(&mut c, fonts, th, s, px, mb + 4.0);
    save(&c, path)
}

/// Ubuntu (GNOME): dark top bar with the tray icon, Ubuntu Dock on the left,
/// and the drawer where GNOME places new windows (centred).
fn ubuntu(fonts: &Fonts, th: &'static Theme, s: &Stats, light: bool, path: &Path) -> Io {
    let k = SCENE_K;
    let mut c = new_scene(fonts, light);
    let tb = 32.0;
    let cy = tb / 2.0;
    let white = hex(0xffffff);
    let dim = [255, 255, 255, 150];
    c.rect(0.0, 0.0, SCENE_W, tb, hex(0x000000), 0.0); // GNOME's top bar is always dark
                                                       // workspace indicator
    c.rect(14.0, cy - 4.0, 22.0, 8.0, white, 4.0);
    c.circle(46.0, cy, 3.5, dim);
    c.circle(58.0, cy, 3.5, dim);
    c.text(SCENE_W / 2.0, cy, "Nov 12  14:32", 13.0, white, SB, "mm");
    // system menu pill: network, volume, battery (generic shapes)
    let pill_w = 84.0;
    let px0 = SCENE_W - 10.0 - pill_w;
    c.rect(px0, 4.0, pill_w, tb - 8.0, [255, 255, 255, 30], 12.0);
    for i in 0..3 {
        let bh = 4.0 + i as f32 * 3.0;
        c.rect(
            px0 + 14.0 + i as f32 * 4.0,
            cy + 5.0 - bh,
            3.0,
            bh,
            white,
            1.0,
        );
    }
    c.rect(px0 + 36.0, cy - 3.0, 4.0, 6.0, white, 1.0);
    c.line(px0 + 42.0, cy - 4.0, px0 + 45.0, cy - 6.0, white, 1.5);
    c.line(px0 + 42.0, cy + 4.0, px0 + 45.0, cy + 6.0, white, 1.5);
    c.card(px0 + 54.0, cy - 5.0, 18.0, 10.0, [0, 0, 0, 0], white, 2.5);
    c.rect(px0 + 56.0, cy - 3.0, 12.0, 6.0, white, 1.0);
    // our tray icon, highlighted because the drawer is open
    let icon = draw_status_icon(fonts, &DARK, s, k, 18.0, false);
    let iw = icon.pm.width() as f32 / k;
    let ix = px0 - 12.0 - iw - 16.0;
    c.rect(ix, 4.0, iw + 16.0, tb - 8.0, [255, 255, 255, 46], 12.0);
    c.draw_at(
        &icon.pm,
        ((ix + 8.0) * k).round() as i32,
        ((cy - 9.0) * k).round() as i32,
    );

    // Ubuntu Dock
    let dock_w = 64.0;
    c.rect(0.0, tb, dock_w, SCENE_H - tb, [24, 24, 24, 215], 0.0);
    for (i, col) in APP_COLORS.iter().take(6).enumerate() {
        let y = tb + 12.0 + i as f32 * 58.0;
        app_icon(&mut c, 10.0, y, 44.0, *col);
        if i == 0 {
            c.circle(4.0, y + 22.0, 2.5, hex(0xe95420));
        }
    }
    for r in 0..3 {
        for q in 0..3 {
            let (gx, gy) = (22.0 + q as f32 * 10.0, SCENE_H - 44.0 + r as f32 * 10.0);
            c.circle(gx, gy, 2.6, white);
        }
    }

    // drawer: centred in the work area
    let (h, ..) = panel_layout(s);
    let (wx, wy, ww, wh) = (dock_w, tb, SCENE_W - dock_w, SCENE_H - tb);
    place_drawer(
        &mut c,
        fonts,
        th,
        s,
        wx + (ww - PANEL_W) / 2.0,
        wy + (wh - h) / 2.0,
    );
    save(&c, path)
}

pub(super) fn render_scenes(dir: &Path, fonts: &Fonts, s: &Stats) -> Io {
    // macOS and Linux builds use the bundled Inter font
    let inter = Fonts::inter_from_assets();
    let unix_fonts = inter.as_ref().unwrap_or(fonts);
    for (name, th, light) in [("dark", &DARK, false), ("light", &LIGHT, true)] {
        for (drawer, file) in [
            (Drawer::Closed, "taskbar"),
            (Drawer::Usage, "desktop-panel"),
            (Drawer::Settings, "desktop-settings"),
        ] {
            desktop(
                fonts,
                th,
                s,
                light,
                drawer,
                &dir.join(format!("{file}-{name}.png")),
            )?;
        }
        macos(
            unix_fonts,
            th,
            s,
            light,
            &dir.join(format!("macos-{name}.png")),
        )?;
        ubuntu(
            unix_fonts,
            th,
            s,
            light,
            &dir.join(format!("ubuntu-{name}.png")),
        )?;
    }
    Ok(())
}
