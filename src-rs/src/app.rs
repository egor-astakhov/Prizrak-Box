//! Application controller: the Rust side of App.vue, MyEvent.vue, MyProxy.vue,
//! MyRule.vue, MyNav/MyBottom/Off/Language, the Home page and the global UI
//! services (toasts, loading overlay, confirm dialog).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::api::Api;
use crate::core::{ConnInfo, Core};
use crate::i18n::{self, tr, tr_args};
use crate::store::*;
use crate::theme::{self, ImageTheme};
use crate::tun::Tun;
use crate::util::{self, str_of};
use crate::*;

pub const APP_VERSION: &str = env!("PRIZRAK_APP_VERSION");
const RELEASES_URL: &str = "https://github.com/legiz-ru/Prizrak-Box/releases/latest";

pub struct State {
    pub menu: MenuState,
    pub setting: SettingState,
    pub proxies: ProxiesState,
    pub home: HomeState,
    pub onboarding: OnboardingState,
    pub updates: UpdatesState,
    pub web: WebState,
    pub conns: ConnectionsState,
    pub profiles: Vec<Value>,
    /// The profile list came from px at least once (Home must not show the
    /// welcome screen while the backend is still starting).
    pub profiles_loaded: bool,
    pub f_profile: Option<Value>,
    pub toasts: Vec<(i32, String, String)>,
    pub toast_seq: i32,
    pub loading: i32,
    pub loading_text: String,
    pub image_theme: Option<ImageTheme>,
    pub sys_dark: bool,
    /// First launch of the Rust shell: the default TUN stack (Mips) replaces
    /// whatever px or the old Electron/Wails settings had, once.
    pub force_default_stack: bool,
    pub bg_seq: u64,
    pub bg_key: String,
    pub start_time: i64,
    pub tun_on: bool,
    pub confirm: Option<tokio::sync::oneshot::Sender<bool>>,
    pub switching_profiles: bool,
    pub admin: bool,
    pub version: String,
    pub mixed_port: String,
    pub logo_key: String,
    pub dark: bool,
    /// The dark-bg flag file already matches `dark`.
    pub dark_flag_synced: bool,
    pub backend_ready: bool,
    pub hotkey_registered: bool,
    pub deeplink_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    pub sub_alert_renew: String,
    pub conn_show_closed: bool,
}

impl State {
    pub fn conns_show_closed(&self) -> bool {
        self.conn_show_closed
    }
}

pub struct Ctx {
    pub ui: slint::Weak<AppWindow>,
    pub rt: tokio::runtime::Handle,
    pub api: Arc<Api>,
    pub core: Arc<Core>,
    pub tun: Arc<Tun>,
    pub store: Store,
    pub st: Mutex<State>,
}

static CTX: OnceLock<Arc<Ctx>> = OnceLock::new();

pub fn ctx() -> &'static Arc<Ctx> {
    CTX.get().expect("app context")
}

/// Runs `f` on the UI thread with the window.
pub fn ui<F: FnOnce(AppWindow) + Send + 'static>(f: F) {
    let _ = ctx().ui.upgrade_in_event_loop(f);
}

pub fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
    ctx().rt.spawn(f);
}

pub fn st() -> parking_lot::MutexGuard<'static, State> {
    ctx().st.lock()
}

pub fn save(key: &str) {
    let s = st();
    let c = &ctx().store;
    match key {
        "menu" => c.set("menu", &s.menu),
        "setting" => c.set("setting", &s.setting),
        "proxies" => c.set("proxies", &s.proxies),
        "home" => c.set("home", &s.home),
        "onboarding" => c.set("onboarding", &s.onboarding),
        "updates" => c.set("updates", &s.updates),
        "web" => c.set("web", &s.web),
        "connections" => c.set("connections", &s.conns),
        _ => {}
    }
}

pub fn username() -> String {
    let n = whoami::username();
    match n.rsplit_once(['\\', '/']) {
        Some((_, last)) => last.to_string(),
        None => n,
    }
}

pub fn os_label() -> String {
    let name = if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "MacOS"
    } else {
        "Linux"
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{name} {arch}")
}

// ---------------------------------------------------------------- services

pub fn toast(kind: &str, text: impl Into<String>) {
    let text: String = text.into();
    if text.is_empty() {
        return;
    }
    let text = if kind == "error" { normalize_error(&text) } else { text };
    let id = {
        let mut s = st();
        s.toasts.retain(|(_, k, t)| !(k == kind && *t == text));
        s.toast_seq += 1;
        let id = s.toast_seq;
        s.toasts.push((id, kind.to_string(), text));
        let len = s.toasts.len();
        if len > 4 {
            s.toasts.drain(0..len - 4);
        }
        id
    };
    push_toasts();
    spawn(async move {
        tokio::time::sleep(Duration::from_millis(2600)).await;
        dismiss_toast(id);
    });
}

fn normalize_error(text: &str) -> String {
    let mut out = text.to_string();
    for (pat, key, fb) in [
        ("请求失败", "errors.request-failed", "Request failed"),
        ("发送请求失败", "errors.request-send-failed", "Failed to send request"),
        ("创建请求失败", "errors.request-create-failed", "Failed to create request"),
        ("响应内容为空", "errors.response-empty", "Response body is empty"),
        ("未知原因", "errors.unknown-reason", "Unknown reason"),
    ] {
        if out.contains(pat) {
            let v = tr(key);
            out = out.replace(pat, if v == key { fb } else { &v });
        }
    }
    out
}

pub fn success(t: impl Into<String>) {
    toast("success", t)
}
pub fn warning(t: impl Into<String>) {
    toast("warning", t)
}
pub fn error(t: impl Into<String>) {
    toast("error", t)
}

pub fn dismiss_toast(id: i32) {
    st().toasts.retain(|(i, _, _)| *i != id);
    push_toasts();
}

fn push_toasts() {
    let list: Vec<(i32, String, String)> = st().toasts.clone();
    ui(move |w| {
        let model: Vec<Toast> = list
            .into_iter()
            .map(|(id, kind, text)| Toast { id, kind: kind.into(), text: text.into() })
            .collect();
        w.global::<App>().set_toasts(ModelRc::new(VecModel::from(model)));
    });
}

pub struct LoadingGuard;

impl Drop for LoadingGuard {
    fn drop(&mut self) {
        let (count, text) = {
            let mut s = st();
            s.loading = (s.loading - 1).max(0);
            if s.loading == 0 {
                s.loading_text.clear();
            }
            (s.loading, s.loading_text.clone())
        };
        ui(move |w| {
            w.global::<App>().set_loading(count > 0);
            w.global::<App>().set_loading_text(text.into());
        });
    }
}

/// `pLoad`: blocking overlay while the guard lives.
pub fn loading(text: impl Into<String>) -> LoadingGuard {
    let text: String = text.into();
    {
        let mut s = st();
        s.loading += 1;
        s.loading_text = text.clone();
    }
    ui(move |w| {
        w.global::<App>().set_loading(true);
        w.global::<App>().set_loading_text(text.into());
    });
    LoadingGuard
}

/// `confirm()` from components/ui/services.ts.
pub async fn confirm(title: String, text: String, ok: String, icon: &'static str) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if let Some(prev) = st().confirm.replace(tx) {
        let _ = prev.send(false);
    }
    ui(move |w| {
        let a = w.global::<App>();
        a.set_confirm_title(title.into());
        a.set_confirm_text(text.into());
        a.set_confirm_ok(ok.into());
        a.set_confirm_icon(icon_image(icon));
        a.set_confirm_open(true);
    });
    rx.await.unwrap_or(false)
}

pub fn icon_image(name: &str) -> slint::Image {
    assets::icon_svg(name)
        .and_then(|b| slint::Image::load_from_svg_data(b).ok())
        .unwrap_or_default()
}

pub fn open_url(url: &str) {
    let u = url.trim().to_string();
    if u.is_empty() {
        return;
    }
    if let Err(e) = open::that_detached(&u) {
        log::warn!("open url failed: {e}");
    }
}

// ---------------------------------------------------------------- init

