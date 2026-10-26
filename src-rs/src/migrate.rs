//! One-time migration of UI state into the Rust shell.
//!
//! What is shared already needs no work: px data (`~/Prizrak-Box-V3` or the
//! directory in `<UserConfigDir>/prizrak-box/home.path`), the settings file
//! `<home>/px-electron.db/config.json` (electron-store / Wails kvstore) and the
//! shell flags next to `home.path`.
//!
//! What lived inside the web engine is recovered here, best effort:
//!   * Electron (1.0.20 and earlier, Linux builds): Chromium localStorage;
//!   * Wails (1.0.21 alphas/betas, Windows/macOS/Linux): WebView2 / WKWebView /
//!     WebKitGTK localStorage — early builds even kept the pinia stores there
//!     (`px:<store>`), later only the `web` store (custom dashboards) and the
//!     connections view mode.
//!   * Electron's "change config dir" (`appConfigDir` in its store) becomes
//!     the shared `home.path` override.
//!
//! The storage engines are scanned as raw bytes (LevelDB log/table files,
//! SQLite pages) for the known keys; values are JSON, so a parse validates
//! every candidate. Nothing is deleted, so the old shells keep working.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::paths;
use crate::store::Store;

const DONE_FLAG: &str = "rs-migrated";
const STORE_KEYS: [&str; 6] = ["menu", "setting", "proxies", "home", "onboarding", "updates"];

pub fn run() {
    let flag = paths::flag_file(DONE_FLAG);
    migrate_home_override();
    if flag.exists() {
        return;
    }
    let store = Store::open();
    let blobs = collect_blobs();
    if !blobs.is_empty() {
        for key in STORE_KEYS {
            if store.has(key) {
                continue;
            }
            if let Some(v) = find_px_key(&blobs, key) {
                log::info!("migrate: imported '{key}' from a webview store");
                store.set_raw(key, Value::String(v));
            }
        }
        if !store.has("web") {
            if let Some(d) = find_dashboards(&blobs) {
                log::info!("migrate: imported custom dashboards");
                let web = serde_json::json!({ "customDashboards": d });
                store.set("web", &web);
            }
        }
        if !store.has("connections") {
            if let Some(mode) = find_conn_view(&blobs) {
                store.set("connections", &serde_json::json!({ "viewMode": mode }));
            }
        }
    }
    if let Some(p) = flag.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let _ = std::fs::write(&flag, env!("CARGO_PKG_VERSION"));
}

/// Electron kept a custom data dir in its own store as `appConfigDir`.
fn migrate_home_override() {
    if std::env::var("PRIZRAK_HOME").is_ok() || paths::flag_file("home.path").exists() {
        return;
    }
    let def = dirs::home_dir().unwrap_or_default().join(paths::WORK_DIR_NAME);
    let cfg = def.join("px-electron.db").join("config.json");
    let Ok(bytes) = std::fs::read(cfg) else { return };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else { return };
    let Some(dir) = v.get("appConfigDir").and_then(Value::as_str) else { return };
    let target = Path::new(dir).join(paths::WORK_DIR_NAME);
    if target.is_dir() && target != def {
        let _ = paths::set_home_override(&target);
        log::info!("migrate: data dir override from Electron: {}", target.display());
    }
}

fn candidate_roots() -> Vec<PathBuf> {
    let mut v = vec![];
    let cfg = dirs::config_dir().unwrap_or_default();
    let data = dirs::data_dir().unwrap_or_default();
    let home = dirs::home_dir().unwrap_or_default();
    // Electron userData
    v.push(cfg.join("Prizrak-Box"));
    v.push(cfg.join("prizrak-box").join("webview2"));
    // WebView2 default profile dirs of older Wails builds
    for exe in ["Prizrak-Box.exe", "prizrak-box-wails.exe", "prizrak-box.exe"] {
        v.push(cfg.join(exe));
    }
    if let Some(local) = dirs::data_local_dir() {
        v.push(local.join("Prizrak-Box"));
    }
    // WKWebView (macOS)
    for id in ["com.legiz-ru.prizrak-box", "Prizrak-Box", "prizrak-box-wails"] {
        v.push(home.join("Library").join("WebKit").join(id));
    }
    // WebKitGTK (Linux)
    for id in ["com.legiz-ru.prizrak-box", "prizrak-box-wails", "prizrak-box", "org.wails.prizrak-box"] {
        v.push(data.join(id));
    }
    v
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, depth - 1, out);
        } else {
            let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            let parent = p.parent().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            let ls = parent.contains("local storage") || parent.contains("localstorage");
            if (ls && (name.ends_with(".log") || name.ends_with(".ldb")))
                || name.contains("localstorage")
                || name.ends_with(".localstorage")
            {
                out.push(p);
            }
        }
    }
}

