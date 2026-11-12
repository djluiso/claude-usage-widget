//! Thin wrappers around the Win32 calls the widget needs.

use std::time::{Duration, Instant};
use windows::core::{w, BSTR, HSTRING, PCWSTR, VARIANT};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn wstr(s: &str) -> HSTRING {
    HSTRING::from(s)
}

pub fn rect_of(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r).ok().map(|_| r) }
}

pub fn taskbar() -> Option<HWND> {
    unsafe {
        FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null())
            .ok()
            .filter(|h| !h.is_invalid())
    }
}

pub fn tray_notify(tray: HWND) -> Option<HWND> {
    unsafe {
        FindWindowExW(tray, HWND::default(), w!("TrayNotifyWnd"), PCWSTR::null())
            .ok()
            .filter(|h| !h.is_invalid())
    }
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn screen_size() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// True when a fullscreen app (game, video) is in the foreground.
pub fn fullscreen_app_active(own_classes: &[&str]) -> bool {
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_invalid() {
        return false;
    }
    let cls = class_name(fg);
    if ["Progman", "WorkerW", "Shell_TrayWnd"].contains(&cls.as_str())
        || own_classes.contains(&cls.as_str())
    {
        return false;
    }
    let (sw, sh) = screen_size();
    rect_of(fg).is_some_and(|r| r.left <= 0 && r.top <= 0 && r.right >= sw && r.bottom >= sh)
}

// ------------------------------------------------------- layered windows ---
/// Push premultiplied BGRA pixels to a WS_EX_LAYERED window (per-pixel alpha).
pub fn layered_show(hwnd: HWND, bgra: &[u8], w: i32, h: i32, x: i32, y: i32, alpha: u8) -> bool {
    unsafe {
        let screen = GetDC(HWND::default());
        let mem = CreateCompatibleDC(screen);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let ok = match CreateDIBSection(
            screen,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            HANDLE::default(),
            0,
        ) {
            Ok(hbmp) if !bits.is_null() => {
                std::ptr::copy_nonoverlapping(
                    bgra.as_ptr(),
                    bits as *mut u8,
                    bgra.len().min((w * h * 4) as usize),
                );
                let old = SelectObject(mem, hbmp);
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: alpha,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                let res = UpdateLayeredWindow(
                    hwnd,
                    screen,
                    Some(&POINT { x, y }),
                    Some(&SIZE { cx: w, cy: h }),
                    mem,
                    Some(&POINT { x: 0, y: 0 }),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
                SelectObject(mem, old);
                let _ = DeleteObject(hbmp);
                res.is_ok()
            }
            _ => false,
        };
        let _ = DeleteDC(mem);
        ReleaseDC(HWND::default(), screen);
        ok
    }
}

/// Move / fade a layered window without re-sending its pixels.
pub fn layered_move(hwnd: HWND, x: i32, y: i32, alpha: u8) {
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: alpha,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    unsafe {
        let _ = UpdateLayeredWindow(
            hwnd,
            HDC::default(),
            Some(&POINT { x, y }),
            None,
            HDC::default(),
            None,
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
    }
}

/// Grab a screen rectangle as top-down BGRA.
pub fn capture_screen(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u8>> {
    if w <= 0 || h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(HWND::default());
        let mem = CreateCompatibleDC(screen);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut out = None;
        if let Ok(hbmp) = CreateDIBSection(
            screen,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            HANDLE::default(),
            0,
        ) {
            let old = SelectObject(mem, hbmp);
            if BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT).is_ok()
                && !bits.is_null()
            {
                let len = (w * h * 4) as usize;
                out = Some(std::slice::from_raw_parts(bits as *const u8, len).to_vec());
            }
            SelectObject(mem, old);
            let _ = DeleteObject(hbmp);
        }
        let _ = DeleteDC(mem);
        ReleaseDC(HWND::default(), screen);
        out
    }
}

// ------------------------------------------------- Widgets button (UIA) ---
pub struct WidgetsLocator {
    uia: Option<IUIAutomation>,
    cache: Option<i32>,
    checked: Option<Instant>,
}

impl WidgetsLocator {
    pub fn new() -> Self {
        Self {
            uia: None,
            cache: None,
            checked: None,
        }
    }

    pub fn invalidate(&mut self) {
        self.checked = None;
    }

    /// Right edge (physical px) of the Windows 11 Widgets/weather button.
    pub fn right_edge(&mut self, tray: HWND) -> Option<i32> {
        if self
            .checked
            .is_some_and(|t| t.elapsed() < Duration::from_secs(30))
        {
            return self.cache;
        }
        self.checked = Some(Instant::now());
        self.cache = self.lookup(tray);
        self.cache
    }

    fn lookup(&mut self, tray: HWND) -> Option<i32> {
        unsafe {
            if self.uia.is_none() {
                self.uia = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
            }
            let uia = self.uia.as_ref()?;
            let root = uia.ElementFromHandle(tray).ok()?;
            let id = VARIANT::from(BSTR::from("WidgetsButton"));
            let cond = uia
                .CreatePropertyCondition(UIA_AutomationIdPropertyId, &id)
                .ok()?;
            let el = root.FindFirst(TreeScope_Descendants, &cond).ok()?;
            let r = el.CurrentBoundingRectangle().ok()?;
            (r.right > r.left).then_some(r.right)
        }
    }
}

// ------------------------------------------------------ start with Windows --
const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_VALUE: PCWSTR = w!("ClaudeUsageWidget");

pub fn startup_enabled() -> bool {
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        )
        .is_ok()
    }
}

pub fn set_startup(on: bool) {
    unsafe {
        if on {
            let exe = std::env::current_exe().unwrap_or_default();
            let val = format!("\"{}\"", exe.display());
            let data: Vec<u16> = val.encode_utf16().chain(std::iter::once(0)).collect();
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(data.as_ptr() as *const _),
                (data.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}

pub fn shell_open(target: &str, args: Option<&str>) {
    let t = wstr(target);
    let a = args.map(wstr);
    unsafe {
        ShellExecuteW(
            HWND::default(),
            w!("open"),
            &t,
            a.as_ref().map_or(PCWSTR::null(), |a| PCWSTR(a.as_ptr())),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}
