#![cfg_attr(windows, windows_subsystem = "windows")]
// Drawing helpers naturally take position, size, colour and style together.
#![allow(clippy::too_many_arguments)]
//! Claude Code Usage: a live readout of Claude Code usage with a detail drawer.
//! - Windows: on the taskbar next to the Widgets button
//! - macOS: in the menu bar
//! - Linux: in the system tray
//!
//! Reads Claude Code's local transcripts; nothing leaves the machine.

mod config;
mod draw;
mod limits;
mod platform;
mod pricing;
mod usage;
mod view;

#[cfg(windows)]
mod sys;
#[cfg(windows)]
mod ui;

#[cfg(not(windows))]
mod app;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;

/// Command-line switches shared by every platform.
#[derive(Clone, Copy, Default)]
#[allow(dead_code)] // not every frontend reads every switch
pub struct Options {
    /// `--demo`: show sample data instead of reading transcripts.
    pub demo: bool,
    /// `--open`: open the drawer right away.
    pub open: bool,
    /// `--settings`: open the drawer on Settings right away.
    pub settings: bool,
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("[{}] {}\n", chrono::Local::now().format("%F %T"), info);
        eprint!("{msg}");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(config::log_path())
            .and_then(|mut f| std::io::Write::write_all(&mut f, msg.as_bytes()));
    }));

    let args: Vec<String> = std::env::args().collect();
    // `--screenshots <dir>`: render README images from sample data, then exit
    if let Some(i) = args.iter().position(|a| a == "--screenshots") {
        let dir = args.get(i + 1).map(String::as_str).unwrap_or("docs");
        if let Err(e) = view::render_screenshots(std::path::Path::new(dir)) {
            eprintln!("screenshots failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    // `--check-limits`: fetch the plan limits once and print them (no token)
    if args.iter().any(|a| a == "--check-limits") {
        println!("{}", limits::check_once());
        return;
    }
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let opts = Options {
        demo: has("--demo"),
        open: has("--open"),
        settings: has("--settings"),
    };

    #[cfg(windows)]
    windows_main(opts);
    #[cfg(target_os = "macos")]
    mac::run(opts);
    #[cfg(target_os = "linux")]
    linux::run(opts);
}

#[cfg(windows)]
fn windows_main(opts: Options) {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        // one instance per user session
        let _mutex = CreateMutexW(None, true, w!("Local\\ClaudeUsageWidget"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // already running: launching again opens the drawer on Settings
            use windows::Win32::Foundation::{LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{
                AllowSetForegroundWindow, FindWindowW, PostMessageW, ASFW_ANY,
            };
            if let Ok(h) = FindWindowW(w!("ClaudeUsageReadout"), windows::core::PCWSTR::null()) {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = PostMessageW(h, ui::WM_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
            }
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        ui::run(opts);
    }
}
