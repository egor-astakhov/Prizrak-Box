//! Settings → Rule tab controller — port of views/rule/*.vue.

use std::collections::HashSet;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};
use slint::ComponentHandle;

use crate::app::{self, ctx, save, spawn, st, ui};
use crate::i18n::{tr, tr_args};
use crate::util::{self, str_of};
use crate::{AppWindow, ProviderView, RuleRow, RulesP, SelectOption};

use super::model;

#[derive(Default)]
struct R {
    rules: Vec<(String, String, String)>,
    templates: Vec<Value>,
    now: Value,
    providers: Vec<Value>,
    updating: HashSet<String>,
    updating_all: bool,
    viewer: String,
    /// Ace's cursor in the viewer: byte offset of the current match.
    viewer_pos: Option<usize>,
}

static R: Mutex<Option<R>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut R) -> T) -> T {
    let mut g = R.lock();
    f(g.get_or_insert_with(Default::default))
}

const INNER: [&str; 3] = ["m1", "m2", "m3"];

// ---- Now ----

pub async fn load_rules() {
    ui(|w| w.global::<RulesP>().set_rules_loading(true));
    let list = ctx().api.rules().await.unwrap_or_default();
    let rows: Vec<(String, String, String)> = list
        .iter()
        .map(|r| (str_of(r, "type"), str_of(r, "payload"), str_of(r, "proxy")))
        .collect();
    with(|r| r.rules = rows);
    ui(|w| {
        w.global::<RulesP>().set_rules_loading(false);
        let q = w.global::<RulesP>().get_query().to_string();
        push_rules(&w, &q);
    });
}

fn push_rules(w: &AppWindow, q: &str) {
    let q = q.to_lowercase();
    let rows: Vec<RuleRow> = with(|r| {
        r.rules
            .iter()
            .filter(|(t, p, x)| q.is_empty() || t.to_lowercase().contains(&q) || p.to_lowercase().contains(&q) || x.to_lowercase().contains(&q))
            .map(|(t, p, x)| RuleRow { kind: t.into(), payload: p.into(), proxy: x.into() })
            .collect()
    });
    w.global::<RulesP>().set_rules(model(rows));
}

// ---- Group (templates) ----

fn title_of(t: &str) -> String {
    super::profiles::template_title(t)
}

pub async fn load_templates() {
    let api = ctx().api.clone();
    let list = api.get("/rule/list").await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let now = list.iter().find(|t| util::bool_of(t, "selected")).or_else(|| list.first()).cloned().unwrap_or(json!({"id": "", "title": "m1", "selected": false}));
    let yaml = if str_of(&now, "id").is_empty() {
        String::new()
    } else {
        api.get(&format!("/rule/template/{}", str_of(&now, "id"))).await.ok().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default()
    };
    // canDelete mirrors the Vue loop: decided by the last item scanned
    let can_delete = {
        let mut cd = false;
        for t in &list {
            cd = !INNER.contains(&str_of(t, "title").as_str());
            if util::bool_of(t, "selected") {
                break;
            }
        }
        cd
    };
    with(|r| {
        r.templates = list;
        r.now = now;
    });
    push_templates(Some(yaml), can_delete);
}

fn push_templates(yaml: Option<String>, can_delete: bool) {
    let (list, now) = with(|r| (r.templates.clone(), r.now.clone()));
    ui(move |w| {
        let rp = w.global::<RulesP>();
        let mut opts: Vec<SelectOption> = list
            .iter()
            .map(|t| SelectOption { value: str_of(t, "id").into(), label: title_of(&str_of(t, "title")).into() })
            .collect();
        if str_of(&now, "id").is_empty() {
            opts.push(SelectOption { value: "".into(), label: title_of(&str_of(&now, "title")).into() });
        }
        rp.set_templates(model(opts));
        rp.set_template_id(str_of(&now, "id").into());
        rp.set_template_on(util::bool_of(&now, "selected"));
        rp.set_can_delete(can_delete);
        if let Some(y) = yaml {
            rp.set_template_yaml(y.into());
        }
    });
}

