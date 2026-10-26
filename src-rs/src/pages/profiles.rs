//! Profiles page controller — port of views/Profiles.vue.

use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};
use slint::ComponentHandle;

use crate::app::{self, ctx, save, spawn, st, ui};
use crate::i18n::{tr, tr_args};
use crate::util::{self, bool_of, str_of};
use crate::{AppWindow, ProfileView, ProfilesP, SelectOption};

use super::model;

#[derive(Default)]
struct P {
    /// Working copy (selected/primary/selectionOrder are adjusted locally).
    list: Vec<Value>,
    order: Vec<String>,
    switching: bool,
    edit_id: String,
    templates: Vec<(String, String)>,
    tv_id: String,
    announce_url: String,
    first_load: bool,
}

static P: Mutex<Option<P>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut P) -> R) -> R {
    let mut g = P.lock();
    f(g.get_or_insert_with(|| P { first_load: true, ..Default::default() }))
}

fn find(id: &str) -> Option<Value> {
    with(|p| p.list.iter().find(|x| str_of(x, "id") == id).cloned())
}

/// applyProfileList.
pub fn apply_list(list: Vec<Value>) {
    let multi = st().setting.multi_profile_enabled;
    let need_exclusive = with(|p| {
        p.list = list;
        // seed selection order from selectionOrder, else list order
        let mut sel: Vec<&Value> = p.list.iter().filter(|x| bool_of(x, "selected")).collect();
        sel.sort_by_key(|x| {
            let o = x.get("selectionOrder").and_then(Value::as_i64).unwrap_or(0);
            if o > 0 { o } else { i64::MAX }
        });
        p.order = sel.iter().map(|x| str_of(x, "id")).collect();
        ensure_primary_first(p);
        apply_order(p);
        if !multi && !p.switching && p.list.iter().filter(|x| bool_of(x, "selected")).count() > 1 {
            p.list
                .iter()
                .find(|x| bool_of(x, "primary"))
                .or_else(|| p.list.iter().find(|x| bool_of(x, "selected")))
                .map(|x| str_of(x, "id"))
        } else {
            None
        }
    });
    push();
    if let Some(id) = need_exclusive {
        spawn(switch_profile(id, Some(true), true));
    }
}

fn ensure_primary_first(p: &mut P) {
    if let Some(pid) = p.list.iter().find(|x| bool_of(x, "selected") && bool_of(x, "primary")).map(|x| str_of(x, "id")) {
        p.order.retain(|x| *x != pid);
        p.order.insert(0, pid);
    }
}

fn apply_order(p: &mut P) {
    let order = p.order.clone();
    for item in p.list.iter_mut() {
        let id = str_of(item, "id");
        let pos = order.iter().position(|x| *x == id);
        if bool_of(item, "selected") {
            if let Some(i) = pos {
                item["selectionOrder"] = json!(i + 1);
                continue;
            }
        }
        if let Some(o) = item.as_object_mut() {
            o.remove("selectionOrder");
        }
    }
}

fn set_primary(p: &mut P, id: Option<&str>) {
    for item in p.list.iter_mut() {
        item["primary"] = json!(id.map(|i| str_of(item, "id") == i).unwrap_or(false));
    }
}

/// Card width and column count of the profiles grid (from the view).
static GRID: Mutex<(f32, usize)> = Mutex::new((0.0, 1));

/// Grid rows sized like the CSS grid: a row is as tall as its tallest card,
/// and a card grows by 30 px per extra line of wrapped action icons.
fn grid_rows(list: &[Value], multi: bool) -> (Vec<f32>, Vec<f32>) {
    let (cw, cols) = *GRID.lock();
    if cw <= 0.0 {
        return (vec![], vec![]);
    }
    let bottom = cw - 36.0;
    let lines = |d: &Value| {
        let n = [
            util::is_http_url(&str_of(d, "content")),
            !str_of(d, "announce").is_empty(),
            !str_of(d, "renewUrl").is_empty(),
            !str_of(d, "support").is_empty(),
            !str_of(d, "home").is_empty(),
        ]
        .iter()
        .filter(|x| **x)
        .count()
            + 2;
        let order = d.get("selectionOrder").and_then(Value::as_i64).unwrap_or(0);
        let avail = if multi {
            let sel = if bool_of(d, "selected") && order > 0 { 40.0 } else { 22.0 };
            bottom - sel - 8.0
        } else {
            bottom - 4.0
        };
        let per = (((avail + 4.0) / 30.0).floor() as usize).max(1);
        n.div_ceil(per)
    };
    let (mut ys, mut hs) = (vec![], vec![]);
    let mut y = 0.0;
    for row in list.chunks(cols.max(1)) {
        let extra = row.iter().map(|d| lines(d).saturating_sub(1)).max().unwrap_or(0);
        let h = 198.0 + 30.0 * extra as f32;
        ys.push(y);
        hs.push(h);
        y += h + 14.0;
    }
    (ys, hs)
}

