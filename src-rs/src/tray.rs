//! System tray — port of `src-wails/tray.go` (itself a port of
//! src-electron/tray.ts): Show, Rule/Global/Direct, Proxy Groups (submenu per
//! group, one checked item per node), Open Dashboard, System Proxy, TUN, Quit.
//! macOS gets the template icon, Windows a light/dark tile with a green badge
//! while System Proxy or TUN is on, Linux the colour icon.

use std::cell::RefCell;
use std::collections::HashMap;

use parking_lot::Mutex;
use serde_json::Value;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::app;
use crate::i18n::tr;
use crate::util;

#[derive(Default, Clone)]
struct TrayState {
    mode: String,
    proxy: bool,
    tun: bool,
    groups: Vec<(String, Vec<(String, bool)>)>,
    dashboards: Vec<(String, String)>,
    global_mode_disabled: bool,
}

#[derive(Clone)]
enum Action {
    Show,
    Mode(String),
    Proxy(String, String),
    Dashboard(String),
    SysProxy,
    Tun,
    Quit,
}

static STATE: Mutex<Option<TrayState>> = Mutex::new(None);
static ACTIONS: Mutex<Option<HashMap<MenuId, Action>>> = Mutex::new(None);

thread_local! {
    static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
}

fn icon_from_png(bytes: &[u8]) -> Option<Icon> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}

fn current_icon(active: bool) -> Option<Icon> {
    #[cfg(target_os = "macos")]
    {
        let _ = active;
        return icon_from_png(include_bytes!("../assets/tray-macos.png"));
    }
    #[cfg(windows)]
    {
        let dark = windows_taskbar_dark();
        let bytes: &[u8] = match (dark, active) {
            (true, true) => include_bytes!("../assets/tray-win-dark-active.png"),
            (true, false) => include_bytes!("../assets/tray-win-dark.png"),
            (false, true) => include_bytes!("../assets/tray-win-light-active.png"),
            (false, false) => include_bytes!("../assets/tray-win-light.png"),
        };
        return icon_from_png(bytes);
    }
    #[cfg(target_os = "linux")]
    {
        let _ = active;
        icon_from_png(include_bytes!("../assets/tray.png"))
    }
}

#[cfg(windows)]
fn windows_taskbar_dark() -> bool {
    let mut cmd = std::process::Command::new("reg");
    cmd.args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "/v", "SystemUsesLightTheme"]);
    crate::core::hide_window(&mut cmd);
    cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).contains("0x0")).unwrap_or(true)
}

/// Re-picks the light/dark tile when the taskbar theme changes while running,
/// as Wails' SetIcon + SetDarkModeIcon pair does. The change notification is
/// armed before the value is read: Settings writes AppsUseLightTheme and
/// SystemUsesLightTheme separately, and a synchronous wait re-armed after the
/// first write misses the second.
#[cfg(windows)]
fn watch_taskbar_theme() {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    std::thread::spawn(|| {
        let path: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut key: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_NOTIFY, &mut key) } != ERROR_SUCCESS {
            return;
        }
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if event.is_null() {
            return;
        }
        let mut dark = windows_taskbar_dark();
        while unsafe { RegNotifyChangeKeyValue(key, 0, REG_NOTIFY_CHANGE_LAST_SET, event, 1) } == ERROR_SUCCESS {
            let now = windows_taskbar_dark();
            if now != dark {
                dark = now;
                rebuild();
            }
            unsafe { WaitForSingleObject(event, INFINITE) };
        }
    });
}

/// Creates the tray on the current thread (the UI thread on macOS/Windows,
/// the GTK thread on Linux).
pub fn create() {
    *STATE.lock() = Some(TrayState { mode: "rule".into(), ..Default::default() });
    let menu = build_menu();
    let mut b = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Prizrak-Box");
    if let Some(i) = current_icon(false) {
        b = b.with_icon(i);
    }
    #[cfg(target_os = "macos")]
    {
        b = b.with_icon_as_template(true);
    }
    #[cfg(target_os = "linux")]
    {
        b = b.with_title("Prizrak-Box").with_id("prizrak-box");
    }
    b = b.with_menu_on_left_click(true);
    match b.build() {
        Ok(t) => {
            TRAY.with(|c| *c.borrow_mut() = Some(t));
            #[cfg(windows)]
            watch_taskbar_theme();
        }
        Err(e) => log::error!("tray: {e}"),
    }
    MenuEvent::set_event_handler(Some(|e: MenuEvent| {
        let action = ACTIONS.lock().as_ref().and_then(|m| m.get(&e.id).cloned());
        if let Some(a) = action {
            handle(a);
        }
    }));
}

fn handle(a: Action) {
    match a {
        Action::Show => crate::window::show(),
        Action::Mode(m) => app::set_rule(m),
        Action::Proxy(g, p) => app::switch_proxy_in_group(g, p),
        Action::Dashboard(u) => app::open_url(&u),
        Action::SysProxy => app::spawn(async { app::proxy_switch().await }),
        Action::Tun => app::spawn(async { app::tun_switch().await }),
        Action::Quit => app::spawn(async { app::quit_via_api().await }),
    }
}