pub fn init(ui_handle: &AppWindow, rt: tokio::runtime::Handle, core: Arc<Core>, tun: Arc<Tun>) {
    let store = Store::open();
    let mut menu: MenuState = store.get("menu");
    let setting: SettingState = store.get("setting");
    let proxies: ProxiesState = store.get("proxies");
    let mut home: HomeState = store.get("home");
    let onboarding: OnboardingState = store.get("onboarding");
    let mut updates: UpdatesState = store.get("updates");
    let web: WebState = store.get("web");
    let conns: ConnectionsState = store.get("connections");
    if menu.language.is_empty() {
        menu.language = i18n::detect_language();
    }
    i18n::set_lang(&menu.language);
    home.os = os_label();
    home.start_time = util::now_ms() as i64;
    updates.current_version = APP_VERSION.to_string();

    let ctx = Arc::new(Ctx {
        ui: ui_handle.as_weak(),
        rt,
        api: Arc::new(Api::new()),
        core,
        tun,
        store,
        st: Mutex::new(State {
            menu,
            setting,
            proxies,
            home,
            onboarding,
            updates,
            web,
            conns,
            profiles: vec![],
            profiles_loaded: false,
            f_profile: None,
            toasts: vec![],
            toast_seq: 0,
            loading: 0,
            loading_text: String::new(),
            image_theme: None,
            sys_dark: theme::system_dark(),
            force_default_stack: !paths::stack_default_applied(),
            bg_seq: 0,
            bg_key: String::new(),
            start_time: util::now_ms() as i64,
            tun_on: false,
            confirm: None,
            switching_profiles: false,
            admin: false,
            version: String::new(),
            mixed_port: String::new(),
            logo_key: String::new(),
            dark: true,
            dark_flag_synced: false,
            backend_ready: false,
            hotkey_registered: false,
            deeplink_cancel: None,
            sub_alert_renew: String::new(),
            conn_show_closed: false,
        }),
    });
    let _ = CTX.set(ctx);
    save("home");
    wire(ui_handle);
    apply_static(ui_handle);
    pages::wire(ui_handle);
    crate::backdrop::wire(ui_handle);
    apply_theme();
}

/// Values that do not depend on the backend.
fn apply_static(w: &AppWindow) {
    let th = w.global::<Theme>();
    th.set_is_mac(cfg!(target_os = "macos"));
    th.set_is_windows(cfg!(windows));
    th.set_is_linux(cfg!(target_os = "linux"));
    // First frame in the last known mode (the Wails shell's dark-bg flag).
    th.set_dark(paths::dark_background());
    let s = st();
    let a = w.global::<App>();
    a.set_language(s.menu.language.clone().into());
    w.global::<T>().set_lang(s.menu.language.clone().into());
    a.set_show_direct(tr("lang") != "ru");
    a.set_rule(s.menu.rule.clone().into());
    a.set_proxy_on(s.menu.proxy);
    a.set_tun_on(false);
    a.set_setting_tab(s.menu.setting_tab.clone().into());
    a.set_os_label(s.home.os.clone().into());
    // Restore last page (MyBottom.vue onBeforeMount → menuStore.path)
    let menu = route_of(&s.menu.path).unwrap_or_else(|| s.menu.menu.clone());
    a.set_menu(menu.into());
    a.set_top_logo(default_logo());
    let hp = w.global::<HomeP>();
    hp.set_os(s.home.os.clone().into());
    hp.set_startup(s.setting.startup);
    apply_ip(w, &s.home.ip);
}

fn route_of(path: &str) -> Option<String> {
    let p = path.trim_start_matches('/');
    let first = p.split('/').next().unwrap_or("");
    match first {
        "Home" | "" => Some("Home".into()),
        "Setting" | "Rule" | "Connection" | "Log" => Some("Setting".into()),
        "Proxies" => Some("Proxies".into()),
        "Profiles" => Some("Profiles".into()),
        _ => None,
    }
}

pub fn default_logo() -> slint::Image {
    static LOGO: OnceLock<image::RgbaImage> = OnceLock::new();
    let img = LOGO.get_or_init(|| {
        image::load_from_memory(include_bytes!("../assets/appicon.png"))
            .map(|i| i.resize(128, 128, image::imageops::FilterType::Lanczos3).to_rgba8())
            .unwrap_or_else(|_| image::RgbaImage::new(1, 1))
    });
    theme::to_slint(img)
}

fn wire(w: &AppWindow) {
    let a = w.global::<App>();
    let t = w.global::<T>();
    t.on_translate(|lang, key| i18n::tr_lang(&lang, &key).into());
    t.on_translate1(|lang, key, name, value| i18n::tr_lang(&lang, &key).replace(&format!("{{{name}}}"), &value).into());
    w.global::<CodeHelper>().on_gutter(|text| {
        let n = text.split('\n').count().max(1);
        (1..=n).map(|i| i.to_string()).collect::<Vec<_>>().join("\n").into()
    });
    w.global::<CodeHelper>().on_digits(|text| (text.split('\n').count().max(1)).to_string().len() as i32);
    w.global::<SelectHelper>().on_label_of(|opts, cur, ph| {
        for o in opts.iter() {
            if o.value == cur {
                return o.label;
            }
        }
        ph
    });

    a.on_navigate(|m| navigate(&m));
    a.on_set_setting_tab(|t| {
        st().menu.setting_tab = t.to_string();
        save("menu");
        let tt = t.clone();
        ui(move |w| {
            w.global::<App>().set_setting_tab(tt);
            w.global::<App>().set_setting_sub("".into());
        });
        pages::on_setting_tab(&t);
    });
    a.on_open_sub(|s| {
        let s2 = s.clone();
        ui(move |w| {
            w.global::<App>().set_setting_sub(s2);
        });
        pages::on_open_sub(&s);
    });
    a.on_set_rule(|r| set_rule(r.to_string()));
    a.on_toggle_proxy(|| spawn(async { proxy_switch().await }));
    a.on_toggle_tun(|| spawn(async { tun_switch().await }));
    a.on_install_service(|| spawn(async { install_service_handler().await }));
    a.on_run_tun_without_service(|| {
        ui(|w| w.global::<App>().set_admin_choice_dialog(false));
        spawn(async {
            if !selected_profile_ok().await {
                tray::set_tun(false);
                return;
            }
            enable_tun().await;
        })
    });
    a.on_win_hide(|| window::hide());
    a.on_win_min(|| window::minimize());
    a.on_win_max(|| window::toggle_maximize());
    a.on_win_close(|| window::close_to_tray());
    a.on_start_drag(|| window::start_drag());
    a.on_start_resize(|d| window::start_resize(d));
    a.on_titlebar_double_click(|| {
        if !cfg!(target_os = "macos") {
            window::toggle_maximize()
        }
    });
    a.on_quit(|| spawn(async { quit_via_api().await }));
    a.on_set_language(|l| set_language(&l));
    a.on_dismiss_toast(dismiss_toast);
    a.on_confirm_result(|ok| {
        ui(|w| w.global::<App>().set_confirm_open(false));
        if let Some(tx) = st().confirm.take() {
            let _ = tx.send(ok);
        }
    });
    a.on_update_dismiss(|| {
        {
            let mut s = st();
            if let Some(t) = s.updates.latest_tag.clone() {
                s.updates.dismissed_tag = Some(t);
            }
        }
        save("updates");
        refresh_update_dialog();
    });
    a.on_update_open(|| {
        let url = st().updates.latest_url.clone().unwrap_or_else(|| RELEASES_URL.into());
        open_url(&url);
        {
            let mut s = st();
            if let Some(t) = s.updates.latest_tag.clone() {
                s.updates.dismissed_tag = Some(t);
            }
        }
        save("updates");
        refresh_update_dialog();
    });
    a.on_hwid_close(|| ui(|w| w.global::<App>().set_hwid_error("".into())));
    a.on_hwid_open_support(|| {
        ui(|w| {
            let url = w.global::<App>().get_hwid_support_url().to_string();
            open_url(&url);
            w.global::<App>().set_hwid_error("".into());
        })
    });
    a.on_sub_alert_close(|| ui(|w| w.global::<App>().set_sub_alert_open(false)));
    a.on_sub_alert_renew_clicked(|| {
        let url = st().sub_alert_renew.clone();
        open_url(&url);
        ui(|w| w.global::<App>().set_sub_alert_open(false));
    });
    a.on_first_profile_ok(|| {
        st().onboarding.first_profile_info_shown = true;
        save("onboarding");
        ui(|w| w.global::<App>().set_first_profile_info(false));
    });
    a.on_deeplink_cancel_clicked(|| {
        if let Some(tx) = st().deeplink_cancel.take() {
            let _ = tx.send(());
        }
    });
    a.on_open_url(|u| open_url(&u));

    // Home page
    let hp = w.global::<HomeP>();
    hp.on_go_renew(|| open_active("renewUrl"));
    hp.on_go_home(|| open_active("home"));
    hp.on_go_support(|| open_active("support"));
    hp.on_switch_profiles(|| navigate("Profiles"));
    hp.on_refresh(|| spawn(async { refresh_active_profile().await }));
    hp.on_open_announce(|| open_active("announceUrl"));
    hp.on_refresh_ip(|| spawn(async { load_ip(false).await }));
    hp.on_open_ip(|| {
        let ip = st().home.ip.query.clone();
        if !ip.is_empty() {
            open_url(&format!("https://ipinfo.io/{}", api::enc(&ip)));
        }
    });
    hp.on_open_asn(|| {
        let asn = st().home.ip.asn.clone();
        if !asn.is_empty() {
            open_url(&format!("https://ipinfo.io/{}", api::enc(&asn)));
        }
    });

    // Add profile (AddProfileDialog / WelcomeScreen / ProfileToolbar)
    let ap = w.global::<AddProfile>();
    ap.on_open_manual(|| {
        ui(|w| {
            let ap = w.global::<AddProfile>();
            ap.set_content("".into());
            ap.set_age_key("".into());
            ap.set_use_age(false);
            ap.set_open(true);
        })
    });
    ap.on_paste(|| {
        // Off the UI thread: a Wayland data-control read takes seconds on KDE
        // (2.5 s measured), which froze the window on every paste.
        spawn(async {
            let text = tokio::task::spawn_blocking(clipboard_text).await.unwrap_or_default();
            // Profiles.vue handlePaste opens the dialog whatever the clipboard holds
            if text.trim().is_empty() && st().menu.menu != "Profiles" {
                if st().profiles.is_empty() {
                    error(tr("onboarding.active-profile.clipboard-empty"));
                } else {
                    warning(tr("onboarding.active-profile.clipboard-empty"));
                }
                return;
            }
            ui(move |w| {
                let ap = w.global::<AddProfile>();
                ap.set_content(text.into());
                ap.set_age_key("".into());
                ap.set_use_age(false);
                ap.set_open(true);
            });
        });
    });
    ap.on_open_file(|| ui(|w| w.global::<App>().set_drop_open(true)));
    ap.on_submit(|| {
        ui(|w| {
            let ap = w.global::<AddProfile>();
            let content = ap.get_content().to_string();
            let age = if ap.get_use_age() { ap.get_age_key().to_string() } else { String::new() };
            // Profiles.vue add() only appends; WelcomeScreen / ProfileToolbar
            // switch to the new profile.
            if st().menu.menu == "Profiles" {
                spawn(async move { pages::profiles::add(content, age).await });
            } else {
                spawn(async move { add_profile_from_input(content, age).await });
            }
        })
    });
}

