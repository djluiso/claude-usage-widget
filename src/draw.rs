//! Small antialiased drawing toolkit on top of tiny-skia + ab_glyph.
//! All coordinates are logical px; `k` converts them to physical pixels.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use tiny_skia::{
    FillRule, FilterQuality, LineCap, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Rect,
    Stroke, Transform,
};

pub type Rgba = [u8; 4];

pub const fn hex(rgb: u32) -> Rgba {
    [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]
}

#[derive(Clone, Copy)]
pub enum Weight {
    Regular,
    Semibold,
}

pub struct Fonts {
    regular: FontVec,
    semibold: FontVec,
}

impl Fonts {
    /// Windows uses Segoe UI from the system. macOS and Linux use Inter,
    /// bundled in the binary (SIL Open Font License, assets/fonts/OFL.txt).
    #[cfg(not(windows))]
    pub fn load() -> Fonts {
        let f = |b: &'static [u8]| FontVec::try_from_vec(b.to_vec()).expect("bundled font");
        Fonts {
            regular: f(include_bytes!("../assets/fonts/Inter-Regular.ttf")),
            semibold: f(include_bytes!("../assets/fonts/Inter-SemiBold.ttf")),
        }
    }

    #[cfg(windows)]
    pub fn load() -> Fonts {
        let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()) + r"\Fonts\";
        let load = |names: &[&str]| -> FontVec {
            for n in names {
                if let Ok(bytes) = std::fs::read(format!("{dir}{n}")) {
                    if let Ok(f) = FontVec::try_from_vec(bytes) {
                        return f;
                    }
                }
            }
            panic!("no usable system font found in {dir}");
        };
        Fonts {
            regular: load(&["segoeui.ttf", "arial.ttf"]),
            semibold: load(&["seguisb.ttf", "segoeuib.ttf", "arialbd.ttf"]),
        }
    }

    fn get(&self, w: Weight) -> &FontVec {
        match w {
            Weight::Regular => &self.regular,
            Weight::Semibold => &self.semibold,
        }
    }
}

pub struct Canvas<'a> {
    pub pm: Pixmap,
    pub k: f32,
    fonts: &'a Fonts,
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], c[3]);
    p.anti_alias = true;
    p
}

pub fn rrect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r == 0.0 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?));
    }
    let c = 0.5523 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + c, y, x + w, y + r - c, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + c, x + w - r + c, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - c, y + h, x, y + h - r + c, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Rounded top corners, square bottom (bars anchored to a baseline).
fn rtop_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h).max(0.0);
    let c = 0.5523 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x, y + h);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + c, y, x + w, y + r - c, x + w, y + r);
    pb.line_to(x + w, y + h);
    pb.close();
    pb.finish()
}

impl<'a> Canvas<'a> {
    /// Transparent canvas of `w`x`h` logical px.
    pub fn new(w: f32, h: f32, k: f32, fonts: &'a Fonts) -> Canvas<'a> {
        let pw = ((w * k).round() as u32).max(1);
        let ph = ((h * k).round() as u32).max(1);
        Canvas {
            pm: Pixmap::new(pw, ph).expect("pixmap"),
            k,
            fonts,
        }
    }

    fn fill(&mut self, path: &Path, c: Rgba) {
        let t = Transform::from_scale(self.k, self.k);
        self.pm
            .fill_path(path, &paint(c), FillRule::Winding, t, None);
    }

    fn stroke(&mut self, path: &Path, c: Rgba, width: f32, round: bool) {
        let t = Transform::from_scale(self.k, self.k);
        let mut s = Stroke {
            width,
            ..Stroke::default()
        };
        if round {
            s.line_cap = LineCap::Round;
        }
        self.pm.stroke_path(path, &paint(c), &s, t, None);
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: Rgba, r: f32) {
        if let Some(p) = rrect_path(x, y, w, h, r) {
            self.fill(&p, fill);
        }
    }

    /// Filled rounded rect with a 1px border drawn just inside the edge.
    pub fn card(&mut self, x: f32, y: f32, w: f32, h: f32, fill: Rgba, border: Rgba, r: f32) {
        self.rect(x, y, w, h, fill, r);
        let inset = 0.5 / self.k.max(1.0);
        if let Some(p) = rrect_path(x + inset, y + inset, w - 2.0 * inset, h - 2.0 * inset, r) {
            self.stroke(&p, border, 1.0 / self.k, false);
        }
    }

    pub fn bar_up(&mut self, x: f32, base: f32, w: f32, h: f32, fill: Rgba, r: f32) {
        if let Some(p) = rtop_path(x, base - h, w, h, r) {
            self.fill(&p, fill);
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, rad: f32, fill: Rgba) {
        if let Some(p) = PathBuilder::from_circle(cx, cy, rad) {
            self.fill(&p, fill);
        }
    }

    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgba, width: f32) {
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y0);
        pb.line_to(x1, y1);
        if let Some(p) = pb.finish() {
            self.stroke(&p, c, width, false);
        }
    }

