//! Theme dialog controller — port of components/menu/Skin.vue
//! (public/json/theme.json tiles, custom upload, sliders, accent swatches).

use rand::seq::SliceRandom;
use serde_json::Value;
use slint::ComponentHandle;

use crate::app::{self, save, st, ui};
use crate::i18n::{tr, tr_args};
use crate::theme;
use crate::{AppWindow, BgTile, ThemeP};

use super::{icon, model};

const THEME_JSON: &str = include_str!("../../../public/json/theme.json");
const CUSTOM_KEY: &str = "customBg";
const MAX_UPLOAD_BYTES: u64 = 1024 * 1024;

fn themes() -> Vec<Value> {
    serde_json::from_str::<Value>(THEME_JSON).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

fn icon_for(id: &str, uploaded: bool) -> &'static str {
    match id {
        "custom" => {
            if uploaded {
                "photo-check"
            } else {
                "photo-plus"
            }
        }
        "default" => "wave-sine",
        "harbor" => "anchor",
        "gradient" => "color-swatch",
        "sea" => "beach",
        "women" => "woman",
        "stars" => "stars",
        "handle" => "ghost-2",
        "comics" => "sparkles",
        "game" => "device-gamepad-2",
        "rails" => "train",
        "random" => "dice-5",
        _ => "photo",
    }
}

fn stored_custom() -> Option<String> {
    st().web.extra.get(CUSTOM_KEY).and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty())
}

fn plain_url(v: &str) -> String {
    let u = theme::extract_url(v).unwrap_or_else(|| v.to_string());
    regex::Regex::new(r"[&?]date=\d+$").unwrap().replace(&u, "").to_string()
}

/// syncThemeId: which tile matches the stored background.
fn theme_id_for(bg: &str) -> Option<String> {
    if bg.contains("/user-images/") {
        return Some("custom".into());
    }
    let url = plain_url(bg);
    for t in themes() {
        let id = t.get("id").and_then(Value::as_str).unwrap_or_default();
        if id == "custom" {
            continue;
        }
        let list: Vec<String> = match t.get("bg") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
            _ => vec![],
        };
        if list.iter().any(|b| plain_url(b) == url) {
            return Some(id.to_string());
        }
    }
    None
}

pub fn push() {
    let menu = st().menu.clone();
    if let Some(id) = theme_id_for(&menu.background) {
        if id != menu.bg_theme {
            st().menu.bg_theme = id;
            save("menu");
        }
    }
    let menu = st().menu.clone();
    let eff = theme::effective(&menu);
    let p = eff.profile.clone();
    let locked_mode = p.as_ref().and_then(|p| p.mode.clone()).map(|m| !m.is_empty()).unwrap_or(false);
    let locked_image = p.as_ref().and_then(|p| p.image.clone()).map(|m| !m.is_empty()).unwrap_or(false);
    let locked_t = p.as_ref().map(|p| p.transparency.is_some()).unwrap_or(false);
    let locked_b = p.as_ref().map(|p| p.blur.is_some()).unwrap_or(false);
    let locked_d = p.as_ref().map(|p| p.dim.is_some()).unwrap_or(false);
    let locked_accent = eff.fixed_accent.is_some();
    let has_img_theme = st().image_theme.is_some();
    let accent_note = if locked_accent {
        tr("theme.accent-from-profile")
    } else if has_img_theme {
        tr("theme.accent-from-image")
    } else {
        tr("theme.accent-fallback")
    };
    let uploaded = stored_custom().is_some();
    let tiles: Vec<(String, String, &'static str, bool, bool, bool)> = themes()
        .iter()
        .map(|t| {
            let id = t.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let custom = id == "custom";
            let up = custom && uploaded;
            (id.clone(), tr(&format!("bg.{id}")), icon_for(&id, up), custom, up, menu.bg_theme == id)
        })
        .collect();
    ui(move |w| {
        let tp = w.global::<ThemeP>();
        tp.set_has_profile_theme(menu.profile_theme.is_some());
        tp.set_profile_desc(tr_args("theme.profile.desc", &[("name", menu.profile_theme_title.clone())]).into());
        tp.set_use_profile_theme(menu.use_profile_theme);
        tp.set_mode(eff.pref.clone().into());
        tp.set_mode_locked(locked_mode);
        tp.set_use_image(eff.use_image);
        tp.set_image_locked(locked_image);
        tp.set_trans(eff.trans.round() as i32);
        tp.set_blur(eff.blur.round() as i32);
        tp.set_dim(eff.dim.round() as i32);
        tp.set_trans_locked(locked_t);
        tp.set_blur_locked(locked_b);
        tp.set_dim_locked(locked_d);
        tp.set_accent_note(accent_note.into());
        tp.set_accent_locked(locked_accent);
        tp.set_accent_value(menu.accent.clone().into());
        tp.set_tiles(model(
            tiles
                .into_iter()
                .map(|(id, label, ic, custom, up, on)| BgTile { id: id.into(), label: label.into(), icon: icon(ic), custom, uploaded: up, on })
                .collect(),
        ));
    });
}

pub fn change_background(id: &str) {
    let locked = {
        let s = st();
        s.menu.use_profile_theme && s.menu.profile_theme.as_ref().and_then(|p| p.image.clone()).map(|i| !i.is_empty()).unwrap_or(false)
    };
    if locked {
        return;
    }
    ui(|w| w.global::<ThemeP>().set_bg_error("".into()));
    if id == "custom" {
        match stored_custom() {
            Some(v) => {
                app::menu_mut(|m| {
                    m.background = v;
                    m.bg_theme = "custom".into();
                });
                push();
            }
            None => upload(),
        }
        return;
    }
    let Some(t) = themes().into_iter().find(|t| t.get("id").and_then(Value::as_str) == Some(id)) else { return };
    let url = match t.get("bg") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => {
            let list: Vec<String> = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            let pick = list.choose(&mut rand::thread_rng()).cloned().unwrap_or_default();
            if t.get("rand").and_then(Value::as_bool).unwrap_or(false) {
                format!("url('{}&date={}')", pick, crate::util::now_ms() as i64)
            } else {
                pick
            }
        }
        _ => return,
    };
    let id = id.to_string();
    app::menu_mut(move |m| {
        m.background = url;
        m.bg_theme = id;
    });
    push();
}

