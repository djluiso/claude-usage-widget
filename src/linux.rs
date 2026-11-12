//! Linux: a StatusNotifierItem tray icon (Ubuntu's GNOME, KDE Plasma,
//! Cinnamon, XFCE...) and the drawer as a GTK 4 window.
//!
//! Wayland doesn't let apps place their own windows, so the drawer opens
//! wherever the desktop puts new windows rather than next to the tray icon.

use crate::app::{Action, App};
use crate::view;
use crate::Options;
use gtk::prelude::*;
use gtk::{cairo, gdk, glib};
use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

enum Msg {
    Toggle,
    Settings,
    Quit,
}

struct Tray {
    tx: async_channel::Sender<Msg>,
    icon: Vec<ksni::Icon>,
    tip: (String, String),
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "claude-usage-widget".into()
    }
    fn title(&self) -> String {
        "Claude Code usage".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.tip.0.clone(),
            description: self.tip.1.clone(),
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send_blocking(Msg::Toggle);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        let item = |label: &str, msg: fn() -> Msg| -> ksni::MenuItem<Self> {
            StandardItem {
                label: label.into(),
                activate: Box::new(move |t: &mut Self| {
                    let _ = t.tx.send_blocking(msg());
                }),
                ..Default::default()
            }
            .into()
        };
        vec![
            item("Show usage", || Msg::Toggle),
            item("Settings", || Msg::Settings),
            ksni::MenuItem::Separator,
            item("Quit", || Msg::Quit),
        ]
    }
}

struct Ui {
    app: App,
    gtk_app: gtk::Application,
    window: gtk::Window,
    area: gtk::DrawingArea,
    surface: Option<cairo::ImageSurface>,
    tray: Option<ksni::blocking::Handle<Tray>>,
    closed_at: Option<Instant>,
}

type Shared = Rc<RefCell<Ui>>;

pub fn run(opts: Options) {
    let gtk_app = gtk::Application::builder()
        .application_id("io.github.djluiso.ClaudeUsageWidget")
        .build();
    let ui: Rc<RefCell<Option<Shared>>> = Rc::new(RefCell::new(None));
    gtk_app.connect_activate(move |gapp| {
        // GApplication is single-instance: launching the app again emits
        // `activate` here, which opens the drawer on Settings.
        let existing = ui.borrow().clone();
        match existing {
            Some(u) => open(&u, true),
            None => *ui.borrow_mut() = Some(build(gapp, opts)),
        }
    });
    // our own flags were parsed already; don't let GTK reject them
    gtk_app.run_with_args(&["claude-usage-widget"]);
}

