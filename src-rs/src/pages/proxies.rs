//! Proxies page controller — port of views/Proxies.vue + MySearch.vue.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use parking_lot::Mutex;
use serde_json::Value;
use slint::ComponentHandle;

use crate::api::{GroupInfo, ProxyNode};
use crate::app::{self, ctx, save, spawn, st, ui};
use crate::i18n::{tr, tr_args};
use crate::util::{self, with_flag};
use crate::{AppWindow, DdEntry, ProxiesP, ProxyGroupView, ProxyNodeView, Search};

use super::{icon, model};

const NODE_TEST_TIMEOUT: i64 = 2000;
const GROUP_TEST_TIMEOUT: i64 = 5000;
const CHUNK: usize = 60;

#[derive(Default)]
struct P {
    groups: Vec<String>,
    type_map: HashMap<String, String>,
    icons: HashMap<String, String>,
    icon_imgs: HashMap<String, image::RgbaImage>,
    nodes: HashMap<String, Vec<ProxyNode>>,
    nested: HashMap<String, String>,
    weights: HashMap<String, Vec<Value>>,
    testing: HashSet<String>,
    limits: HashMap<String, usize>,
    loaded: bool,
    bulk: bool,
    // header selector (MySearch.vue)
    s_groups: Vec<GroupInfo>,
    s_nodes: Vec<ProxyNode>,
}

static P: Mutex<Option<P>> = Mutex::new(None);
static REFRESHING: AtomicBool = AtomicBool::new(false);
/// proxyNodeDelayLimiter: one app-wide cap of 5 node dials in flight, also
/// while "test all" runs three groups at once.
static NODE_DELAY_LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(5);
/// earlyRetry: 3 s retries left while the core has not reported groups yet.
static EARLY_RETRIES: AtomicU32 = AtomicU32::new(0);

fn with<R>(f: impl FnOnce(&mut P) -> R) -> R {
    let mut g = P.lock();
    f(g.get_or_insert_with(Default::default))
}

// ---- proxyType.ts ----

fn group_icon(t: &str) -> Option<&'static str> {
    Some(match t {
        "selector" => "pointer",
        "urltest" => "clock",
        "fallback" => "reload",
        "loadbalance" => "scale",
        "relay" => "route",
        "smart" => "brain",
        _ => return None,
    })
}

fn service_icon(t: &str) -> Option<&'static str> {
    Some(match t {
        "direct" => "arrow-right",
        "reject" => "ban",
        "rejectdrop" => "circle-x",
        "pass" => "player-track-next",
        "passrule" => "arrow-forward-up",
        "rematch" => "refresh",
        "dns" => "server-2",
        "compatible" => "settings",
        _ => return None,
    })
}

fn protocol_icon(t: &str) -> Option<&'static str> {
    Some(match t {
        "vless" => "xray",
        "trojan" => "trojan",
        "trusttunnel" => "trusttunnel",
        "vmess" => "letter-v",
        "shadowsocks" | "shadowsocksr" => "activity",
        "hysteria" | "hysteria2" => "bolt",
        "tuic" => "rocket",
        "wireguard" => "wireguard",
        "tailscale" => "tailscale",
        "openvpn" => "openvpn",
        "http" => "world",
        "socks5" => "network",
        "ssh" => "terminal-2",
        "masque" => "mask",
        "anytls" => "topology-star-3",
        "sudoku" => "sudoku",
        "gostrelay" => "gost-relay",
        _ => return None,
    })
}

fn kind(t: &str, is_group: bool) -> &'static str {
    let k = t.to_lowercase();
    if k.is_empty() || k == "unknown" {
        "unknown"
    } else if is_group || group_icon(&k).is_some() {
        "group"
    } else if service_icon(&k).is_some() {
        "service"
    } else {
        "protocol"
    }
}

