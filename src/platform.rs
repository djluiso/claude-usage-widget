//! Small per-OS services: light/dark detection, start at login, opening a
//! folder. Everything else in the core is the same on every platform.

use std::path::Path;

// ------------------------------------------------------------------ Windows --
#[cfg(windows)]
pub fn system_dark() -> bool {
    !crate::ui::personalize_flag(windows::core::w!("AppsUseLightTheme"))
}

// -------------------------------------------------------------------- macOS --
/// `defaults read -g AppleInterfaceStyle` prints "Dark" in dark mode and
/// fails in light mode. Cached briefly: it runs on every redraw.
#[cfg(target_os = "macos")]
pub fn system_dark() -> bool {
    cached(|| {
        std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("Dark"))
            .unwrap_or(false)
    })
}

// -------------------------------------------------------------------- Linux --
/// GNOME 42+ `color-scheme`, falling back to a "-dark" GTK theme name.
#[cfg(target_os = "linux")]
pub fn system_dark() -> bool {
    cached(|| {
        let get = |key: &str| {
            std::process::Command::new("gsettings")
                .args(["get", "org.gnome.desktop.interface", key])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase())
                .unwrap_or_default()
        };
        let scheme = get("color-scheme");
        if scheme.contains("dark") {
            return true;
        }
        if scheme.contains("light") {
            return false;
        }
        get("gtk-theme").contains("dark")
    })
}

#[cfg(not(windows))]
fn cached(f: impl FnOnce() -> bool) -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *c {
        if at.elapsed() < Duration::from_secs(5) {
            return v;
        }
    }
    let v = f();
    *c = Some((Instant::now(), v));
    v
}

// ----------------------------------------------------------- start at login --
#[cfg(windows)]
pub fn autostart_enabled() -> bool {
    crate::sys::startup_enabled()
}

#[cfg(windows)]
pub fn set_autostart(on: bool) {
    crate::sys::set_startup(on)
}

/// macOS: a LaunchAgent that runs the app at login.
#[cfg(target_os = "macos")]
fn autostart_file() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        Path::new(&home)
            .join("Library/LaunchAgents")
            .join("io.github.djluiso.claude-usage-widget.plist"),
    )
}

#[cfg(target_os = "macos")]
fn autostart_contents(exe: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>io.github.djluiso.claude-usage-widget</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#,
        xml_escape(exe)
    )
}

#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Linux: an XDG autostart entry (GNOME, KDE, Cinnamon, XFCE all read it).
#[cfg(target_os = "linux")]
fn autostart_file() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))?;
    Some(base.join("autostart").join("claude-usage-widget.desktop"))
}

#[cfg(target_os = "linux")]
fn autostart_contents(exe: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Claude Usage Widget\n\
         Comment=Claude Code usage in the tray\nExec=\"{exe}\"\n\
         Icon=claude-usage-widget\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

#[cfg(not(windows))]
pub fn autostart_enabled() -> bool {
    autostart_file().is_some_and(|p| p.exists())
}

#[cfg(not(windows))]
pub fn set_autostart(on: bool) {
    let Some(path) = autostart_file() else { return };
    if on {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, autostart_contents(&exe.to_string_lossy()));
    } else {
        let _ = std::fs::remove_file(&path);
    }
}

// ------------------------------------------------------------------ folders --
pub fn open_folder(p: &Path) {
    #[cfg(windows)]
    crate::sys::shell_open(&p.display().to_string(), None);
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn autostart_entry_runs_this_exe() {
        let body = autostart_contents("/opt/Claude Usage/widget");
        assert!(body.contains("/opt/Claude Usage/widget"));
    }
}
