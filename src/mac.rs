//! macOS: a menu-bar item (ring icon with sparkline, plus the token count as
//! text) and the drawer, which drops down below it. No Dock icon.

use crate::app::{Action, App};
use crate::view::{self, SHADOW_M};
use crate::Options;
use std::time::{Duration, Instant};
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{ElementState, Event, MouseButton, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::keyboard::Key;
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS, WindowBuilderExtMacOS};
use tao::window::{Window, WindowBuilder};
use tray_icon::{
    Icon, MouseButton as TrayButton, MouseButtonState, Rect, TrayIcon, TrayIconBuilder,
    TrayIconEvent,
};

enum UserEvent {
    Tray(TrayIconEvent),
}

/// Menu bar icon height in points.
const ICON_PT: f32 = 18.0;

struct Drawer {
    window: Window,
    layer: blit::Layer,
    open: bool,
    closed_at: Option<Instant>,
    anchor: Option<Rect>,
}

impl Drawer {
    /// Render the drawer and (re)position it under the menu-bar item.
    fn show(&mut self, app: &mut App) {
        let k = self.window.scale_factor();
        let (lw, lh) = app.drawer_size();
        self.window
            .set_inner_size(LogicalSize::new(lw as f64, lh as f64));
        let (bgra, w, h) = app.render_drawer(k as f32);
        self.layer.set_image(bgra, w, h, k);
        if !self.open {
            self.place(w as f64, k);
            self.window.set_visible(true);
            self.window.set_focus();
            self.open = true;
        }
    }

    fn place(&self, w: f64, k: f64) {
        let m = SHADOW_M as f64 * k;
        let monitor = self.window.current_monitor();
        let (mx, my, mw) = monitor.as_ref().map_or((0.0, 0.0, 1440.0 * k), |mon| {
            let p = mon.position();
            (p.x as f64, p.y as f64, mon.size().width as f64)
        });
        let (x, y) = match &self.anchor {
            // right-align the drawer with the menu-bar item, just below it
            Some(r) => (
                r.position.x + r.size.width as f64 - w + m,
                r.position.y + r.size.height as f64 + 4.0 * k - m,
            ),
            // opened without a click (launch flags, relaunch): top right
            None => (mx + mw - w - 8.0 * k + m, my + 30.0 * k - m),
        };
        let x = x.clamp(mx - m, mx + mw - w + m);
        self.window
            .set_outer_position(PhysicalPosition::new(x, y.max(my - m)));
    }

    fn hide(&mut self) {
        if self.open {
            self.window.set_visible(false);
            self.open = false;
            self.closed_at = Some(Instant::now());
        }
    }
}

fn update_tray(tray: &Option<TrayIcon>, app: &App) {
    let Some(tray) = tray else { return };
    let k = 2.0; // Retina; macOS scales it down on standard displays
    let c = view::draw_status_icon(&app.fonts, app.bar_theme(), &app.stats, k, ICON_PT, true);
    if let Ok(icon) = Icon::from_rgba(c.to_rgba(), c.pm.width(), c.pm.height()) {
        let _ = tray.set_icon(Some(icon));
    }
    let (big, small) = app.summary();
    tray.set_title(Some(format!("{big} · {small}")));
    let _ = tray.set_tooltip(Some(format!("Claude Code: {big} tokens, {small}")));
}

