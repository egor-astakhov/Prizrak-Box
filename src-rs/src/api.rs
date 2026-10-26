//! HTTP client for the px control API — port of `src/api/*` and
//! `src/util/axiosRequest.ts`. Every request carries `Authorization: Bearer
//! <secret>`; a non-2xx answer becomes an [`ApiError`] carrying the JSON body
//! px sent (its `message`, and HWID fields for profile requests).

use std::collections::HashMap;
use std::time::Duration;

use parking_lot::RwLock;
use reqwest::Method;
use serde_json::{json, Map, Value};

use crate::core::ConnInfo;

#[derive(Debug, Clone)]
pub struct ApiError {
    pub message: String,
    pub body: Value,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

pub type ApiResult<T = Value> = Result<T, ApiError>;

fn err(msg: impl Into<String>) -> ApiError {
    ApiError { message: msg.into(), body: Value::Null }
}

pub struct Api {
    conn: RwLock<ConnInfo>,
    client: reqwest::Client,
    origins: tokio::sync::Mutex<(Option<Map<String, Value>>, std::time::Instant)>,
    providers: tokio::sync::Mutex<(Option<Map<String, Value>>, std::time::Instant)>,
}

const EXCLUDE_GROUPS: &[&str] = &["DIRECT", "REJECT", "REJECT-DROP", "PASS", "PASS-RULE", "COMPATIBLE", "GLOBAL"];
const INCLUDE_GROUP_TYPES: &[&str] = &["Selector", "URLTest", "Fallback", "LoadBalance", "Smart", "Relay"];

#[derive(Clone, Debug, Default)]
pub struct GroupInfo {
    pub name: String,
    pub icon: String,
    pub kind: String,
}

#[derive(Clone, Debug, Default)]
pub struct ProxyNode {
    pub name: String,
    pub kind: String,
    pub display_type: String,
    pub icon: String,
    pub delay: i64,
    pub now: bool,
    pub display_name: String,
    pub origin: String,
}

impl Api {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .no_proxy()
            .build()
            .expect("http client");
        let stale = std::time::Instant::now() - Duration::from_secs(60);
        Api {
            conn: RwLock::new(ConnInfo { host: "127.0.0.1".into(), port: 9686, secret: String::new() }),
            client,
            origins: tokio::sync::Mutex::new((None, stale)),
            providers: tokio::sync::Mutex::new((None, stale)),
        }
    }

    pub fn set_conn(&self, info: ConnInfo) {
        *self.conn.write() = info;
    }

    pub fn conn(&self) -> ConnInfo {
        self.conn.read().clone()
    }

    pub fn base_url(&self) -> String {
        let c = self.conn.read();
        format!("http://{}:{}", c.host, c.port)
    }

    pub fn ws_url(&self, path: &str) -> String {
        let c = self.conn.read();
        let sep = if path.contains('?') { '&' } else { '?' };
        format!("ws://{}:{}{}{}token={}", c.host, c.port, path, sep, c.secret)
    }

    pub async fn request(&self, method: Method, path: &str, body: Option<Value>) -> ApiResult {
        self.request_timeout(method, path, body, None).await
    }