async fn pick_template(id: String) {
    let Some(item) = with(|r| r.templates.iter().find(|t| str_of(t, "id") == id).cloned()) else { return };
    let yaml = ctx().api.get(&format!("/rule/template/{id}")).await.ok().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
    let can_delete = !INNER.contains(&str_of(&item, "title").as_str());
    with(|r| r.now = item);
    push_templates(Some(yaml), can_delete);
}

async fn save_template(yaml: String) {
    let trim = yaml.trim().to_string();
    if trim.is_empty() {
        app::error(tr("rule.group.add.tip"));
        return;
    }
    let _g = app::loading(tr("rule.group.save-ing"));
    let api = ctx().api.clone();
    let now = with(|r| r.now.clone());
    let res: Result<(), crate::api::ApiError> = async {
        api.post("/rule/test", json!({ "data": trim })).await?;
        if !str_of(&now, "id").is_empty() {
            api.put("/rule/template", json!({ "data": trim, "template": now })).await?;
            if util::bool_of(&now, "selected") {
                api.post("/rule/switch", now.clone()).await?;
                st().proxies.active.clear();
                save("proxies");
                if let Ok(n) = api.rule_num().await {
                    st().menu.rule_num = n;
                    save("menu");
                }
            }
            app::success(tr("rule.success"));
        } else {
            api.post("/rule/template", json!({ "data": trim, "title": str_of(&now, "title") })).await?;
            let list = api.get("/rule/list").await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let title = str_of(&now, "title");
            let found = list.iter().find(|t| str_of(t, "title") == title).cloned();
            with(|r| {
                r.templates = list;
                if let Some(f) = found {
                    r.now = f;
                }
            });
            push_templates(None, true);
            app::success(tr("rule.group.add.success"));
        }
        Ok(())
    }
    .await;
    if let Err(e) = res {
        app::error(e.message);
    }
}

async fn delete_template() {
    let now = with(|r| r.now.clone());
    let id = str_of(&now, "id");
    if id.is_empty() {
        return;
    }
    let ok = app::confirm(tr("confirm.delete-template.title"), tr("confirm.delete-template.text"), tr("confirm.delete-template.ok"), "trash").await;
    if !ok {
        return;
    }
    match ctx().api.delete(&format!("/rule/template/{id}")).await {
        Ok(_) => {
            load_templates().await;
            app::success(tr("rule.group.delete.success"));
        }
        Err(e) => app::error(e.message),
    }
}

async fn switch_template(on: bool) {
    let mut now = with(|r| r.now.clone());
    if str_of(&now, "id").is_empty() || util::bool_of(&now, "selected") == on {
        return;
    }
    now["selected"] = json!(on);
    with(|r| r.now = now.clone());
    let _g = app::loading(tr("rule.group.switch.ing"));
    let api = ctx().api.clone();
    match api.post("/rule/switch", now.clone()).await {
        Ok(_) => {
            let list = api.get("/rule/list").await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let id = str_of(&now, "id");
            let updated = list.iter().find(|t| str_of(t, "id") == id).cloned();
            with(|r| {
                r.templates = list;
                if let Some(u) = updated {
                    r.now = u;
                }
            });
            api.wait_running(Duration::from_secs(5)).await;
            app::success(tr("rule.group.switch.success"));
            st().proxies.active.clear();
            save("proxies");
            if let Ok(n) = api.rule_num().await {
                st().menu.rule_num = n;
                save("menu");
            }
            let cd = !INNER.contains(&str_of(&now, "title").as_str());
            push_templates(None, cd);
        }
        Err(e) => app::error(e.message),
    }
}

// ---- Providers ----

pub async fn load_providers() {
    ui(|w| w.global::<RulesP>().set_providers_loading(true));
    match ctx().api.get("/providers/rules").await {
        Ok(d) => {
            let mut list: Vec<Value> = d
                .get("providers")
                .and_then(Value::as_object)
                .map(|m| m.iter().map(|(n, v)| {
                    let mut v = v.clone();
                    v["name"] = json!(n);
                    v
                }).collect())
                .unwrap_or_default();
            list.sort_by(|a, b| str_of(a, "name").cmp(&str_of(b, "name")));
            with(|r| r.providers = list);
        }
        Err(e) => app::error(e.message),
    }
    push_providers(false);
}

