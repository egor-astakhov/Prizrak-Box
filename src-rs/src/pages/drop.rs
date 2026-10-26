//! "Open file" overlay + age key prompt — port of components/MyDrop.vue.

use std::path::PathBuf;
use std::time::Duration;

use slint::ComponentHandle;

use crate::app::{self, spawn, ui};
use crate::i18n::tr;
use crate::{App, AppWindow, DropP};

static PENDING: parking_lot::Mutex<Option<(String, String)>> = parking_lot::Mutex::new(None);
/// Files of the drop in progress (winit reports one DroppedFile per file).
static DROPPED: parking_lot::Mutex<Vec<PathBuf>> = parking_lot::Mutex::new(Vec::new());

/// MyDrop.vue handleDrop: one file only; .age files ask for their key first.
pub fn import_dropped(files: Vec<PathBuf>) {
    ui(|w| w.global::<App>().set_drop_open(false));
    if files.len() > 1 {
        app::warning(tr("drag.size"));
        return;
    }
    let Some(path) = files.into_iter().next() else { return };
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let content = match std::fs::read(&path) {
        Ok(b) => String::from_utf8_lossy(&b).to_string(),
        Err(e) => {
            log::warn!("reading dropped {}: {e}", path.display());
            app::error(tr("drag.error"));
            return;
        }
    };
    if name.to_lowercase().ends_with(".age") {
        ask_age_key(content, name);
    } else {
        spawn(app::import_profile_file(content, name, String::new()));
    }
}

/// Dragging files over the window opens the drop overlay (dragenter on
/// document.body); dropping them imports the profile.
fn wire_file_drop(w: &AppWindow) {
    use slint::winit_030::winit::event::WindowEvent;
    use slint::winit_030::{EventResult, WinitWindowAccessor};
    w.window().on_winit_window_event(|_, ev| {
        match ev {
            WindowEvent::HoveredFile(_) => ui(|w| w.global::<App>().set_drop_open(true)),
            WindowEvent::DroppedFile(path) => {
                let first = {
                    let mut d = DROPPED.lock();
                    d.push(path.clone());
                    d.len() == 1
                };
                if first {
                    slint::Timer::single_shot(Duration::from_millis(150), || {
                        let files = std::mem::take(&mut *DROPPED.lock());
                        import_dropped(files);
                    });
                }
            }
            _ => {}
        }
        EventResult::Propagate
    });
}

pub fn ask_age_key(content: String, title: String) {
    *PENDING.lock() = Some((content, title));
    ui(|w| {
        let d = w.global::<DropP>();
        d.set_age_key("".into());
        d.set_age_open(true);
    });
}

pub fn wire(w: &AppWindow) {
    wire_file_drop(w);
    let d = w.global::<DropP>();
    d.on_open_picker(|| spawn(super::settings::import_file(true)));
    d.on_age_confirm(|| {
        ui(|w| {
            let d = w.global::<DropP>();
            let key = d.get_age_key().trim().to_string();
            d.set_age_open(false);
            if let Some((content, title)) = PENDING.lock().take() {
                spawn(app::import_profile_file(content, title, key));
            }
        })
    });
    d.on_age_cancel(|| {
        PENDING.lock().take();
        ui(|w| w.global::<DropP>().set_age_open(false));
    });
}
