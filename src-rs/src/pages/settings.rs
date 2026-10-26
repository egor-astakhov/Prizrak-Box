//! Settings controller — port of MyConfig.vue, MyPort/MyBind/MyTun/MyService,
//! MyAgeKeypair, Dns.vue (+MyEditor), Shortcut.vue (+MyHotkeyInput), the
//! custom-dashboard dialog and LogLevelSelect.

use std::time::Duration;

use serde_json::{json, Value};
use slint::ComponentHandle;

use crate::app::{self, ctx, save, spawn, st, ui};
use crate::i18n::{tr, tr_args};
use crate::store::{CustomDashboard, GroupTestUrl};
use crate::util::str_of;
use crate::{AppWindow, DashboardRow, DnsRecord, SettingsP, TestUrlRow};

use super::model;

/// Mihomo core version (src-go/go.mod `github.com/metacubex/mihomo` line).
const CORE_VERSION: &str = env!("PRIZRAK_CORE_VERSION");

static DNS_LOADED: parking_lot::Mutex<String> = parking_lot::Mutex::new(String::new());

pub fn push() {
    let (setting, conn, customs) = {
        let s = st();
        (s.setting.clone(), app::conn_info(), s.web.custom_dashboards.clone())
    };
    let base = format!("http://{}:{}", conn.host, conn.port);
    let dashboards: Vec<(String, String)> = super::dashboard_options(&customs).into_iter().map(|(_, n, u)| (n, u)).collect();
    let mut tip_lines = vec![tr("setting.tips.hwid"), String::new()];
    let h = &setting.hwid_headers;
    let mut hl = vec![];
    if !h.hwid.is_empty() {
        hl.push(format!("HWID={}", h.hwid));
    }
    if !h.os.is_empty() {
        hl.push(format!("OS={}", h.os));
    }
    if !h.os_version.is_empty() {
        hl.push(format!("OS Version={}", h.os_version));
    }
    if !h.model.is_empty() {
        hl.push(format!("Model={}", h.model));
    }
    if hl.is_empty() {
        hl = vec!["HWID=—".into(), "OS=—".into(), "OS Version=—".into(), "Model=—".into()];
    }
    tip_lines.extend(hl);
    let hwid_tip = tip_lines.join("\n");
    let mac_warn = cfg!(target_os = "macos") && {
        let parts: Vec<String> = setting.sc_switch_key.split('+').map(|x| x.to_lowercase()).collect();
        let mods = &parts[..parts.len().saturating_sub(1)];
        mods.iter().any(|m| m == "alt") && !mods.iter().any(|m| m == "ctrl" || m == "cmd")
    };
    ui(move |w| {
        let sp = w.global::<SettingsP>();
        sp.set_core_version(CORE_VERSION.into());
        sp.set_port(setting.port as i32);
        sp.set_bind(setting.bind_address.clone().into());
        sp.set_stack(setting.stack.clone().into());
        sp.set_api_url(base.into());
        sp.set_secret(conn.secret.into());
        sp.set_dashboards(model(dashboards.into_iter().map(|(n, u)| DashboardRow { name: n.into(), url: u.into() }).collect()));
        sp.set_custom_dashboards(model(customs.iter().map(|d| DashboardRow { name: d.name.clone().into(), url: d.url.clone().into() }).collect()));
        sp.set_dns(setting.dns);
        sp.set_ipv6(setting.ipv6);
        sp.set_independent(setting.independent_delay_test);
        sp.set_test_urls(model(setting.group_test_urls.iter().map(|u| TestUrlRow { name: u.name.clone().into(), url: u.url.clone().into() }).collect()));
        sp.set_hwid(setting.hwid);
        sp.set_hwid_tip(hwid_tip.into());
        sp.set_startup(setting.startup);
        sp.set_start_minimized(setting.start_minimized);
        sp.set_system_proxy_mode(setting.system_proxy_mode);
        sp.set_auth(setting.auth);
        sp.set_shortcut(setting.sc_switch);
        sp.set_sub_alerts(setting.notify_subscription_alerts);
        sp.set_hotkey(setting.sc_switch_key.clone().into());
        sp.set_hotkey_mac_warning(mac_warn);
        sp.set_log_level({
            let l = crate::ws::LOG_LEVEL.lock().clone();
            if l.is_empty() { "info".into() } else { l.into() }
        });
    });
    refresh_update_status_async();
}

