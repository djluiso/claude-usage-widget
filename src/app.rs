//! macOS and Linux: the state behind the menu-bar/tray item and the drawer.
//! Frontends own the windows and the event loop; this owns the data, the
//! settings and what a click in the drawer does.

use crate::config::{self, ConfigFile};
use crate::draw::Fonts;
use crate::platform;
use crate::usage::{fmt_left, fmt_tokens, sample_stats, Stats, Store};
use crate::view::settings::{Hit, Hits};
use crate::view::{self, Theme, PANEL_W, SHADOW_M};
use std::time::{Duration, Instant};

/// What the frontend should do after an input.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    None,
    /// The drawer image changed: re-render and show it.
    Redraw,
    /// Close the drawer.
    Hide,
    Quit,
}

pub struct App {
    pub fonts: Fonts,
    pub cfg: ConfigFile,
    store: Store,
    pub stats: Stats,
    demo: bool,
    pub show_settings: bool,
    hits: Hits,
    last_scan: Option<Instant>,
    limits: crate::limits::Fetcher,
}

impl App {
    pub fn new(demo: bool) -> App {
        let cfg = ConfigFile::load();
        let limits = crate::limits::Fetcher::start(cfg.cfg.plan_limits && !demo);
        let mut app = App {
            fonts: Fonts::load(),
            cfg,
            store: Store::new(config::projects_dir()),
            stats: sample_stats(),
            demo,
            show_settings: false,
            hits: Vec::new(),
            last_scan: None,
            limits,
        };
        app.tick();
        app
    }

    /// Call about once a second. Rescans transcripts when the refresh
    /// interval has passed; returns true if the numbers were updated.
    pub fn tick(&mut self) -> bool {
        self.cfg.reload_if_changed();
        let every = Duration::from_millis(self.cfg.cfg.refresh_ms.clamp(500, 60_000) as u64);
        if self.last_scan.is_some_and(|t| t.elapsed() < every) {
            return false;
        }
        self.last_scan = Some(Instant::now());
        if self.demo {
            self.stats = sample_stats();
        } else {
            self.store.scan();
            self.stats = self.store.stats();
            self.stats.apply_limits(self.limits.snapshot());
        }
        true
    }

    pub fn theme(&self) -> &'static Theme {
        view::resolve_theme(&self.cfg.cfg.theme)
    }

    /// Theme of the menu bar / panel the status item sits in.
    pub fn bar_theme(&self) -> &'static Theme {
        if platform::system_dark() {
            &view::DARK
        } else {
            &view::LIGHT
        }
    }

    /// Short text for the menu bar title / tray tooltip.
    pub fn summary(&self) -> (String, String) {
        let s = &self.stats;
        if s.block_start.is_some() {
            (
                fmt_tokens(s.block.all as f64),
                format!("{} left", fmt_left(s.block_left)),
            )
        } else {
            (fmt_tokens(s.today.all as f64), "idle · today".into())
        }
    }

    /// The drawer is about to open: ask for fresh plan limits.
    pub fn drawer_opening(&mut self) {
        self.limits.poke();
    }

    /// Render the drawer at scale `k`: premultiplied BGRA bytes plus the
    /// pixel size. Also records where the clickable controls are.
    pub fn render_drawer(&mut self, k: f32) -> (Vec<u8>, u32, u32) {
        let startup = self.show_settings && platform::autostart_enabled();
        let view = self.show_settings.then_some((&self.cfg.cfg, startup));
        let (c, hits) = view::draw_panel(&self.fonts, self.theme(), &self.stats, k, None, view);
        self.hits = hits;
        (c.to_bgra(0), c.pm.width(), c.pm.height())
    }

    /// The drawer's size in logical px, shadow margin included.
    pub fn drawer_size(&self) -> (f32, f32) {
        let (h, ..) = view::panel_layout(&self.stats);
        (PANEL_W + 2.0 * SHADOW_M, h + 2.0 * SHADOW_M)
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        let (x, y) = (x - SHADOW_M, y - SHADOW_M);
        self.hits
            .iter()
            .find(|(r, _)| x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3])
            .map(|(_, h)| *h)
    }

    /// Is there a control under this point (logical px in the drawer window)?
    #[allow(dead_code)] // cursor feedback is frontend-specific
    pub fn is_clickable(&self, x: f32, y: f32) -> bool {
        self.hit(x, y).is_some()
    }

    /// A click at logical px (x, y) in the drawer window.
    pub fn click(&mut self, x: f32, y: f32) -> Action {
        let Some(hit) = self.hit(x, y) else {
            // clicks in the transparent shadow margin close the drawer
            let (w, h) = self.drawer_size();
            let outside = x < SHADOW_M || y < SHADOW_M || x > w - SHADOW_M || y > h - SHADOW_M;
            return if outside { Action::Hide } else { Action::None };
        };
        match hit {
            Hit::Gear => self.show_settings = !self.show_settings,
            Hit::Theme(t) => {
                self.cfg.cfg.theme = t.into();
                self.cfg.save();
            }
            Hit::Dock(d) => {
                self.cfg.cfg.dock = d.into();
                self.cfg.save();
            }
            Hit::Startup => platform::set_autostart(!platform::autostart_enabled()),
            Hit::PlanLimits => {
                self.cfg.cfg.plan_limits = !self.cfg.cfg.plan_limits;
                self.cfg.save();
                self.limits
                    .set_enabled(self.cfg.cfg.plan_limits && !self.demo);
                self.last_scan = None; // show the change right away
                self.tick();
            }
            Hit::Refresh(ms) => {
                self.cfg.cfg.refresh_ms = ms;
                self.cfg.save();
            }
            Hit::Tip(url) => {
                platform::open_url(url);
                return Action::None;
            }
            Hit::OpenTranscripts => {
                platform::open_folder(&config::projects_dir());
                return Action::None;
            }
            Hit::Quit => return Action::Quit,
        }
        Action::Redraw
    }

    /// Esc: back from Settings first, then close.
    pub fn escape(&mut self) -> Action {
        if self.show_settings {
            self.show_settings = false;
            Action::Redraw
        } else {
            Action::Hide
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gear_toggles_settings_and_escape_goes_back() {
        let mut app = App::new(true);
        app.render_drawer(1.0);
        // the gear sits at the top right of the drawer header
        let (w, _) = app.drawer_size();
        let gear = (w - SHADOW_M - 20.0 - 16.0, SHADOW_M + 19.0 + 16.0);
        assert_eq!(app.click(gear.0, gear.1), Action::Redraw);
        assert!(app.show_settings);
        assert_eq!(app.escape(), Action::Redraw);
        assert!(!app.show_settings);
        assert_eq!(app.escape(), Action::Hide);
        // the shadow margin counts as outside
        assert_eq!(app.click(2.0, 2.0), Action::Hide);
    }
}
