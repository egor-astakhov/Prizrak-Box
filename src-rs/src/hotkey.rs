//! Global "show/hide window" hotkey — port of `src-wails/hotkeys.go`.
//! The accelerator string is the one MyHotkeyInput.vue records
//! ("Ctrl+Shift+X", "Cmd+Alt+ArrowUp", …).

use std::cell::RefCell;
use std::str::FromStr;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

thread_local! {
    static MANAGER: RefCell<Option<(GlobalHotKeyManager, Option<HotKey>)>> = const { RefCell::new(None) };
}

/// Converts the recorder's string to a global-hotkey accelerator.
pub fn normalize(key: &str) -> String {
    let mut key = key.trim().to_string();
    let plus_key = key.ends_with("++") || key == "+";
    if plus_key {
        key.truncate(key.len() - 1);
        key.push_str("Equal");
    }
    let parts: Vec<&str> = key.split('+').filter(|p| !p.is_empty()).collect();
    let mut out = vec![];
    for (i, p) in parts.iter().enumerate() {
        let lp = p.trim().to_lowercase();
        if i + 1 < parts.len() {
            out.push(match lp.as_str() {
                "cmd" | "command" | "meta" | "win" => "super".to_string(),
                "control" => "ctrl".to_string(),
                "option" => "alt".to_string(),
                other => other.to_string(),
            });
        } else {
            out.push(match lp.as_str() {
                "arrowup" => "ArrowUp".into(),
                "arrowdown" => "ArrowDown".into(),
                "arrowleft" => "ArrowLeft".into(),
                "arrowright" => "ArrowRight".into(),
                " " | "space" => "Space".into(),
                "esc" | "escape" => "Escape".into(),
                "pageup" => "PageUp".into(),
                "pagedown" => "PageDown".into(),
                _ => p.trim().to_string(),
            });
        }
    }
    out.join("+")
}

fn fire() {
    crate::window::toggle_visible();
}

pub fn init_on_main() {
    match GlobalHotKeyManager::new() {
        Ok(m) => MANAGER.with(|c| *c.borrow_mut() = Some((m, None))),
        Err(e) => log::warn!("global hotkeys unavailable: {e}"),
    }
    GlobalHotKeyEvent::set_event_handler(Some(|e: GlobalHotKeyEvent| {
        if e.state == HotKeyState::Pressed {
            fire();
        }
    }));
}

/// Swaps the active hotkey; "" just unregisters. Reports the outcome.
pub fn register(key: &str) {
    let acc = if key.is_empty() { String::new() } else { normalize(key) };
    let _ = slint::invoke_from_event_loop(move || {
        MANAGER.with(|c| {
            let mut g = c.borrow_mut();
            let Some((m, cur)) = g.as_mut() else { return };
            if let Some(h) = cur.take() {
                let _ = m.unregister(h);
            }
            if acc.is_empty() {
                return;
            }
            match HotKey::from_str(&acc) {
                Ok(h) => match m.register(h) {
                    Ok(()) => *cur = Some(h),
                    // MyEvent.vue only logs the shortcut:result
                    Err(e) => log::warn!("hotkey register {acc}: {e}"),
                },
                Err(e) => log::warn!("hotkey parse {acc}: {e}"),
            }
        });
    });
}

pub fn unregister_all() {
    register("");
}