    /// Progress ring starting at 12 o'clock, clockwise, round caps.
    pub fn ring(
        &mut self,
        cx: f32,
        cy: f32,
        rad: f32,
        width: f32,
        frac: f64,
        c: Rgba,
        track: Rgba,
    ) {
        let rm = rad - width / 2.0;
        if let Some(p) = PathBuilder::from_circle(cx, cy, rm) {
            self.stroke(&p, track, width, false);
        }
        let frac = frac.clamp(0.0, 1.0) as f32;
        if frac > 0.002 {
            let sweep = 360.0 * frac;
            let n = ((sweep / 3.0).ceil() as usize).max(8);
            let mut pb = PathBuilder::new();
            for i in 0..=n {
                let a = (-90.0 + sweep * i as f32 / n as f32).to_radians();
                let (x, y) = (cx + rm * a.cos(), cy + rm * a.sin());
                if i == 0 {
                    pb.move_to(x, y)
                } else {
                    pb.line_to(x, y)
                }
            }
            if let Some(p) = pb.finish() {
                self.stroke(&p, c, width, true);
            }
        }
    }

    /// Eight-ray spark glyph.
    pub fn spark(&mut self, cx: f32, cy: f32, rad: f32, c: Rgba) {
        let mut pb = PathBuilder::new();
        for i in 0..8 {
            let a = ((i * 45) as f32 + 11.0).to_radians();
            let ln = if i % 2 == 0 { rad } else { rad * 0.72 };
            pb.move_to(cx, cy);
            pb.line_to(cx + ln * a.cos(), cy + ln * a.sin());
        }
        if let Some(p) = pb.finish() {
            self.stroke(&p, c, rad * 0.32, true);
        }
        self.circle(cx, cy, rad * 0.22, c);
    }

    /// Paint `img` stretched over the whole canvas, clipped to a rounded rect.
    pub fn backdrop(&mut self, img: &Pixmap, r: f32) {
        let (w, h) = (self.pm.width() as f32, self.pm.height() as f32);
        let mut mask = Mask::new(self.pm.width(), self.pm.height()).expect("mask");
        if let Some(p) = rrect_path(0.0, 0.0, w, h, r * self.k) {
            mask.fill_path(&p, FillRule::Winding, true, Transform::identity());
        }
        let t = Transform::from_scale(w / img.width() as f32, h / img.height() as f32);
        let pp = PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        self.pm.draw_pixmap(0, 0, img.as_ref(), &pp, t, Some(&mask));
    }

    pub fn draw_at(&mut self, img: &Pixmap, x: i32, y: i32) {
        self.pm.draw_pixmap(
            x,
            y,
            img.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    // ---------------------------------------------------------------- text --
    fn scaled(&self, size: f32, wt: Weight) -> (&FontVec, PxScale) {
        let f = self.fonts.get(wt);
        let s = f
            .pt_to_px_scale(size * self.k * 0.75)
            .unwrap_or(PxScale::from(size * self.k));
        (f, s)
    }

    /// Text width in logical px.
    pub fn tw(&self, s: &str, size: f32, wt: Weight) -> f32 {
        let (f, scale) = self.scaled(size, wt);
        let sf = f.as_scaled(scale);
        let mut w = 0.0;
        let mut prev = None;
        for ch in s.chars() {
            let g = f.glyph_id(ch);
            if let Some(p) = prev {
                w += sf.kern(p, g);
            }
            w += sf.h_advance(g);
            prev = Some(g);
        }
        w / self.k
    }

    pub fn fit(&self, s: &str, size: f32, wt: Weight, maxw: f32) -> String {
        if self.tw(s, size, wt) <= maxw {
            return s.to_string();
        }
        let mut chars: Vec<char> = s.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let t: String = chars.iter().collect::<String>() + "…";
            if self.tw(&t, size, wt) <= maxw {
                return t;
            }
        }
        "…".into()
    }

    /// Anchor like Pillow: h = l/m/r, v = a (top) / t (top) / m (middle) / s (baseline).
    pub fn text(&mut self, x: f32, y: f32, s: &str, size: f32, c: Rgba, wt: Weight, anchor: &str) {
        let width = self.tw(s, size, wt) * self.k;
        let fonts = self.fonts;
        let f = fonts.get(wt);
        let scale = f
            .pt_to_px_scale(size * self.k * 0.75)
            .unwrap_or(PxScale::from(size * self.k));
        let sf = f.as_scaled(scale);
        let a: Vec<char> = anchor.chars().collect();
        let (ha, va) = (
            a.first().copied().unwrap_or('l'),
            a.get(1).copied().unwrap_or('a'),
        );
        let mut caret = match ha {
            'm' => x * self.k - width / 2.0,
            'r' => x * self.k - width,
            _ => x * self.k,
        };
        let baseline = match va {
            's' => y * self.k,
            'm' => y * self.k + (sf.ascent() + sf.descent()) / 2.0,
            'd' => y * self.k + sf.descent(),
            _ => y * self.k + sf.ascent(),
        };
        let (pw, ph) = (self.pm.width() as i32, self.pm.height() as i32);
        let data = self.pm.data_mut();
        let mut prev = None;
        for ch in s.chars() {
            let gid = f.glyph_id(ch);
            if let Some(p) = prev {
                caret += sf.kern(p, gid);
            }
            let glyph = gid.with_scale_and_position(scale, ab_glyph::point(caret, baseline));
            if let Some(og) = f.outline_glyph(glyph) {
                let b = og.px_bounds();
                og.draw(|gx, gy, cov| {
                    let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px < 0 || py < 0 || px >= pw || py >= ph {
                        return;
                    }
                    let i = ((py * pw + px) * 4) as usize;
                    let sa = (c[3] as f32 / 255.0) * cov.clamp(0.0, 1.0);
                    let inv = 1.0 - sa;
                    for ch in 0..3 {
                        let v = c[ch] as f32 * sa + data[i + ch] as f32 * inv;
                        data[i + ch] = v.round().min(255.0) as u8;
                    }
                    let v = 255.0 * sa + data[i + 3] as f32 * inv;
                    data[i + 3] = v.round().min(255.0) as u8;
                });
            }
            caret += sf.h_advance(gid);
            prev = Some(gid);
        }
    }

    /// Premultiplied BGRA bytes for UpdateLayeredWindow.
    /// Straight-alpha RGBA bytes (what tray-icon wants).
    #[allow(dead_code)] // used by the macOS frontend
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pm.data().len());
        for p in self.pm.pixels() {
            let c = p.demultiply();
            out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        out
    }

