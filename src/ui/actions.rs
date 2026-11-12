//! Windows: what the drawer's Settings controls and menu items do.

use super::*;
use crate::view::settings::Hit;

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
                crate::platform::set_autostart(!crate::platform::autostart_enabled());
                self.redraw_panel();
            }
            Some(Hit::Refresh(ms)) => self.set_refresh(ms),
            Some(Hit::PlanLimits) => {
                let on = !self.cfg.cfg.plan_limits;
                self.cfg.cfg.plan_limits = on;
                self.cfg.save();
                self.limits.set_enabled(on && !self.demo);
                self.tick();
            }
            Some(Hit::OpenTranscripts) => crate::platform::open_folder(&config::projects_dir()),
            Some(Hit::Tip(url)) => crate::platform::open_url(url),
            Some(Hit::Quit) => self.handle_menu(M_QUIT),
            None => {}
        }
    }
}