fn build_menu() -> Menu {
    let st = STATE.lock().clone().unwrap_or_default();
    let mut actions = HashMap::new();
    let menu = Menu::new();
    let add_item = |m: &Menu, label: String, a: Action, actions: &mut HashMap<MenuId, Action>| {
        let it = MenuItem::new(label, true, None);
        actions.insert(it.id().clone(), a);
        let _ = m.append(&it);
    };
    add_item(&menu, tr("tray.show"), Action::Show, &mut actions);
    let _ = menu.append(&PredefinedMenuItem::separator());
    if !st.global_mode_disabled {
        for (key, mode) in [("tray.rule", "rule"), ("tray.global", "global"), ("tray.direct", "direct")] {
            let it = CheckMenuItem::new(tr(key), true, st.mode == mode, None);
            actions.insert(it.id().clone(), Action::Mode(mode.into()));
            let _ = menu.append(&it);
        }
        let _ = menu.append(&PredefinedMenuItem::separator());
    }
    let groups = Submenu::new(tr("tray.proxyGroups"), true);
    for (g, nodes) in &st.groups {
        if nodes.is_empty() {
            continue;
        }
        let sub = Submenu::new(g, true); // raw names, as MyEvent.vue sends them
        for (n, now) in nodes {
            let it = CheckMenuItem::new(n, true, *now, None);
            actions.insert(it.id().clone(), Action::Proxy(g.clone(), n.clone()));
            let _ = sub.append(&it);
        }
        let _ = groups.append(&sub);
    }
    let _ = menu.append(&groups);
    let dash = Submenu::new(tr("tray.dashboard"), true);
    for (name, url) in &st.dashboards {
        let it = MenuItem::new(name, true, None);
        actions.insert(it.id().clone(), Action::Dashboard(url.clone()));
        let _ = dash.append(&it);
    }
    let _ = menu.append(&dash);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let p = CheckMenuItem::new(tr("tray.proxy"), true, st.proxy, None);
    actions.insert(p.id().clone(), Action::SysProxy);
    let _ = menu.append(&p);
    let t = CheckMenuItem::new(tr("tray.tun"), true, st.tun, None);
    actions.insert(t.id().clone(), Action::Tun);
    let _ = menu.append(&t);
    let _ = menu.append(&PredefinedMenuItem::separator());
    add_item(&menu, tr("tray.quit"), Action::Quit, &mut actions);
    *ACTIONS.lock() = Some(actions);
    menu
}

fn on_tray_thread<F: FnOnce() + Send + 'static>(f: F) {
    #[cfg(target_os = "linux")]
    {
        gtk::glib::idle_add_once(f);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = slint::invoke_from_event_loop(f);
    }
}

fn rebuild() {
    on_tray_thread(|| {
        let menu = build_menu();
        let active = STATE.lock().as_ref().map(|s| s.proxy || s.tun).unwrap_or(false);
        TRAY.with(|c| {
            if let Some(t) = c.borrow().as_ref() {
                t.set_menu(Some(Box::new(menu)));
                if cfg!(windows) {
                    let _ = t.set_icon(current_icon(active));
                }
            }
        });
    });
}

fn mutate<F: FnOnce(&mut TrayState)>(f: F) {
    {
        let mut g = STATE.lock();
        let s = g.get_or_insert_with(Default::default);
        f(s);
    }
    rebuild();
}

pub fn set_mode(m: &str) {
    let m = m.to_string();
    mutate(|s| s.mode = m);
}
pub fn set_proxy(v: bool) {
    mutate(|s| s.proxy = v);
}
pub fn set_tun(v: bool) {
    mutate(|s| s.tun = v);
}
pub fn set_groups(g: Vec<(String, Vec<(String, bool)>)>) {
    mutate(|s| s.groups = g);
}
pub fn set_dashboards(d: Vec<(String, String)>) {
    mutate(|s| s.dashboards = d);
}
pub fn set_profiles(list: &[Value]) {
    let disabled = app::pick_active(list).map(|p| util::bool_of(&p, "globalModeDisabled")).unwrap_or(false);
    mutate(|s| s.global_mode_disabled = disabled);
}
pub fn refresh_labels() {
    rebuild();
}

/// Linux: the tray needs a running GTK main loop on its own thread, and only
/// makes sense when a StatusNotifierItem host is present.
#[cfg(target_os = "linux")]
pub fn spawn_linux() {
    std::thread::spawn(|| {
        if gtk::init().is_err() {
            log::error!("gtk init failed: no tray");
            return;
        }
        create();
        gtk::main();
    });
}

/// Whether something can host the tray icon (GNOME without the AppIndicator
/// extension cannot — start-minimized must then show the window).
pub fn host_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        return std::process::Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus.NameHasOwner",
                "string:org.kde.StatusNotifierWatcher",
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("true"))
            .unwrap_or(true);
    }
    #[cfg(not(target_os = "linux"))]
    true
}