pub fn type_icon(t: &str, is_group: bool) -> &'static str {
    let k = t.to_lowercase();
    match kind(t, is_group) {
        "group" => group_icon(&k).unwrap_or("git-branch"),
        "service" => service_icon(&k).unwrap_or("settings"),
        "protocol" => protocol_icon(&k).unwrap_or("shield-lock"),
        _ => "help-circle",
    }
}

pub fn type_tip(t: &str, is_group: bool) -> String {
    let raw = t.trim();
    let k = raw.to_lowercase();
    match kind(raw, is_group) {
        "unknown" => tr("proxies.type.unknown"),
        "group" if group_icon(&k).is_some() => tr(&format!("proxies.type.{k}")),
        "service" if service_icon(&k).is_some() => tr(&format!("proxies.type.{k}")),
        "protocol" => tr_args("proxies.type.protocol", &[("type", raw.to_string())]),
        _ => raw.to_string(),
    }
}

fn tone(delay: i64) -> i32 {
    match util::delay_tone(delay) {
        "good" => 1,
        "fair" => 2,
        "slow" => 3,
        _ => 0,
    }
}

// ---- data loading ----

fn independent() -> (bool, String, Vec<crate::store::GroupTestUrl>) {
    let s = st();
    (s.setting.independent_delay_test, s.setting.test_url.clone(), s.setting.group_test_urls.clone())
}

fn has_profile() -> bool {
    let s = st();
    s.f_profile.as_ref().map(|p| !util::str_of(p, "id").is_empty()).unwrap_or(false) || !s.profiles.is_empty()
}

async fn load_groups() {
    if !has_profile() {
        with(|p| {
            p.groups.clear();
            p.icons.clear();
            p.type_map.clear();
        });
        return;
    }
    let Ok(raw) = ctx().api.groups().await else { return };
    let rule = st().menu.rule.clone();
    let active = st().proxies.active.clone();
    let names: Vec<String> = raw.iter().map(|g| g.name.clone()).collect();
    let mut new_active = None;
    with(|p| {
        p.icons = raw.iter().filter(|g| !g.icon.is_empty()).map(|g| (g.name.clone(), g.icon.clone())).collect();
        p.type_map = raw.iter().map(|g| (g.name.clone(), g.kind.clone())).collect();
        p.type_map.insert("GLOBAL".into(), "Selector".into());
        match rule.as_str() {
            "global" => {
                p.groups = names.iter().cloned().chain(std::iter::once("GLOBAL".to_string())).collect();
                if active.is_empty() {
                    new_active = names.first().cloned();
                }
            }
            "direct" => p.groups.clear(),
            _ => {
                p.groups = names.clone();
                if !names.contains(&active) || active == "GLOBAL" {
                    new_active = names.first().cloned();
                }
            }
        }
    });
    if let Some(a) = new_active {
        st().proxies.active = a;
        save("proxies");
    }
    // default expansion: keep known state, first group open
    {
        let groups = with(|p| p.groups.clone());
        if !groups.is_empty() {
            let mut s = st();
            let prev = s.proxies.group_expansion.clone();
            let mut m = serde_json::Map::new();
            for (i, g) in groups.iter().enumerate() {
                let v = prev.get(g).and_then(Value::as_bool).unwrap_or(i == 0);
                m.insert(g.clone(), Value::Bool(v));
            }
            // the 10 s refresh lands here: write only a real change
            let changed = m != prev;
            s.proxies.group_expansion = m;
            drop(s);
            if changed {
                save("proxies");
            }
        }
    }
    // fetch icons
    let icons = with(|p| p.icons.clone());
    for (g, url) in icons {
        if with(|p| p.icon_imgs.contains_key(&g)) {
            continue;
        }
        if let Some(img) = crate::images::load_remote(&url).await {
            with(|p| p.icon_imgs.insert(g, img));
        }
    }
}