fn push_providers(keep_first: bool) {
    let (list, updating, all) = with(|r| (r.providers.clone(), r.updating.clone(), r.updating_all));
    let view = st().menu.providers_view.clone();
    ui(move |w| {
        let rp = w.global::<RulesP>();
        rp.set_providers_loading(false);
        if !keep_first {
            rp.set_providers_first_load(false);
        }
        rp.set_updating_all(all);
        rp.set_providers_view(view.into());
        let rows: Vec<ProviderView> = list
            .iter()
            .map(|p| {
                let name = str_of(p, "name");
                let up = p.get("updatedAt").and_then(Value::as_str).map(|s| Value::String(s.to_string()));
                ProviderView {
                    updating: updating.contains(&name) || all,
                    name: name.into(),
                    vehicle: str_of(p, "vehicleType").into(),
                    behavior: str_of(p, "behavior").into(),
                    count: p.get("ruleCount").and_then(Value::as_i64).map(|c| c.to_string()).unwrap_or_else(|| "—".into()).into(),
                    updated: up.as_ref().map(|u| util::relative_date(Some(u))).unwrap_or_else(|| tr("rule.providers.never")).into(),
                    updated_exact: up.as_ref().map(|u| util::format_exact(Some(u))).unwrap_or_default().into(),
                }
            })
            .collect();
        rp.set_providers(model(rows));
    });
}

async fn update_provider(name: String) {
    if !with(|r| r.updating.insert(name.clone())) {
        return;
    }
    push_providers(true);
    let res = ctx().api.put(&format!("/providers/rules/{}", crate::api::enc(&name)), Value::Null).await;
    with(|r| r.updating.remove(&name));
    load_providers().await;
    match res {
        Ok(_) => app::success(tr_args("rule.providers.updateSuccess", &[("name", name)])),
        Err(e) => app::error(e.message),
    }
}

async fn update_all() {
    if with(|r| std::mem::replace(&mut r.updating_all, true)) {
        return;
    }
    push_providers(true);
    let names: Vec<String> = with(|r| r.providers.iter().map(|p| str_of(p, "name")).collect());
    let mut errors = vec![];
    for n in names {
        if let Err(e) = ctx().api.put(&format!("/providers/rules/{}", crate::api::enc(&n)), Value::Null).await {
            errors.push(format!("{n}: {}", e.message));
        }
    }
    with(|r| r.updating_all = false);
    load_providers().await;
    if errors.is_empty() {
        app::success(tr("rule.providers.updateAllSuccess"));
    } else {
        app::error(errors.join(""));
    }
}

async fn view_provider(name: String) {
    ui({
        let name = name.clone();
        move |w| {
            let rp = w.global::<RulesP>();
            rp.set_viewer_title(name.into());
            rp.set_viewer_loading(true);
            rp.set_viewer_content("".into());
            rp.set_viewer_search("".into());
            rp.set_viewer_found("".into());
            rp.set_viewer_open(true);
        }
    });
    let res = ctx().api.get(&format!("/rule/provider/{}", crate::api::enc(&name))).await;
    let content = match res {
        Ok(v) => v.as_str().map(str::to_string).unwrap_or_else(|| if v.is_null() { String::new() } else { v.to_string() }),
        Err(e) => {
            app::error(e.message);
            String::new()
        }
    };
    with(|r| r.viewer = content.clone());
    ui(move |w| {
        let rp = w.global::<RulesP>();
        rp.set_viewer_loading(false);
        rp.set_viewer_content(content.into());
    });
}

// ---- Ignore ----

pub async fn load_ignore() {
    if let Ok(v) = ctx().api.get("/rule/ignore").await {
        let text = v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        ui(move |w| w.global::<RulesP>().set_ignore(text.into()));
    }
}

async fn save_ignore(text: String) {
    let v = text.trim().to_string();
    if v.is_empty() {
        return;
    }
    let list: Vec<&str> = v.split('\n').collect();
    match ctx().api.put("/rule/ignore", json!(list)).await {
        Ok(_) => app::success(tr("rule.success")),
        Err(e) => app::error(e.message),
    }
}