pub fn push() {
    let (list, first) = with(|p| (p.list.clone(), p.first_load));
    let multi = st().setting.multi_profile_enabled;
    let (row_y, row_h) = grid_rows(&list, multi);
    ui(move |w| {
        let pp = w.global::<ProfilesP>();
        pp.set_first_load(first);
        pp.set_multi(multi);
        pp.set_row_y(model(row_y.iter().map(|v| *v).collect()));
        pp.set_row_h(model(row_h.iter().map(|v| *v).collect()));
        let v: Vec<ProfileView> = list
            .iter()
            .map(|d| {
                let content = str_of(d, "content");
                ProfileView {
                    id: str_of(d, "id").into(),
                    title: util::profile_title(d).into(),
                    selected: bool_of(d, "selected"),
                    order: d.get("selectionOrder").and_then(Value::as_i64).unwrap_or(0) as i32,
                    remote: d.get("type").and_then(Value::as_i64) == Some(1),
                    used: util::format_traffic(d.get("used")).into(),
                    available: util::format_traffic(d.get("available")).into(),
                    expire: util::format_date_value(d.get("expire")).into(),
                    update: util::relative_date(d.get("update")).into(),
                    update_exact: util::format_exact(d.get("update")).into(),
                    can_tv: util::is_http_url(&content),
                    has_announce: !str_of(d, "announce").is_empty(),
                    has_renew: !str_of(d, "renewUrl").is_empty(),
                    has_support: !str_of(d, "support").is_empty(),
                    has_home: !str_of(d, "home").is_empty(),
                }
            })
            .collect();
        pp.set_profiles(model(v));
    });
}

pub async fn load() {
    match ctx().api.profiles().await {
        Ok(list) => {
            with(|p| p.first_load = false);
            apply_list(list.clone());
            app::set_profiles(list);
        }
        Err(_) => {
            with(|p| p.first_load = false);
            push();
        }
    }
    let templates = ctx().api.get("/rule/list").await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    with(|p| {
        p.templates = std::iter::once(("m0".to_string(), "m0".to_string()))
            .chain(templates.iter().map(|t| (str_of(t, "id"), str_of(t, "title"))))
            .collect();
    });
}

pub fn template_title(t: &str) -> String {
    if ["m0", "m1", "m2", "m3"].contains(&t) {
        tr(&format!("rule.group.{t}"))
    } else {
        t.to_string()
    }
}