async fn fetch_group(api: &crate::api::Api, group: &str) -> Vec<ProxyNode> {
    let (ind, test_url, urls) = independent();
    let (hide, sort) = {
        let s = st();
        (s.proxies.is_hide, s.proxies.is_sort)
    };
    let ov = if ind { urls.iter().find(|x| x.name == group).map(|x| x.url.clone()).filter(|u| !u.is_empty()) } else { None };
    api.proxies(group, hide, sort, ind, ov.as_deref(), if ind { Some(test_url.as_str()) } else { None })
        .await
        .unwrap_or_default()
}

pub async fn load_nodes(only: Option<String>) {
    if !has_profile() || st().menu.rule == "direct" {
        with(|p| {
            p.nodes.clear();
            p.nested.clear();
        });
        push();
        return;
    }
    let api = ctx().api.clone();
    if let Some(g) = only {
        let list = fetch_group(&api, &g).await;
        with(|p| p.nodes.insert(g, list));
        push();
        return;
    }
    let groups = with(|p| p.groups.clone());
    let futs = groups.iter().map(|g| {
        let api = api.clone();
        let g = g.clone();
        async move {
            let l = fetch_group(&api, &g).await;
            (g, l)
        }
    });
    let pairs = futures_util::future::join_all(futs).await;
    with(|p| p.nodes = pairs.into_iter().collect());
    update_nested().await;
    push();
    fetch_weights().await;
}

async fn update_nested() {
    let types = ["urltest", "selector", "fallback", "loadbalance", "relay"];
    let nested: Vec<String> = with(|p| {
        let mut v: Vec<String> = vec![];
        for nodes in p.nodes.values() {
            for n in nodes {
                if types.contains(&n.kind.to_lowercase().as_str()) && !v.contains(&n.name) {
                    v.push(n.name.clone());
                }
            }
        }
        v
    });
    let api = ctx().api.clone();
    let futs = nested.into_iter().map(|g| {
        let api = api.clone();
        async move {
            let l = api.proxies(&g, false, false, false, None, None).await.unwrap_or_default();
            (g, l.into_iter().find(|n| n.now).map(|n| n.name))
        }
    });
    let res = futures_util::future::join_all(futs).await;
    with(|p| p.nested = res.into_iter().filter_map(|(g, n)| n.map(|n| (g, n))).collect());
}

async fn fetch_weights() {
    let names: Vec<String> = with(|p| {
        let mut v: HashSet<String> = p.type_map.iter().filter(|(_, t)| *t == "Smart").map(|(n, _)| n.clone()).collect();
        for nodes in p.nodes.values() {
            for n in nodes {
                if n.kind == "Smart" {
                    v.insert(n.name.clone());
                }
            }
        }
        v.into_iter().collect()
    });
    if names.is_empty() {
        return;
    }
    let api = ctx().api.clone();
    for n in names {
        let w = api.group_weights(&n).await;
        with(|p| p.weights.insert(n, w));
    }
    push();
}

pub async fn refresh() {
    if REFRESHING.swap(true, Ordering::SeqCst) {
        return;
    }
    load_groups().await;
    load_nodes(None).await;
    with(|p| p.loaded = true);
    push();
    REFRESHING.store(false, Ordering::SeqCst);
}

// ---- view ----

fn rank_label(r: &str) -> String {
    match r {
        "MostUsed" => tr("proxies.smart.most-used"),
        "OccasionalUsed" => tr("proxies.smart.occasional-used"),
        "RarelyUsed" => tr("proxies.smart.rarely-used"),
        other => other.to_string(),
    }
}