pub fn wire(w: &AppWindow) {
    let rp = w.global::<RulesP>();
    rp.on_query_edited(|q| ui(move |w| push_rules(&w, &q)));
    rp.on_pick_template(|id| spawn(pick_template(id.to_string())));
    rp.on_save_template(|| ui(|w| spawn(save_template(w.global::<RulesP>().get_template_yaml().to_string()))));
    rp.on_delete_template(|| spawn(delete_template()));
    rp.on_set_template_on(|on| spawn(switch_template(on)));
    rp.on_add_template(|| {
        ui(|w| {
            let rp = w.global::<RulesP>();
            let title = rp.get_add_title().to_string();
            if title.is_empty() {
                app::error(tr("profiles.edit.title-tip"));
                return;
            }
            with(|r| r.now = json!({"id": "", "title": title, "selected": false}));
            rp.set_template_yaml("".into());
            rp.set_add_open(false);
            push_templates(None, false);
        })
    });
    rp.on_set_providers_view(|v| {
        st().menu.providers_view = v.to_string();
        save("menu");
        push_providers(false);
    });
    rp.on_refresh_providers(|| spawn(load_providers()));
    rp.on_update_all(|| spawn(update_all()));
    rp.on_update_provider(|n| spawn(update_provider(n.to_string())));
    rp.on_view_provider(|n| spawn(view_provider(n.to_string())));
    // searchInEditor: typing jumps to the first match from the start, like
    // editor.find() on a fresh query; ↑/↓ move between matches with wrap.
    rp.on_viewer_search_edited(|q| {
        let content = with(|r| {
            r.viewer_pos = None;
            r.viewer.clone()
        });
        let n = if q.trim().is_empty() { None } else { Some(find_matches(&content, &q).len()) };
        ui(move |w| {
            w.global::<RulesP>().set_viewer_found(n.map(|n| tr_args("ui.found", &[("n", n.to_string())])).unwrap_or_default().into());
        });
        viewer_jump(&q, true);
    });
    rp.on_viewer_find(|forward| {
        ui(move |w| {
            let q = w.global::<RulesP>().get_viewer_search().to_string();
            viewer_jump(&q, forward);
        })
    });
    rp.on_save_ignore(|| ui(|w| spawn(save_ignore(w.global::<RulesP>().get_ignore().to_string()))));
}

/// Byte ranges of case-insensitive matches (ASCII-insensitive, which is what
/// rule payloads contain; non-ASCII must match exactly).
fn find_matches(content: &str, q: &str) -> Vec<(usize, usize)> {
    let q = q.trim();
    if q.is_empty() {
        return vec![];
    }
    let hay = content.as_bytes();
    let needle = q.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()].eq_ignore_ascii_case(needle) && content.is_char_boundary(i) && content.is_char_boundary(i + needle.len()) {
            out.push((i, i + needle.len()));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// editor.find(q, {wrap: true, backwards: !forward}): select the next match
/// after (or before) the current one and scroll it into view.
fn viewer_jump(q: &str, forward: bool) {
    let (content, pos) = with(|r| (r.viewer.clone(), r.viewer_pos));
    let m = find_matches(&content, q);
    let pick = if m.is_empty() {
        None
    } else if forward {
        let next = pos.map(|p| m.iter().position(|x| x.0 > p)).unwrap_or(Some(0));
        Some(m[next.unwrap_or(0)])
    } else {
        let p = pos.unwrap_or(usize::MAX);
        let prev = m.iter().rposition(|x| x.0 < p);
        Some(m[prev.unwrap_or(m.len() - 1)])
    };
    with(|r| r.viewer_pos = pick.map(|x| x.0));
    let line = pick.map(|x| content[..x.0].matches('\n').count() as i32).unwrap_or(-1);
    ui(move |w| {
        let rp = w.global::<RulesP>();
        let (a, b) = pick.map(|(a, b)| (a as i32, b as i32)).unwrap_or((-1, -1));
        rp.set_viewer_sel_start(a);
        rp.set_viewer_sel_end(b);
        rp.set_viewer_sel_line(line);
        rp.set_viewer_sel_tick(rp.get_viewer_sel_tick() + 1);
    });
}

pub fn on_show() {
    spawn(async {
        load_rules().await;
        load_templates().await;
        load_providers().await;
        load_ignore().await;
    });
}

pub fn on_profile_switched() {
    spawn(async {
        ctx().api.wait_running(Duration::from_secs(5)).await;
        load_rules().await;
        load_providers().await;
    });
}
