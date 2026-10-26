//! Connections + Log tabs — port of ConnectionTab.vue, ConnectionTopology.vue
//! (an echarts sankey; laid out here and drawn with Slint paths) and LogTab.vue.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;
use slint::ComponentHandle;

use crate::app::{self, ctx, save, spawn, st, ui};
use crate::i18n::{tr, tr_args};
use crate::util::{self, pretty_bytes, str_of};
use crate::{AppWindow, ConnP, ConnRow, LogKV, LogLine, LogSection, LogsP, ProcRow, SelectOption, TopoLink, TopoNode};

use super::model;

const MIHOMO_CORE_KEY: &str = "__prizrak_mihomo_core__";
const MIHOMO_CORE_NAME: &str = "Prizrak (mihomo core)";
const MAX_CLOSED: usize = 500;
const PALETTE: [u32; 9] = [0x5470c6, 0x91cc75, 0xfac858, 0xee6666, 0x73c0de, 0x3ba272, 0xfc8452, 0x9a60b4, 0xea7ccc];

#[derive(Default)]
struct C {
    active: Vec<Value>,
    closed: Vec<Value>,
    frozen: Option<Vec<Value>>,
    prev: HashMap<String, (f64, f64, Instant)>,
    search: String,
    sort_key: String,
    sort_desc: bool,
    paused: bool,
    selected_proc: String,
    log_id: String,
    log_item: Option<Value>,
    icons: HashMap<String, Option<image::RgbaImage>>,
    icon_loading: HashSet<String>,
    topo_paused: bool,
    topo_size: (f32, f32),
    topo_sig: String,
    topo_last: Option<Instant>,
    visible: bool,
    // logs
    log_search: String,
    log_paused: bool,
    log_snapshot: Vec<crate::ws::LogEntry>,
    log_autoscroll: bool,
    log_dirty: bool,
}

static C: Mutex<Option<C>> = Mutex::new(None);
pub static SHOW_CLOSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn with<T>(f: impl FnOnce(&mut C) -> T) -> T {
    let mut g = C.lock();
    f(g.get_or_insert_with(|| C { sort_key: "time".into(), sort_desc: true, log_autoscroll: true, ..Default::default() }))
}

fn host_of(m: &Value) -> String {
    let h = str_of(m, "host");
    let h = if h.is_empty() { str_of(m, "destinationIP") } else { h };
    format!("{}:{}", h, str_of(m, "destinationPort"))
}

fn conn_type(m: &Value) -> String {
    let net = str_of(m, "network").to_uppercase();
    let t = str_of(m, "type");
    if !net.is_empty() && !t.is_empty() && net != t {
        format!("{net}({t})")
    } else if !net.is_empty() {
        net
    } else if !t.is_empty() {
        t
    } else {
        "—".into()
    }
}