/// (icon, tone, tip) — weightInfo().
fn weight_info(p: &P, group: &str, node: &ProxyNode) -> Option<(&'static str, i32, String)> {
    if p.type_map.get(group).map(|t| t == "Smart").unwrap_or(false) {
        let entry = p.weights.get(group).and_then(|w| w.iter().find(|x| util::str_of(x, "Name") == node.name));
        let weight = entry.map(|e| util::str_of(e, "Weight")).unwrap_or_default();
        return Some(match entry.map(|e| util::str_of(e, "Rank")).as_deref() {
            Some("MostUsed") => ("shield", 1, tr_args("proxies.smart.most-used-tip", &[("weight", weight)])),
            Some("OccasionalUsed") => ("shield-half", 2, tr_args("proxies.smart.occasional-used-tip", &[("weight", weight)])),
            Some("RarelyUsed") => ("shield", 0, tr_args("proxies.smart.rarely-used-tip", &[("weight", weight)])),
            _ => ("shield-question", 0, tr("proxies.smart.no-data")),
        });
    }
    if node.kind == "Smart" {
        let data = p.weights.get(&node.name).filter(|w| !w.is_empty());
        let Some(w) = data else { return Some(("shield-question", 0, tr("proxies.smart.no-data"))) };
        let tip = w
            .iter()
            .map(|x| format!("{}: {} ({})", util::str_of(x, "Name"), rank_label(&util::str_of(x, "Rank")), util::str_of(x, "Weight")))
            .collect::<Vec<_>>()
            .join("\n");
        return Some(("shield-check", 4, tip));
    }
    None
}

