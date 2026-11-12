//! README scenes for `--screenshots`: the widget on a Windows 11-style desktop
//! and the Settings window. Everything here is drawn from sample data with
//! generic placeholder icons, so no real desktop, apps or usage appear.

use super::settings::{draw_settings, FieldEdit};
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

/// Windows 11-style taskbar with generic icons, the readout docked next to
/// the weather button, and (optionally) the panel open above it.
fn desktop(
    fonts: &Fonts,
    th: &'static Theme,
    s: &Stats,
    light: bool,
    panel: bool,
    path: &Path,
) -> Io {
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
        let p = draw_panel(fonts, th, s, k, backdrop.as_ref());
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

/// The Settings window with a Windows-style title bar and drop shadow.
fn settings_window(fonts: &Fonts, th: &'static Theme, light: bool, path: &Path) -> Io {
    let k = 2.0;
    let cfg = Config::default();
    let edit = Some(FieldEdit {
        model: "opus",
        j: 1,
        buf: "25",
        fresh: true,
        caret: false,
    });
    let (body, _) = draw_settings(fonts, th, &cfg, true, k, edit);
    let (bw, bh) = (body.pm.width() as f32 / k, body.pm.height() as f32 / k);
    let (m, title_h) = (28.0, 32.0);
    let (ww, wh) = (bw, bh + title_h);
    let mut c = Canvas::new(ww + 2.0 * m, wh + 2.0 * m, k, fonts);
    for i in 0..18 {
        let i = i as f32;
        c.rect(
            m - i,
            m - i + 10.0,
            ww + 2.0 * i,
            wh + 2.0 * i,
            [0, 0, 0, 5],
            8.0 + i,
        );
    }
    let bg = if light {
        [243, 243, 243, 255]
    } else {
        [32, 32, 32, 255]
    };
    c.rect(m, m, ww, wh, bg, 8.0);
    c.draw_at(&body.pm, (m * k) as i32, ((m + title_h) * k) as i32);
    let border = if light {
        [0, 0, 0, 40]
    } else {
        [255, 255, 255, 34]
    };
    c.card(m, m, ww, wh, [0, 0, 0, 0], border, 8.0);

    // title bar
    let ty = m + title_h / 2.0;
    c.spark(m + 18.0, ty, 6.0, th.accent);
    c.text(
        m + 34.0,
        ty,
        "Claude Usage Widget — Settings",
        11.0,
        th.text1,
        R,
        "lm",
    );
    let bx = m + ww - 46.0 * 3.0;
    let g = th.text2;
    c.line(bx + 18.0, ty, bx + 28.0, ty, g, 1.0); // minimise
    c.card(
        bx + 46.0 + 18.0,
        ty - 5.0,
        10.0,
        10.0,
        [0, 0, 0, 0],
        th.text3,
        1.0,
    ); // maximise (disabled)
    let cx = bx + 92.0 + 23.0;
    c.line(cx - 5.0, ty - 5.0, cx + 5.0, ty + 5.0, g, 1.0); // close
    c.line(cx - 5.0, ty + 5.0, cx + 5.0, ty - 5.0, g, 1.0);
    save(&c, path)
}

pub(super) fn render_scenes(dir: &Path, fonts: &Fonts, s: &Stats) -> Io {
    for (name, th, light) in [("dark", &DARK, false), ("light", &LIGHT, true)] {
        desktop(
            fonts,
            th,
            s,
            light,
            false,
            &dir.join(format!("taskbar-{name}.png")),
        )?;
        desktop(
            fonts,
            th,
            s,
            light,
            true,
            &dir.join(format!("desktop-panel-{name}.png")),
        )?;
        settings_window(fonts, th, light, &dir.join(format!("settings-{name}.png")))?;
    }
    Ok(())
}