/// switchProfile(data, desired, exclusive).
pub async fn switch_profile(id: String, desired: Option<bool>, mut exclusive: bool) {
    let Some(data) = find(&id) else { return };
    if with(|p| p.switching) {
        return;
    }
    let multi = st().setting.multi_profile_enabled;
    let mut next = desired.unwrap_or(!bool_of(&data, "selected"));
    if !multi && !exclusive {
        exclusive = true;
        next = true;
    }
    let was_primary = bool_of(&data, "primary");
    let (count, has_primary) = with(|p| {
        (
            p.list.iter().filter(|x| bool_of(x, "selected")).count(),
            p.list.iter().any(|x| bool_of(x, "selected") && bool_of(x, "primary")),
        )
    });
    if !exclusive && !next && count <= 1 {
        app::warning(tr("select-profile-warning"));
        return;
    }
    with(|p| p.switching = true);
    {
        let _g = app::loading(tr("profiles.switch.ing"));
        let api = ctx().api.clone();
        match api.switch_profile(&id, next, exclusive).await {
            Ok(_) => {
                st().proxies.active.clear();
                save("proxies");
                api.wait_running(Duration::from_secs(5)).await;
                with(|p| {
                    if exclusive {
                        for item in p.list.iter_mut() {
                            item["selected"] = json!(str_of(item, "id") == id && next);
                        }
                        p.order = if next { vec![id.clone()] } else { vec![] };
                        set_primary(p, if next { Some(&id) } else { None });
                    } else {
                        if let Some(item) = p.list.iter_mut().find(|x| str_of(x, "id") == id) {
                            item["selected"] = json!(next);
                        }
                        if next {
                            if !p.order.contains(&id) {
                                p.order.push(id.clone());
                            }
                            if count == 0 || !has_primary {
                                set_primary(p, Some(&id));
                            }
                        } else {
                            p.order.retain(|x| *x != id);
                            if was_primary {
                                let first = p.order.first().cloned();
                                set_primary(p, first.as_deref());
                            }
                        }
                    }
                    ensure_primary_first(p);
                    apply_order(p);
                });
                let list = with(|p| p.list.clone());
                if let Some(active) = app::pick_active(&list) {
                    st().f_profile = Some(active);
                }
                app::check_alerts_after_profile_change();
                if let Ok(n) = api.rule_num().await {
                    st().menu.rule_num = n;
                    save("menu");
                }
                app::set_profiles(list);
                push();
                api.reset_caches();
                super::on_profile_switched();
                let _ = api.close_all_connections().await;
                app::update_proxy_groups_in_tray().await;
                app::success(tr("profiles.switch.success"));
            }
            Err(e) => app::error(e.message),
        }
    }
    with(|p| p.switching = false);
}

async fn refresh_one(id: String) {
    let Some(data) = find(&id) else { return };
    let _g = app::loading(tr("profiles.refresh.ing"));
    match ctx().api.refresh_profile(data).await {
        Ok(re) => {
            with(|p| {
                if let Some(item) = p.list.iter_mut().find(|x| str_of(x, "id") == id) {
                    if let (Some(o), Some(n)) = (item.as_object_mut(), re.as_object()) {
                        for (k, v) in n {
                            o.insert(k.clone(), v.clone());
                        }
                    }
                }
            });
            let list = with(|p| p.list.clone());
            app::set_profiles(list);
            push();
            app::check_alerts_after_profile_change();
            app::success(tr("profiles.refresh.success"));
            if bool_of(&re, "hwidNotSupported") || bool_of(&re, "hwidMaxDevicesReached") {
                let mut b = re.clone();
                b["supportUrl"] = json!(str_of(&re, "support"));
                app::hwid_from_error(&b);
            }
        }
        Err(e) => app::error(e.message),
    }
}

async fn delete(id: String) {
    let Some(data) = find(&id) else { return };
    let name = util::profile_title(&data);
    let ok = app::confirm(
        tr("confirm.delete-profile.title"),
        tr_args("confirm.delete-profile.text", &[("name", name)]),
        tr("confirm.delete-profile.ok"),
        "trash",
    )
    .await;
    if !ok {
        return;
    }
    let selected = bool_of(&data, "selected");
    match ctx().api.delete_profile(data).await {
        Ok(_) => {
            with(|p| p.list.retain(|x| str_of(x, "id") != id));
            let list = with(|p| p.list.clone());
            app::set_profiles(list.clone());
            push();
            if list.is_empty() {
                {
                    let mut s = st();
                    s.f_profile = None;
                    s.proxies.active.clear();
                    s.proxies.now.clear();
                    s.proxies.group_expansion.clear();
                }
                save("proxies");
                super::on_profile_switched();
            }
            if selected {
                app::warning(tr("profiles.deleted-select-new"));
            }
        }
        Err(e) => app::error(e.message),
    }
}

fn send_order() {
    let mut list = with(|p| p.list.clone());
    for item in list.iter_mut() {
        for k in ["available", "used", "total"] {
            if let Some(v) = item.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))) {
                item[k] = json!(v.clamp(-9007199254740991.0, 9007199254740991.0) as i64);
            }
        }
    }
    app::set_profiles(list.clone());
    crate::ws::send_order(serde_json::to_string(&list).unwrap_or_default());
}