pub fn clipboard_text() -> String {
    arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_else(|e| {
        log::info!("clipboard: {e}");
        String::new()
    })
}

/// Copies `text` and reports it with a toast (`ok_msg` on success); the write
/// runs off the UI thread for the same reason as the paste above (seconds on
/// KDE Wayland).
pub fn copy_text(text: String, ok_msg: String) {
    spawn(async move {
        let ok = tokio::task::spawn_blocking(move || arboard::Clipboard::new().and_then(|mut c| c.set_text(text)).is_ok())
            .await
            .unwrap_or(false);
        if ok {
            success(ok_msg);
        } else {
            error(tr("copy.fail"));
        }
    });
}

fn open_active(key: &str) {
    let url = active_profile().map(|p| str_of(&p, key)).unwrap_or_default();
    open_url(&url);
}

// ---------------------------------------------------------------- navigation

pub fn navigate(menu: &str) {
    {
        let mut s = st();
        s.menu.menu = menu.to_string();
        s.menu.path = format!("/{menu}");
    }
    save("menu");
    let m = menu.to_string();
    ui(move |w| {
        w.global::<App>().set_menu(m.clone().into());
        w.global::<App>().set_setting_sub("".into());
        tooltip_hide(&w);
    });
    pages::on_navigate(menu);
}

pub fn tooltip_hide(w: &AppWindow) {
    w.global::<Tip>().set_visible(false);
}

pub fn set_language(lang: &str) {
    let lang = i18n::normalize(lang);
    i18n::set_lang(&lang);
    st().menu.language = lang.clone();
    save("menu");
    ui(move |w| {
        w.global::<App>().set_language(lang.clone().into());
        w.global::<T>().set_lang(lang.into());
        w.global::<App>().set_show_direct(tr("lang") != "ru");
    });
    tray::refresh_labels();
    refresh_home();
    pages::on_language();
}

// ---------------------------------------------------------------- backend

/// Called (off the UI thread) once px reported its connection info.
pub fn on_backend_ready(info: ConnInfo) {
    ctx().api.set_conn(info);
    st().backend_ready = true;
    ui(|w| w.global::<App>().set_backend_ready(true));
    spawn(async {
        update_http_client_config().await;
        after_mount().await;
    });
}

pub fn on_backend_error(err: String) {
    log::error!("backend failed: {err}");
    error(err);
}

/// Applies fresh connection info after a px restart (backendConn.ts).
pub fn apply_backend_conn(info: ConnInfo) -> bool {
    if info.port == 0 {
        return false;
    }
    let changed = ctx().api.conn() != info;
    ctx().api.set_conn(info);
    if changed {
        ws::reconnect_all();
    }
    true
}

pub async fn restart_backend_and_sync() -> bool {
    let tun = ctx().tun.clone();
    let res = tokio::task::spawn_blocking(move || tun.restart_backend()).await;
    match res {
        Ok(Ok(info)) => apply_backend_conn(info),
        _ => false,
    }
}