pub fn run(opts: Options) {
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory); // no Dock icon
    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Tray(e));
    }));

    let window = WindowBuilder::new()
        .with_title("Claude Code usage")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top(true)
        .with_resizable(false)
        .with_visible(false)
        .with_has_shadow(false) // the drawer image has its own shadow
        .with_inner_size(LogicalSize::new(808.0, 700.0))
        .build(&event_loop)
        .expect("drawer window");
    let layer = blit::Layer::attach(&window);
    let mut drawer = Drawer {
        window,
        layer,
        open: false,
        closed_at: None,
        anchor: None,
    };
    let mut app = App::new(opts.demo);
    let mut tray: Option<TrayIcon> = None;
    let mut cursor = PhysicalPosition::new(0.0, 0.0);

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1));
        let action = match event {
            Event::NewEvents(StartCause::Init) => {
                // the status item must be created once the app is running
                tray = TrayIconBuilder::new()
                    .with_tooltip("Claude Code usage")
                    .build()
                    .ok();
                update_tray(&tray, &app);
                if opts.open || opts.settings {
                    app.show_settings = opts.settings;
                    Action::Redraw
                } else {
                    Action::None
                }
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if app.tick() {
                    update_tray(&tray, &app);
                    if drawer.open {
                        Action::Redraw
                    } else {
                        Action::None
                    }
                } else {
                    Action::None
                }
            }
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::Click {
                button: TrayButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            })) => {
                drawer.anchor = Some(rect);
                let just_closed = drawer
                    .closed_at
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(300));
                if drawer.open {
                    Action::Hide
                } else if just_closed {
                    // this click is what took focus away and closed it
                    Action::None
                } else {
                    app.show_settings = false;
                    Action::Redraw
                }
            }
            // launched again from Finder / Launchpad while running: Settings
            Event::Reopen { .. } => {
                app.show_settings = true;
                Action::Redraw
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::Focused(false) => Action::Hide,
                WindowEvent::CursorMoved { position, .. } => {
                    cursor = position;
                    Action::None
                }
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                } => {
                    let k = drawer.window.scale_factor();
                    app.click((cursor.x / k) as f32, (cursor.y / k) as f32)
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed && event.logical_key == Key::Escape =>
                {
                    app.escape()
                }
                _ => Action::None,
            },
            _ => Action::None,
        };
        match action {
            Action::Redraw => {
                drawer.show(&mut app);
                update_tray(&tray, &app);
            }
            Action::Hide => drawer.hide(),
            Action::Quit => *flow = ControlFlow::Exit,
            Action::None => {}
        }
    });
}

/// Shows a premultiplied BGRA image in the window through a CALayer, keeping
/// per-pixel transparency (rounded corners, soft shadow).
mod blit {
    use core_graphics::base::{
        kCGBitmapByteOrder32Little, kCGImageAlphaPremultipliedFirst, kCGRenderingIntentDefault,
    };
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::data_provider::CGDataProvider;
    use core_graphics::image::CGImage;
    use foreign_types::ForeignType;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
    use tao::platform::macos::WindowExtMacOS;
    use tao::window::Window;

    pub struct Layer {
        view: Retained<NSView>,
    }

    impl Layer {
        /// Add a layer-backed subview that fills the window's content view.
        pub fn attach(window: &Window) -> Layer {
            let mtm = MainThreadMarker::new().expect("main thread");
            // SAFETY: tao returns the window's live NSView.
            let content: &NSView = unsafe { &*(window.ns_view() as *const NSView) };
            let view = NSView::initWithFrame(NSView::alloc(mtm), content.bounds());
            view.setWantsLayer(true);
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            content.addSubview(&view);
            Layer { view }
        }

        pub fn set_image(&self, bgra: Vec<u8>, w: u32, h: u32, scale: f64) {
            let (w, h) = (w as usize, h as usize);
            let provider = CGDataProvider::from_buffer(std::sync::Arc::new(bgra));
            let image = CGImage::new(
                w,
                h,
                8,
                32,
                w * 4,
                &CGColorSpace::create_device_rgb(),
                kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little,
                &provider,
                false,
                kCGRenderingIntentDefault,
            );
            if let Some(layer) = self.view.layer() {
                // SAFETY: a CGImageRef is a valid CALayer `contents` object.
                unsafe {
                    let obj = &*(image.as_ptr() as *const AnyObject);
                    layer.setContents(Some(obj));
                }
                layer.setContentsScale(scale);
            }
        }
    }
}