    pub async fn request_timeout(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        timeout: Option<Duration>,
    ) -> ApiResult {
        let (url, secret) = {
            let c = self.conn.read();
            (format!("http://{}:{}{}", c.host, c.port, path), c.secret.clone())
        };
        let mut rb = self
            .client
            .request(method, &url)
            .header("Authorization", format!("Bearer {secret}"))
            .header("Content-Type", "application/json");
        if let Some(t) = timeout {
            rb = rb.timeout(t);
        }
        if let Some(b) = body {
            rb = rb.body(serde_json::to_vec(&b).unwrap_or_default());
        }
        let resp = rb.send().await.map_err(|e| err(e.to_string()))?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| err(e.to_string()))?;
        let value: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()))
        };
        if status.is_success() {
            if status.as_u16() == 200 {
                Ok(value)
            } else {
                Ok(Value::Null)
            }
        } else {
            let message = match &value {
                Value::Object(m) => m
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| status.to_string()),
                Value::String(s) if !s.is_empty() => s.clone(),
                _ => status.to_string(),
            };
            Err(ApiError { message, body: value })
        }
    }

    pub async fn get(&self, path: &str) -> ApiResult {
        self.request(Method::GET, path, None).await
    }
    pub async fn post(&self, path: &str, body: Value) -> ApiResult {
        self.request(Method::POST, path, Some(body)).await
    }
    pub async fn put(&self, path: &str, body: Value) -> ApiResult {
        self.request(Method::PUT, path, Some(body)).await
    }
    pub async fn patch(&self, path: &str, body: Value) -> ApiResult {
        self.request(Method::PATCH, path, Some(body)).await
    }
    pub async fn delete(&self, path: &str) -> ApiResult {
        self.request(Method::DELETE, path, None).await
    }

    // ---- home ----
    pub async fn version(&self) -> ApiResult<String> {
        let v = self.get("/version").await?;
        Ok(v.get("version").and_then(Value::as_str).unwrap_or_default().to_string())
    }
    pub async fn configs(&self) -> ApiResult {
        self.get("/configs").await
    }
    pub async fn update_configs(&self, v: Value) -> ApiResult {
        self.patch("/configs", v).await
    }
    /// md5 over the sorted (name, now) list of groups — `getGroupMd5`.
    pub async fn group_md5(&self) -> ApiResult<String> {
        let data = self.get("/group").await?;
        let Some(list) = data.get("proxies").and_then(Value::as_array) else {
            return Ok(String::new());
        };
        let mut end: Vec<(String, Value)> = list
            .iter()
            .map(|p| {
                (
                    p.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                    p.get("now").cloned().unwrap_or(Value::Null),
                )
            })
            .collect();
        end.sort_by(|a, b| a.0.cmp(&b.0));
        let arr: Vec<Value> = end.into_iter().map(|(n, now)| json!({"name": n, "now": now})).collect();
        let s = serde_json::to_string(&arr).unwrap_or_default();
        Ok(format!("{:x}", md5::compute(s)))
    }
    pub async fn web_test_ip(&self, url: &str) -> ApiResult {
        self.post("/webtest/ip", json!({ "url": url })).await
    }

    // ---- mihomo / prizrak ----
    pub async fn mihomo(&self) -> ApiResult {
        self.get("/mihomo").await
    }
    pub async fn update_mihomo(&self, v: Value) -> ApiResult {
        self.put("/mihomo", v).await
    }
    pub async fn admin(&self) -> bool {
        self.get("/mihomo/admin")
            .await
            .ok()
            .and_then(|v| v.get("data").and_then(Value::as_bool))
            .unwrap_or(false)
    }
    /// Polls /version until px answers; never fails (returns false on timeout).
    pub async fn wait_running(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.request_timeout(Method::GET, "/version", None, Some(Duration::from_secs(3))).await.is_ok() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    pub async fn enable_proxy(&self, bind: &str, port: i64, username: &str) -> ApiResult {
        self.put("/prizrak/enableProxy", json!({"bindAddress": bind, "port": port, "username": username})).await
    }
    pub async fn disable_proxy(&self, username: &str) -> ApiResult {
        self.put("/prizrak/disableProxy", json!({ "username": username })).await
    }
    pub async fn check_address_port(&self, bind: &str, port: i64) -> ApiResult {
        self.put("/prizrak/checkAddressPort", json!({"bindAddress": bind, "port": port})).await
    }
    pub async fn config_dir(&self) -> ApiResult {
        self.get("/prizrak/configDir").await
    }
    pub async fn exit(&self) -> ApiResult {
        self.request_timeout(Method::GET, "/prizrak/exit", None, Some(Duration::from_secs(5))).await
    }
    pub async fn update_http_client_config(&self, v: Value) -> ApiResult {
        self.put("/prizrak/httpClientConfig", v).await
    }

    // ---- profiles ----
    pub async fn profiles(&self) -> ApiResult<Vec<Value>> {
        let v = self.get("/profile").await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }
    pub async fn add_profile(&self, profile: Value) -> ApiResult<Vec<Value>> {
        let v = self.request_timeout(Method::POST, "/profile", Some(profile), Some(Duration::from_secs(120))).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }
    pub async fn delete_profile(&self, profile: Value) -> ApiResult {
        self.post("/profile/delete", profile).await
    }
    pub async fn update_profile(&self, profile: Value) -> ApiResult {
        self.put("/profile", profile).await
    }
    pub async fn refresh_profile(&self, profile: Value) -> ApiResult {
        self.request_timeout(Method::PUT, "/profile/refresh", Some(profile), Some(Duration::from_secs(120))).await
    }
    pub async fn switch_profile(&self, id: &str, selected: bool, exclusive: bool) -> ApiResult {
        self.request_timeout(
            Method::PATCH,
            "/profile",
            Some(json!({"id": id, "selected": selected, "exclusive": exclusive})),
            Some(Duration::from_secs(120)),
        )
        .await
    }
    pub async fn ack_subscription_alert(&self, id: &str) -> ApiResult {
        self.put("/profile/ackSubscriptionAlert", json!({ "id": id })).await
    }

    // ---- dns ----
    pub async fn dns(&self) -> ApiResult {
        self.get("/pDns").await
    }
    pub async fn update_dns(&self, v: Value) -> ApiResult {
        self.put("/pDns", v).await
    }
    pub async fn switch_dns(&self, enable: bool) -> ApiResult {
        self.post("/pDns/switch", json!({ "enable": enable })).await
    }

    // ---- rules ----
    pub async fn rules(&self) -> ApiResult<Vec<Value>> {
        let v = self.get("/rules").await?;
        Ok(v.get("rules").and_then(Value::as_array).cloned().unwrap_or_default())
    }
    pub async fn rule_num(&self) -> ApiResult<i64> {
        let v = self.get("/rule/num").await?;
        Ok(v.get("data").and_then(Value::as_i64).unwrap_or(0))
    }

    // ---- connections ----
    pub async fn close_connection(&self, id: &str) -> ApiResult {
        self.delete(&format!("/connections/{}", enc(id))).await
    }
    pub async fn close_all_connections(&self) -> ApiResult {
        self.delete("/connections").await
    }

    // ---- proxies ----
    pub async fn group_delay(&self, group: &str, url: &str, timeout: i64) -> ApiResult {
        self.request_timeout(
            Method::GET,
            &format!("/group/{}/delay?timeout={}&url={}", enc(group), timeout, enc(url)),
            None,
            Some(Duration::from_millis((timeout as u64) + 15000)),
        )
        .await
    }
    pub async fn proxy_delay(&self, name: &str, url: &str, timeout: i64) -> bool {
        self.request_timeout(
            Method::GET,
            &format!("/proxies/{}/delay?timeout={}&url={}", enc(name), timeout, enc(url)),
            None,
            Some(Duration::from_millis((timeout as u64) + 5000)),
        )
        .await
        .is_ok()
    }
    pub async fn group_weights(&self, name: &str) -> Vec<Value> {
        self.get(&format!("/group/{}/weights", enc(name)))
            .await
            .ok()
            .and_then(|v| v.get("weights").and_then(Value::as_array).cloned())
            .unwrap_or_default()
    }
    pub async fn group_test_url(&self, name: &str) -> Option<String> {
        let v = self.get(&format!("/proxies/{}", enc(name))).await.ok()?;
        v.get("testUrl")
            .or_else(|| v.get("url"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
    pub async fn set_proxy(&self, group: &str, name: &str) -> ApiResult {
        self.put(&format!("/proxies/{}", enc(group)), json!({ "name": name })).await
    }

    pub async fn groups(&self) -> ApiResult<Vec<GroupInfo>> {
        let data = self.get("/proxies").await?;
        let Some(proxies) = data.get("proxies").and_then(Value::as_object) else {
            return Ok(vec![]);
        };
        let sort_index: Vec<String> = proxies
            .get("GLOBAL")
            .and_then(|g| g.get("all"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let mut out = vec![];
        for (name, g) in proxies {
            if EXCLUDE_GROUPS.contains(&name.as_str()) {
                continue;
            }
            let kind = g.get("type").and_then(Value::as_str).unwrap_or_default();
            if !INCLUDE_GROUP_TYPES.contains(&kind) {
                continue;
            }
            if g.get("hidden").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            out.push(GroupInfo {
                name: name.clone(),
                icon: g.get("icon").and_then(Value::as_str).unwrap_or_default().to_string(),
                kind: kind.to_string(),
            });
        }
        let idx = |n: &str| sort_index.iter().position(|x| x == n);
        out.sort_by(|a, b| match (idx(&a.name), idx(&b.name)) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(x), Some(y)) => x.cmp(&y),
        });
        Ok(out)
    }

    pub fn reset_caches(&self) {
        if let Ok(mut g) = self.origins.try_lock() {
            g.0 = None;
        }
        if let Ok(mut g) = self.providers.try_lock() {
            g.0 = None;
        }
    }

    async fn proxy_origins(&self) -> Map<String, Value> {
        let mut g = self.origins.lock().await;
        if let Some(m) = &g.0 {
            if g.1.elapsed() < Duration::from_secs(2) {
                return m.clone();
            }
        }
        let m = match self.get("/profile/proxy-origins").await {
            Ok(Value::Object(m)) => m,
            Ok(_) => Map::new(),
            Err(_) => g.0.clone().unwrap_or_default(),
        };
        *g = (Some(m.clone()), std::time::Instant::now());
        m
    }

    async fn provider_proxies(&self) -> Map<String, Value> {
        let mut g = self.providers.lock().await;
        if let Some(m) = &g.0 {
            if g.1.elapsed() < Duration::from_secs(2) {
                return m.clone();
            }
        }
        let mut merged = Map::new();
        if let Ok(data) = self.get("/providers/proxies").await {
            if let Some(providers) = data.get("providers").and_then(Value::as_object) {
                for (name, p) in providers {
                    if name == "default" || p.get("type").and_then(Value::as_str) == Some("Compatible") {
                        continue;
                    }
                    if let Some(list) = p.get("proxies").and_then(Value::as_array) {
                        for px in list {
                            if let Some(n) = px.get("name").and_then(Value::as_str) {
                                merged.insert(n.to_string(), px.clone());
                            }
                        }
                    }
                }
            }
        }
        *g = (Some(merged.clone()), std::time::Instant::now());
        merged
    }

    /// Port of `getProxies(active, isHide, isSort, useIndependentUrl, override, fallback)`.
    pub async fn proxies(
        &self,
        active: &str,
        is_hide: bool,
        is_sort: bool,
        independent: bool,
        override_url: Option<&str>,
        fallback_url: Option<&str>,
    ) -> ApiResult<Vec<ProxyNode>> {
        let data = self.get("/proxies").await?;
        let descriptions: Map<String, Value> = match self.get("/profile/serverDescriptions").await {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        };
        let Some(proxies) = data.get("proxies").and_then(Value::as_object) else { return Ok(vec![]) };
        let Some(group) = proxies.get(active) else { return Ok(vec![]) };
        let group_test_url: Option<String> = if independent {
            override_url
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| group.get("testUrl").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string))
                .or_else(|| group.get("url").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string))
                .or_else(|| fallback_url.map(str::to_string))
        } else {
            None
        };
        let origins = self.proxy_origins().await;
        let has_origins = !origins.is_empty();
        let mut lookup = self.provider_proxies().await;
        for (k, v) in proxies {
            lookup.insert(k.clone(), v.clone());
        }
        let names: Vec<String> = group
            .get("all")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let now_name = group.get("now").and_then(Value::as_str).unwrap_or_default();
        let mut all = vec![];
        for name in &names {
            let Some(node) = lookup.get(name) else { continue };
            let kind = node.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
            let display_type = display_type(node, descriptions.get(name).and_then(Value::as_str));
            let delay = proxy_delay(node, &lookup, 0, group_test_url.as_deref());
            let mut origin = origins.get(name).and_then(Value::as_str).map(str::to_string);
            if origin.is_none() && has_origins {
                origin = parse_origin(name);
            }
            let display_name = format_display_name(name, origin.as_deref());
            all.push(ProxyNode {
                name: name.clone(),
                kind,
                display_type,
                icon: node.get("icon").and_then(Value::as_str).unwrap_or_default().to_string(),
                delay,
                now: name == now_name,
                display_name,
                origin: origin.unwrap_or_default(),
            });
        }
        if is_hide {
            all.retain(|p| p.delay != 99999);
        }
        if is_sort {
            let pos: HashMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
            all.sort_by(|a, b| a.delay.cmp(&b.delay).then(pos.get(a.name.as_str()).cmp(&pos.get(b.name.as_str()))));
        }
        Ok(all)
    }
}

