//! Window controls (MyTitleBar.vue / px:fe:* events of the Wails shell).
//!
//! Windows / Linux use a frameless window with the custom title-bar buttons;
//! macOS keeps the native traffic lights over a transparent, full-size title
//! bar (the Electron/Wails "hiddenInset" look). Closing hides to the tray.

use slint::winit_030::{winit, WinitWindowAccessor};
use slint::ComponentHandle;

use crate::app::ui;
use crate::{App, AppWindow};

fn with_winit<F: FnOnce(&winit::window::Window) + Send + 'static>(f: F) {
    ui(move |w| {
        w.window().with_winit_window(|ww| f(ww));
    });
}

/// Wails opens the window centred on the monitor's work area (WindowCentered);
/// left to Windows it cascades and, on a short screen, runs under the taskbar.
/// A window taller or wider than the work area (1100x760 at 125% on a 1366x768
/// or 1920x1080@150% screen) is first shrunk to it, down to the min size the
/// window already enforces, so the bottom bar stays reachable.
#[cfg(windows)]
fn center_on_work_area(ww: &winit::window::Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    let Ok(handle) = ww.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let ok = unsafe { GetMonitorInfoW(MonitorFromWindow(h.hwnd.get() as _, MONITOR_DEFAULTTONEAREST), &mut info) };
    if ok == 0 {
        return;
    }
    let work = info.rcWork;
    let (work_w, work_h) = ((work.right - work.left).max(0) as u32, (work.bottom - work.top).max(0) as u32);
    let mut size = ww.outer_size();
    if size.width > work_w || size.height > work_h {
        let inner = ww.inner_size();
        let (dw, dh) = (size.width.saturating_sub(inner.width), size.height.saturating_sub(inner.height));
        let _ = ww.request_inner_size(winit::dpi::PhysicalSize::new(
            size.width.min(work_w).saturating_sub(dw),
            size.height.min(work_h).saturating_sub(dh),
        ));
        size = ww.outer_size();
    }
    let x = work.left + (work_w as i32 - size.width as i32) / 2;
    let y = work.top + ((work_h as i32 - size.height as i32) / 2).max(0);
    ww.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
}

pub fn show() {
    ui(|w| {
        let _ = w.show();
        w.window().with_winit_window(|ww: &winit::window::Window| {
            #[cfg(windows)]
            {
                static CENTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if !CENTERED.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    center_on_work_area(ww);
                }
            }
            ww.set_visible(true);
            ww.set_minimized(false);
            ww.focus_window();
        });
        #[cfg(target_os = "macos")]
        crate::platform::macos_activate();
    });
}

pub fn hide() {
    ui(|w| {
        let _ = w.hide();
    });
}

pub fn is_visible(w: &AppWindow) -> bool {
    w.window().is_visible()
}

pub fn toggle_visible() {
    ui(|w| {
        if w.window().is_visible() && !w.window().is_minimized() {
            let _ = w.hide();
        } else {
            show();
        }
    });
}

pub fn minimize() {
    ui(|w| w.window().set_minimized(true));
}

/// Title-bar close and Alt+F4 on Windows / Linux: hide to the tray, or
/// minimise when nothing can host the tray icon (GNOME without AppIndicator),
/// so the window is never left unreachable. The check spawns dbus-send on
/// Linux, hence the thread.
pub fn close_to_tray() {
    std::thread::spawn(|| {
        if crate::tray::host_available() {
            hide();
        } else {
            minimize();
        }
    });
}

pub fn toggle_maximize() {
    ui(|w| {
        let m = !w.window().is_maximized();
        w.window().set_maximized(m);
        w.global::<App>().set_maximized(m);
    });
}

pub fn start_drag() {
    with_winit(|ww| {
        let _ = ww.drag_window();
    });
}

pub fn start_resize(dir: i32) {
    use winit::window::ResizeDirection as R;
    let d = match dir {
        1 => R::North,
        2 => R::South,
        3 => R::West,
        4 => R::East,
        5 => R::NorthWest,
        6 => R::NorthEast,
        7 => R::SouthWest,
        _ => R::SouthEast,
    };
    with_winit(move |ww| {
        let _ = ww.drag_resize_window(d);
    });
}

/// Window attribute hook (applied before the native window is created).
pub fn attributes(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    let attrs = attrs
        .with_title("Prizrak-Box")
        .with_min_inner_size(winit::dpi::LogicalSize::new(960.0, 660.0));
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::WindowAttributesExtMacOS;
        return attrs
            .with_titlebar_transparent(true)
            .with_title_hidden(true)
            .with_fullsize_content_view(true);
    }
    #[cfg(target_os = "linux")]
    {
        use winit::platform::wayland::WindowAttributesExtWayland;
        use winit::platform::x11::WindowAttributesExtX11;
        let attrs = WindowAttributesExtWayland::with_name(attrs, "com.legiz-ru.prizrak-box", "prizrak-box");
        return WindowAttributesExtX11::with_name(attrs, "com.legiz-ru.prizrak-box", "Prizrak-Box");
    }
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowAttributesExtWindows;
        // Frameless, but keep the DWM drop shadow of a normal window.
        attrs.with_undecorated_shadow(true)
    }
}

/// Windows 11: rounded corners for the frameless window (DWM corner preference).
pub fn polish_native() {
    #[cfg(windows)]
    ui(|w| {
        w.window().with_winit_window(|ww: &winit::window::Window| {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = ww.window_handle() {
                if let RawWindowHandle::Win32(h) = h.as_raw() {
                    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
                    const DWMWCP_ROUND: u32 = 2;
                    let pref: u32 = DWMWCP_ROUND;
                    unsafe {
                        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                            h.hwnd.get() as _,
                            DWMWA_WINDOW_CORNER_PREFERENCE,
                            &pref as *const u32 as *const _,
                            std::mem::size_of::<u32>() as u32,
                        );
                    }
                }
            }
        });
    });
}

/// Keeps App.maximized in sync with maximise changes the OS makes itself
/// (Win+Up, snap, double-click on the title bar …).
pub fn poll_state(w: &AppWindow) {
    let m = w.window().is_maximized();
    if w.global::<App>().get_maximized() != m {
        w.global::<App>().set_maximized(m);
    }
}