pub async fn wait_backend_ready(timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if ctx().api.version().await.is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

async fn update_http_client_config() {
    let api = &ctx().api;
    let Ok(version) = api.version().await else { return };
    let hwid = st().setting.hwid;
    match api
        .update_http_client_config(json!({
            "enableHWID": hwid, "version": version, "deviceOS": "", "deviceOSVer": "", "deviceModel": ""
        }))
        .await
    {
        Ok(d) => {
            {
                let mut s = st();
                s.setting.hwid_headers = HwidHeaders {
                    hwid: str_of(&d, "hwid"),
                    os: str_of(&d, "os"),
                    os_version: str_of(&d, "osVersion"),
                    model: str_of(&d, "model"),
                };
            }
            save("setting");
        }
        Err(e) => log::warn!("httpClientConfig: {e}"),
    }
}

pub fn set_hwid(v: bool) {
    st().setting.hwid = v;
    save("setting");
    spawn(async { update_http_client_config().await });
}

/// Everything the Vue components did on mount once px answered.
async fn after_mount() {
    let api = ctx().api.clone();
    // App.vue: profiles + logs websocket
    load_profiles().await;
    ws::start_logs();
    ws::start_traffic();
    // MyEvent.vue onMounted
    let (start_min, startup, sc, key) = {
        let s = st();
        (s.setting.start_minimized, s.setting.startup, s.setting.sc_switch, s.setting.sc_switch_key.clone())
    };
    paths::set_start_minimized(start_min);
    autostart::apply(startup);
    if sc {
        hotkey::register(&key);
    }
    if let Ok(m) = api.mihomo().await {
        {
            let mut s = st();
            s.menu.rule = str_of(&m, "mode");
            if s.menu.rule.is_empty() {
                s.menu.rule = "rule".into();
            }
            s.menu.proxy = m.get("proxy").and_then(Value::as_bool).unwrap_or(false);
            if let Some(p) = m.get("port").and_then(Value::as_i64) {
                s.setting.port = p;
            }
            let b = str_of(&m, "bindAddress");
            if !b.is_empty() {
                s.setting.bind_address = b;
            }
            let st_ = str_of(&m, "stack");
            if s.force_default_stack {
                s.setting.stack = DEFAULT_STACK.into();
            } else if !st_.is_empty() {
                s.setting.stack = st_;
            }
            s.setting.dns = m.get("dns").and_then(Value::as_bool).unwrap_or(s.setting.dns);
            s.setting.ipv6 = m.get("ipv6").and_then(Value::as_bool).unwrap_or(s.setting.ipv6);
        }
        save("menu");
        save("setting");
        // First Rust launch: hand the default stack to px (MyTun.vue's
        // update), so the core runs with it and remembers it; from then on
        // the user's own choice in Settings → Core is kept.
        if std::mem::replace(&mut st().force_default_stack, false) {
            if str_of(&m, "stack") == DEFAULT_STACK {
                paths::set_stack_default_applied();
            } else {
                let tun = st().menu.tun;
                if tun {
                    let _ = api.update_configs(json!({"tun": {"enable": false}})).await;
                }
                if api.update_configs(json!({"tun": {"enable": tun, "stack": DEFAULT_STACK}})).await.is_ok() {
                    update_mihomo();
                    paths::set_stack_default_applied();
                }
            }
        }
        let (rule, proxy) = {
            let s = st();
            (s.menu.rule.clone(), s.menu.proxy)
        };
        ui(move |w| {
            w.global::<App>().set_rule(rule.into());
            w.global::<App>().set_proxy_on(proxy);
        });
        pages::on_settings_changed();
    }
    tray::set_mode(&st().menu.rule.clone());
    tray::set_proxy(st().menu.proxy);
    update_proxy_groups_in_tray().await;
    tray::set_dashboards(dashboards());

    // MyProxy.vue onMounted
    let (proxy_on, tun_desired) = {
        let s = st();
        (s.menu.proxy, s.menu.tun)
    };
    paths::set_tun_desired(tun_desired);
    if proxy_on {
        api.wait_running(Duration::from_secs(5)).await;
        let mode = st().setting.system_proxy_mode;
        let _ = apply_system_proxy_mode(mode, false).await;
    }
    if tun_desired {
        api.wait_running(Duration::from_secs(5)).await;
        let mut privileged = api.admin().await;
        if !privileged {
            let tun = ctx().tun.clone();
            let installed = tokio::task::spawn_blocking(move || tun.status().installed).await.unwrap_or(false);
            // after a service failure in this run px was started directly on
            // purpose; asking the service again would only stop it for nothing
            if installed && !ctx().tun.service_px_failed() && restart_backend_and_sync().await {
                wait_backend_ready(Duration::from_secs(10)).await;
                privileged = api.admin().await;
            }
        }
        if privileged {
            if selected_profile_ok_silent().await {
                st().menu.tun = false;
                st().tun_on = false;
                enable_tun().await;
            } else {
                set_tun_state(false);
            }
        } else {
            set_tun_state(false);
            warning(tr("tun-unavailable"));
        }
    }

    // MyIp.vue onMounted
    if let Ok(v) = api.version().await {
        st().version = v;
    }
    if let Ok(c) = api.configs().await {
        st().mixed_port = c.get("mixed-port").map(|v| v.to_string()).unwrap_or_default();
    }
    st().admin = api.admin().await;
    refresh_home();
    load_ip(true).await;
    // main.ts setupUpdateChecker
    spawn(async {
        loop {
            check_for_updates().await;
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
    pages::on_backend_ready();
}

// ---------------------------------------------------------------- profiles

pub fn active_profile() -> Option<Value> {
    pick_active(&st().profiles)
}

pub fn pick_active(list: &[Value]) -> Option<Value> {
    list.iter()
        .find(|p| util::bool_of(p, "primary"))
        .or_else(|| list.iter().find(|p| util::bool_of(p, "selected")))
        .or_else(|| list.first())
        .cloned()
}

/// App.vue loadProfiles + Home.vue getProfileList.
pub async fn load_profiles() {
    let api = ctx().api.clone();
    match api.profiles().await {
        Ok(list) => {
            set_profiles(list.clone());
            let enabled = st().setting.notify_subscription_alerts;
            check_pending_alerts(list, enabled).await;
        }
        Err(e) => log::warn!("load profiles: {e}"),
    }
}

/// Central "profiles" event: header logo/title, theme, tray, Home, Profiles.
pub fn set_profiles(list: Vec<Value>) {
    let was_empty = {
        let s = st();
        !s.onboarding.has_ever_had_profiles
    };
    let has = !list.is_empty();
    st().profiles_loaded = true;
    {
        let mut s = st();
        s.profiles = list.clone();
        if s.f_profile.as_ref().map(|p| str_of(p, "id").is_empty()).unwrap_or(true) {
            s.f_profile = pick_active(&list);
        }
        if has {
            s.onboarding.has_ever_had_profiles = true;
        }
    }
    if has {
        save("onboarding");
        if was_empty && !st().onboarding.first_profile_info_shown {
            ui(|w| w.global::<App>().set_first_profile_info(true));
        }
    }
    apply_profile(pick_active(&list));
    tray::set_profiles(&list);
    refresh_home();
    pages::on_profiles_changed();
}

fn apply_profile(p: Option<Value>) {
    let theme = p.as_ref().and_then(|p| p.get("theme")).filter(|t| t.is_object()).cloned();
    let pt: Option<ProfileTheme> = theme.and_then(|t| serde_json::from_value(t).ok());
    let changed = {
        let mut s = st();
        let title = if pt.is_some() { p.as_ref().map(util::profile_title).unwrap_or_default() } else { String::new() };
        let changed = s.menu.profile_theme != pt || s.menu.profile_theme_title != title;
        s.menu.profile_theme = pt;
        s.menu.profile_theme_title = title;
        changed
    };
    if changed {
        save("menu");
        apply_theme();
    }
    let gm_disabled = p.as_ref().map(|p| util::bool_of(p, "globalModeDisabled")).unwrap_or(false);
    if gm_disabled && st().menu.rule == "global" {
        set_rule("rule".into());
    }
    let logo = p.as_ref().map(|p| str_of(p, "logo").trim().to_string()).unwrap_or_default();
    let header = p.as_ref().map(|p| str_of(p, "headerTitle").trim().to_string()).unwrap_or_default();
    let title = if logo.is_empty() || header.is_empty() { "Prizrak-Box".to_string() } else { header };
    ui(move |w| {
        w.global::<App>().set_hide_mode_switch(gm_disabled);
        w.global::<App>().set_top_title(title.into());
    });
    let prev = std::mem::replace(&mut st().logo_key, logo.clone());
    if prev != logo || logo.is_empty() {
        if logo.is_empty() {
            ui(|w| w.global::<App>().set_top_logo(default_logo()));
        } else {
            spawn(async move {
                let img = images::load_remote(&logo).await;
                ui(move |w| {
                    let im = img.map(|i| theme::to_slint(&i)).unwrap_or_else(default_logo);
                    w.global::<App>().set_top_logo(im);
                });
            });
        }
    }
}

pub fn refresh_home() {
    let (profiles, loaded, s_startup, admin, port, version, os) = {
        let s = st();
        (s.profiles.clone(), s.profiles_loaded, s.setting.startup, s.admin, s.mixed_port.clone(), s.version.clone(), s.home.os.clone())
    };
    let active = pick_active(&profiles);
    ui(move |w| {
        let hp = w.global::<HomeP>();
        hp.set_loaded(loaded);
        hp.set_has_profiles(!profiles.is_empty());
        hp.set_startup(s_startup);
        hp.set_admin(admin);
        hp.set_port(port.into());
        hp.set_version(version.into());
        hp.set_os(os.into());
        if let Some(p) = active {
            hp.set_title(util::profile_title(&p).into());
            let hv = |k: &str| util::has_value(p.get(k));
            hp.set_has_renew(hv("renewUrl"));
            hp.set_has_home(hv("home"));
            hp.set_has_support(hv("support"));
            hp.set_used(util::format_traffic(p.get("used")).into());
            hp.set_available(util::format_traffic(p.get("available")).into());
            hp.set_expire(util::format_date_value(p.get("expire")).into());
            hp.set_update(util::relative_date(p.get("update")).into());
            hp.set_update_exact(util::format_exact(p.get("update")).into());
            hp.set_announce(announce_lines(&str_of(&p, "announce")));
            hp.set_announce_clickable(hv("announceUrl"));
            hp.set_show_renew_button(util::should_show_renew(&p));
        }
    });
}

/// AnnounceText.vue: `#RRGGBBword` colours the following word.
pub fn announce_lines(text: &str) -> ModelRc<AnnounceLine> {
    let text = text.replace("\\r\\n", "\n").replace("\\n", "\n");
    let text = text.trim();
    if text.is_empty() {
        return ModelRc::new(VecModel::from(Vec::<AnnounceLine>::new()));
    }
    let re = regex::Regex::new(r"#([0-9A-Fa-f]{6})(\S+)").unwrap();
    let mut lines = vec![];
    for line in text.split('\n') {
        // white-space: pre-wrap lets trailing spaces hang past the line end;
        // a Slint Text would wrap them onto an extra empty line.
        let line = line.trim_end();
        let mut segs = vec![];
        let mut last = 0;
        for m in re.captures_iter(line) {
            let whole = m.get(0).unwrap();
            if whole.start() > last {
                segs.push(AnnounceSeg { text: line[last..whole.start()].into(), color: Default::default(), colored: false });
            }
            let c = theme::parse_color(&format!("#{}", &m[1])).map(theme::rgb).unwrap_or_default();
            segs.push(AnnounceSeg { text: m[2].into(), color: c, colored: true });
            last = whole.end();
        }
        if last < line.len() {
            segs.push(AnnounceSeg { text: line[last..].into(), color: Default::default(), colored: false });
        }
        if segs.is_empty() {
            segs.push(AnnounceSeg { text: " ".into(), color: Default::default(), colored: false });
        }
        lines.push(AnnounceLine { segs: ModelRc::new(VecModel::from(segs)) });
    }
    ModelRc::new(VecModel::from(lines))
}

pub fn hwid_from_error(body: &Value) -> bool {
    if util::bool_of(body, "hwidNotSupported") {
        ui(|w| w.global::<App>().set_hwid_error("not-supported".into()));
        return true;
    }
    if util::bool_of(body, "hwidMaxDevicesReached") {
        let url = str_of(body, "supportUrl");
        ui(move |w| {
            w.global::<App>().set_hwid_support_url(url.into());
            w.global::<App>().set_hwid_error("max-devices".into());
        });
        return true;
    }
    false
}

async fn refresh_active_profile() {
    let Some(p) = active_profile() else { return };
    ui(|w| w.global::<HomeP>().set_refreshing(true));
    {
        let _g = loading(tr("profiles.refresh.ing"));
        match ctx().api.refresh_profile(p).await {
            Ok(refreshed) => {
                load_profiles().await;
                success(tr("profiles.refresh.success"));
                if util::bool_of(&refreshed, "hwidNotSupported") {
                    ui(|w| w.global::<App>().set_hwid_error("not-supported".into()));
                } else if util::bool_of(&refreshed, "hwidMaxDevicesReached") {
                    let url = str_of(&refreshed, "support");
                    ui(move |w| {
                        w.global::<App>().set_hwid_support_url(url.into());
                        w.global::<App>().set_hwid_error("max-devices".into());
                    });
                }
            }
            Err(e) => error(e.message),
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    ui(|w| w.global::<HomeP>().set_refreshing(false));
}

/// WelcomeScreen / ProfileToolbar addProfile.
pub async fn add_profile_from_input(content: String, age: String) {
    if content.trim().is_empty() {
        return;
    }
    let api = ctx().api.clone();
    ui(|w| w.global::<AddProfile>().set_busy(true));
    let mut body = json!({ "content": content });
    if !age.is_empty() {
        body["ageSecretKey"] = json!(age);
    }
    let res = api.add_profile(body).await;
    match res {
        Ok(new_profiles) => {
            // WelcomeScreen refreshes a first subscription before switching to
            // it (logo, title); ProfileToolbar switches straight away.
            let welcome = st().profiles.is_empty();
            if let Some(first) = new_profiles.first() {
                let id = str_of(first, "id");
                if welcome && first.get("type").and_then(Value::as_i64) == Some(1) {
                    let _ = api.refresh_profile(first.clone()).await;
                }
                let _ = api.switch_profile(&id, true, true).await;
                api.wait_running(Duration::from_secs(5)).await;
            }
            if let Ok(list) = api.profiles().await {
                if let Some(active) = pick_active(&list) {
                    st().f_profile = Some(active);
                }
                set_profiles(list);
            }
            check_alerts_after_profile_change();
            api.reset_caches();
            pages::on_profile_switched();
            success(tr("drag.success"));
            ui(|w| w.global::<AddProfile>().set_open(false));
        }
        Err(e) => {
            if !hwid_from_error(&e.body) {
                error(e.message);
            }
        }
    }
    ui(|w| w.global::<AddProfile>().set_busy(false));
}

/// MyDrop.vue doImportProfile (file import).
pub async fn import_profile_file(content: String, title: String, age: String) {
    let _g = loading(tr("drag.add"));
    let mut body = json!({ "content": content, "title": title });
    if !age.is_empty() {
        body["ageSecretKey"] = json!(age);
    }
    match ctx().api.add_profile(body).await {
        Ok(list) if !list.is_empty() => {
            success(tr("drag.success"));
            ui(|w| w.global::<App>().set_drop_open(false));
            navigate("Profiles");
            load_profiles().await;
        }
        Ok(_) => {}
        Err(e) => {
            if !hwid_from_error(&e.body) {
                error(if e.message.is_empty() { tr("drag.error") } else { e.message });
            }
        }
    }
}

/// `watch(webStore.fProfile) → loadProfiles()` in App.vue: after a switch, an
/// add or a refresh, reminders px computed in that pass are delivered.
pub fn check_alerts_after_profile_change() {
    spawn(async {
        if let Ok(list) = ctx().api.profiles().await {
            let enabled = st().setting.notify_subscription_alerts;
            check_pending_alerts(list, enabled).await;
        }
    });
}

async fn check_pending_alerts(list: Vec<Value>, enabled: bool) {
    for p in list {
        let alerts = p.get("pendingAlerts").and_then(Value::as_array).cloned().unwrap_or_default();
        if alerts.is_empty() {
            continue;
        }
        if enabled {
            for a in &alerts {
                let title = util::notification_title(a, &p);
                let body = {
                    let t = str_of(&p, "title");
                    if t.is_empty() { str_of(&p, "headerTitle") } else { t }
                };
                notify::alert(&title, &body, str_of(&p, "id"), a.clone());
            }
        }
        let _ = ctx().api.ack_subscription_alert(&str_of(&p, "id")).await;
    }
}

/// Click on a subscription-alert notification (SubscriptionAlertModal.vue).
pub fn show_subscription_alert(profile_id: String, alert: Value) {
    spawn(async move {
        let mut p = st().profiles.iter().find(|p| str_of(p, "id") == profile_id).cloned();
        if p.is_none() {
            if let Ok(list) = ctx().api.profiles().await {
                p = list.into_iter().find(|x| str_of(x, "id") == profile_id);
            }
        }
        let p = p.unwrap_or(Value::Null);
        let (msg, detail) = util::describe_alert(&alert, &p);
        let name = {
            let t = str_of(&p, "title");
            if t.is_empty() { str_of(&p, "headerTitle") } else { t }
        };
        let renew = str_of(&p, "renewUrl");
        st().sub_alert_renew = renew.clone();
        // profile.logo (profile-logo header) above the title, as in the modal.
        let logo_url = str_of(&p, "logo").trim().to_string();
        let logo = if logo_url.is_empty() { None } else { images::load_remote(&logo_url).await };
        window::show();
        ui(move |w| {
            let a = w.global::<App>();
            a.set_sub_alert_logo(logo.as_ref().map(theme::to_slint).unwrap_or_default());
            a.set_sub_alert_profile(name.into());
            a.set_sub_alert_title(msg.into());
            a.set_sub_alert_text(detail.into());
            a.set_sub_alert_renew(!renew.is_empty());
            a.set_sub_alert_open(true);
        });
    });
}

// ---------------------------------------------------------------- IP card

fn apply_ip(w: &AppWindow, ip: &IpInfo) {
    let hp = w.global::<HomeP>();
    hp.set_ip(ip.query.clone().into());
    hp.set_city(ip.city.clone().into());
    hp.set_country(ip.country.clone().into());
    hp.set_isp(ip.isp.clone().into());
    hp.set_asn(ip.asn.clone().into());
    hp.set_timezone(ip.timezone.clone().into());
}

pub async fn load_ip(hide: bool) {
    ui(|w| w.global::<HomeP>().set_ip_loading(true));
    let api = ctx().api.clone();
    let lang = st().menu.language.clone();
    let mut md6 = api.group_md5().await.unwrap_or_default();
    md6.push_str(&lang);
    let same = {
        let s = st();
        s.home.md6 == md6 && !s.home.ip.query.is_empty()
    };
    if !same || !hide {
        let url = format!("http://ip-api.com/json/?lang={}", tr("lang"));
        let res = api.web_test_ip(&url).await;
        let parsed = res.as_ref().ok().and_then(|d| {
            let q = str_of(d, "query");
            if q.is_empty() {
                return None;
            }
            Some(IpInfo {
                query: q,
                region_name: str_of(d, "regionName"),
                country: str_of(d, "country"),
                city: str_of(d, "city"),
                isp: str_of(d, "isp"),
                timezone: str_of(d, "timezone"),
                asn: str_of(d, "as").split(' ').next().unwrap_or("").to_string(),
            })
        });
        let info = match parsed {
            Some(i) => Some(i),
            None => {
                let url = format!("https://ipwhois.app/json/?lang={}", tr("lang"));
                match api.web_test_ip(&url).await {
                    Ok(d) if !str_of(&d, "ip").is_empty() => Some(IpInfo {
                        query: str_of(&d, "ip"),
                        region_name: str_of(&d, "region"),
                        country: str_of(&d, "country"),
                        city: str_of(&d, "city"),
                        isp: str_of(&d, "isp"),
                        timezone: str_of(&d, "timezone"),
                        asn: str_of(&d, "asn"),
                    }),
                    _ => {
                        if !hide {
                            if let Err(e) = res {
                                error(e.message);
                            }
                        }
                        None
                    }
                }
            }
        };
        if let Some(info) = info {
            {
                let mut s = st();
                s.home.ip = info.clone();
                s.home.md6 = md6;
            }
            save("home");
            ui(move |w| apply_ip(&w, &info));
        }
    }
    ui(|w| w.global::<HomeP>().set_ip_loading(false));
}

/// Called every second (MyIp.vue updateTimer).
pub fn tick_runtime(w: &AppWindow) {
    let start = st().start_time;
    let elapsed = (util::now_ms() as i64 - start).max(0) / 1000;
    let days = elapsed / 86400;
    let h = (elapsed % 86400) / 3600;
    let m = (elapsed % 3600) / 60;
    let s = elapsed % 60;
    let mut show = format!("{h:02}:{m:02}:{s:02}");
    if days > 0 {
        show = format!("{} {} {}", days, tr("home.system.day"), show);
    }
    w.global::<HomeP>().set_runtime(show.into());
}

// ---------------------------------------------------------------- mode / proxy / TUN

pub fn mihomo_payload() -> Value {
    let s = st();
    json!({
        "mode": s.menu.rule,
        "proxy": s.menu.proxy,
        "tun": s.menu.tun,
        "port": s.setting.port,
        "bindAddress": s.setting.bind_address,
        "stack": s.setting.stack,
        "dns": s.setting.dns,
        "ipv6": s.setting.ipv6,
        "systemProxyMode": s.setting.system_proxy_mode,
        "username": username(),
    })
}

/// `pUpdateMihomo`.
pub fn update_mihomo() {
    let body = mihomo_payload();
    spawn(async move {
        let _ = ctx().api.update_mihomo(body).await;
    });
}

pub fn set_rule(rule: String) {
    if st().menu.rule == rule {
        return;
    }
    st().menu.rule = rule.clone();
    save("menu");
    let r = rule.clone();
    ui(move |w| w.global::<App>().set_rule(r.into()));
    tray::set_mode(&rule);
    spawn(async move {
        if ctx().api.update_configs(json!({ "mode": rule })).await.is_ok() {
            success(tr(&format!("rules.{rule}-switch")));
            update_mihomo();
            pages::on_rule_changed();
        }
    });
}

async fn selected_profile_ok() -> bool {
    match ctx().api.profiles().await {
        Ok(list) if !list.is_empty() => {
            if list.iter().any(|p| util::bool_of(p, "selected")) {
                true
            } else {
                warning(tr("select-profile-warning"));
                false
            }
        }
        _ => {
            warning(tr("no-profile-warning"));
            false
        }
    }
}

async fn selected_profile_ok_silent() -> bool {
    ctx().api.profiles().await.map(|l| l.iter().any(|p| util::bool_of(p, "selected"))).unwrap_or(false)
}

/// Linux system proxy = the user's GNOME settings (the keys px writes, see
/// src-go/pkg/sys/proxy/proxy_linux.go). px started by the TUN service runs as
/// root and reaches them through `runuser`, which the service's capability set
/// does not allow ("cannot set groups"), so the switch did nothing. The shell
/// runs as the user and sets them itself in that case.
#[cfg(target_os = "linux")]
mod gnome_proxy {
    use std::process::Command;

    const SCHEMA: &str = "org.gnome.system.proxy";

    fn gsettings(args: &[&str]) -> Result<(), String> {
        match Command::new("gsettings").args(args).output() {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(format!("gsettings {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim())),
            Err(e) => Err(format!("gsettings: {e}")),
        }
    }

    pub fn enable(host: &str, port: i64) -> Result<(), String> {
        let port = port.to_string();
        for sub in ["http", "https", "socks"] {
            let schema = format!("{SCHEMA}.{sub}");
            gsettings(&["set", &schema, "host", host])?;
            gsettings(&["set", &schema, "port", &port])?;
        }
        gsettings(&["set", SCHEMA, "mode", "manual"])
    }

    pub fn disable() -> Result<(), String> {
        gsettings(&["reset", SCHEMA, "mode"])?;
        for sub in ["http", "https", "socks"] {
            let schema = format!("{SCHEMA}.{sub}");
            gsettings(&["reset", &schema, "host"])?;
            gsettings(&["reset", &schema, "port"])?;
        }
        Ok(())
    }
}

/// The shell itself sets the system proxy (Linux, px under the service).
fn shell_sets_system_proxy() -> bool {
    cfg!(target_os = "linux") && ctx().core.started_by_svc()
}

#[cfg(target_os = "linux")]
async fn shell_system_proxy(enable: bool) -> Result<(), String> {
    let (bind, mut port) = {
        let s = st();
        (s.setting.bind_address.clone(), s.setting.port)
    };
    if enable {
        // px points the system at the port it actually listens on when that
        // differs from the saved one (pandora.go enableProxy): same here.
        if let Ok(c) = ctx().api.configs().await {
            if let Some(p) = c.get("mixed-port").and_then(Value::as_i64).filter(|p| *p > 0) {
                port = p;
            }
        }
    }
    tokio::task::spawn_blocking(move || if enable { gnome_proxy::enable(&bind, port) } else { gnome_proxy::disable() })
        .await
        .unwrap_or_else(|e| Err(e.to_string()))
}

#[cfg(not(target_os = "linux"))]
async fn shell_system_proxy(_enable: bool) -> Result<(), String> {
    Ok(())
}

pub async fn apply_system_proxy_mode(enable: bool, notify: bool) -> Result<(), String> {
    if shell_sets_system_proxy() {
        let res = shell_system_proxy(enable).await;
        if notify {
            match &res {
                Ok(()) if enable => success(tr("proxy-switch-on")),
                Ok(()) => warning(tr("proxy-switch-off")),
                Err(e) => error(e.clone()),
            }
        }
        return res;
    }
    let api = &ctx().api;
    let (bind, port) = {
        let s = st();
        (s.setting.bind_address.clone(), s.setting.port)
    };
    let res = if enable {
        api.enable_proxy(&bind, port, &username()).await
    } else {
        api.disable_proxy(&username()).await
    };
    match res {
        Ok(_) => {
            if notify {
                if enable {
                    success(tr("proxy-switch-on"));
                } else {
                    warning(tr("proxy-switch-off"));
                }
            }
            Ok(())
        }
        Err(e) => {
            if notify {
                error(e.message.clone());
            }
            Err(e.message)
        }
    }
}

/// MyProxy.vue doSwitch (wrapped in pLoad).
pub async fn proxy_switch() {
    let _g = loading(tr("switch.ing"));
    let api = ctx().api.clone();
    let on = st().menu.proxy;
    let mut ok = false;
    if !on {
        if selected_profile_ok().await {
            let (bind, port, mode) = {
                let s = st();
                (s.setting.bind_address.clone(), s.setting.port, s.setting.system_proxy_mode)
            };
            let r = async {
                api.check_address_port(&bind, port).await?;
                api.update_configs(json!({"allow-lan": true, "mixed-port": port, "bind-address": bind})).await?;
                Ok::<(), api::ApiError>(())
            }
            .await;
            match r {
                Ok(()) => {
                    let _ = apply_system_proxy_mode(mode, false).await;
                    ok = true;
                    success(tr("proxy-switch-on"));
                }
                Err(e) => error(e.message),
            }
        } else {
            return;
        }
    } else {
        let _ = api.update_configs(json!({ "mixed-port": 0 })).await;
        let _ = apply_system_proxy_mode(false, false).await;
        ok = true;
        warning(tr("proxy-switch-off"));
    }
    if ok {
        let v = {
            let mut s = st();
            s.menu.proxy = !s.menu.proxy;
            s.menu.proxy
        };
        save("menu");
        update_mihomo();
        ui(move |w| {
            let a = w.global::<App>();
            a.set_proxy_on(v);
            if v {
                a.set_proxy_pulse(a.get_proxy_pulse() + 1);
            }
        });
    }
    tray::set_proxy(st().menu.proxy);
}

fn set_tun_state(on: bool) {
    {
        let mut s = st();
        s.menu.tun = on;
        s.tun_on = on;
    }
    save("menu");
    paths::set_tun_desired(on);
    ui(move |w| {
        let a = w.global::<App>();
        a.set_tun_on(on);
        if on {
            a.set_tun_pulse(a.get_tun_pulse() + 1);
        }
    });
    tray::set_tun(on);
}

/// MyProxy.vue tunSwitch.
pub async fn tun_switch() {
    if st().tun_on {
        enable_tun().await;
        return;
    }
    let api = ctx().api.clone();
    let has_admin = api.admin().await;
    let tun = ctx().tun.clone();
    let status = tokio::task::spawn_blocking(move || tun.status()).await.unwrap_or_default();
    let service_elevated = status.running && status.is_admin;
    if !has_admin && !service_elevated {
        ui(|w| w.global::<App>().set_service_dialog(true));
        tray::set_tun(false);
        return;
    }
    if has_admin && !service_elevated {
        ui(|w| w.global::<App>().set_admin_choice_dialog(true));
        return;
    }
    if !selected_profile_ok().await {
        tray::set_tun(false);
        return;
    }
    enable_tun().await;
}

/// MyProxy.vue enableTun: toggles TUN in mihomo.
pub async fn enable_tun() {
    let api = ctx().api.clone();
    let next = !st().tun_on;
    st().menu.tun = next;
    save("menu");
    paths::set_tun_desired(next);
    let stack = st().setting.stack.clone();
    let body = if next { json!({"tun": {"enable": true, "stack": stack}}) } else { json!({"tun": {"enable": false}}) };
    if api.update_configs(body).await.is_ok() {
        set_tun_state(next);
        if next {
            success(tr("tun-switch-on"));
        } else {
            warning(tr("tun-switch-off"));
        }
        update_mihomo();
        pages::on_service_status_changed();
    }
}

async fn install_service_handler() {
    ui(|w| {
        w.global::<App>().set_service_dialog(false);
        w.global::<App>().set_admin_choice_dialog(false);
    });
    let _g = loading(tr("service.installing"));
    let tun = ctx().tun.clone();
    let installed = tokio::task::spawn_blocking(move || tun.install()).await.unwrap_or(false);
    if !installed {
        error(tr("service.install-failed"));
        return;
    }
    success(tr("service.install-success"));
    let restarted = restart_after_install().await;
    pages::on_service_status_changed();
    if !restarted {
        return;
    }
    if !ctx().api.admin().await {
        warning(tr("service.restart-required"));
        return;
    }
    if selected_profile_ok().await {
        enable_tun().await;
    }
}

pub async fn restart_after_install() -> bool {
    if !restart_backend_and_sync().await {
        warning(tr("service.restart-required"));
        return false;
    }
    if !wait_backend_ready(Duration::from_secs(20)).await {
        log::warn!("px restarted but its control API did not answer in time");
    }
    true
}

/// Settings → remove service: TUN can no longer be on.
pub fn force_tun_off() {
    set_tun_state(false);
}

// ---------------------------------------------------------------- tray-driven

pub async fn update_proxy_groups_in_tray() {
    let api = ctx().api.clone();
    let Ok(groups) = api.groups().await else { return };
    let mut out = vec![];
    for g in groups {
        if let Ok(list) = api.proxies(&g.name, false, false, false, None, None).await {
            if !list.is_empty() {
                out.push((g.name.clone(), list.into_iter().map(|p| (p.name, p.now)).collect::<Vec<_>>()));
            }
        }
    }
    if !out.is_empty() {
        tray::set_groups(out);
    }
}

pub fn switch_proxy_in_group(group: String, proxy: String) {
    spawn(async move {
        let api = ctx().api.clone();
        if let Err(e) = api.set_proxy(&group, &proxy).await {
            error(e.message);
            return;
        }
        {
            let mut s = st();
            s.proxies.active = group.clone();
            s.proxies.now = proxy.clone();
        }
        save("proxies");
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Ok(list) = api.proxies(&group, false, false, false, None, None).await {
                if list.iter().any(|p| p.now && p.name == proxy) {
                    break;
                }
            }
        }
        update_proxy_groups_in_tray().await;
        pages::on_proxy_changed();
    });
}

pub fn dashboards() -> Vec<(String, String)> {
    let c = ctx().api.conn();
    let customs = st().web.custom_dashboards.clone();
    pages::dashboard_options(&customs)
        .into_iter()
        .map(|(_, name, url)| (name, pages::format_dashboard_url(&url, &c)))
        .filter(|(n, u)| !n.is_empty() && util::is_http_url(u))
        .collect()
}

// ---------------------------------------------------------------- quit

/// px clears the system proxy on exit only in the registry hive of the account
/// it runs as. Started through the TUN service that is SYSTEM, so the user's
/// proxy outlived px, pointing at a dead port (no network until it was turned
/// off by hand; found on the Windows 10 VM). While px still answers, clear it
/// for the user the way the Proxy switch does; the saved "proxy on" flag is
/// kept, so the next launch restores it as before.
async fn release_user_proxy() {
    let on = {
        let s = st();
        s.menu.proxy && s.setting.system_proxy_mode
    };
    if on {
        if shell_sets_system_proxy() {
            let _ = tokio::time::timeout(Duration::from_secs(2), shell_system_proxy(false)).await;
        } else {
            let _ = tokio::time::timeout(Duration::from_secs(2), ctx().api.disable_proxy(&username())).await;
        }
    }
}

/// Off.vue quit: api.exit then quit the shell.
pub async fn quit_via_api() {
    release_user_proxy().await;
    let _ = ctx().api.exit().await;
    shut_down();
}

/// SIGTERM / SIGINT / SIGHUP (session logout, shutdown, `systemctl stop`,
/// `kill`) quit like the tray's Quit. Without it the process just died and a
/// GNOME proxy the shell had set for a service-run px stayed pointing at a
/// dead port.
#[cfg(unix)]
pub fn watch_termination() {
    spawn(async {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut term), Ok(mut int), Ok(mut hup)) =
            (signal(SignalKind::terminate()), signal(SignalKind::interrupt()), signal(SignalKind::hangup()))
        else {
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
            _ = hup.recv() => {}
        }
        log::info!("termination signal: quitting");
        quit_app();
    });
}

/// Title-bar close, relaunch: quit without the round trip through the dialog.
pub fn quit_app() {
    if tokio::runtime::Handle::try_current().is_ok() {
        // inside a task (relaunch after moving the data folder): no blocking
        spawn(async {
            release_user_proxy().await;
            shut_down();
        });
    } else {
        // the UI thread: wait for the reset before px is asked to exit
        ctx().rt.block_on(release_user_proxy());
        shut_down();
    }
}

fn shut_down() {
    ctx().core.request_exit();
    let core = ctx().core.clone();
    std::thread::spawn(move || core.kill_px());
    let _ = slint::invoke_from_event_loop(|| {
        let _ = slint::quit_event_loop();
    });
}

// ---------------------------------------------------------------- updates

pub async fn check_for_updates() -> bool {
    ui(|w| pages::set_update_status_checking(&w));
    let client = reqwest::Client::builder()
        .user_agent("Prizrak-Box")
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let res = client
        .get("https://api.github.com/repos/legiz-ru/Prizrak-Box/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await;
    let prev = st().updates.latest_tag.clone();
    match res {
        Ok(r) if r.status().is_success() => {
            let d: Value = r.json().await.unwrap_or(Value::Null);
            let tag = str_of(&d, "tag_name");
            let mut s = st();
            s.updates.latest_tag = if tag.is_empty() { None } else { Some(tag.clone()) };
            s.updates.latest_name = Some(str_of(&d, "name")).filter(|x| !x.is_empty());
            s.updates.latest_url = Some(str_of(&d, "html_url")).filter(|x| !x.is_empty());
            s.updates.published_at = Some(str_of(&d, "published_at")).filter(|x| !x.is_empty());
            s.updates.last_checked = Some(util::now_ms() as i64);
            s.updates.update_available = !tag.is_empty() && util::compare_versions(&tag, APP_VERSION) > 0;
            if !s.updates.update_available || prev != s.updates.latest_tag {
                s.updates.dismissed_tag = None;
                s.updates.notified_tag = None;
            }
            s.updates.last_check_status = if s.updates.update_available { "update-available" } else { "up-to-date" }.into();
            s.updates.last_error = None;
        }
        Ok(r) => {
            let mut s = st();
            s.updates.last_check_status = "error".into();
            s.updates.last_error = Some(format!("GitHub responded with status {}", r.status().as_u16()));
        }
        Err(e) => {
            let mut s = st();
            s.updates.last_check_status = "error".into();
            s.updates.last_error = Some(e.to_string());
        }
    }
    save("updates");
    // notify once per tag (main.ts shouldNotify watcher)
    let notify = {
        let s = st();
        s.updates.update_available
            && s.updates.latest_tag.is_some()
            && s.updates.latest_tag != s.updates.dismissed_tag
            && s.updates.latest_tag != s.updates.notified_tag
    };
    if notify {
        let label = version_label();
        notify::send(&tr("updates.notification.title"), &tr_args("updates.notification.message", &[("version", label)]));
        let mut s = st();
        s.updates.notified_tag = s.updates.latest_tag.clone();
        drop(s);
        save("updates");
    }
    refresh_update_dialog();
    ui(|w| pages::refresh_update_status(&w));
    st().updates.update_available
}

pub fn version_label() -> String {
    let s = st();
    s.updates
        .latest_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| s.updates.latest_tag.clone())
        .unwrap_or_else(|| tr("updates.banner.version-unknown"))
}

fn refresh_update_dialog() {
    let (show, label) = {
        let s = st();
        (
            s.updates.update_available && s.updates.latest_tag.is_some() && s.updates.latest_tag != s.updates.dismissed_tag,
            String::new(),
        )
    };
    let label = if label.is_empty() { version_label() } else { label };
    ui(move |w| {
        w.global::<App>().set_update_version(label.into());
        w.global::<App>().set_update_dialog(show);
    });
}

// ---------------------------------------------------------------- theme

/// Re-resolves the theme (useAppTheme) and reloads the background if needed.
pub fn apply_theme() {
    let (menu, sys_dark) = {
        let s = st();
        (s.menu.clone(), s.sys_dark)
    };
    let eff = theme::effective(&menu);
    let key = format!("{}|{}", eff.background, eff.blur);
    let need_reload = eff.use_image && st().bg_key != key;
    if need_reload {
        st().bg_key = key;
        let seq = {
            let mut s = st();
            s.bg_seq += 1;
            s.bg_seq
        };
        let bg = eff.background.clone();
        let blur = eff.blur;
        spawn(async move {
            let img = match theme::load_background(&bg).await {
                Some(i) => Some(i),
                None => theme::load_background(DEFAULT_BACKGROUND).await,
            };
            if st().bg_seq != seq {
                return;
            }
            let Some(img) = img else { return };
            let analysed = theme::analyze_image(&img);
            let (display, blurred) = tokio::task::spawn_blocking(move || theme::prepare(&img, blur))
                .await
                .unwrap_or_else(|_| (image::RgbaImage::new(1, 1), None));
            if st().bg_seq != seq {
                return;
            }
            st().image_theme = analysed;
            ui(move |w| {
                let th = w.global::<Theme>();
                th.set_bg_image(theme::to_slint(&display));
                match &blurred {
                    Some(b) => {
                        th.set_bg_blur(theme::to_slint(b));
                        th.set_has_blur(true);
                    }
                    None => th.set_has_blur(false),
                }
            });
            push_theme_vars();
        });
    }
    push_theme_vars();
}

fn push_theme_vars() {
    let (menu, sys_dark, img_theme) = {
        let s = st();
        (s.menu.clone(), s.sys_dark, s.image_theme)
    };
    let eff = theme::effective(&menu);
    let dark = match eff.pref.as_str() {
        "light" => false,
        "dark" => true,
        _ => {
            if eff.use_image {
                if let Some(t) = img_theme {
                    t.white
                } else {
                    sys_dark
                }
            } else {
                sys_dark
            }
        }
    };
    let accent_from_image = eff.fixed_accent.is_none() && eff.use_image && img_theme.is_some();
    let (accent, on_accent) = if accent_from_image {
        let a = theme::accent_for(&img_theme.unwrap(), dark);
        (a, if dark { [255.0; 3] } else { [0.0; 3] })
    } else {
        let c = eff.fixed_accent.clone().unwrap_or(menu.accent.clone());
        let a = theme::parse_color(&c).unwrap_or([91.0, 103.0, 232.0]);
        (a, theme::on_color(a))
    };
    let mode_changed = {
        let mut s = st();
        let changed = s.dark != dark || !s.dark_flag_synced;
        s.dark = dark;
        s.dark_flag_synced = true;
        if s.menu.use_white != dark {
            s.menu.use_white = dark;
        }
        changed
    };
    if mode_changed {
        paths::set_dark_background(dark);
    }
    let use_image = eff.use_image;
    let (trans, blur, dim) = (eff.trans, eff.blur, eff.dim);
    ui(move |w| {
        let th = w.global::<Theme>();
        th.set_dark(dark);
        th.set_use_image(use_image);
        th.set_accent(theme::rgb(accent));
        th.set_on_accent(theme::rgb(on_accent));
        if use_image {
            let (r, g, b) = if dark { (20, 21, 27) } else { (255, 255, 255) };
            let alpha = if dark { 1.0 - trans / 100.0 } else { (1.0 - trans / 100.0).max(0.74) };
            let alpha = (alpha * 100.0).round() / 100.0;
            let panel = theme::rgba(r, g, b, alpha);
            th.set_panel_bg(panel);
            th.set_side_bg(panel);
            th.set_title_bg(panel);
            th.set_side_glass(blur >= 2.0);
            let overlay = if dark {
                theme::rgba(6, 7, 11, dim / 100.0)
            } else {
                theme::rgba(255, 255, 255, dim.max(30.0) / 100.0)
            };
            th.set_overlay(slint::Brush::SolidColor(overlay));
        } else {
            let c = |hex: u32, a: f64| theme::rgba((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, a);
            th.set_panel_bg(if dark { c(0x14151b, 0.74) } else { c(0xffffff, 0.82) });
            th.set_side_bg(if dark { c(0xffffff, 0.07) } else { c(0x000000, 0.045) });
            th.set_title_bg(slint::Color::from_argb_u8(0, 0, 0, 0));
            th.set_side_glass(false);
            th.set_has_blur(false);
        }
    });
    pages::on_theme_changed();
    crate::backdrop::refresh_soon();
}

pub fn set_system_dark(d: bool) {
    if st().sys_dark != d {
        st().sys_dark = d;
        push_theme_vars();
    }
}

pub fn menu_mut<F: FnOnce(&mut MenuState)>(f: F) {
    f(&mut st().menu);
    save("menu");
    apply_theme();
}

pub fn conn_info() -> ConnInfo {
    ctx().api.conn()
}

pub fn shared(s: impl AsRef<str>) -> SharedString {
    SharedString::from(s.as_ref())
}

pub fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

pub fn model_len<T: Clone + 'static>(m: &ModelRc<T>) -> usize {
    m.row_count()
}