fn chains(c: &Value) -> Vec<String> {
    c.get("chains").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn format_chains(c: &[String]) -> String {
    if c.is_empty() {
        "—".into()
    } else {
        c.iter().rev().cloned().collect::<Vec<_>>().join(" → ")
    }
}

fn route_of(c: &Value) -> String {
    let rule = [str_of(c, "rule"), str_of(c, "rulePayload")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" / ");
    let ch = chains(c);
    let ch = if ch.is_empty() { String::new() } else { format_chains(&ch) };
    [rule, ch].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

fn start_ms(c: &Value) -> f64 {
    chrono::DateTime::parse_from_rfc3339(&str_of(c, "start")).map(|d| d.timestamp_millis() as f64).unwrap_or(0.0)
}

fn num(c: &Value, k: &str) -> f64 {
    c.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

/// Raw websocket frame from /connections.
pub fn on_frame(v: Value) {
    let mut next: Vec<Value> = v.get("connections").and_then(Value::as_array).cloned().unwrap_or_default();
    let now = Instant::now();
    with(|c| {
        let mut stats = HashMap::new();
        for conn in next.iter_mut() {
            let id = str_of(conn, "id");
            let (up, down) = (num(conn, "upload"), num(conn, "download"));
            if let Some((pu, pd, pt)) = c.prev.get(&id) {
                let dt = now.duration_since(*pt).as_secs_f64();
                if dt > 0.0 {
                    conn["uploadSpeed"] = serde_json::json!(((up - pu) / dt).round().max(0.0));
                    conn["downloadSpeed"] = serde_json::json!(((down - pd) / dt).round().max(0.0));
                }
            } else {
                conn["uploadSpeed"] = serde_json::json!(0);
                conn["downloadSpeed"] = serde_json::json!(0);
            }
            stats.insert(id, (up, down, now));
        }
        c.prev = stats;
        // recordClosed
        let ids: HashSet<String> = next.iter().map(|x| str_of(x, "id")).collect();
        let ts = util::now_ms();
        let mut just: Vec<Value> = c
            .active
            .iter()
            .filter(|x| !ids.contains(&str_of(x, "id")))
            .map(|x| {
                let mut x = x.clone();
                x["closedAt"] = serde_json::json!(ts);
                x
            })
            .collect();
        if !just.is_empty() {
            just.reverse();
            just.extend(c.closed.drain(..));
            just.truncate(MAX_CLOSED);
            c.closed = just;
        }
        c.active = next;
    });
    if with(|c| c.visible) {
        push();
        push_topology(false);
    }
}

fn display_source(c: &C) -> Vec<Value> {
    if let Some(f) = &c.frozen {
        return f.clone();
    }
    if SHOW_CLOSED.load(std::sync::atomic::Ordering::Relaxed) {
        c.closed.clone()
    } else {
        c.active.clone()
    }
}

fn filter_sort(list: Vec<Value>, search: &str, key: &str, desc: bool) -> Vec<Value> {
    let q = search.to_lowercase();
    let mut out: Vec<Value> = list
        .into_iter()
        .filter(|d| {
            let m = d.get("metadata").cloned().unwrap_or(Value::Null);
            q.is_empty()
                || host_of(&m).to_lowercase().contains(&q)
                || str_of(d, "rule").to_lowercase().contains(&q)
                || str_of(&m, "process").to_lowercase().contains(&q)
        })
        .collect();
    let val = |c: &Value| -> (f64, String) {
        match key {
            "download" => (num(c, "download"), String::new()),
            "upload" => (num(c, "upload"), String::new()),
            "dl-speed" => (num(c, "downloadSpeed"), String::new()),
            "ul-speed" => (num(c, "uploadSpeed"), String::new()),
            "host" => (0.0, host_of(c.get("metadata").unwrap_or(&Value::Null)).to_lowercase()),
            _ => (start_ms(c), String::new()),
        }
    };
    out.sort_by(|a, b| {
        let (x, y) = (val(a), val(b));
        let o = x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal);
        if desc {
            o.reverse()
        } else {
            o
        }
    });
    out
}

fn age(c: &Value) -> (String, String) {
    let ms = start_ms(c);
    if ms == 0.0 {
        return (String::new(), String::new());
    }
    let secs = ((util::now_ms() - ms) / 1000.0).max(0.0);
    let exact = chrono::DateTime::parse_from_rfc3339(&str_of(c, "start"))
        .map(|d| d.with_timezone(&chrono::Local).format("%d.%m.%Y, %H:%M:%S").to_string())
        .unwrap_or_default();
    (util::relative_seconds(secs), exact)
}

fn row(c: &Value, live_allowed: bool, with_process: bool) -> ConnRow {
    let m = c.get("metadata").cloned().unwrap_or(Value::Null);
    let (a, ae) = age(c);
    let (us, ds) = (num(c, "uploadSpeed"), num(c, "downloadSpeed"));
    ConnRow {
        id: str_of(c, "id").into(),
        host: host_of(&m).into(),
        conn_type: conn_type(&m).into(),
        process: if with_process && str_of(&m, "type") != "Inner" { str_of(&m, "process").into() } else { "".into() },
        route: route_of(c).into(),
        upload: pretty_bytes(num(c, "upload")).into(),
        download: pretty_bytes(num(c, "download")).into(),
        up_speed: pretty_bytes(us).into(),
        down_speed: pretty_bytes(ds).into(),
        live: live_allowed && (us > 0.0 || ds > 0.0),
        age: a.into(),
        age_exact: ae.into(),
    }
}

fn proc_key(c: &Value) -> (String, String, bool) {
    let m = c.get("metadata").cloned().unwrap_or(Value::Null);
    if str_of(&m, "type") == "Inner" {
        return (MIHOMO_CORE_KEY.into(), MIHOMO_CORE_NAME.into(), true);
    }
    let p = str_of(&m, "process");
    let name = if p.is_empty() { tr("connections.unknown-process") } else { p };
    let path = str_of(&m, "processPath");
    (if path.is_empty() { name.clone() } else { path }, name, false)
}

pub fn push() {
    let show_closed = SHOW_CLOSED.load(std::sync::atomic::Ordering::Relaxed);
    let view = st().conns.view_mode.clone();
    let (rows, procs, count, search, key, desc, paused, sel, sel_name, need_icons) = with(|c| {
        let source = display_source(c);
        let sorted = filter_sort(source.clone(), &c.search, &c.sort_key, c.sort_desc);
        let count = sorted.len() as i32;
        // processes
        let mut groups: BTreeMap<String, (String, i32, f64, f64, bool)> = BTreeMap::new();
        let mut order: Vec<String> = vec![];
        for conn in &source {
            let (k, n, core) = proc_key(conn);
            let e = groups.entry(k.clone()).or_insert_with(|| {
                order.push(k.clone());
                (n, 0, 0.0, 0.0, core)
            });
            e.1 += 1;
            e.2 += num(conn, "download");
            e.3 += num(conn, "upload");
        }
        let mut need = vec![];
        let q = c.search.to_lowercase();
        let mut procs: Vec<(String, String, i32, f64, f64, bool, Option<image::RgbaImage>)> = order
            .into_iter()
            .filter_map(|k| {
                let (n, cnt, d, u, core) = groups.get(&k).cloned()?;
                if !q.is_empty() && !n.to_lowercase().contains(&q) {
                    return None;
                }
                let icon = if core || k == n {
                    None
                } else {
                    match c.icons.get(&k) {
                        Some(i) => i.clone(),
                        None => {
                            need.push(k.clone());
                            None
                        }
                    }
                };
                Some((k, n, cnt, d, u, core, icon))
            })
            .collect();
        procs.sort_by(|a, b| b.2.cmp(&a.2));
        let rows: Vec<Value> = if view == "process" {
            if c.selected_proc.is_empty() {
                vec![]
            } else {
                sorted.into_iter().filter(|x| proc_key(x).0 == c.selected_proc).collect()
            }
        } else {
            sorted
        };
        let sel_name = groups.get(&c.selected_proc).map(|g| g.0.clone()).unwrap_or_default();
        (rows, procs, count, c.search.clone(), c.sort_key.clone(), c.sort_desc, c.paused, c.selected_proc.clone(), sel_name, need)
    });
    for k in need_icons {
        load_icon(k);
    }
    let live_ok = !show_closed;
    let in_process = view == "process";
    let rows: Vec<ConnRow> = rows.iter().map(|c| row(c, live_ok, !in_process)).collect();
    ui(move |w| {
        let cp = w.global::<ConnP>();
        cp.set_view(view.into());
        cp.set_count(count);
        cp.set_sort_key(key.into());
        cp.set_sort_desc(desc);
        cp.set_paused(paused);
        cp.set_show_closed(show_closed);
        cp.set_selected_proc(sel.into());
        cp.set_selected_proc_name(sel_name.into());
        if cp.get_search().as_str() != search {
            cp.set_search(search.into());
        }
        let sort_options: Vec<SelectOption> = ["time", "download", "upload", "dl-speed", "ul-speed", "host"]
            .iter()
            .map(|k| SelectOption { value: (*k).into(), label: tr(&format!("connections.sort.{k}")).into() })
            .collect();
        if let Some(m) = super::sync_model(cp.get_sort_options(), sort_options) {
            cp.set_sort_options(m);
        }
        // in place: a model swap every second made the ListView rebuild its rows
        if let Some(m) = super::sync_model(cp.get_rows(), rows) {
            cp.set_rows(m);
        }
        if !in_process {
            return;
        }
        cp.set_procs(model(
            procs
                .into_iter()
                .map(|(k, n, cnt, d, u, core, icon)| ProcRow {
                    key: k.into(),
                    name: n.into(),
                    count: cnt,
                    download: pretty_bytes(d).into(),
                    upload: pretty_bytes(u).into(),
                    has_icon: icon.is_some(),
                    icon: icon.as_ref().map(crate::theme::to_slint).unwrap_or_default(),
                    core,
                })
                .collect(),
        ));
    });
}

fn load_icon(path: String) {
    if !with(|c| c.icon_loading.insert(path.clone())) {
        return;
    }
    std::thread::spawn(move || {
        let img = crate::fileicon::icon(&path);
        with(|c| {
            c.icons.insert(path.clone(), img);
            c.icon_loading.remove(&path);
        });
        push();
    });
}

// ---- topology (sankey) ----

fn rule_type(rule: &str) -> String {
    if rule.is_empty() {
        return "Direct".into();
    }
    if rule == "Match" {
        return "Match".into();
    }
    for p in ["RuleSet:", "DOMAIN", "IP-CIDR", "GEOIP", "PROCESS", "AND", "OR", "NOT"] {
        if rule.starts_with(p) {
            return p.trim_end_matches(':').into();
        }
    }
    let re = regex::Regex::new(r"^([A-Z-]+)[,:]").unwrap();
    re.captures(rule).map(|m| m[1].to_string()).unwrap_or_else(|| rule.to_string())
}

fn format_rule(rule: &str, payload: &str) -> String {
    if rule.is_empty() {
        return "Direct".into();
    }
    let t = rule_type(rule);
    if payload.is_empty() {
        return t;
    }
    let p: String = if payload.chars().count() > 40 { format!("{}...", payload.chars().take(40).collect::<String>()) } else { payload.to_string() };
    format!("{t}: {p}")
}

fn add_flag(name: &str) -> String {
    if name.is_empty() || name.eq_ignore_ascii_case("direct") {
        return name.to_string();
    }
    let map = [
        ("US", "🇺🇸"), ("UK", "🇬🇧"), ("HK", "🇭🇰"), ("JP", "🇯🇵"), ("SG", "🇸🇬"), ("KR", "🇰🇷"), ("TW", "🇹🇼"), ("CN", "🇨🇳"), ("DE", "🇩🇪"), ("FR", "🇫🇷"),
        ("CA", "🇨🇦"), ("AU", "🇦🇺"), ("RU", "🇷🇺"), ("IN", "🇮🇳"), ("BR", "🇧🇷"), ("NL", "🇳🇱"), ("SE", "🇸🇪"), ("CH", "🇨🇭"), ("IT", "🇮🇹"), ("ES", "🇪🇸"),
    ];
    for (code, flag) in map {
        let re = regex::Regex::new(&format!(r"(?i)\b{code}\b")).unwrap();
        if re.is_match(name) && !name.contains(flag) {
            return format!("{flag} {name}");
        }
    }
    name.to_string()
}

struct Sankey {
    nodes: Vec<(f32, f32, f32, f32, u32, String, String)>,
    links: Vec<(String, u32, String)>,
}

fn layout(conns: &[Value], w: f32, h: f32) -> Sankey {
    let mut node_names: BTreeSet<String> = BTreeSet::new();
    let mut links: HashMap<(String, String), f64> = HashMap::new();
    let mut dests: HashMap<String, BTreeSet<String>> = HashMap::new();
    for c in conns {
        let m = c.get("metadata").cloned().unwrap_or(Value::Null);
        let mut flow = vec![];
        let p = str_of(&m, "process");
        let t = str_of(&m, "type");
        flow.push(if !p.is_empty() { p } else if !t.is_empty() { t } else { "Unknown".into() });
        flow.push(format_rule(&str_of(c, "rule"), &str_of(c, "rulePayload")));
        let ch = chains(c);
        if ch.is_empty() {
            flow.push("Direct".into());
        } else {
            for x in ch.iter().rev() {
                flow.push(add_flag(x));
            }
        }
        let host = str_of(&m, "host");
        let dest = if !host.is_empty() { host } else { str_of(&m, "destinationIP") };
        dests.entry(flow.last().cloned().unwrap_or_default()).or_default().insert(if dest.is_empty() { "Unknown".into() } else { dest });
        for n in &flow {
            node_names.insert(n.clone());
        }
        for pair in flow.windows(2) {
            if pair[0] != pair[1] {
                *links.entry((pair[0].clone(), pair[1].clone())).or_insert(0.0) += 1.0;
            }
        }
    }
    if node_names.is_empty() || w < 50.0 || h < 50.0 {
        return Sankey { nodes: vec![], links: vec![] };
    }
    // scaled values like the original (log10(n+1)*10)
    let links: Vec<(String, String, f64, f64)> = links.into_iter().map(|((s, t), n)| (s, t, (n + 1.0).log10() * 10.0, n)).collect();
    let names: Vec<String> = node_names.into_iter().collect();
    let idx: HashMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
    // depth = longest path from a source (cycles guarded)
    let mut depth = vec![0usize; names.len()];
    for _ in 0..names.len() {
        let mut changed = false;
        for (s, t, _, _) in &links {
            let (si, ti) = (idx[s.as_str()], idx[t.as_str()]);
            if depth[ti] < depth[si] + 1 && depth[si] + 1 < names.len() {
                depth[ti] = depth[si] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let max_depth = *depth.iter().max().unwrap_or(&0);
    // sinks to the last column (sankey "justify")
    let has_out: HashSet<usize> = links.iter().map(|(s, _, _, _)| idx[s.as_str()]).collect();
    for (i, d) in depth.iter_mut().enumerate() {
        if !has_out.contains(&i) {
            *d = max_depth;
        }
    }
    let mut vin = vec![0f64; names.len()];
    let mut vout = vec![0f64; names.len()];
    for (s, t, v, _) in &links {
        vout[idx[s.as_str()]] += v;
        vin[idx[t.as_str()]] += v;
    }
    let value: Vec<f64> = (0..names.len()).map(|i| vin[i].max(vout[i]).max(1.0)).collect();
    let (top, bottom, left, right) = (10.0f32, 10.0f32, 5.0f32, 80.0f32);
    let node_w = 25.0f32;
    let gap = 12.0f32;
    let ih = h - top - bottom;
    let cols = max_depth + 1;
    let col_x = |d: usize| left + if cols <= 1 { 0.0 } else { (w - left - right - node_w) * d as f32 / (cols - 1) as f32 };
    // scale so the fullest column fits
    let mut ky = f32::MAX;
    for d in 0..cols {
        let members: Vec<usize> = (0..names.len()).filter(|i| depth[*i] == d).collect();
        if members.is_empty() {
            continue;
        }
        let sum: f64 = members.iter().map(|i| value[*i]).sum();
        let k = (ih - gap * (members.len() as f32 - 1.0)).max(10.0) / sum as f32;
        ky = ky.min(k);
    }
    let mut pos = vec![(0f32, 0f32, 0f32); names.len()]; // x, y, height
    for d in 0..cols {
        let mut y = top;
        for i in (0..names.len()).filter(|i| depth[*i] == d) {
            let hh = value[i] as f32 * ky;
            pos[i] = (col_x(d), y, hh);
            y += hh + gap;
        }
    }
    let color = |i: usize| PALETTE[i % PALETTE.len()];
    // links as filled bands
    let mut out_off = vec![0f32; names.len()];
    let mut in_off = vec![0f32; names.len()];
    let mut sorted = links.clone();
    sorted.sort_by(|a, b| {
        let (ay, by) = (pos[idx[a.1.as_str()]].1, pos[idx[b.1.as_str()]].1);
        ay.partial_cmp(&by).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut link_out = vec![];
    for (s, t, v, n) in &sorted {
        let (si, ti) = (idx[s.as_str()], idx[t.as_str()]);
        let th = *v as f32 * ky;
        let x0 = pos[si].0 + node_w;
        let x1 = pos[ti].0;
        let y0 = pos[si].1 + out_off[si];
        let y1 = pos[ti].1 + in_off[ti];
        out_off[si] += th;
        in_off[ti] += th;
        let xm = (x0 + x1) / 2.0;
        let path = format!(
            "M {x0} {y0} C {xm} {y0} {xm} {y1} {x1} {y1} L {x1} {} C {xm} {} {xm} {} {x0} {} Z",
            y1 + th,
            y1 + th,
            y0 + th,
            y0 + th
        );
        link_out.push((path, color(si), format!("{s} → {t}\nConnections: {n}")));
    }
    let nodes = (0..names.len())
        .map(|i| {
            let name = &names[i];
            let label = if name.chars().count() > 35 { format!("{}...", name.chars().take(35).collect::<String>()) } else { name.clone() };
            let tip = match dests.get(name) {
                Some(d) if !d.is_empty() => {
                    let mut t = String::from("Destinations:\n");
                    for x in d.iter().take(10) {
                        t.push_str(&format!("• {x}\n"));
                    }
                    if d.len() > 10 {
                        t.push_str(&format!("... and {} more", d.len() - 10));
                    }
                    t.trim_end().to_string()
                }
                _ => name.clone(),
            };
            (pos[i].0, pos[i].1, node_w, pos[i].2, color(i), label, tip)
        })
        .collect();
    Sankey { nodes, links: link_out }
}

fn push_topology(force: bool) {
    let (conns, size, sig_prev, last, paused, visible) = with(|c| (c.active.clone(), c.topo_size, c.topo_sig.clone(), c.topo_last, c.topo_paused, c.visible));
    if !visible || paused || st().conns.view_mode != "topology" {
        return;
    }
    if !force {
        if let Some(l) = last {
            if l.elapsed() < Duration::from_millis(3000) {
                return;
            }
        }
    }
    let lay = layout(&conns, size.0, size.1);
    let sig = format!("{:?}", lay.nodes.iter().map(|n| (&n.5, n.1 as i32, n.3 as i32)).collect::<Vec<_>>());
    if !force && sig == sig_prev {
        return;
    }
    with(|c| {
        c.topo_sig = sig;
        c.topo_last = Some(Instant::now());
    });
    ui(move |w| {
        let cp = w.global::<ConnP>();
        let rgb = |c: u32| slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8);
        cp.set_topo_links(model(lay.links.into_iter().map(|(p, c, t)| TopoLink { path: p.into(), color: rgb(c), tip: t.into() }).collect()));
        cp.set_topo_nodes(model(
            lay.nodes
                .into_iter()
                .map(|(x, y, w, h, c, l, t)| TopoNode { x, y, w, h, color: rgb(c), label: l.into(), tip: t.into() })
                .collect(),
        ));
    });
}

// ---- connection detail dialog ----

fn duration_of(c: &Value) -> String {
    let ms = start_ms(c);
    let el = ((util::now_ms() - ms) / 1000.0).floor();
    if el < 0.0 || ms == 0.0 {
        return "—".into();
    }
    let el = el as i64;
    format!("{:02}:{:02}:{:02}", el / 3600, (el % 3600) / 60, el % 60)
}

fn push_log_dialog() {
    let Some(item) = with(|c| {
        let id = c.log_id.clone();
        c.active.iter().chain(c.closed.iter()).find(|x| str_of(x, "id") == id).cloned().or(c.log_item.clone())
    }) else {
        return;
    };
    let m = item.get("metadata").cloned().unwrap_or(Value::Null);
    let kv = |l: &str, v: String, k: &str, p: &str| LogKV { label: tr(l).into(), value: v.into(), kind: k.into(), path: p.into() };
    let mut sections: Vec<(String, Vec<LogKV>)> = vec![];
    let us = num(&item, "uploadSpeed");
    let ds = num(&item, "downloadSpeed");
    sections.push((
        tr("connections.section-traffic"),
        vec![
            kv("connections.upload-speed", if us > 0.0 { format!("{}/s", pretty_bytes(us)) } else { "—".into() }, "text", ""),
            kv("connections.download-speed", if ds > 0.0 { format!("{}/s", pretty_bytes(ds)) } else { "—".into() }, "text", ""),
            kv("connections.upload", pretty_bytes(num(&item, "upload")), "text", ""),
            kv("connections.download", pretty_bytes(num(&item, "download")), "text", ""),
            kv("connections.duration", duration_of(&item), "text", ""),
        ],
    ));
    let mut routing = vec![];
    let rule = str_of(&item, "rule");
    if !rule.is_empty() {
        let v = [rule, str_of(&item, "rulePayload")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" / ");
        routing.push(kv("connections.rule", v, "text", ""));
    }
    routing.push(kv("connections.proxy-chain", format_chains(&chains(&item)), "text", ""));
    routing.push(kv("connections.conn-type", conn_type(&m), "text", ""));
    sections.push((tr("connections.section-routing"), routing));
    let mut net = vec![];
    let opt = |k: &str| str_of(&m, k);
    if !opt("host").is_empty() {
        net.push(kv("connections.host", opt("host"), "host", ""));
    }
    if !opt("sniffHost").is_empty() && opt("sniffHost") != opt("host") {
        net.push(kv("connections.sniff-host", opt("sniffHost"), "host", ""));
    }
    if !opt("destinationIP").is_empty() {
        net.push(kv("connections.dest-ip", opt("destinationIP"), "ip", ""));
    }
    if !opt("sourceIP").is_empty() {
        net.push(kv("connections.source-ip", opt("sourceIP"), "ip", ""));
    }
    if !opt("sourcePort").is_empty() {
        net.push(kv("connections.source-port", opt("sourcePort"), "text", ""));
    }
    if !opt("destinationPort").is_empty() {
        net.push(kv("connections.dest-port", opt("destinationPort"), "text", ""));
    }
    if !opt("remoteDestination").is_empty() {
        net.push(kv("connections.remote-dest", opt("remoteDestination"), "ip", ""));
    }
    sections.push((tr("connections.section-network"), net));
    if opt("type") != "Inner" && (!opt("process").is_empty() || !opt("processPath").is_empty()) {
        let mut p = vec![];
        if !opt("process").is_empty() {
            p.push(kv("connections.process-label", opt("process"), "process", &opt("processPath")));
        }
        if !opt("processPath").is_empty() && opt("processPath") != opt("process") {
            p.push(kv("connections.process-path", opt("processPath"), "path", &opt("processPath")));
        }
        sections.push((tr("connections.section-process"), p));
    }
    if ["inboundIP", "inboundPort", "inboundName", "inboundUser"].iter().any(|k| !opt(k).is_empty()) {
        let mut p = vec![];
        if !opt("inboundIP").is_empty() {
            p.push(kv("connections.inbound-ip", opt("inboundIP"), "ip", ""));
        }
        if !opt("inboundPort").is_empty() {
            p.push(kv("connections.inbound-port", opt("inboundPort"), "text", ""));
        }
        if !opt("inboundName").is_empty() {
            p.push(kv("connections.inbound-name", opt("inboundName"), "text", ""));
        }
        if !opt("inboundUser").is_empty() {
            p.push(kv("connections.inbound-user", opt("inboundUser"), "text", ""));
        }
        sections.push((tr("connections.section-inbound"), p));
    }
    if ["dnsMode", "specialProxy", "specialRules", "dscp"].iter().any(|k| !opt(k).is_empty() && opt(k) != "0") {
        let mut p = vec![];
        for (k, l) in [("dnsMode", "connections.dns-mode"), ("specialProxy", "connections.special-proxy"), ("specialRules", "connections.special-rules"), ("dscp", "connections.dscp")] {
            if !opt(k).is_empty() && opt(k) != "0" {
                p.push(kv(l, opt(k), "text", ""));
            }
        }
        sections.push((tr("connections.section-other"), p));
    }
    let title = {
        let h = if !opt("host").is_empty() { opt("host") } else { opt("destinationIP") };
        let p = opt("destinationPort");
        if !h.is_empty() && !p.is_empty() {
            format!("{h}:{p}")
        } else if !h.is_empty() {
            h
        } else {
            tr("connections.dialog-title")
        }
    };
    ui(move |w| {
        let cp = w.global::<ConnP>();
        cp.set_log_title(title.into());
        cp.set_log_sections(model(sections.into_iter().map(|(t, rows)| LogSection { title: t.into(), rows: model(rows) }).collect()));
    });
}

fn copy_conn(id: &str) {
    let item = with(|c| c.active.iter().chain(c.closed.iter()).find(|x| str_of(x, "id") == id).cloned());
    if let Some(i) = item {
        app::copy_text(serde_json::to_string_pretty(&i).unwrap_or_default(), tr("copy.success"));
    }
}

async fn close_all() {
    let ok = app::confirm(tr("confirm.close-all.title"), tr("confirm.close-all.text"), tr("confirm.close-all.ok"), "plug-connected-x").await;
    if !ok {
        return;
    }
    let (data, search) = with(|c| (filter_sort(c.active.clone(), &c.search, &c.sort_key, c.sort_desc), c.search.clone()));
    if data.is_empty() {
        return;
    }
    let api = ctx().api.clone();
    if search.is_empty() {
        let _ = api.close_all_connections().await;
    } else {
        for c in data {
            let _ = api.close_connection(&str_of(&c, "id")).await;
        }
    }
}

// ---- logs ----

/// segments() of LogTab.vue as markdown: every case-insensitive match in a
/// coloured bold span, the rest escaped so log text is taken literally.
fn mark_hits(text: &str, q_lower: &str, color: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if q_lower.is_empty() || lower.len() != text.len() || !lower.contains(q_lower) {
        return None;
    }
    let esc = |s: &str| {
        let mut o = String::with_capacity(s.len() + 8);
        for c in s.chars() {
            if c.is_ascii_punctuation() {
                o.push('\\');
            }
            o.push(c);
        }
        o
    };
    let mut out = String::new();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(q_lower) {
        let at = from + rel;
        let end = at + q_lower.len();
        if !text.is_char_boundary(at) || !text.is_char_boundary(end) {
            return None;
        }
        out.push_str(&esc(&text[from..at]));
        out.push_str(&format!("<font color=\"{color}\">**{}**</font>", esc(&text[at..end])));
        from = end;
    }
    out.push_str(&esc(&text[from..]));
    Some(out)
}

pub fn push_logs() {
    let (paused, snapshot, search, auto) = with(|c| (c.log_paused, c.log_snapshot.clone(), c.log_search.clone(), c.log_autoscroll));
    let source: Vec<crate::ws::LogEntry> = if paused { snapshot } else { crate::ws::LOGS.lock().iter().cloned().collect() };
    let has_any = !source.is_empty();
    let q = search.trim().to_lowercase();
    let hit_color = if st().dark { "#f2c97d" } else { "#f0a020" };
    let lines: Vec<(i64, String, String, i32, String, Option<String>)> = source
        .into_iter()
        .filter(|l| q.is_empty() || l.payload.to_lowercase().contains(&q) || l.kind.to_lowercase().contains(&q))
        .map(|l| {
            let tone = match l.kind.as_str() {
                "ERROR" => 2,
                "WARNING" | "WARN" => 1,
                "DEBUG" => 3,
                _ => 0,
            };
            let marked = if q.is_empty() { None } else { mark_hits(&l.payload, &q, hit_color) };
            (l.id as i64, l.time, l.kind, tone, l.payload, marked)
        })
        .collect();
    let _ = auto;
    // A changed filter (search / pause) changes which lines show and how they
    // are marked: rebuild then. Otherwise new lines only arrive at the end and
    // old ones leave at the front (1000-line ring), so the list is trimmed and
    // appended in place and the cost per update does not grow with its length.
    static LAST_FILTER: Mutex<Option<(String, bool)>> = Mutex::new(None);
    let rebuild = LAST_FILTER.lock().replace((q.clone(), paused)) != Some((q, paused));
    ui(move |w| {
        use slint::Model;
        let lp = w.global::<LogsP>();
        lp.set_has_any(has_any);
        lp.set_paused(paused);
        lp.set_autoscroll(auto);
        lp.set_lines_tick(lp.get_lines_tick().wrapping_add(1));
        let to_line = |(id, t, k, tone, p, marked): (i64, String, String, i32, String, Option<String>)| {
            let has_marks = marked.is_some();
            let marked = marked.and_then(|m| slint::StyledText::from_markdown(&m).ok()).unwrap_or_default();
            LogLine { id: id as i32, time: t.into(), kind: k.into(), tone, payload: p.into(), marked, has_marks }
        };
        let cur = lp.get_lines();
        if let Some(vm) = cur.as_any().downcast_ref::<slint::VecModel<LogLine>>() {
            if !rebuild {
                let first_new = lines.first().map(|l| l.0 as i32);
                while vm.row_count() > 0 && first_new.map(|f| vm.row_data(0).map(|r| r.id < f).unwrap_or(false)).unwrap_or(true) {
                    vm.remove(0);
                }
                let n = vm.row_count();
                let prefix_ok = n <= lines.len()
                    && (n == 0
                        || (vm.row_data(0).map(|r| r.id as i64) == Some(lines[0].0)
                            && vm.row_data(n - 1).map(|r| r.id as i64) == Some(lines[n - 1].0)));
                if prefix_ok {
                    for l in lines.into_iter().skip(n) {
                        vm.push(to_line(l));
                    }
                    return;
                }
            }
            vm.set_vec(lines.into_iter().map(to_line).collect::<Vec<_>>());
            return;
        }
        lp.set_lines(model(lines.into_iter().map(to_line).collect()));
    });
}

fn filtered_logs_text() -> (usize, String) {
    let (paused, snapshot, search) = with(|c| (c.log_paused, c.log_snapshot.clone(), c.log_search.clone()));
    let source: Vec<crate::ws::LogEntry> = if paused { snapshot } else { crate::ws::LOGS.lock().iter().cloned().collect() };
    let q = search.trim().to_lowercase();
    let list: Vec<String> = source
        .into_iter()
        .filter(|l| q.is_empty() || l.payload.to_lowercase().contains(&q) || l.kind.to_lowercase().contains(&q))
        .map(|l| format!("{} [{}] {}", l.time, l.kind, l.payload))
        .collect();
    (list.len(), list.join("\n"))
}

pub fn on_log() {
    let paused = with(|c| {
        c.log_dirty = true;
        c.log_paused
    });
    let _ = paused;
}

/// Throttled log refresh (called from the 1 s ticker and on new lines).
pub fn flush_logs(force: bool) {
    let visible_log = {
        let s = st();
        s.menu.setting_tab == "log" && s.menu.menu == "Setting"
    };
    let dirty = with(|c| std::mem::replace(&mut c.log_dirty, false));
    if visible_log && (dirty || force) {
        push_logs();
    }
}

pub fn wire(w: &AppWindow) {
    let cp = w.global::<ConnP>();
    cp.on_set_view(|v| {
        st().conns.view_mode = v.to_string();
        save("connections");
        with(|c| {
            c.selected_proc.clear();
            c.topo_sig.clear();
        });
        push();
        push_topology(true);
    });
    cp.on_set_show_closed(|v| {
        SHOW_CLOSED.store(v, std::sync::atomic::Ordering::Relaxed);
        with(|c| c.frozen = None);
        push();
    });
    cp.on_search_edited(|s| {
        with(|c| c.search = s.to_string());
        push();
    });
    cp.on_set_sort(|k| {
        with(|c| c.sort_key = k.to_string());
        push();
    });
    cp.on_toggle_dir(|| {
        with(|c| c.sort_desc = !c.sort_desc);
        push();
    });
    cp.on_toggle_pause(|| {
        with(|c| {
            c.paused = !c.paused;
            c.frozen = if c.paused { Some(display_source(c)) } else { None };
        });
        push();
    });
    cp.on_close_all(|| spawn(close_all()));
    cp.on_open_log(|id| {
        with(|c| {
            c.log_id = id.to_string();
            c.log_item = c.active.iter().chain(c.closed.iter()).find(|x| str_of(x, "id") == id.as_str()).cloned();
        });
        push_log_dialog();
        ui(|w| w.global::<ConnP>().set_log_open(true));
    });
    cp.on_copy_log(|id| copy_conn(&id));
    cp.on_log_copy(|| {
        let id = with(|c| c.log_id.clone());
        copy_conn(&id);
    });
    cp.on_kv_copy(|v| app::copy_text(v.to_string(), tr("copy.success")));
    cp.on_kv_open(|u| app::open_url(&u));
    cp.on_kv_folder(|p| crate::fileicon::show_in_folder(&p));
    cp.on_select_proc(|k| {
        with(|c| c.selected_proc = k.to_string());
        push();
    });
    cp.on_topo_toggle_pause(|| {
        let p = with(|c| {
            c.topo_paused = !c.topo_paused;
            c.topo_paused
        });
        ui(move |w| w.global::<ConnP>().set_topo_paused(p));
        if !p {
            push_topology(true);
        }
    });
    cp.on_topo_size(|w, h| {
        let changed = with(|c| {
            let ch = (c.topo_size.0 - w).abs() > 1.0 || (c.topo_size.1 - h).abs() > 1.0;
            c.topo_size = (w, h);
            ch
        });
        if changed {
            push_topology(true);
        }
    });

    let lp = w.global::<LogsP>();
    lp.on_search_edited(|s| {
        with(|c| c.log_search = s.to_string());
        push_logs();
    });
    lp.on_toggle_pause(|| {
        with(|c| {
            c.log_paused = !c.log_paused;
            c.log_snapshot = if c.log_paused { crate::ws::LOGS.lock().iter().cloned().collect() } else { vec![] };
        });
        push_logs();
    });
    lp.on_toggle_autoscroll(|| {
        with(|c| c.log_autoscroll = !c.log_autoscroll);
        push_logs();
    });
    lp.on_copy_all(|| {
        let (n, text) = filtered_logs_text();
        if n == 0 {
            return;
        }
        app::copy_text(text, tr_args("logs.copied", &[("n", n.to_string())]));
    });
    lp.on_export(|| {
        let (n, text) = filtered_logs_text();
        if n == 0 {
            return;
        }
        spawn(async move {
            let name = format!("prizrak-box-{}.log", chrono::Local::now().format("%Y-%m-%d"));
            if let Some(f) = rfd::AsyncFileDialog::new().set_file_name(&name).save_file().await {
                let _ = std::fs::write(f.path(), text + "\n");
            }
        });
    });
    lp.on_clear(|| {
        spawn(async {
            if crate::ws::LOGS.lock().is_empty() {
                return;
            }
            if app::confirm(tr("logs.clear"), String::new(), tr("logs.clear"), "trash").await {
                crate::ws::LOGS.lock().clear();
                with(|c| c.log_snapshot.clear());
                push_logs();
                app::toast("info", tr("logs.cleared"));
            }
        });
    });
}

pub fn on_show(tab: &str) {
    match tab {
        "connection" => {
            with(|c| {
                c.visible = true;
                c.topo_sig.clear();
            });
            crate::ws::start_connections();
            push();
        }
        "log" => push_logs(),
        _ => {}
    }
}

pub fn on_hide() {
    if with(|c| std::mem::replace(&mut c.visible, false)) {
        crate::ws::stop_connections();
        with(|c| {
            c.active.clear();
            c.prev.clear();
        });
    }
}

/// Once a second while the dialog is open (durations/ages tick).
pub fn tick() {
    if with(|c| c.visible) {
        if !with(|c| c.paused) {
            push();
        }
        if ui_log_open() {
            push_log_dialog();
        }
    }
    flush_logs(false);
}

static LOG_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn ui_log_open() -> bool {
    LOG_OPEN.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn sync_flags(w: &AppWindow) {
    LOG_OPEN.store(w.global::<ConnP>().get_log_open(), std::sync::atomic::Ordering::Relaxed);
}