fn refresh_update_status_async() {
    ui(|w| refresh_update_status(&w));
}

pub fn refresh_update_status(w: &AppWindow) {
    let (text, tone, checking) = {
        let s = st();
        let u = &s.updates;
        if u.last_check_status == "checking" {
            (tr("updates.status.checking"), "", true)
        } else if u.update_available {
            drop(s);
            (tr_args("updates.status.available", &[("version", app::version_label())]), "warning", false)
        } else if u.last_check_status == "up-to-date" {
            (tr("updates.status.up-to-date"), "success", false)
        } else if u.last_check_status == "error" {
            match &u.last_error {
                Some(e) => (tr_args("updates.status.error-details", &[("message", e.clone())]), "error", false),
                None => (tr("updates.status.error"), "error", false),
            }
        } else {
            (String::new(), "", false)
        }
    };
    let sp = w.global::<SettingsP>();
    sp.set_update_text(text.into());
    sp.set_update_tone(tone.into());
    sp.set_update_checking(checking);
}

pub fn set_update_status_checking(w: &AppWindow) {
    st().updates.last_check_status = "checking".into();
    refresh_update_status(w);
}

// ---- service (MyService.vue) ----

pub async fn refresh_service() {
    ui(|w| w.global::<SettingsP>().set_service_loading(true));
    let tun = ctx().tun.clone();
    let s = tokio::task::spawn_blocking(move || tun.status()).await.unwrap_or_default();
    let (text, tone) = if !s.installed {
        (tr("service.status-not-installed"), "")
    } else if s.running && s.is_admin {
        (tr("service.status-running"), "success")
    } else if s.running {
        (tr("service.status-no-admin"), "error")
    } else {
        (tr("service.status-stopped"), "warning")
    };
    let installed = s.installed || s.running;
    ui(move |w| {
        let sp = w.global::<SettingsP>();
        sp.set_service_text(text.into());
        sp.set_service_tone(tone.into());
        sp.set_service_installed(installed);
        sp.set_service_loading(false);
    });
}

async fn install_service() {
    ui(|w| w.global::<SettingsP>().set_service_loading(true));
    let tun = ctx().tun.clone();
    let ok = tokio::task::spawn_blocking(move || tun.install()).await.unwrap_or(false);
    if ok {
        app::success(tr("service.install-success"));
        app::restart_after_install().await;
    } else {
        app::error(tr("service.install-failed"));
    }
    refresh_service().await;
}

async fn uninstall_service() {
    ui(|w| w.global::<SettingsP>().set_service_loading(true));
    let _ = ctx().api.exit().await;
    let tun = ctx().tun.clone();
    let ok = tokio::task::spawn_blocking(move || tun.uninstall()).await.unwrap_or(false);
    if ok {
        app::success(tr("service.uninstall-success"));
        app::force_tun_off();
        if app::restart_backend_and_sync().await {
            app::wait_backend_ready(Duration::from_secs(10)).await;
        }
        app::warning(tr("service.restart-required"));
    } else {
        app::error(tr("service.uninstall-failed"));
        if app::restart_backend_and_sync().await {
            app::wait_backend_ready(Duration::from_secs(10)).await;
        }
    }
    refresh_service().await;
}

// ---- port / bind ----

fn valid_port(s: &str) -> bool {
    regex::Regex::new(r"^[1-9]\d{0,4}$").unwrap().is_match(s) && s.parse::<u32>().map(|p| p <= 65535).unwrap_or(false)
}

