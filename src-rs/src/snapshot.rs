//! Debug aid: `PRIZRAK_SNAPSHOT_DIR=<dir>` (+ optional
//! `PRIZRAK_SNAPSHOT_SEQ=Home,Proxies,Setting:core,Bg:sea,BgImg:sea1.jpg,Theme:light,
//! Max,EditProfile,...`) renders the window
//! into PNG files once the backend is up, then quits. Used for visual checks
//! in CI / headless sessions; inert otherwise.

use std::time::Duration;

use slint::ComponentHandle;

use crate::{App, AppWindow};

pub fn schedule(w: &AppWindow) {
    let Ok(dir) = std::env::var("PRIZRAK_SNAPSHOT_DIR") else { return };
    let seq: Vec<String> = std::env::var("PRIZRAK_SNAPSHOT_SEQ")
        .unwrap_or_else(|_| "Home".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let delay: u64 = std::env::var("PRIZRAK_SNAPSHOT_DELAY").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    let weak = w.as_weak();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(delay));
        for (i, step) in seq.iter().enumerate() {
            let st = step.clone();
            let _ = weak.upgrade_in_event_loop(move |w| apply_step(&w, &st));
            std::thread::sleep(Duration::from_millis(2500));
            let dir = dir.clone();
            let name = format!("{:02}-{}.png", i, step.replace([':', '/'], "_"));
            let _ = weak.upgrade_in_event_loop(move |w| {
                match w.window().take_snapshot() {
                    Ok(buf) => {
                        let img = image::RgbaImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec());
                        if let Some(img) = img {
                            let _ = img.save(std::path::Path::new(&dir).join(&name));
                        }
                    }
                    Err(e) => eprintln!("snapshot failed: {e}"),
                }
            });
            std::thread::sleep(Duration::from_millis(500));
        }
        if std::env::var("PRIZRAK_SNAPSHOT_KEEP").is_err() {
            crate::app::quit_app();
        }
    });
}