fn build(gapp: &gtk::Application, opts: Options) -> Shared {
    std::mem::forget(gapp.hold()); // keep running while the drawer is hidden

    let css = gtk::CssProvider::new();
    css.load_from_string("window.cuw { background: transparent; box-shadow: none; }");
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    let area = gtk::DrawingArea::new();
    let window = gtk::Window::builder()
        .application(gapp)
        .title("Claude Code usage")
        .decorated(false)
        .resizable(false)
        .child(&area)
        .build();
    window.add_css_class("cuw");
    window.set_hide_on_close(true);

    let ui = Rc::new(RefCell::new(Ui {
        app: App::new(opts.demo),
        gtk_app: gapp.clone(),
        window: window.clone(),
        area: area.clone(),
        surface: None,
        tray: None,
        closed_at: None,
    }));

    let u = ui.clone();
    area.set_draw_func(move |_, cr, _, _| {
        if let Ok(u) = u.try_borrow() {
            if let Some(s) = &u.surface {
                cr.set_operator(cairo::Operator::Source);
                let _ = cr.set_source_surface(s, 0.0, 0.0);
                let _ = cr.paint();
            }
        }
    });

    let click = gtk::GestureClick::new();
    let u = ui.clone();
    click.connect_released(move |_, _, x, y| {
        let action = u.borrow_mut().app.click(x as f32, y as f32);
        apply(&u, action);
    });
    area.add_controller(click);

    let keys = gtk::EventControllerKey::new();
    let u = ui.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            let action = u.borrow_mut().app.escape();
            apply(&u, action);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);

    let u = ui.clone();
    window.connect_is_active_notify(move |w| {
        if !w.is_active() && w.is_visible() {
            hide(&u);
        }
    });

    // tray icon: talks to the desktop over D-Bus on its own thread
    let (tx, rx) = async_channel::unbounded();
    let tray = Tray {
        tx,
        icon: Vec::new(),
        tip: Default::default(),
    };
    {
        use ksni::blocking::TrayMethods;
        ui.borrow_mut().tray = tray.spawn().ok(); // None: no tray on this desktop
    }
    let u = ui.clone();
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                Msg::Toggle => toggle(&u),
                Msg::Settings => open(&u, true),
                Msg::Quit => apply(&u, Action::Quit),
            }
        }
    });

    let u = ui.clone();
    glib::timeout_add_local(Duration::from_secs(1), move || {
        let changed = u.borrow_mut().app.tick();
        if changed {
            update_tray(&u);
            if u.borrow().window.is_visible() {
                render(&u);
            }
        }
        glib::ControlFlow::Continue
    });

    update_tray(&ui);
    let no_tray = ui.borrow().tray.is_none();
    if opts.open || opts.settings || no_tray {
        // without a tray there'd be no way in, so show the drawer
        open(&ui, opts.settings);
    }
    ui
}

fn update_tray(ui: &Shared) {
    let u = ui.borrow();
    let Some(handle) = &u.tray else { return };
    let k = 2.0;
    let c = view::draw_status_icon(
        &u.app.fonts,
        u.app.bar_theme(),
        &u.app.stats,
        k,
        22.0,
        false,
    );
    let icon = ksni::Icon {
        width: c.pm.width() as i32,
        height: c.pm.height() as i32,
        data: c.to_argb_be(),
    };
    let (big, small) = u.app.summary();
    handle.update(move |t: &mut Tray| {
        t.icon = vec![icon];
        t.tip = (format!("Claude Code: {big}"), small);
    });
}

fn render(ui: &Shared) {
    let mut u = ui.borrow_mut();
    let k = u.window.scale_factor().max(1);
    let (bgra, w, h) = u.app.render_drawer(k as f32);
    let surface = cairo::ImageSurface::create_for_data(
        bgra,
        cairo::Format::ARgb32,
        w as i32,
        h as i32,
        (w * 4) as i32,
    )
    .ok();
    if let Some(s) = &surface {
        s.set_device_scale(k as f64, k as f64);
    }
    u.surface = surface;
    let (lw, lh) = u.app.drawer_size();
    let area = u.area.clone();
    drop(u);
    area.set_content_width(lw as i32);
    area.set_content_height(lh as i32);
    area.queue_draw();
}

fn open(ui: &Shared, settings: bool) {
    {
        let mut u = ui.borrow_mut();
        u.app.show_settings = settings;
        u.app.drawer_opening();
    }
    render(ui);
    let window = ui.borrow().window.clone();
    window.present();
}

fn hide(ui: &Shared) {
    let window = {
        let mut u = ui.borrow_mut();
        u.closed_at = Some(Instant::now());
        u.window.clone()
    };
    window.set_visible(false);
}

fn toggle(ui: &Shared) {
    let (visible, just_closed) = {
        let u = ui.borrow();
        let just = u
            .closed_at
            .is_some_and(|t| t.elapsed() < Duration::from_millis(300));
        (u.window.is_visible(), just)
    };
    if visible {
        hide(ui);
    } else if !just_closed {
        open(ui, false);
    }
}

fn apply(ui: &Shared, action: Action) {
    match action {
        Action::Redraw => render(ui),
        Action::Hide => hide(ui),
        Action::Quit => {
            let gapp = ui.borrow().gtk_app.clone();
            gapp.quit();
        }
        Action::None => {}
    }
}