fn valid_ip(s: &str) -> bool {
    let v4 = regex::Regex::new(r"^(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$").unwrap();
    let v6 = regex::Regex::new(r"^(([0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}|(::[0-9a-fA-F]{1,4}){1,7}[0-9a-fA-F]{0,4})$").unwrap();
    v4.is_match(s) || v6.is_match(s)
}

async fn reassert_system_proxy() {
    let (proxy, mode) = {
        let s = st();
        (s.menu.proxy, s.setting.system_proxy_mode)
    };
    if proxy {
        let _ = app::apply_system_proxy_mode(mode, false).await;
    }
}

async fn save_port(v: String) {
    if !valid_port(&v) {
        app::error(tr("setting.mihomo.port-error"));
        return;
    }
    let port: i64 = v.parse().unwrap_or(0);
    if port == st().setting.port {
        ui(|w| w.global::<SettingsP>().set_port_editing(false));
        return;
    }
    let api = ctx().api.clone();
    let bind = st().setting.bind_address.clone();
    if let Err(e) = api.check_address_port(&bind, port).await {
        app::error(e.message);
        return;
    }
    if api.update_configs(json!({ "mixed-port": port })).await.is_ok() {
        st().setting.port = port;
        save("setting");
        ui(|w| w.global::<SettingsP>().set_port_editing(false));
        app::update_mihomo();
        reassert_system_proxy().await;
        push();
    }
}

async fn save_bind(v: String) {
    if !valid_ip(&v) {
        app::error(tr("setting.mihomo.bind-error"));
        return;
    }
    if v == st().setting.bind_address {
        ui(|w| w.global::<SettingsP>().set_bind_editing(false));
        return;
    }
    let api = ctx().api.clone();
    let port = st().setting.port;
    if let Err(e) = api.check_address_port(&v, port).await {
        app::error(e.message);
        return;
    }
    if api.update_configs(json!({ "allow-lan": true, "bind-address": v })).await.is_ok() {
        st().setting.bind_address = v;
        save("setting");
        ui(|w| w.global::<SettingsP>().set_bind_editing(false));
        app::update_mihomo();
        reassert_system_proxy().await;
        push();
    }
}

async fn set_stack(stack: String) {
    st().setting.stack = stack.clone();
    save("setting");
    push();
    let api = ctx().api.clone();
    let tun = st().menu.tun;
    if tun {
        let _ = api.update_configs(json!({"tun": {"enable": false}})).await;
    }
    if api.update_configs(json!({"tun": {"enable": tun, "stack": stack}})).await.is_ok() {
        app::update_mihomo();
    }
}

// ---- DNS query ----

async fn run_dns_query(name: String, kind: String) {
    let name = name.trim().to_string();
    if name.is_empty() {
        return;
    }
    ui(|w| {
        let sp = w.global::<SettingsP>();
        sp.set_dns_loading(true);
        sp.set_dns_error("".into());
        sp.set_dns_results(model(vec![]));
    });
    let res = ctx().api.get(&format!("/dns/query?name={}&type={}", crate::api::enc(&name), kind)).await;
    let (err, rows) = match res {
        Ok(d) => {
            let answers = d.get("Answer").and_then(Value::as_array).cloned().unwrap_or_default();
            if answers.is_empty() {
                (tr("setting.mihomo.dnsQuery.noResults"), vec![])
            } else {
                let map = |t: i64| match t {
                    1 => "A".to_string(),
                    5 => "CNAME".into(),
                    28 => "AAAA".into(),
                    15 => "MX".into(),
                    16 => "TXT".into(),
                    2 => "NS".into(),
                    o => o.to_string(),
                };
                (
                    String::new(),
                    answers
                        .iter()
                        .map(|a| (map(a.get("type").and_then(Value::as_i64).unwrap_or(0)), str_of(a, "data"), str_of(a, "TTL")))
                        .collect(),
                )
            }
        }
        Err(e) => (e.message, vec![]),
    };
    ui(move |w| {
        let sp = w.global::<SettingsP>();
        sp.set_dns_loading(false);
        sp.set_dns_error(err.into());
        sp.set_dns_results(model(rows.into_iter().map(|(k, d, t)| DnsRecord { kind: k.into(), data: d.into(), ttl: t.into() }).collect()));
    });
}