fn apply_step(w: &AppWindow, step: &str) {
    let (page, arg) = step.split_once(':').unwrap_or((step, ""));
    match page {
        "Setting" => {
            if !arg.is_empty() {
                w.global::<App>().invoke_set_setting_tab(arg.into());
            }
            w.global::<App>().invoke_navigate("Setting".into());
        }
        "Sub" => w.global::<App>().invoke_open_sub(arg.into()),
        "Lang" => w.global::<App>().invoke_set_language(arg.into()),
        "Theme" => crate::app::menu_mut(|m| m.theme_pref = arg.to_string()),
        "Bg" => crate::pages::theme::change_background(arg),
        "BgImg" => {
            let url = format!("url('/images/{arg}')");
            crate::app::menu_mut(move |m| m.background = url);
            crate::pages::theme::push();
        }
        "BgUrl" => {
            let url = format!("url('{arg}')");
            crate::app::menu_mut(move |m| m.background = url);
            crate::pages::theme::push();
        }
        "Blur" => {
            let v: f64 = arg.parse().unwrap_or(0.0);
            crate::app::menu_mut(move |m| m.ui_blur = v);
        }
        "Max" => {
            w.window().set_maximized(true);
            w.global::<App>().set_maximized(true);
        }
        "Rule" => w.global::<crate::RulesP>().set_sub(arg.into()),
        "Conn" => w.global::<crate::ConnP>().invoke_set_view(arg.into()),
        "EditProfile" => {
            if let Some(id) = crate::pages::profiles::id_at(0) {
                w.global::<crate::ProfilesP>().invoke_edit(id.into());
            }
        }
        // SubAlert:<card index>:<expired|expires_in|traffic_used>:<days or percent>
        "SubAlert" => {
            let mut it = arg.split(':');
            let idx = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
            let kind = it.next().unwrap_or("expires_in").to_string();
            let n: i64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
            if let Some(id) = crate::pages::profiles::id_at(idx) {
                let alert = match kind.as_str() {
                    "traffic_used" => serde_json::json!({"kind": kind, "percent": n}),
                    "expires_in" => serde_json::json!({"kind": kind, "days": n}),
                    _ => serde_json::json!({"kind": kind}),
                };
                crate::app::show_subscription_alert(id, alert);
            }
        }
        // Profile:<switch|edit|announce|tv|bell>:<card index>
        "Profile" => {
            let (action, idx) = arg.split_once(':').unwrap_or((arg, "0"));
            if let Some(id) = crate::pages::profiles::id_at(idx.parse().unwrap_or(0)) {
                let pp = w.global::<crate::ProfilesP>();
                match action {
                    "switch" => pp.invoke_switch(id.into()),
                    "edit" => pp.invoke_edit(id.into()),
                    "announce" => pp.invoke_announce(id.into()),
                    "tv" => pp.invoke_tv(id.into()),
                    "refresh" => pp.invoke_refresh(id.into()),
                    "delete" => pp.invoke_delete(id.into()),
                    "pick" => pp.invoke_pick(id.into()),
                    // the bell of the open edit dialog (open it first with Profile:edit)
                    "bell" => pp.invoke_edit_bell_clicked(),
                    _ => {}
                }
            }
        }
        // AddSubmit:<content> — the add dialog's submit on the current page
        "AddSubmit" => {
            let ap = w.global::<crate::AddProfile>();
            ap.set_content(arg.into());
            ap.invoke_submit();
        }
        "Reorder" => {
            if let Some((f, t)) = arg.split_once(':') {
                w.global::<crate::ProfilesP>().invoke_reorder(f.parse().unwrap_or(0), t.parse().unwrap_or(0));
            }
        }
        // DropFile:<path>[|<path>] — files dropped on the window
        "DropFile" => crate::pages::drop::import_dropped(arg.split('|').map(std::path::PathBuf::from).collect()),
        // Hwid:not-supported|max-devices
        "Hwid" => {
            w.global::<App>().set_hwid_support_url("https://t.me/support".into());
            w.global::<App>().set_hwid_error(arg.into());
        }
        "Mode" => w.global::<App>().invoke_set_rule(arg.into()),
        "ProxyBtn" => w.global::<App>().invoke_toggle_proxy(),
        "TunBtn" => w.global::<App>().invoke_toggle_tun(),
        // The "paste from clipboard" button: opens the add dialog with the
        // clipboard text (checks the clipboard backend on X11 / Wayland).
        "Paste" => w.global::<crate::AddProfile>().invoke_paste(),
        "Multi" => w.global::<crate::ProfilesP>().invoke_toggle_multi(),
        "MultiYes" => w.global::<crate::ProfilesP>().invoke_multi_info_accept(),
        "Dialog" => match arg {
            "theme" => w.global::<crate::ThemeDialog>().set_open(true),
            "quit" => w.global::<App>().set_quit_dialog(true),
            "add" => w.global::<crate::AddProfile>().set_open(true),
            "service" => w.global::<App>().set_service_dialog(true),
            "age" => {
                w.global::<crate::SettingsP>().set_age_open(true);
                w.global::<crate::SettingsP>().invoke_age_regenerate();
            }
            "dash" => w.global::<crate::SettingsP>().set_dash_open(true),
            "first" => w.global::<App>().set_first_profile_info(true),
            "update" => {
                w.global::<App>().set_update_version("v1.0.20".into());
                w.global::<App>().set_update_dialog(true);
            }
            _ => {}
        },
        "Import" => {
            let content = std::fs::read_to_string(arg).unwrap_or_default();
            crate::app::spawn(crate::app::add_profile_from_input(content, String::new()));
        }
        // Core:<dnsq|ipv6|dns|stack|port>:<value> — Settings → Core actions
        "Core" => {
            let (what, v) = arg.split_once(':').unwrap_or((arg, ""));
            let sp = w.global::<crate::SettingsP>();
            match what {
                "dnsq" => {
                    sp.set_dns_query(v.into());
                    sp.invoke_run_dns_query();
                }
                "ipv6" => sp.invoke_set_ipv6(v == "1"),
                "dns" => sp.invoke_set_dns(v == "1"),
                "stack" => sp.invoke_set_stack(v.into()),
                "port" => {
                    sp.set_port_input(v.into());
                    sp.set_port_editing(true);
                    sp.invoke_save_port();
                }
                _ => {}
            }
        }
        "Group" => w.global::<crate::ProxiesP>().invoke_toggle_group(arg.into()),
        "Jump" => w.global::<crate::ProxiesP>().invoke_jump(arg.into()),
        "TestAll" => w.global::<crate::ProxiesP>().invoke_test_all(),
        "TestGroup" => w.global::<crate::ProxiesP>().invoke_test_group(arg.into()),
        "Hide" => w.global::<crate::ProxiesP>().invoke_toggle_hide(),
        "Sort" => w.global::<crate::ProxiesP>().invoke_toggle_sort(),
        "ToggleAll" => w.global::<crate::ProxiesP>().invoke_toggle_all(),
        // Pick:<group>:<node>
        "Pick" => {
            if let Some((g, n)) = arg.split_once(':') {
                w.global::<crate::ProxiesP>().invoke_pick(g.into(), n.into());
            }
        }
        "WelcomeMenu" => {
            let h = w.global::<crate::HomeP>();
            h.set_debug_open_welcome(h.get_debug_open_welcome() + 1);
        }
                "OpenProxyList" => {
            let s = w.global::<crate::Search>();
            s.set_debug_open_proxies(s.get_debug_open_proxies() + 1);
        }
        // simulated hover over row <i> of an open header list
        "PopupTip" => {
            let i: f32 = arg.parse().unwrap_or(0.0);
            let pt = w.global::<crate::PopupTip>();
            pt.set_text(format!("row {i}").into());
            pt.set_x(130.0);
            pt.set_top(8.0 + i * 36.0);
            pt.set_bottom(8.0 + i * 36.0 + 34.0);
            pt.set_visible(true);
        }
        "ConnDetail" => {
            let cp = w.global::<crate::ConnP>();
            if let Some(r) = slint::Model::row_data(&cp.get_rows(), 0) {
                cp.invoke_open_log(r.id);
            }
        }
        "Proc" => {
            let cp = w.global::<crate::ConnP>();
            if let Some(r) = slint::Model::row_data(&cp.get_procs(), arg.parse().unwrap_or(0)) {
                cp.invoke_select_proc(r.key);
            }
        }
        "LogSearch" => {
            let lp = w.global::<crate::LogsP>();
            lp.set_search(arg.into());
            lp.invoke_search_edited(arg.into());
        }
        "LogPause" => w.global::<crate::LogsP>().invoke_toggle_pause(),
        "ProvView" => w.global::<crate::RulesP>().invoke_set_providers_view(arg.into()),
        "ProvOpen" => {
            let rp = w.global::<crate::RulesP>();
            if let Some(p) = slint::Model::row_data(&rp.get_providers(), arg.parse().unwrap_or(0)) {
                rp.invoke_view_provider(p.name);
            }
        }
        "ProvFind" => {
            let rp = w.global::<crate::RulesP>();
            rp.set_viewer_search(arg.into());
            rp.invoke_viewer_search_edited(arg.into());
        }
        "ProvNext" => w.global::<crate::RulesP>().invoke_viewer_find(arg != "prev"),
        // TplNew:<title>  — new group template with the YAML of the current one
        "TplNew" => {
            // as a user would: name it in the add dialog, paste YAML, save
            let rp = w.global::<crate::RulesP>();
            let yaml = rp.get_template_yaml();
            rp.set_add_title(arg.into());
            rp.invoke_add_template();
            let weak = w.as_weak();
            slint::Timer::single_shot(Duration::from_millis(600), move || {
                if let Some(w) = weak.upgrade() {
                    let rp = w.global::<crate::RulesP>();
                    rp.set_template_yaml(yaml);
                    rp.invoke_save_template();
                }
            });
        }
        "TplSaveAppend" => {
            let rp = w.global::<crate::RulesP>();
            let y = format!("{}\n# {}\n", rp.get_template_yaml().trim_end(), arg);
            rp.set_template_yaml(y.into());
            rp.invoke_save_template();
        }
        "TplDelete" => w.global::<crate::RulesP>().invoke_delete_template(),
        "ConfirmYes" => w.global::<App>().invoke_confirm_result(true),
        "IgnoreAdd" => {
            let rp = w.global::<crate::RulesP>();
            let t = format!("{}\n{}", rp.get_ignore().trim_end(), arg);
            rp.set_ignore(t.trim_start().into());
            rp.invoke_save_ignore();
        }
        "Wait" => {}
        "Close" => {
            w.global::<crate::ThemeDialog>().set_open(false);
            w.global::<App>().set_quit_dialog(false);
            w.global::<crate::AddProfile>().set_open(false);
            w.global::<App>().set_service_dialog(false);
            w.global::<App>().set_update_dialog(false);
            w.global::<crate::ProfilesP>().set_edit_open(false);
            w.global::<crate::ProfilesP>().set_announce_open(false);
            w.global::<crate::ProfilesP>().set_tv_open(false);
            w.global::<App>().set_sub_alert_open(false);
            w.global::<crate::ConnP>().set_log_open(false);
            w.global::<crate::RulesP>().set_viewer_open(false);
            w.global::<crate::ProfilesP>().set_alert_info_open(false);
            w.global::<crate::SettingsP>().set_age_open(false);
            w.global::<crate::SettingsP>().set_dash_open(false);
        }
        other => w.global::<App>().invoke_navigate(other.into()),
    }
}