fn upload() {
    app::spawn(async {
        let Some(file) = rfd::AsyncFileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
            .pick_file()
            .await
        else {
            return;
        };
        let path = file.path().to_path_buf();
        if std::fs::metadata(&path).map(|m| m.len() > MAX_UPLOAD_BYTES).unwrap_or(true) {
            ui(|w| w.global::<ThemeP>().set_bg_error(tr("bg.too-large").into()));
            return;
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .filter(|e| e.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or_else(|| "png".into());
        let name = format!("custom-{:032x}.{}", rand::random::<u128>(), ext);
        let dest = crate::paths::custom_images_dir().join(&name);
        if std::fs::copy(&path, &dest).is_err() {
            ui(|w| w.global::<ThemeP>().set_bg_error(tr("bg.upload-failed").into()));
            return;
        }
        // drop the previous upload
        if let Some(prev) = stored_custom() {
            if let Some(n) = prev.split("/user-images/").nth(1) {
                let n = n.trim_end_matches(['\'', '"', ')']);
                if !n.contains('/') && n != name {
                    let _ = std::fs::remove_file(crate::paths::custom_images_dir().join(n));
                }
            }
        }
        let value = format!("url('/user-images/{name}')");
        st().web.extra.insert(CUSTOM_KEY.into(), Value::String(value.clone()));
        save("web");
        app::menu_mut(move |m| {
            m.background = value;
            m.bg_theme = "custom".into();
        });
        push();
    });
}

pub fn wire(w: &AppWindow) {
    let tp = w.global::<ThemeP>();
    tp.on_opened(push);
    tp.on_set_use_profile_theme(|v| {
        app::menu_mut(|m| m.use_profile_theme = v);
        push();
    });
    tp.on_set_mode(|v| {
        app::menu_mut(|m| m.theme_pref = v.to_string());
        push();
    });
    tp.on_set_use_image(|v| {
        app::menu_mut(|m| m.use_bg_image = v);
        push();
    });
    tp.on_pick_tile(|id| change_background(&id));
    tp.on_upload(|_| upload());
    tp.on_set_slider(|k, v| {
        let v = v as f64;
        match k.as_str() {
            "uiTrans" => st().menu.ui_trans = v,
            "uiBlur" => st().menu.ui_blur = v,
            "bgDim" => st().menu.bg_dim = v,
            _ => {}
        }
        save("menu");
        ui(move |w| {
            let tp = w.global::<ThemeP>();
            match k.as_str() {
                "uiTrans" => tp.set_trans(v as i32),
                "uiBlur" => tp.set_blur(v as i32),
                _ => tp.set_dim(v as i32),
            }
        });
        app::apply_theme();
    });
    tp.on_set_accent(|c| {
        app::menu_mut(|m| m.accent = c.to_string());
        push();
    });
    tp.on_reset(|| {
        ui(|w| w.global::<ThemeP>().set_bg_error("".into()));
        app::menu_mut(|m| {
            m.theme_pref = "auto".into();
            m.use_bg_image = true;
            m.ui_trans = 78.0;
            m.ui_blur = 1.0;
            m.bg_dim = 18.0;
            m.bg_theme = "default".into();
            m.background = crate::store::DEFAULT_BACKGROUND.into();
        });
        push();
    });
}