// ---- config dir / import ----

async fn change_config_dir() {
    let cur = crate::paths::home_dir();
    if !cur.to_string_lossy().ends_with(crate::paths::WORK_DIR_NAME) {
        app::warning(tr("setting.px.change-warn"));
    }
    let Some(dir) = rfd::AsyncFileDialog::new().set_title(crate::paths::WORK_DIR_NAME).pick_folder().await else { return };
    let mut dest = dir.path().to_path_buf();
    if dest.file_name().map(|n| n != crate::paths::WORK_DIR_NAME).unwrap_or(true) {
        dest = dest.join(crate::paths::WORK_DIR_NAME);
    }
    let core = ctx().core.clone();
    let res = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        if cur != dest {
            core.kill_px();
            std::thread::sleep(Duration::from_millis(400));
            move_dir(&cur, &dest)?;
        }
        crate::paths::set_home_override(&dest)?;
        Ok(())
    })
    .await;
    match res {
        Ok(Ok(())) => {
            app::success(tr("setting.px.change-success"));
            relaunch();
        }
        Ok(Err(e)) => app::error(e.to_string()),
        Err(e) => app::error(e.to_string()),
    }
}

fn move_dir(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_tree(src, dst)?;
    std::fs::remove_dir_all(src)?;
    Ok(())
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
    } else {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

/// Starts a fresh instance after a short delay and quits this one.
pub fn relaunch() {
    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.to_string_lossy().to_string();
        #[cfg(windows)]
        {
            let mut c = std::process::Command::new("cmd");
            c.args(["/c", &format!("ping 127.0.0.1 -n 6 >nul & start \"\" \"{exe}\"")]);
            crate::core::hide_window(&mut c);
            let _ = c.spawn();
        }
        #[cfg(not(windows))]
        {
            let _ = std::process::Command::new("sh").args(["-c", &format!("sleep 5; exec \"{exe}\"")]).spawn();
        }
    }
    app::quit_app();
}

pub async fn import_file(from_drop: bool) {
    let mut dlg = rfd::AsyncFileDialog::new();
    dlg = if from_drop {
        dlg.add_filter("YAML / age", &["yaml", "yml", "age"])
    } else {
        dlg.add_filter("YAML", &["yaml", "yml"])
    };
    let Some(file) = dlg.pick_file().await else { return };
    let name = file.file_name();
    let bytes = file.read().await;
    let content = String::from_utf8_lossy(&bytes).to_string();
    if name.to_lowercase().ends_with(".age") {
        super::drop::ask_age_key(content, name);
        return;
    }
    app::import_profile_file(content, name, String::new()).await;
}

// ---- age keypair ----

async fn age_generate(kind: String) {
    ui(|w| {
        let sp = w.global::<SettingsP>();
        sp.set_age_loading(true);
        sp.set_age_error("".into());
        sp.set_age_copied_pub(false);
        sp.set_age_copied_sec(false);
    });
    let res = ctx().api.get(&format!("/age/keypair?type={kind}")).await;
    ui(move |w| {
        let sp = w.global::<SettingsP>();
        sp.set_age_loading(false);
        match res {
            Ok(v) => {
                sp.set_age_public(str_of(&v, "publicKey").into());
                sp.set_age_secret(str_of(&v, "secretKey").into());
            }
            Err(e) => sp.set_age_error(e.message.into()),
        }
    });
}

// ---- DNS editor ----

async fn dns_reload() {
    if let Ok(d) = ctx().api.dns().await {
        let content = str_of(&d, "content");
        *DNS_LOADED.lock() = content.clone();
        ui(move |w| {
            let sp = w.global::<SettingsP>();
            sp.set_dns_yaml(content.into());
            sp.set_dns_dirty(false);
        });
    }
}