/// Profiles.vue add(): append the new profiles and save the order, without
/// switching to them.
pub async fn add(content: String, age: String) {
    if content.is_empty() {
        return;
    }
    ui(|w| w.global::<crate::AddProfile>().set_busy(true));
    let mut body = json!({ "content": content });
    if !age.is_empty() {
        body["ageSecretKey"] = json!(age);
    }
    match ctx().api.add_profile(body).await {
        Ok(list) => {
            with(|p| {
                p.first_load = false;
                p.list.extend(list);
            });
            push();
            send_order();
            app::success(tr("drag.success"));
            ui(|w| w.global::<crate::AddProfile>().set_open(false));
        }
        Err(e) => {
            if !app::hwid_from_error(&e.body) {
                app::error(e.message);
            }
        }
    }
    ui(|w| w.global::<crate::AddProfile>().set_busy(false));
}

fn valid_interval(v: &str) -> bool {
    if v.is_empty() {
        return true;
    }
    regex::Regex::new(r"^[1-9][0-9]?$|^1[0-2][0-8]$").unwrap().is_match(v)
}

/// Snapshot aid: id of the n-th profile card.
pub fn id_at(i: usize) -> Option<String> {
    with(|p| p.list.get(i).map(|x| str_of(x, "id")))
}

fn open_edit(id: &str) {
    let Some(d) = find(id) else { return };
    with(|p| p.edit_id = id.to_string());
    let templates = with(|p| p.templates.clone());
    let pxd = !str_of(&d, "pxdTemplateUrl").is_empty();
    let mut opts: Vec<(String, String)> = vec![];
    if pxd {
        opts.push(("pxd_subscription".into(), tr("profiles.edit.pxd-subscription")));
    }
    for (id, title) in templates {
        opts.push((id, template_title(&title)));
    }
    let template = if pxd { "pxd_subscription".to_string() } else { str_of(&d, "template") };
    let has_age = !str_of(&d, "ageSecretKey").trim().is_empty();
    let locked = bool_of(&d, "intervalFromHeader");
    let interval = str_of(&d, "interval");
    let bell = d.get("notifyExpireDays").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false)
        || d.get("notifyTrafficPercent").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false);
    ui(move |w| {
        let pp = w.global::<ProfilesP>();
        pp.set_edit_title(str_of(&d, "title").into());
        pp.set_edit_url(str_of(&d, "content").into());
        pp.set_edit_interval(interval.clone().into());
        pp.set_edit_remote(d.get("type").and_then(Value::as_i64) == Some(1));
        pp.set_edit_interval_locked(locked);
        pp.set_edit_interval_invalid(!valid_interval(&interval));
        pp.set_templates(model(opts.into_iter().map(|(v, l)| SelectOption { value: v.into(), label: l.into() }).collect()));
        pp.set_edit_template(template.into());
        pp.set_edit_template_locked(pxd);
        pp.set_edit_has_age(has_age);
        pp.set_edit_show_age(false);
        pp.set_edit_age(str_of(&d, "ageSecretKey").into());
        pp.set_edit_hwid(bool_of(&d, "hwidActive"));
        pp.set_edit_theme(d.get("theme").map(|t| t.is_object()).unwrap_or(false));
        pp.set_edit_bell(bell);
        pp.set_edit_renew(!str_of(&d, "renewUrl").is_empty());
        pp.set_edit_saving(false);
        pp.set_edit_open(true);
    });
}

fn save_edit(w: &AppWindow) {
    let pp = w.global::<ProfilesP>();
    let id = with(|p| p.edit_id.clone());
    let Some(mut d) = find(&id) else { return };
    let title = pp.get_edit_title().to_string();
    let url = pp.get_edit_url().to_string();
    let interval = pp.get_edit_interval().to_string();
    let kind = d.get("type").and_then(Value::as_i64).unwrap_or(0);
    if (kind == 1 || kind == 2) && title.is_empty() {
        app::error(tr("profiles.edit.title-tip"));
        return;
    }
    if kind == 1 {
        if url.is_empty() {
            app::error(tr("profiles.edit.url-tip"));
            return;
        }
        if !util::is_http_url(&url) {
            app::error(tr("profiles.edit.url-error"));
            return;
        }
        if !valid_interval(&interval) {
            app::error(tr("profiles.edit.update-tip"));
            return;
        }
    }
    d["title"] = json!(title);
    if kind == 1 {
        d["content"] = json!(url);
        d["interval"] = json!(interval);
    }
    d["template"] = json!(pp.get_edit_template().to_string());
    d["ageSecretKey"] = json!(pp.get_edit_age().to_string());
    pp.set_edit_saving(true);
    spawn(async move {
        let res = ctx().api.update_profile(d.clone()).await;
        ui(|w| w.global::<ProfilesP>().set_edit_saving(false));
        match res {
            Ok(_) => {
                with(|p| {
                    if let Some(item) = p.list.iter_mut().find(|x| str_of(x, "id") == id) {
                        *item = d;
                    }
                });
                let list = with(|p| p.list.clone());
                app::set_profiles(list);
                push();
                ui(|w| w.global::<ProfilesP>().set_edit_open(false));
                app::success(tr("profiles.edit.success"));
                if let Ok(n) = ctx().api.rule_num().await {
                    st().menu.rule_num = n;
                    save("menu");
                }
            }
            Err(e) => app::error(e.message),
        }
    });
}