/// Raw file contents as text, both byte-wise (Latin-1/UTF-8) and UTF-16LE.
fn collect_blobs() -> Vec<String> {
    let mut files = vec![];
    for root in candidate_roots() {
        if root.is_dir() {
            walk(&root, 9, &mut files);
        }
    }
    // newest first so the most recent value wins
    files.sort_by_key(|p| std::cmp::Reverse(p.metadata().and_then(|m| m.modified()).ok()));
    let mut out = vec![];
    for f in files {
        let Ok(b) = std::fs::read(&f) else { continue };
        if b.len() > 64 << 20 {
            continue;
        }
        out.push(b.iter().map(|&c| c as char).collect::<String>());
        let u16s: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        out.push(String::from_utf16_lossy(&u16s));
        if b.len() > 1 {
            let u16s: Vec<u16> = b[1..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            out.push(String::from_utf16_lossy(&u16s));
        }
    }
    out
}

/// Parses a JSON value starting at `s` (object, array or string).
fn json_at(s: &str) -> Option<(Value, usize)> {
    let mut de = serde_json::Deserializer::from_str(s).into_iter::<Value>();
    let v = de.next()?.ok()?;
    Some((v, de.byte_offset()))
}

/// `px:<key>` → the pinia JSON string stored by early Wails builds.
fn find_px_key(blobs: &[String], key: &str) -> Option<String> {
    let needle = format!("px:{key}");
    for b in blobs {
        let mut best: Option<String> = None;
        let mut from = 0;
        while let Some(i) = b[from..].find(&needle) {
            let start = from + i + needle.len();
            from = start;
            let tail: String = b[start..].chars().take(200_000).collect();
            if let Some(q) = tail.find(['"', '{']) {
                if q > 8 {
                    continue;
                }
                if let Some((v, _)) = json_at(&tail[q..]) {
                    let s = match v {
                        Value::String(s) => s,
                        Value::Object(_) => v.to_string(),
                        _ => continue,
                    };
                    if serde_json::from_str::<Value>(&s).map(|x| x.is_object()).unwrap_or(false) {
                        best = Some(s);
                    }
                }
            }
        }
        if best.is_some() {
            return best;
        }
    }
    None
}

fn find_dashboards(blobs: &[String]) -> Option<Value> {
    let needle = "\"customDashboards\":";
    for b in blobs {
        let mut found: Option<Value> = None;
        let mut from = 0;
        while let Some(i) = b[from..].find(needle) {
            let start = from + i + needle.len();
            from = start;
            if let Some((v, _)) = json_at(&b[start..]) {
                if let Some(arr) = v.as_array() {
                    if arr.iter().all(|d| d.get("name").is_some() && d.get("url").is_some()) {
                        found = Some(v.clone());
                    }
                }
            }
        }
        if let Some(f) = found {
            if f.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                return Some(f);
            }
        }
    }
    None
}

fn find_conn_view(blobs: &[String]) -> Option<String> {
    let re = regex::Regex::new(r#"connections.{0,12}\{"viewMode":"(list|topology|process)"\}"#).ok()?;
    for b in blobs {
        if let Some(m) = re.captures_iter(b).last() {
            return Some(m[1].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_double_encoded_pinia_value() {
        let blob = "\u{0}_https://wails.localhost\u{0}\u{1}px:menu\u{20}\u{1}\"{\\\"menu\\\":\\\"Home\\\",\\\"language\\\":\\\"ru\\\"}\"garbage".to_string();
        let v = find_px_key(&[blob], "menu").unwrap();
        let parsed: Value = serde_json::from_str(&v).unwrap();
        assert_eq!(parsed["language"], "ru");
    }

    #[test]
    fn finds_dashboards() {
        let blob = r#"xx{"host":"127.0.0.1","customDashboards":[{"name":"A","url":"https://a"}],"logs":[]}"#.to_string();
        let v = find_dashboards(&[blob]).unwrap();
        assert_eq!(v[0]["name"], "A");
    }
}