async fn dns_save(yaml: String) {
    ui(|w| w.global::<SettingsP>().set_dns_saving(true));
    let res = ctx().api.update_dns(json!({ "data": yaml })).await;
    match res {
        Ok(_) => {
            *DNS_LOADED.lock() = yaml;
            app::success(tr("dns.success"));
            ui(|w| {
                let sp = w.global::<SettingsP>();
                sp.set_dns_saving(false);
                sp.set_dns_dirty(false);
                sp.set_dns_saved(true);
            });
            tokio::time::sleep(Duration::from_millis(1800)).await;
            ui(|w| w.global::<SettingsP>().set_dns_saved(false));
        }
        Err(e) => {
            app::error(e.message);
            ui(|w| w.global::<SettingsP>().set_dns_saving(false));
        }
    }
}

// ---- hotkey recorder ----

fn key_name(text: &str) -> Option<String> {
    let c = text.chars().next()?;
    let named = match c as u32 {
        0xF700 => "ArrowUp",
        0xF701 => "ArrowDown",
        0xF702 => "ArrowLeft",
        0xF703 => "ArrowRight",
        0xF72C => "PageUp",
        0xF72D => "PageDown",
        0xF729 => "Home",
        0xF72B => "End",
        0xF727 => "Insert",
        0x7F => "Delete",
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0A | 0x0D => "Enter",
        0x20 => " ",
        n @ 0xF704..=0xF71B => return Some(format!("F{}", n - 0xF704 + 1)),
        0x10 | 0x11 | 0x12 | 0x13 | 0x14 | 0x15 | 0x16 | 0x17 | 0x18 => return None,
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    if (0xF700..=0xF8FF).contains(&(c as u32)) || c.is_control() {
        return None;
    }
    Some(text.to_uppercase())
}

fn record_hotkey(text: String, ctrl: bool, alt: bool, shift: bool, meta: bool) {
    let Some(key) = key_name(&text) else { return };
    let mut parts = vec![];
    if ctrl {
        parts.push("Ctrl".to_string());
    }
    if alt {
        parts.push("Alt".into());
    }
    if shift {
        parts.push("Shift".into());
    }
    if meta {
        parts.push("Cmd".into());
    }
    parts.push(key);
    if parts.len() > 1 {
        let combo = parts.join("+");
        st().setting.sc_switch_key = combo.clone();
        save("setting");
        ui(|w| w.global::<SettingsP>().set_hotkey_recording(false));
        if st().setting.sc_switch {
            crate::hotkey::register(&combo);
        }
        push();
    }
}

fn set_bool(f: impl FnOnce(&mut crate::store::SettingState)) {
    f(&mut st().setting);
    save("setting");
    push();
}

pub fn wire(w: &AppWindow) {
    let sp = w.global::<SettingsP>();
    sp.on_save_port(|| ui(|w| spawn(save_port(w.global::<SettingsP>().get_port_input().to_string()))));
    sp.on_save_bind(|| ui(|w| spawn(save_bind(w.global::<SettingsP>().get_bind_input().to_string()))));
    sp.on_set_stack(|s| spawn(set_stack(s.to_string())));
    sp.on_copy(|t| app::copy_text(t.to_string(), tr("copy.success")));
    sp.on_open_dashboard(|i| {
        let customs = st().web.custom_dashboards.clone();
        if let Some((_, _, url)) = super::dashboard_options(&customs).get(i as usize) {
            app::open_url(&super::format_dashboard_url(url, &app::conn_info()));
        }
    });
    sp.on_set_dns(|v| {
        set_bool(|s| s.dns = v);
        spawn(async move {
            let _ = ctx().api.switch_dns(v).await;
        });
    });
    sp.on_set_ipv6(|v| {
        set_bool(|s| s.ipv6 = v);
        spawn(async move {
            if ctx().api.update_configs(json!({ "ipv6": v })).await.is_ok() {
                app::update_mihomo();
            }
        });
    });
    sp.on_set_independent(|v| set_bool(|s| s.independent_delay_test = v));
    sp.on_test_url_edited(|i, n, u| {
        let mut s = st();
        if let Some(row) = s.setting.group_test_urls.get_mut(i as usize) {
            row.name = n.to_string();
            row.url = u.to_string();
        }
        drop(s);
        save("setting");
    });
    sp.on_test_url_add(|| set_bool(|s| s.group_test_urls.push(GroupTestUrl::default())));
    sp.on_test_url_remove(|i| {
        set_bool(|s| {
            if (i as usize) < s.group_test_urls.len() {
                s.group_test_urls.remove(i as usize);
            }
        })
    });
    sp.on_run_dns_query(|| {
        ui(|w| {
            let sp = w.global::<SettingsP>();
            spawn(run_dns_query(sp.get_dns_query().to_string(), sp.get_dns_type().to_string()));
        })
    });
    sp.on_check_updates(|| spawn(async {
        app::check_for_updates().await;
    }));
    sp.on_open_releases(|| app::open_url("https://github.com/legiz-ru/Prizrak-Box/releases/latest"));
    sp.on_service_install(|| spawn(install_service()));
    sp.on_service_uninstall(|| spawn(uninstall_service()));
    sp.on_service_refresh(|| spawn(refresh_service()));
    sp.on_set_hwid(|v| {
        app::set_hwid(v);
        push();
    });
    sp.on_set_startup(|v| {
        set_bool(|s| s.startup = v);
        crate::autostart::apply(v);
        app::update_mihomo();
        app::refresh_home();
    });
    sp.on_set_start_minimized(|v| {
        set_bool(|s| s.start_minimized = v);
        crate::paths::set_start_minimized(v);
    });
    sp.on_set_system_proxy_mode(|v| {
        set_bool(|s| s.system_proxy_mode = v);
        spawn(async move {
            if st().menu.proxy {
                let _ = app::apply_system_proxy_mode(v, true).await;
            }
        });
    });
    sp.on_set_auth(|v| set_bool(|s| s.auth = v));
    sp.on_set_shortcut(|v| {
        set_bool(|s| s.sc_switch = v);
        if v {
            crate::hotkey::register(&st().setting.sc_switch_key.clone());
        } else {
            crate::hotkey::unregister_all();
        }
    });
    sp.on_set_sub_alerts(|v| set_bool(|s| s.notify_subscription_alerts = v));
    sp.on_open_config_dir(|| {
        let _ = open::that_detached(crate::paths::home_dir());
    });
    sp.on_change_config_dir(|| spawn(change_config_dir()));
    sp.on_import_file(|| spawn(import_file(false)));

    // dashboards dialog
    sp.on_dash_submit(|| {
        ui(|w| {
            let sp = w.global::<SettingsP>();
            let (name, url) = (sp.get_dash_name().trim().to_string(), sp.get_dash_url().trim().to_string());
            if name.is_empty() || url.is_empty() {
                sp.set_dash_error(tr("setting.dashboard.error").into());
                return;
            }
            let idx = sp.get_dash_editing();
            {
                let mut s = st();
                if idx >= 0 && (idx as usize) < s.web.custom_dashboards.len() {
                    s.web.custom_dashboards[idx as usize] = CustomDashboard { name, url };
                } else {
                    s.web.custom_dashboards.push(CustomDashboard { name, url });
                }
            }
            save("web");
            sp.set_dash_error("".into());
            sp.set_dash_editing(-1);
            sp.set_dash_name("".into());
            sp.set_dash_url("".into());
            push();
            crate::tray::set_dashboards(app::dashboards());
        })
    });
    sp.on_dash_cancel(|| {
        ui(|w| {
            let sp = w.global::<SettingsP>();
            sp.set_dash_error("".into());
            sp.set_dash_editing(-1);
            sp.set_dash_name("".into());
            sp.set_dash_url("".into());
        })
    });
    sp.on_dash_edit(|i| {
        let d = st().web.custom_dashboards.get(i as usize).cloned();
        if let Some(d) = d {
            ui(move |w| {
                let sp = w.global::<SettingsP>();
                sp.set_dash_editing(i);
                sp.set_dash_name(d.name.into());
                sp.set_dash_url(d.url.into());
                sp.set_dash_error("".into());
            });
        }
    });
    sp.on_dash_remove(|i| {
        spawn(async move {
            let Some(d) = st().web.custom_dashboards.get(i as usize).cloned() else { return };
            let ok = app::confirm(
                tr("confirm.delete-dashboard.title"),
                tr_args("confirm.delete-dashboard.text", &[("name", d.name)]),
                tr("confirm.delete-dashboard.ok"),
                "trash",
            )
            .await;
            if !ok {
                return;
            }
            {
                let mut s = st();
                if (i as usize) < s.web.custom_dashboards.len() {
                    s.web.custom_dashboards.remove(i as usize);
                }
            }
            save("web");
            ui(move |w| {
                let sp = w.global::<SettingsP>();
                let e = sp.get_dash_editing();
                if e == i {
                    sp.set_dash_editing(-1);
                    sp.set_dash_name("".into());
                    sp.set_dash_url("".into());
                } else if e > i {
                    sp.set_dash_editing(e - 1);
                }
            });
            push();
            crate::tray::set_dashboards(app::dashboards());
        });
    });

    // age
    sp.on_age_set_type(|t| {
        let t2 = t.clone();
        ui(move |w| w.global::<SettingsP>().set_age_type(t2));
        spawn(age_generate(t.to_string()));
    });
    sp.on_age_regenerate(|| ui(|w| spawn(age_generate(w.global::<SettingsP>().get_age_type().to_string()))));
    sp.on_age_copy(|public| {
        ui(move |w| {
            let sp = w.global::<SettingsP>();
            let text = if public { sp.get_age_public() } else { sp.get_age_secret() };
            app::copy_text(text.to_string(), tr("copy.success"));
            if public {
                sp.set_age_copied_pub(true);
            } else {
                sp.set_age_copied_sec(true);
            }
        });
        spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            ui(move |w| {
                if public {
                    w.global::<SettingsP>().set_age_copied_pub(false);
                } else {
                    w.global::<SettingsP>().set_age_copied_sec(false);
                }
            });
        });
    });

    // dns editor
    sp.on_dns_reload(|| spawn(dns_reload()));
    sp.on_dns_save(|| ui(|w| spawn(dns_save(w.global::<SettingsP>().get_dns_yaml().to_string()))));
    sp.on_dns_edited(|t| {
        let dirty = *DNS_LOADED.lock() != t.as_str();
        ui(move |w| w.global::<SettingsP>().set_dns_dirty(dirty));
    });

    sp.on_hotkey_record(|t, c, a, s, m| record_hotkey(t.to_string(), c, a, s, m));
    sp.on_set_log_level(|l| {
        crate::ws::set_log_level(&l);
        let l2 = l.clone();
        ui(move |w| w.global::<SettingsP>().set_log_level(l2));
        super::on_log();
    });
}

pub fn on_show_tab(tab: &str) {
    push();
    if tab == "app" {
        spawn(refresh_service());
    }
}

pub fn on_open_sub(sub: &str) {
    push();
    if sub == "dns" {
        spawn(dns_reload());
    }
}

pub async fn load_log_level() {
    if crate::ws::LOG_LEVEL.lock().is_empty() {
        if let Ok(c) = ctx().api.configs().await {
            let l = str_of(&c, "log-level");
            if !l.is_empty() {
                ui(move |w| w.global::<SettingsP>().set_log_level(l.into()));
            }
        }
    }
}
