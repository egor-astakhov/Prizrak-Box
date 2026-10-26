//! Filesystem locations shared with the Electron and Wails shells.
//!
//! Port of `src-wails/internal/locate`: the px data directory, the bundled
//! binaries and the small preference flags persisted outside the data dir.
//! Every path matches the other shells so switching between them (or
//! upgrading from one to another) keeps profiles, settings and flags.

use std::path::{Path, PathBuf};

/// Per-user data sub-directory shared with Electron, Wails and px.
pub const WORK_DIR_NAME: &str = "Prizrak-Box-V3";

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// Path to the px backend binary (env override, next to the exe,
/// `<exeDir>/resources`, macOS `Contents/Resources`, then the repo dev layout).
pub fn px_binary() -> PathBuf {
    resolve_binary("PRIZRAK_PX_BIN", &exe_name("px"), "src-go")
}

/// Path to the px-service binary (TUN helper).
pub fn service_binary() -> PathBuf {
    resolve_binary("PRIZRAK_PX_SERVICE_BIN", &exe_name("px-service"), "src-service")
}

fn resolve_binary(env_var: &str, exe: &str, dev_dir: &str) -> PathBuf {
    if let Ok(v) = std::env::var(env_var) {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(cur) = std::env::current_exe() {
        if let Some(dir) = cur.parent() {
            let candidates = [
                dir.join(exe),
                dir.join("resources").join(exe),
                // macOS bundle: Contents/MacOS/<app> -> Contents/Resources/<exe>
                dir.join("..").join("Resources").join(exe),
                // Linux packages install helpers under /usr/lib/prizrak-box
                dir.join("..").join("lib").join("prizrak-box").join(exe),
            ];
            for c in candidates {
                if c.is_file() {
                    return c;
                }
            }
        }
    }
    if let Ok(wd) = std::env::current_dir() {
        for c in [wd.join("..").join(dev_dir).join(exe), wd.join(dev_dir).join(exe)] {
            if c.is_file() {
                return c;
            }
        }
    }
    PathBuf::from(exe)
}

/// `<UserConfigDir>/prizrak-box/<name>` — same as Wails' locate.flagFile.
pub fn flag_file(name: &str) -> PathBuf {
    let base = dirs::config_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("prizrak-box").join(name)
}

fn read_flag(name: &str) -> Option<String> {
    std::fs::read_to_string(flag_file(name)).ok().map(|s| s.trim().to_string())
}

fn write_flag(name: &str, value: &str) -> std::io::Result<()> {
    let f = flag_file(name);
    if let Some(p) = f.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(f, value)
}

/// The px data directory passed via `-home`.
pub fn home_dir() -> PathBuf {
    if let Ok(v) = std::env::var("PRIZRAK_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    let dir = match read_flag("home.path").filter(|s| !s.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(WORK_DIR_NAME),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn set_home_override(dir: &Path) -> std::io::Result<()> {
    write_flag("home.path", &dir.to_string_lossy())
}

pub fn start_minimized() -> bool {
    read_flag("start-minimized").as_deref() == Some("1")
}

pub fn set_start_minimized(v: bool) {
    let _ = write_flag("start-minimized", if v { "1" } else { "0" });
}

pub fn tun_desired() -> bool {
    read_flag("tun-desired").as_deref() == Some("1")
}

pub fn set_tun_desired(v: bool) {
    let _ = write_flag("tun-desired", if v { "1" } else { "0" });
}

pub fn dark_background() -> bool {
    read_flag("dark-bg").map(|s| s != "0").unwrap_or(true)
}

pub fn set_dark_background(v: bool) {
    let _ = write_flag("dark-bg", if v { "1" } else { "0" });
}

/// The Rust shell's default TUN stack was applied once (see
/// app::after_mount); later choices of the user are kept.
pub fn stack_default_applied() -> bool {
    read_flag("rs-stack-default").is_some()
}

pub fn set_stack_default_applied() {
    let _ = write_flag("rs-stack-default", "1");
}

/// Crash/error log of the shell (no console in GUI builds).
pub fn shell_log_file() -> PathBuf {
    flag_file("shell.log")
}

/// UI settings file shared with Electron (electron-store) and Wails (kvstore).
pub fn store_file() -> PathBuf {
    home_dir().join("px-electron.db").join("config.json")
}

/// Directory for uploaded custom backgrounds (same as the other shells).
pub fn custom_images_dir() -> PathBuf {
    let d = std::env::temp_dir().join(WORK_DIR_NAME).join("images");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Cache dir for downloaded backgrounds/logos of this shell.
pub fn cache_dir() -> PathBuf {
    let d = flag_file("rs-cache");
    let _ = std::fs::create_dir_all(&d);
    d
}
