//! Page controllers (Proxies, Profiles, Settings with its tabs).

pub mod conn;
pub mod drop;
pub mod profiles;
pub mod settings;
pub mod theme;
pub mod proxies;
pub mod rules;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use slint::{ModelRc, VecModel};

use crate::app::{self, st};
use crate::core::ConnInfo;
use crate::store::CustomDashboard;
use crate::AppWindow;

thread_local! {
    static ICONS: RefCell<HashMap<String, slint::Image>> = RefCell::new(HashMap::new());
}

/// Tabler / protocol icon by name (UI thread only).
pub fn icon(name: &str) -> slint::Image {
    ICONS.with(|c| {
        c.borrow_mut().entry(name.to_string()).or_insert_with(|| app::icon_image(name)).clone()
    })
}

pub fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

/// Updates a VecModel in place (changed rows, then growth/shrink at the end)
/// instead of swapping in a new model: a ListView keeps its row instances and
/// only re-renders what changed. Falls back to a fresh model otherwise.
pub fn sync_model<T: Clone + PartialEq + 'static>(cur: ModelRc<T>, rows: Vec<T>) -> Option<ModelRc<T>> {
    use slint::Model;
    let Some(vm) = cur.as_any().downcast_ref::<VecModel<T>>() else { return Some(model(rows)) };
    let old = vm.row_count();
    for (i, r) in rows.iter().enumerate().take(old) {
        if vm.row_data(i).as_ref() != Some(r) {
            vm.set_row_data(i, r.clone());
        }
    }
    if rows.len() > old {
        for r in rows.into_iter().skip(old) {
            vm.push(r);
        }
    } else {
        for i in (rows.len()..old).rev() {
            vm.remove(i);
        }
    }
    None
}

static TICK: AtomicU64 = AtomicU64::new(0);

fn current_menu() -> String {
    st().menu.menu.clone()
}

pub fn wire(w: &AppWindow) {
    proxies::wire(w);
    profiles::wire(w);
    settings::wire(w);
    rules::wire(w);
    conn::wire(w);
    theme::wire(w);
    drop::wire(w);
}
pub fn on_setting_tab(t: &str) {
    settings::on_show_tab(t);
    if t != "connection" {
        conn::on_hide();
    }
    conn::on_show(t);
    if t == "rule" {
        rules::on_show();
    }
}
pub fn on_open_sub(s: &str) {
    settings::on_open_sub(s);
}
pub fn on_navigate(m: &str) {
    if m != "Setting" {
        conn::on_hide();
    }
    match m {
        "Proxies" => proxies::on_show(),
        "Profiles" => profiles::on_show(),
        "Setting" => {
            let tab = st().menu.setting_tab.clone();
            on_setting_tab(&tab);
        }
        _ => {}
    }
}
pub fn on_language() {
    proxies::push();
    profiles::push();
    settings::push();
}
pub fn on_settings_changed() {
    settings::push();
}
pub fn on_profiles_changed() {
    profiles::on_profiles_changed();
}
pub fn on_backend_ready() {
    app::spawn(proxies::refresh_search());
    settings::push();
    app::spawn(settings::load_log_level());
    match current_menu().as_str() {
        "Proxies" => proxies::on_show(),
        "Profiles" => profiles::on_show(),
        // the restored page (MyBottom.vue pushes menuStore.path) may be a
        // settings tab: Connections must open its stream, Rules load its data
        "Setting" => {
            let tab = st().menu.setting_tab.clone();
            on_setting_tab(&tab);
        }
        _ => {}
    }
}
pub fn on_profile_switched() {
    proxies::on_profile_switched();
    rules::on_profile_switched();
}
pub fn on_rule_changed() {
    proxies::on_rule_changed();
}
pub fn on_service_status_changed() {
    app::spawn(settings::refresh_service());
}
pub fn on_proxy_changed() {
    app::spawn(async {
        proxies::refresh_search().await;
        proxies::load_nodes(None).await;
    });
}
pub fn on_theme_changed() {
    theme::push();
}
pub fn on_log() {
    conn::on_log();
}
pub fn on_connections(v: Value) {
    conn::on_frame(v);
}
pub fn tick(w: &AppWindow) {
    let t = TICK.fetch_add(1, Ordering::SeqCst) + 1;
    conn::sync_flags(w);
    conn::tick();
    if !st().backend_ready {
        return;
    }
    if current_menu() == "Proxies" {
        proxies::periodic(t);
    }
}
pub fn set_update_status_checking(w: &AppWindow) {
    settings::set_update_status_checking(w);
}
pub fn refresh_update_status(w: &AppWindow) {
    settings::refresh_update_status(w);
}

pub const DEFAULT_DASHBOARDS: [(&str, &str, &str); 3] = [
    ("metacubexd", "MetaCubeXD", "https://metacubex.github.io/metacubexd/#/setup?http=true&hostname=%host&port=%port&secret=%secret"),
    ("yacd", "Yacd", "https://yacd.metacubex.one/?hostname=%host&port=%port&secret=%secret"),
    ("zashboard", "Zashboard", "https://board.zash.run.place/#/setup?http=true&hostname=%host&port=%port&secret=%secret"),
];

/// (key, name, url template) — resolveDashboardOptions.
pub fn dashboard_options(custom: &[CustomDashboard]) -> Vec<(String, String, String)> {
    let mut v: Vec<(String, String, String)> =
        DEFAULT_DASHBOARDS.iter().map(|(k, n, u)| (k.to_string(), n.to_string(), u.to_string())).collect();
    for (i, d) in custom.iter().enumerate() {
        let (n, u) = (d.name.trim().to_string(), d.url.trim().to_string());
        if !n.is_empty() && !u.is_empty() {
            v.push((format!("custom-{i}"), n, u));
        }
    }
    v
}

pub fn format_dashboard_url(tpl: &str, c: &ConnInfo) -> String {
    tpl.replace("%host", &c.host).replace("%port", &c.port.to_string()).replace("%secret", &c.secret)
}