/// `encodeURIComponent`: everything but A–Z a–z 0–9 - _ . ! ~ * ' ( ).
/// px's router matches names in paths literally after its own decoding,
/// which leaves e.g. `%2D` alone — so "ads-all" must stay "ads-all".
const URI_COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

pub fn enc(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, URI_COMPONENT).to_string()
}

#[cfg(test)]
mod enc_tests {
    #[test]
    fn matches_encode_uri_component() {
        assert_eq!(super::enc("ads-all"), "ads-all");
        assert_eq!(super::enc("a_b.c!~*'()"), "a_b.c!~*'()");
        assert_eq!(super::enc("🌍 VPN/x"), "%F0%9F%8C%8D%20VPN%2Fx");
    }
}

fn last_delay(history: &Value) -> Option<i64> {
    history.as_array().and_then(|a| a.last()).and_then(|h| h.get("delay")).and_then(Value::as_i64)
}

pub fn get_delay(node: &Value, test_url: Option<&str>) -> i64 {
    if let Some(url) = test_url {
        if let Some(h) = node.get("extra").and_then(|e| e.get(url)).and_then(|e| e.get("history")) {
            if h.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                let d = last_delay(h).unwrap_or(0);
                return if d > 0 { d } else { 99999 };
            }
        }
    }
    if !node.get("alive").and_then(Value::as_bool).unwrap_or(false) {
        return 99999;
    }
    match node.get("history") {
        Some(h) if h.as_array().map(|a| !a.is_empty()).unwrap_or(false) => last_delay(h).unwrap_or(99999),
        _ => 99999,
    }
}