    /// Straight-alpha ARGB32 in network byte order (StatusNotifierItem icons).
    #[allow(dead_code)] // used by the Linux frontend
    pub fn to_argb_be(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pm.data().len());
        for p in self.pm.pixels() {
            let c = p.demultiply();
            out.extend_from_slice(&[c.alpha(), c.red(), c.green(), c.blue()]);
        }
        out
    }

    pub fn to_bgra(&self, min_alpha: u8) -> Vec<u8> {
        let mut out = self.pm.data().to_vec();
        for px in out.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            if px[3] < min_alpha {
                px[3] = min_alpha;
            }
        }
        out
    }
}

/// Downscale BGRA screen pixels by `f` and box-blur them: a cheap acrylic look.
pub fn blurred_backdrop(bgra: &[u8], w: usize, h: usize, f: usize) -> Option<Pixmap> {
    let (sw, sh) = ((w / f).max(1), (h / f).max(1));
    let mut px = vec![[0f32; 3]; sw * sh];
    for y in 0..sh {
        for x in 0..sw {
            let mut acc = [0f32; 3];
            let mut n = 0.0;
            for yy in (y * f)..((y + 1) * f).min(h) {
                for xx in (x * f)..((x + 1) * f).min(w) {
                    let i = (yy * w + xx) * 4;
                    acc[0] += bgra[i + 2] as f32;
                    acc[1] += bgra[i + 1] as f32;
                    acc[2] += bgra[i] as f32;
                    n += 1.0;
                }
            }
            px[y * sw + x] = [acc[0] / n, acc[1] / n, acc[2] / n];
        }
    }
    let r = 3i32;
    for _ in 0..3 {
        // horizontal then vertical box blur
        let src = px.clone();
        for y in 0..sh {
            for x in 0..sw {
                let mut acc = [0f32; 3];
                let mut n = 0.0;
                for dx in -r..=r {
                    let xx = (x as i32 + dx).clamp(0, sw as i32 - 1) as usize;
                    let p = src[y * sw + xx];
                    acc[0] += p[0];
                    acc[1] += p[1];
                    acc[2] += p[2];
                    n += 1.0;
                }
                px[y * sw + x] = [acc[0] / n, acc[1] / n, acc[2] / n];
            }
        }
        let src = px.clone();
        for y in 0..sh {
            for x in 0..sw {
                let mut acc = [0f32; 3];
                let mut n = 0.0;
                for dy in -r..=r {
                    let yy = (y as i32 + dy).clamp(0, sh as i32 - 1) as usize;
                    let p = src[yy * sw + x];
                    acc[0] += p[0];
                    acc[1] += p[1];
                    acc[2] += p[2];
                    n += 1.0;
                }
                px[y * sw + x] = [acc[0] / n, acc[1] / n, acc[2] / n];
            }
        }
    }
    let mut pm = Pixmap::new(sw as u32, sh as u32)?;
    for (i, p) in px.iter().enumerate() {
        let d = &mut pm.data_mut()[i * 4..i * 4 + 4];
        d[0] = p[0] as u8;
        d[1] = p[1] as u8;
        d[2] = p[2] as u8;
        d[3] = 255;
    }
    Some(pm)
}