async fn submit_tv(ip: String, port: String) {
    let id = with(|p| p.tv_id.clone());
    let content = find(&id).map(|d| str_of(&d, "content")).unwrap_or_default();
    if ip.trim().is_empty() || port.trim().is_empty() {
        app::error(format!("{} / {}", tr("profiles.tv-dialog.ip"), tr("profiles.tv-dialog.port")));
        return;
    }
    ui(|w| w.global::<ProfilesP>().set_tv_sending(true));
    let client = reqwest::Client::builder().timeout(Duration::from_secs(4)).no_proxy().build().unwrap();
    let url = format!("http://{}:{}/Prizrak-BoxTVimport/submit", ip.trim(), port.trim());
    let ok = match client.post(&url).json(&json!({ "content": content })).send().await {
        Ok(r) => r.json::<Value>().await.map(|v| str_of(&v, "status") == "ok").unwrap_or(false),
        Err(_) => false,
    };
    ui(move |w| {
        w.global::<ProfilesP>().set_tv_sending(false);
        if ok {
            w.global::<ProfilesP>().set_tv_open(false);
        }
    });
    if ok {
        app::success(tr("profiles.tv-dialog.success"));
    } else {
        app::error(tr("profiles.tv-dialog.error"));
    }
}

fn disable_multi() {
    st().setting.multi_profile_enabled = false;
    save("setting");
    let (primary, count) = with(|p| {
        (
            p.list
                .iter()
                .find(|x| bool_of(x, "primary"))
                .or_else(|| p.list.iter().find(|x| bool_of(x, "selected")))
                .map(|x| str_of(x, "id")),
            p.list.iter().filter(|x| bool_of(x, "selected")).count(),
        )
    });
    push();
    if let (Some(id), true) = (primary, count > 1) {
        spawn(switch_profile(id, Some(true), true));
    }
}