#[derive(Clone)]
struct NodeData {
    name: String,
    label: String,
    type_icon: &'static str,
    type_tip: String,
    desc: String,
    desc_tip: String,
    weight: Option<(&'static str, i32, String)>,
    delay: i64,
    selected: bool,
}

#[derive(Clone)]
struct GroupData {
    name: String,
    label: String,
    icon: Option<image::RgbaImage>,
    type_icon: &'static str,
    type_tip: String,
    selected: String,
    expanded: bool,
    testing: bool,
    loaded: bool,
    count: i32,
    hidden: i32,
    nodes: Vec<NodeData>,
}

pub fn push() {
    let expansion = st().proxies.group_expansion.clone();
    let (is_hide, is_sort, rule) = {
        let s = st();
        (s.proxies.is_hide, s.proxies.is_sort, s.menu.rule.clone())
    };
    let (groups, loaded, bulk) = with(|p| {
        let mut out = vec![];
        for g in &p.groups {
            let t = p.type_map.get(g).cloned().unwrap_or_default();
            let all = p.nodes.get(g);
            let limit = *p.limits.get(g).unwrap_or(&CHUNK);
            let selected = all
                .and_then(|n| n.iter().find(|x| x.now))
                .filter(|_| t != "Smart" && t != "LoadBalance")
                .map(|x| with_flag(&x.name))
                .unwrap_or_default();
            let nodes = all
                .map(|list| {
                    list.iter()
                        .take(limit)
                        .map(|n| {
                            let is_group = p.type_map.contains_key(&n.name);
                            let server_desc = if n.display_type != n.kind { n.display_type.clone() } else { String::new() };
                            let lk = n.kind.to_lowercase();
                            let nested = if lk != "smart" && lk != "loadbalance" {
                                p.nested.get(&n.name).map(|s| with_flag(s)).unwrap_or_default()
                            } else {
                                String::new()
                            };
                            let parts: Vec<String> = [server_desc.clone(), n.origin.clone(), nested].into_iter().filter(|s| !s.is_empty()).collect();
                            let tip_parts: Vec<String> = [server_desc, n.origin.clone(), p.nested.get(&n.name).cloned().unwrap_or_default()]
                                .into_iter()
                                .filter(|s| !s.is_empty())
                                .collect();
                            NodeData {
                                name: n.name.clone(),
                                label: with_flag(if n.display_name.is_empty() { &n.name } else { &n.display_name }),
                                type_icon: type_icon(&n.kind, is_group),
                                type_tip: type_tip(&n.kind, is_group),
                                desc: parts.join(" • "),
                                desc_tip: tip_parts.join(" • "),
                                weight: weight_info(p, g, n),
                                delay: n.delay,
                                selected: n.now,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push(GroupData {
                name: g.clone(),
                label: with_flag(g),
                icon: p.icon_imgs.get(g).cloned(),
                type_icon: type_icon(&t, true),
                type_tip: type_tip(&t, true),
                selected,
                expanded: expansion.get(g).and_then(Value::as_bool).unwrap_or(false),
                testing: p.testing.contains(g),
                loaded: all.is_some(),
                count: all.map(|a| a.len() as i32).unwrap_or(0),
                hidden: all.map(|a| a.len().saturating_sub(limit) as i32).unwrap_or(0),
                nodes,
            });
        }
        (out, p.loaded, p.bulk)
    });
    let any_open = groups.iter().any(|g| g.expanded);
    ui(move |w| {
        let pp = w.global::<ProxiesP>();
        pp.set_loaded(loaded);
        pp.set_direct(rule == "direct");
        pp.set_is_hide(is_hide);
        pp.set_is_sort(is_sort);
        pp.set_bulk_testing(bulk);
        pp.set_any_open(any_open);
        let gv: Vec<ProxyGroupView> = groups
            .into_iter()
            .map(|g| ProxyGroupView {
                name: g.name.into(),
                label: g.label.into(),
                has_icon: g.icon.is_some(),
                icon: g.icon.as_ref().map(crate::theme::to_slint).unwrap_or_default(),
                type_icon: icon(g.type_icon),
                type_tip: g.type_tip.into(),
                selected: g.selected.into(),
                expanded: g.expanded,
                testing: g.testing,
                loaded: g.loaded,
                count: g.count,
                hidden: g.hidden,
                nodes: model(
                    g.nodes
                        .into_iter()
                        .map(|n| ProxyNodeView {
                            name: n.name.into(),
                            label: n.label.into(),
                            type_icon: icon(n.type_icon),
                            type_tip: n.type_tip.into(),
                            desc: n.desc.into(),
                            desc_tip: n.desc_tip.into(),
                            has_weight: n.weight.is_some(),
                            weight_icon: n.weight.as_ref().map(|w| icon(w.0)).unwrap_or_default(),
                            weight_tone: n.weight.as_ref().map(|w| w.1).unwrap_or(0),
                            weight_tip: n.weight.map(|w| w.2).unwrap_or_default().into(),
                            delay_text: util::delay_label(n.delay).into(),
                            delay_tone: tone(n.delay),
                            selected: n.selected,
                        })
                        .collect(),
                ),
            })
            .collect();
        pp.set_groups(model(gv));
    });
}

// ---- actions ----

fn set_expansion(group: &str, v: bool) {
    st().proxies.group_expansion.insert(group.to_string(), Value::Bool(v));
    save("proxies");
}

pub async fn set_proxy(group: String, name: String) {
    let t = with(|p| p.type_map.get(&group).cloned().unwrap_or_default());
    if t != "Selector" {
        app::warning(tr("proxies.auto-group-no-manual-select"));
        return;
    }
    let api = ctx().api.clone();
    match api.set_proxy(&group, &name).await {
        Ok(_) => {
            let _ = api.close_all_connections().await;
            {
                let mut s = st();
                s.proxies.active = group.clone();
                s.proxies.now = name.clone();
            }
            save("proxies");
            load_nodes(None).await;
            refresh_search().await;
            app::update_proxy_groups_in_tray().await;
        }
        Err(e) => app::error(e.message),
    }
}

async fn test_group(group: String) {
    if !with(|p| p.testing.insert(group.clone())) {
        return;
    }
    push();
    let api = ctx().api.clone();
    let (ind, test_url, urls) = independent();
    let mut url = test_url;
    if ind {
        if let Some(u) = urls.iter().find(|x| x.name == group).map(|x| x.url.clone()).filter(|u| !u.is_empty()) {
            url = u;
        } else if let Some(gu) = api.group_test_url(&group).await {
            url = gu;
        }
    }
    let gt = with(|p| p.type_map.get(&group).cloned().unwrap_or_default()).to_lowercase();
    if ind && ["selector", "loadbalance", "smart"].contains(&gt.as_str()) {
        let nodes = api.proxies(&group, false, false, false, None, None).await.unwrap_or_default();
        let futs = nodes.iter().map(|n| {
            let api = api.clone();
            let url = url.clone();
            let name = n.name.clone();
            async move {
                let _p = NODE_DELAY_LIMIT.acquire().await;
                api.proxy_delay(&name, &url, NODE_TEST_TIMEOUT).await
            }
        });
        let res = futures_util::future::join_all(futs).await;
        let total = res.len();
        let failed = res.iter().filter(|ok| !**ok).count();
        if total > 0 {
            let msg = tr_args(
                "proxies.test-result",
                &[("name", group.clone()), ("succeeded", (total - failed).to_string()), ("total", total.to_string())],
            );
            if failed == 0 {
                app::success(msg);
            } else {
                app::warning(msg);
            }
        }
    } else if let Err(e) = api.group_delay(&group, &url, GROUP_TEST_TIMEOUT).await {
        app::error(e.message);
    }
    load_nodes(Some(group.clone())).await;
    with(|p| p.testing.remove(&group));
    push();
    fetch_weights().await;
}

async fn test_all() {
    if with(|p| std::mem::replace(&mut p.bulk, true)) {
        return;
    }
    push();
    let groups = with(|p| p.groups.clone());
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
    let futs = groups.into_iter().map(|g| {
        let sem = sem.clone();
        async move {
            let _p = sem.acquire().await;
            test_group(g).await;
        }
    });
    futures_util::future::join_all(futs).await;
    with(|p| p.bulk = false);
    push();
}

/// Auto-test after mount / profile switch (runDelayTestSilent).
async fn silent_test() {
    let active = st().proxies.active.clone();
    if active.is_empty() {
        return;
    }
    let (ind, test_url, _) = independent();
    if ind {
        test_group(active).await;
    } else {
        let _ = ctx().api.group_delay(&active, &test_url, GROUP_TEST_TIMEOUT).await;
        load_nodes(None).await;
    }
}

// ---- header selector (MySearch.vue) ----

pub async fn refresh_search() {
    refresh_search_ok().await;
}

/// false when px did not answer the group list (e.g. still reloading).
async fn refresh_search_ok() -> bool {
    if !has_profile() {
        with(|p| {
            p.s_groups.clear();
            p.s_nodes.clear();
        });
        push_search();
        return true;
    }
    let api = ctx().api.clone();
    let groups_res = api.groups().await;
    let answered = groups_res.is_ok();
    if let Ok(groups) = groups_res {
        let active = st().proxies.active.clone();
        if !groups.iter().any(|g| g.name == active) {
            if let Some(f) = groups.first() {
                st().proxies.active = f.name.clone();
                save("proxies");
            }
        }
        with(|p| p.s_groups = groups);
    }
    let active = st().proxies.active.clone();
    if !active.is_empty() {
        let list = api.proxies(&active, false, false, false, None, None).await.unwrap_or_default();
        if let Some(cur) = list.iter().find(|n| n.now) {
            let changed = std::mem::replace(&mut st().proxies.now, cur.name.clone()) != cur.name;
            if changed {
                save("proxies");
            }
        }
        with(|p| p.s_nodes = list);
    }
    // icons for the selector
    let icons: Vec<(String, String)> = with(|p| p.s_groups.iter().filter(|g| !g.icon.is_empty()).map(|g| (g.name.clone(), g.icon.clone())).collect());
    for (g, url) in icons {
        if !with(|p| p.icon_imgs.contains_key(&g)) {
            if let Some(img) = crate::images::load_remote(&url).await {
                with(|p| p.icon_imgs.insert(g, img));
            }
        }
    }
    push_search();
    answered
}

fn push_search() {
    let active = st().proxies.active.clone();
    let now = st().proxies.now.clone();
    let (groups, nodes, imgs) = with(|p| {
        let imgs: HashMap<String, image::RgbaImage> = p
            .s_groups
            .iter()
            .filter_map(|g| p.icon_imgs.get(&g.name).map(|i| (g.name.clone(), i.clone())))
            .collect();
        (p.s_groups.clone(), p.s_nodes.clone(), imgs)
    });
    let cur_group = groups.iter().find(|g| g.name == active).cloned();
    let cur_node = nodes.iter().find(|n| n.now).cloned();
    let proxy_name = cur_node
        .as_ref()
        .map(|n| with_flag(if n.display_name.is_empty() { &n.name } else { &n.display_name }))
        .unwrap_or_else(|| with_flag(&now));
    let proxy_tone = cur_node.as_ref().map(|n| tone(n.delay)).unwrap_or(0);
    ui(move |w| {
        let s = w.global::<Search>();
        s.set_group(with_flag(&active).into());
        let gi = imgs.get(&active);
        s.set_group_has_icon(gi.is_some());
        if let Some(i) = gi {
            s.set_group_icon(crate::theme::to_slint(i));
        }
        let gt = cur_group.as_ref().map(|g| g.kind.clone()).unwrap_or_default();
        s.set_group_type_icon(icon(type_icon(&gt, true)));
        s.set_group_type_tip(type_tip(&gt, true).into());
        s.set_proxy(proxy_name.into());
        s.set_proxy_tone(proxy_tone);
        let gl: Vec<DdEntry> = groups
            .iter()
            .map(|g| {
                let img = imgs.get(&g.name);
                DdEntry {
                    name: g.name.clone().into(),
                    label: with_flag(&g.name).into(),
                    icon: img.map(crate::theme::to_slint).unwrap_or_else(|| icon(type_icon(&g.kind, true))),
                    has_icon: img.is_some(),
                    tip: type_tip(&g.kind, true).into(),
                    selected: g.name == active,
                    delay: 0,
                    delay_text: "".into(),
                    delay_tone: 0,
                }
            })
            .collect();
        s.set_groups(model(gl));
        let nl: Vec<DdEntry> = nodes
            .iter()
            .map(|n| DdEntry {
                name: n.name.clone().into(),
                label: with_flag(if n.display_name.is_empty() { &n.name } else { &n.display_name }).into(),
                icon: Default::default(),
                has_icon: false,
                tip: "".into(),
                selected: n.now,
                delay: n.delay as i32,
                delay_text: util::delay_label(n.delay).into(),
                delay_tone: tone(n.delay),
            })
            .collect();
        s.set_proxies(model(nl));
    });
}

async fn search_silent_test() {
    let g = st().proxies.active.clone();
    if g.is_empty() {
        return;
    }
    let url = st().setting.test_url.clone();
    let _ = ctx().api.group_delay(&g, &url, 3000).await;
    refresh_search().await;
}

pub fn wire(w: &AppWindow) {
    let pp = w.global::<ProxiesP>();
    pp.on_toggle_group(|g| {
        let cur = st().proxies.group_expansion.get(g.as_str()).and_then(Value::as_bool).unwrap_or(false);
        set_expansion(&g, !cur);
        push();
    });
    pp.on_toggle_all(|| {
        let groups = with(|p| p.groups.clone());
        let any = {
            let s = st();
            groups.iter().any(|g| s.proxies.group_expansion.get(g).and_then(Value::as_bool).unwrap_or(false))
        };
        {
            let mut s = st();
            s.proxies.group_expansion = groups.iter().map(|g| (g.clone(), Value::Bool(!any))).collect();
        }
        save("proxies");
        push();
    });
    pp.on_toggle_hide(|| {
        let v = !st().proxies.is_hide;
        st().proxies.is_hide = v;
        save("proxies");
        push();
        spawn(load_nodes(None));
    });
    pp.on_toggle_sort(|| {
        let v = !st().proxies.is_sort;
        st().proxies.is_sort = v;
        save("proxies");
        push();
        spawn(load_nodes(None));
    });
    pp.on_test_group(|g| spawn(test_group(g.to_string())));
    pp.on_test_all(|| spawn(test_all()));
    pp.on_pick(|g, n| {
        let selected = with(|p| p.nodes.get(g.as_str()).and_then(|l| l.iter().find(|x| x.name == n.as_str()).map(|x| x.now))).unwrap_or(false);
        if !selected {
            spawn(set_proxy(g.to_string(), n.to_string()));
        }
    });
    pp.on_show_more(|g| {
        with(|p| {
            let l = *p.limits.get(g.as_str()).unwrap_or(&CHUNK);
            p.limits.insert(g.to_string(), l + CHUNK * 2);
        });
        push();
    });
    pp.on_jump(|g| {
        set_expansion(&g, true);
        push();
        // the card scrolls itself into view once the expanded layout is in
        ui(move |w| {
            let pp = w.global::<ProxiesP>();
            pp.set_jump_to(g.clone());
            pp.set_jump_tick(pp.get_jump_tick() + 1);
        });
    });

    let s = w.global::<Search>();
    s.on_open_groups(|| {
        spawn(async {
            refresh_search().await;
            search_silent_test().await;
        })
    });
    s.on_open_proxies(|| {
        spawn(async {
            refresh_search().await;
            search_silent_test().await;
        })
    });
    s.on_pick_group(|g| {
        st().proxies.active = g.to_string();
        save("proxies");
        spawn(async {
            refresh_search().await;
            push();
        });
    });
    s.on_pick_proxy(|n| {
        let group = st().proxies.active.clone();
        let is_now = with(|p| p.s_nodes.iter().any(|x| x.now && x.name == n.as_str()));
        if is_now {
            return;
        }
        let gt = with(|p| p.s_groups.iter().find(|g| g.name == group).map(|g| g.kind.clone())).unwrap_or_default();
        if gt != "Selector" {
            app::warning(tr("proxies.auto-group-no-manual-select"));
            return;
        }
        let n = n.to_string();
        spawn(async move {
            let api = ctx().api.clone();
            match api.set_proxy(&group, &n).await {
                Ok(_) => {
                    let _ = api.close_all_connections().await;
                    st().proxies.now = n;
                    save("proxies");
                    refresh_search().await;
                    load_nodes(None).await;
                    app::update_proxy_groups_in_tray().await;
                }
                Err(e) => app::error(e.message),
            }
        });
    });
}

/// Page shown.
pub fn on_show() {
    spawn(async {
        refresh().await;
        // earlyRetry: pxd-template configs can take a while on first start
        let empty = with(|p| p.groups.is_empty());
        EARLY_RETRIES.store(if empty && st().menu.rule != "direct" { 10 } else { 0 }, Ordering::SeqCst);
        silent_test().await;
    });
}

pub fn on_profile_switched() {
    with(|p| {
        p.loaded = false;
        p.nodes.clear();
        p.icon_imgs.clear();
    });
    with(|p| {
        p.s_groups.clear();
        p.s_nodes.clear();
    });
    push_search();
    spawn(async {
        {
            let mut s = st();
            s.proxies.active.clear();
            s.proxies.now.clear();
        }
        // the header must show the new profile: px may still be reloading
        for _ in 0..5 {
            if refresh_search_ok().await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        }
        refresh().await;
        silent_test().await;
    });
}

pub fn on_rule_changed() {
    spawn(async {
        refresh().await;
    });
}

/// Called every 10 s while the page is visible (the Vue `fresh` interval).
pub fn periodic(tick: u64) {
    if tick % 3 == 0 && EARLY_RETRIES.load(Ordering::SeqCst) > 0 {
        EARLY_RETRIES.fetch_sub(1, Ordering::SeqCst);
        if with(|p| p.groups.is_empty()) {
            spawn(async {
                load_groups().await;
                load_nodes(None).await;
            });
        } else {
            EARLY_RETRIES.store(0, Ordering::SeqCst);
        }
    }
    if tick % 10 == 0 {
        spawn(async {
            let empty = with(|p| p.groups.is_empty());
            if empty && st().menu.rule != "direct" {
                load_groups().await;
            }
            load_nodes(None).await;
        });
    }
    if tick % 120 == 0 {
        spawn(fetch_weights());
    }
}
