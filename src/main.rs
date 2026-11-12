#![windows_subsystem = "windows"]
//! Claude Code Usage — Windows 11 taskbar readout + slide-in detail panel.
//! Reads Claude Code's local transcripts; nothing leaves the machine.

mod config;
mod draw;
mod sys;
mod ui;
mod usage;

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("[{}] {}\n", chrono::Local::now().format("%F %T"), info);
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(config::log_path())
            .and_then(|mut f| std::io::Write::write_all(&mut f, msg.as_bytes()));
    }));

    unsafe {
        // one instance per user session
        let _mutex = CreateMutexW(None, true, w!("Local\\ClaudeUsageWidget"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        ui::run();
    }
}
