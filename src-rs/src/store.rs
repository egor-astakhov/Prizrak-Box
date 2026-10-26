//! Persistent UI state.
//!
//! The file is the electron-store / Wails-kvstore one
//! (`<home>/px-electron.db/config.json`): a flat JSON object whose values are
//! the JSON *strings* pinia-plugin-persistedstate wrote (`"menu": "{...}"`).
//! Reading and writing that exact shape keeps settings shared with the
//! Electron and Wails shells in both directions. Unknown fields are kept
//! verbatim (`extra`) so a newer/older shell never loses its own keys.

use std::path::PathBuf;

use parking_lot::Mutex;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::paths;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProfileTheme {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transparency: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dim: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
}

impl Default for ProfileTheme {
    fn default() -> Self {
        ProfileTheme { image: None, transparency: None, blur: None, dim: None, mode: None, accent: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuState {
    pub menu: String,
    pub path: String,
    pub rule: String,
    #[serde(rename = "ruleNum")]
    pub rule_num: i64,
    pub proxy: bool,
    pub tun: bool,
    pub language: String,
    #[serde(rename = "ruleMenu")]
    pub rule_menu: String,
    pub background: String,
    #[serde(rename = "useWhite")]
    pub use_white: bool,
    #[serde(rename = "settingTab")]
    pub setting_tab: String,
    #[serde(rename = "providersView")]
    pub providers_view: String,
    #[serde(rename = "themePref")]
    pub theme_pref: String,
    #[serde(rename = "useBgImage")]
    pub use_bg_image: bool,
    #[serde(rename = "uiTrans")]
    pub ui_trans: f64,
    #[serde(rename = "uiBlur")]
    pub ui_blur: f64,
    #[serde(rename = "bgDim")]
    pub bg_dim: f64,
    pub accent: String,
    #[serde(rename = "bgTheme")]
    pub bg_theme: String,
    #[serde(rename = "profileTheme")]
    pub profile_theme: Option<ProfileTheme>,
    #[serde(rename = "profileThemeTitle")]
    pub profile_theme_title: String,
    #[serde(rename = "useProfileTheme")]
    pub use_profile_theme: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub const DEFAULT_BACKGROUND: &str = "url(\"/images/default.jpg\")";

impl Default for MenuState {
    fn default() -> Self {
        MenuState {
            menu: "Home".into(),
            path: "/Home".into(),
            rule: "rule".into(),
            rule_num: 0,
            proxy: false,
            tun: false,
            language: String::new(),
            rule_menu: "Now".into(),
            background: DEFAULT_BACKGROUND.into(),
            use_white: true,
            setting_tab: "app".into(),
            providers_view: "cards".into(),
            theme_pref: "auto".into(),
            use_bg_image: true,
            ui_trans: 78.0,
            ui_blur: 1.0,
            bg_dim: 18.0,
            accent: "#5b67e8".into(),
            bg_theme: "default".into(),
            profile_theme: None,
            profile_theme_title: String::new(),
            use_profile_theme: true,
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HwidHeaders {
    pub hwid: String,
    pub os: String,
    #[serde(rename = "osVersion")]
    pub os_version: String,
    pub model: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct GroupTestUrl {
    pub name: String,
    pub url: String,
}

/// TUN stack of a fresh install.
pub const DEFAULT_STACK: &str = "Mips";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingState {
    #[serde(rename = "testUrl")]
    pub test_url: String,
    pub port: i64,
    #[serde(rename = "bindAddress")]
    pub bind_address: String,
    pub stack: String,
    pub ipv6: bool,
    pub dns: bool,
    pub startup: bool,
    #[serde(rename = "startMinimized")]
    pub start_minimized: bool,
    pub auth: bool,
    pub hwid: bool,
    #[serde(rename = "systemProxyMode")]
    pub system_proxy_mode: bool,
    #[serde(rename = "multiProfileEnabled")]
    pub multi_profile_enabled: bool,
    #[serde(rename = "multiProfileHintShown")]
    pub multi_profile_hint_shown: bool,
    #[serde(rename = "hwidHeaders")]
    pub hwid_headers: HwidHeaders,
    pub sc_switch: bool,
    pub sc_switch_key: String,
    #[serde(rename = "independentDelayTest")]
    pub independent_delay_test: bool,
    #[serde(rename = "groupTestUrls")]
    pub group_test_urls: Vec<GroupTestUrl>,
    #[serde(rename = "notifySubscriptionAlerts")]
    pub notify_subscription_alerts: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for SettingState {
    fn default() -> Self {
        SettingState {
            test_url: "https://www.google.com/blank.html".into(),
            port: 12345,
            bind_address: "127.0.0.1".into(),
            // Rust shell default (px itself still reports "Mixed" for a fresh
            // home; see app::after_mount).
            stack: DEFAULT_STACK.into(),
            ipv6: false,
            dns: false,
            startup: false,
            start_minimized: false,
            auth: false,
            hwid: true,
            system_proxy_mode: true,
            multi_profile_enabled: false,
            multi_profile_hint_shown: false,
            hwid_headers: HwidHeaders::default(),
            sc_switch: false,
            sc_switch_key: "Ctrl+Shift+X".into(),
            independent_delay_test: true,
            group_test_urls: vec![],
            notify_subscription_alerts: true,
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxiesState {
    #[serde(rename = "isHide")]
    pub is_hide: bool,
    #[serde(rename = "isSort")]
    pub is_sort: bool,
    #[serde(rename = "viewMode")]
    pub view_mode: String,
    pub active: String,
    pub now: String,
    #[serde(rename = "groupExpansion")]
    pub group_expansion: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ProxiesState {
    fn default() -> Self {
        ProxiesState {
            is_hide: false,
            is_sort: false,
            view_mode: "full".into(),
            active: String::new(),
            now: String::new(),
            group_expansion: Map::new(),
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IpInfo {
    pub query: String,
    #[serde(rename = "regionName")]
    pub region_name: String,
    pub country: String,
    pub city: String,
    pub isp: String,
    pub timezone: String,
    #[serde(rename = "as")]
    pub asn: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HomeState {
    #[serde(rename = "startTime")]
    pub start_time: i64,
    pub os: String,
    pub md5: String,
    pub md6: String,
    pub ip: IpInfo,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct OnboardingState {
    #[serde(rename = "firstProfileInfoShown")]
    pub first_profile_info_shown: bool,
    #[serde(rename = "hasEverHadProfiles")]
    pub has_ever_had_profiles: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UpdatesState {
    #[serde(rename = "currentVersion")]
    pub current_version: String,
    #[serde(rename = "latestTag")]
    pub latest_tag: Option<String>,
    #[serde(rename = "latestName")]
    pub latest_name: Option<String>,
    #[serde(rename = "latestUrl")]
    pub latest_url: Option<String>,
    #[serde(rename = "publishedAt")]
    pub published_at: Option<String>,
    #[serde(rename = "lastChecked")]
    pub last_checked: Option<i64>,
    #[serde(rename = "updateAvailable")]
    pub update_available: bool,
    #[serde(rename = "dismissedTag")]
    pub dismissed_tag: Option<String>,
    #[serde(rename = "notifiedTag")]
    pub notified_tag: Option<String>,
    #[serde(rename = "lastCheckStatus")]
    pub last_check_status: String,
    #[serde(rename = "lastError")]
    pub last_error: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct CustomDashboard {
    pub name: String,
    pub url: String,
}

/// Electron/Wails kept this store in the webview's localStorage; the Rust
/// shell keeps it in the shared file under the same key ("web").
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WebState {
    #[serde(rename = "customDashboards")]
    pub custom_dashboards: Vec<CustomDashboard>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionsState {
    #[serde(rename = "viewMode")]
    pub view_mode: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ConnectionsState {
    fn default() -> Self {
        ConnectionsState { view_mode: "topology".into(), extra: Map::new() }
    }
}

/// The key/value file.
pub struct Store {
    path: PathBuf,
    map: Mutex<Map<String, Value>>,
}

impl Store {
    pub fn open() -> Self {
        let path = paths::store_file();
        let map = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        Store { path, map: Mutex::new(map) }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn has(&self, key: &str) -> bool {
        self.map.lock().contains_key(key)
    }

    /// Value of a pinia store (stored as a JSON string, or a plain object).
    pub fn get_value(&self, key: &str) -> Option<Value> {
        let v = self.map.lock().get(key).cloned()?;
        match v {
            Value::String(s) => serde_json::from_str(&s).ok(),
            Value::Null => None,
            other => Some(other),
        }
    }

    pub fn get<T: DeserializeOwned + Default>(&self, key: &str) -> T {
        self.get_value(key)
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    pub fn set<T: Serialize>(&self, key: &str, value: &T) {
        let Ok(s) = serde_json::to_string(value) else { return };
        let snapshot = {
            let mut m = self.map.lock();
            m.insert(key.to_string(), Value::String(s));
            Value::Object(m.clone())
        };
        self.write(snapshot);
    }

    pub fn set_raw(&self, key: &str, value: Value) {
        let snapshot = {
            let mut m = self.map.lock();
            m.insert(key.to_string(), value);
            Value::Object(m.clone())
        };
        self.write(snapshot);
    }

    pub fn get_raw(&self, key: &str) -> Option<Value> {
        self.map.lock().get(key).cloned()
    }

    fn write(&self, snapshot: Value) {
        if let Some(p) = self.path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let tmp = self.path.with_extension("json.tmp");
        if let Ok(bytes) = serde_json::to_vec_pretty(&snapshot) {
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}