pub fn wire(w: &AppWindow) {
    let pp = w.global::<ProfilesP>();
    pp.on_switch(|id| spawn(switch_profile(id.to_string(), Some(true), true)));
    pp.on_pick(|id| spawn(switch_profile(id.to_string(), None, false)));
    pp.on_refresh(|id| spawn(refresh_one(id.to_string())));
    pp.on_delete(|id| spawn(delete(id.to_string())));
    pp.on_renew(|id| app::open_url(&find(&id).map(|d| str_of(&d, "renewUrl")).unwrap_or_default()));
    pp.on_support(|id| app::open_url(&find(&id).map(|d| str_of(&d, "support")).unwrap_or_default()));
    pp.on_home(|id| app::open_url(&find(&id).map(|d| str_of(&d, "home")).unwrap_or_default()));
    pp.on_edit(|id| open_edit(&id));
    pp.on_edit_save(|| ui(|w| save_edit(&w)));
    pp.on_edit_renew_clicked(|| {
        let id = with(|p| p.edit_id.clone());
        app::open_url(&find(&id).map(|d| str_of(&d, "renewUrl")).unwrap_or_default());
    });
    pp.on_edit_step(|delta| {
        ui(move |w| {
            let pp = w.global::<ProfilesP>();
            if pp.get_edit_interval_locked() {
                return;
            }
            let cur: Option<i32> = pp.get_edit_interval().parse().ok();
            let base = cur.unwrap_or(if delta > 0 { 0 } else { 2 });
            let v = (base + delta).clamp(1, 127).to_string();
            pp.set_edit_interval_invalid(!valid_interval(&v));
            pp.set_edit_interval(v.into());
        })
    });
    pp.on_edit_interval_edited(|t| {
        let clean: String = t.chars().filter(|c| c.is_ascii_digit()).take(3).collect();
        ui(move |w| {
            let pp = w.global::<ProfilesP>();
            if pp.get_edit_interval().as_str() != clean {
                pp.set_edit_interval(clean.clone().into());
            }
            pp.set_edit_interval_invalid(!valid_interval(&clean));
        })
    });
    pp.on_edit_bell_clicked(|| {
        let id = with(|p| p.edit_id.clone());
        let Some(d) = find(&id) else { return };
        let mut lines = vec![tr("subscriptionAlert.infoBody")];
        let list = |k: &str| {
            let mut v: Vec<i64> = d.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
            v.sort();
            v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")
        };
        let days = list("notifyExpireDays");
        if !days.is_empty() {
            lines.push(tr_args("subscriptionAlert.infoExpire", &[("days", days)]));
        }
        let pct = list("notifyTrafficPercent");
        if !pct.is_empty() {
            lines.push(tr_args("subscriptionAlert.infoTraffic", &[("percent", pct)]));
        }
        let disabled = !st().setting.notify_subscription_alerts;
        if disabled {
            lines.push(tr("subscriptionAlert.infoDisabledLocally"));
        }
        ui(move |w| {
            let pp = w.global::<ProfilesP>();
            pp.set_alert_info_lines(model(lines.into_iter().map(Into::into).collect()));
            pp.set_alert_info_disabled(disabled);
            pp.set_alert_info_open(true);
        });
    });
    pp.on_tv(|id| {
        with(|p| p.tv_id = id.to_string());
        // tvWarning in Profiles.vue: the alert already has its own icon.
        let warning = tr("profiles.tv-dialog.warning");
        let warning = warning.trim_start().trim_start_matches("⚠️").trim_start_matches('⚠').trim_start().to_string();
        ui(move |w| {
            let pp = w.global::<ProfilesP>();
            pp.set_tv_warning(warning.into());
            pp.set_tv_open(true);
        });
    });
    pp.on_tv_submit(|| {
        ui(|w| {
            let pp = w.global::<ProfilesP>();
            let (ip, port) = (pp.get_tv_ip().to_string(), pp.get_tv_port().to_string());
            spawn(submit_tv(ip, port));
        })
    });
    pp.on_announce(|id| {
        let Some(d) = find(&id) else { return };
        let text = str_of(&d, "announce");
        let url = str_of(&d, "announceUrl");
        with(|p| p.announce_url = url.clone());
        ui(move |w| {
            let pp = w.global::<ProfilesP>();
            pp.set_announce_lines(crate::app::announce_lines(&text));
            pp.set_announce_has_url(!url.is_empty());
            pp.set_announce_open(true);
        });
    });
    pp.on_announce_open_url(|| {
        let url = with(|p| p.announce_url.clone());
        app::open_url(&url);
    });
    pp.on_toggle_multi(|| {
        let next = !st().setting.multi_profile_enabled;
        if !next {
            disable_multi();
            return;
        }
        st().setting.multi_profile_enabled = true;
        save("setting");
        push();
        if !st().setting.multi_profile_hint_shown {
            ui(|w| w.global::<ProfilesP>().set_multi_info(true));
        }
    });
    pp.on_multi_info_accept(|| {
        st().setting.multi_profile_hint_shown = true;
        save("setting");
        ui(|w| w.global::<ProfilesP>().set_multi_info(false));
    });
    pp.on_multi_info_decline(|| {
        ui(|w| w.global::<ProfilesP>().set_multi_info(false));
        disable_multi();
    });
    pp.on_relayout(|cw, cols| {
        *GRID.lock() = (cw, cols.max(1) as usize);
        push();
    });
    pp.on_reorder(|from, to| {
        with(|p| {
            let (f, t) = (from as usize, to as usize);
            if f < p.list.len() && t < p.list.len() {
                let item = p.list.remove(f);
                p.list.insert(t, item);
            }
        });
        push();
        send_order();
    });
}

pub fn on_show() {
    push();
    spawn(load());
}

/// The global profile list changed elsewhere (Home, tray, deep link).
pub fn on_profiles_changed() {
    let list = st().profiles.clone();
    let same = with(|p| p.list.iter().map(|x| str_of(x, "id")).eq(list.iter().map(|x| str_of(x, "id"))) && p.list == list);
    if !same && !with(|p| p.switching) {
        with(|p| p.first_load = false);
        apply_list(list);
    }
}