fn proxy_delay(node: &Value, map: &Map<String, Value>, depth: u32, test_url: Option<&str>) -> i64 {
    if depth > 5 {
        return get_delay(node, test_url);
    }
    if node.get("type").and_then(Value::as_str) == Some("Smart") {
        if let Some(now) = node.get("now").and_then(Value::as_str) {
            if let Some(next) = map.get(now) {
                return proxy_delay(next, map, depth + 1, test_url);
            }
        }
    }
    get_delay(node, test_url)
}

fn display_type(node: &Value, fallback: Option<&str>) -> String {
    let keys = ["serverDescription", "server_description", "server-description"];
    let mut desc: Option<&str> = None;
    for k in keys {
        if let Some(s) = node.get(k).and_then(Value::as_str) {
            desc = Some(s);
            break;
        }
    }
    if desc.is_none() {
        if let Some(extra) = node.get("extra") {
            for k in keys {
                if let Some(s) = extra.get(k).and_then(Value::as_str) {
                    desc = Some(s);
                    break;
                }
            }
        }
    }
    let desc = desc.or(fallback).map(str::trim).filter(|s| !s.is_empty());
    desc.map(str::to_string)
        .unwrap_or_else(|| node.get("type").and_then(Value::as_str).unwrap_or_default().to_string())
}

fn format_display_name(name: &str, origin: Option<&str>) -> String {
    match origin {
        Some(o) if !o.is_empty() => {
            let suffix = format!(" [{o}]");
            name.strip_suffix(&suffix).map(|s| s.trim().to_string()).unwrap_or_else(|| name.to_string())
        }
        _ => name.to_string(),
    }
}

fn parse_origin(name: &str) -> Option<String> {
    if !name.ends_with(']') {
        return None;
    }
    let start = name.rfind(" [")?;
    let o = name[start + 2..name.len() - 1].trim();
    if o.is_empty() {
        None
    } else {
        Some(o.to_string())
    }
}
