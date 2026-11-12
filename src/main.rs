#![windows_subsystem = "windows"]
// Drawing helpers naturally take position, size, colour and style together.
#![allow(clippy::too_many_arguments)]
//! Claude Code Usage — Windows 11 taskbar readout + slide-in detail panel.
//! Reads Claude Code's local transcripts; nothing leaves the machine.

mod config;
mod draw;
mod pricing;
mod sys;
mod ui;
mod usage;

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("[{}] {}\n", chrono::Local::now().format("%F %T"), info);
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(config::log_path())
            .and_then(|mut f| std::io::Write::write_all(&mut f, msg.as_bytes()));
    }));

    // `--screenshots <dir>`: render README images from sample data, then exit
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--screenshots") {
        let dir = args.get(i + 1).map(String::as_str).unwrap_or("docs");
        let _ = ui::render_screenshots(std::path::Path::new(dir));
        return;
    }

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
        ui::run();
    }
}
