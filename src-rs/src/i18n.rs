//! Translations: the very same `src/locales/*.yaml` the Vue app uses, embedded
//! at build time. Keys are flattened with dots (`nav.home`) like vue-i18n;
//! `{name}` placeholders are interpolated by [`tr_args`].

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::RwLock;
use serde_yaml::Value;

const EN: &str = include_str!("../../src/locales/en.yaml");
const RU: &str = include_str!("../../src/locales/ru.yaml");
const ZH: &str = include_str!("../../src/locales/zh.yaml");

type Table = HashMap<String, String>;

fn flatten(prefix: &str, v: &Value, out: &mut Table) {
    match v {
        Value::Mapping(m) => {
            for (k, v) in m {
                let key = match k {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                let full = if prefix.is_empty() { key } else { format!("{prefix}.{key}") };
                flatten(&full, v, out);
            }
        }
        Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        Value::Number(n) => {
            out.insert(prefix.to_string(), n.to_string());
        }
        Value::Bool(b) => {
            out.insert(prefix.to_string(), b.to_string());
        }
        _ => {}
    }
}

fn parse(src: &str) -> Table {
    let mut t = Table::new();
    if let Ok(v) = serde_yaml::from_str::<Value>(src) {
        flatten("", &v, &mut t);
    }
    t
}

fn tables() -> &'static HashMap<&'static str, Table> {
    static T: OnceLock<HashMap<&'static str, Table>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert("en", parse(EN));
        m.insert("ru", parse(RU));
        m.insert("zh", parse(ZH));
        m
    })
}

static LANG: RwLock<String> = RwLock::new(String::new());

pub fn set_lang(lang: &str) {
    *LANG.write() = normalize(lang);
}

pub fn lang() -> String {
    let l = LANG.read().clone();
    if l.is_empty() {
        "en".into()
    } else {
        l
    }
}

pub fn normalize(lang: &str) -> String {
    let l = lang.to_lowercase();
    if l.starts_with("zh") {
        "zh".into()
    } else if l.starts_with("ru") {
        "ru".into()
    } else {
        "en".into()
    }
}

/// `detectLanguage()` from util/menu.ts, using the OS locale.
pub fn detect_language() -> String {
    let loc = sys_locale();
    let l = loc.to_lowercase();
    if l.starts_with("zh") {
        "zh".into()
    } else if l.starts_with("en") {
        "en".into()
    } else if l.starts_with("ru") {
        "ru".into()
    } else {
        "en".into()
    }
}

fn sys_locale() -> String {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() && v != "C" && v != "POSIX" {
                return v;
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(o) = std::process::Command::new("defaults").args(["read", "-g", "AppleLanguages"]).output() {
            let s = String::from_utf8_lossy(&o.stdout);
            if let Some(first) = s.split('"').nth(1) {
                return first.to_string();
            }
        }
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-Command", "(Get-UICulture).Name"]);
        crate::core::hide_window(&mut cmd);
        if let Ok(o) = cmd.output() {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "en".into()
}

pub fn tr_lang(lang: &str, key: &str) -> String {
    let t = tables();
    let l = normalize(lang);
    t.get(l.as_str())
        .and_then(|m| m.get(key))
        .or_else(|| t.get("en").and_then(|m| m.get(key)))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

pub fn tr(key: &str) -> String {
    tr_lang(&lang(), key)
}

pub fn tr_args(key: &str, args: &[(&str, String)]) -> String {
    let mut s = tr(key);
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}
