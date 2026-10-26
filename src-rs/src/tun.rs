//! Privileged px-service (TUN helper) management — port of
//! `src-wails/services/tun.go`, `elevate.go`, `svcstate_*.go`, `tun_dial_*.go`.
//!
//! The IPC protocol is the one of `src-service/ipc`: one JSON request per line
//! (`{"command":..,"data":..}`), one JSON response line back
//! (`{"success":..,"data":..,"error":..}`).

use std::io::{BufRead, BufReader, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::core::{ConnInfo, Core};
use crate::paths;

/// How long px started by the service may take to call back (it normally
/// does within a second; a px that dies is noticed sooner through `status`).
const SERVICE_PX_TIMEOUT: Duration = Duration::from_secs(30);

/// Set when the service failed to bring px up in this run of the app.
static SERVICE_PX_FAILED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Default, Serialize)]
pub struct ServiceStatus {
    pub installed: bool,
    pub running: bool,
    #[serde(rename = "isAdmin")]
    pub is_admin: bool,
}

#[derive(Default, Debug, Clone, Copy)]
struct ServiceState {
    installed: bool,
    running: bool,
    starting: bool,
    disabled: bool,
}

impl ServiceState {
    fn may_come_up(&self) -> bool {
        self.installed && !self.disabled && (self.running || self.starting)
    }
}

pub struct Tun {
    core: Arc<Core>,
}

trait Duplex: std::io::Read + std::io::Write {}
impl<T: std::io::Read + std::io::Write> Duplex for T {}

#[cfg(unix)]
fn dial(timeout: Duration) -> std::io::Result<Box<dyn Duplex>> {
    use std::os::unix::net::UnixStream;
    let primary = if cfg!(target_os = "macos") {
        "/var/run/prizrak-box/service.sock"
    } else {
        "/run/prizrak-box/service.sock"
    };
    let s = UnixStream::connect(primary).or_else(|_| UnixStream::connect("/tmp/prizrak-box-service.sock"))?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    Ok(Box::new(s))
}

#[cfg(windows)]
fn dial(_timeout: Duration) -> std::io::Result<Box<dyn Duplex>> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\pipe\prizrak-box-service")?;
    Ok(Box::new(f))
}

impl Tun {
    pub fn new(core: Arc<Core>) -> Self {
        Tun { core }
    }

    fn request(&self, cmd: &str, data: Option<Value>, timeout: Duration) -> anyhow::Result<Value> {
        let (tx, rx) = std::sync::mpsc::channel();
        let cmd = cmd.to_string();
        // Named pipes have no read timeout on Windows: run on a helper thread.
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<Value> {
                let mut conn = dial(timeout)?;
                let mut req = json!({ "command": cmd });
                if let Some(d) = data {
                    req["data"] = d;
                }
                let mut payload = serde_json::to_vec(&req)?;
                payload.push(b'\n');
                conn.write_all(&payload)?;
                conn.flush()?;
                let mut line = Vec::new();
                BufReader::new(conn).read_until(b'\n', &mut line)?;
                Ok(serde_json::from_slice(&line)?)
            })();
            let _ = tx.send(res);
        });
        rx.recv_timeout(timeout)
            .map_err(|_| anyhow::anyhow!("service request timed out"))?
    }

    fn ok(&self, cmd: &str, data: Option<Value>, timeout: Duration) -> anyhow::Result<Value> {
        let v = self.request(cmd, data, timeout)?;
        if v.get("success").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("data").cloned().unwrap_or(Value::Null))
        } else {
            Err(anyhow::anyhow!(
                "{}",
                v.get("error").and_then(Value::as_str).unwrap_or("service error")
            ))
        }
    }

    pub fn ping(&self) -> bool {
        self.ok("ping", None, Duration::from_secs(2)).is_ok()
    }

    pub fn is_running(&self) -> bool {
        self.ping()
    }

    pub fn status(&self) -> ServiceStatus {
        let running = self.ping();
        let mut st = ServiceStatus { installed: query_state().installed || running, ..Default::default() };
        if !running {
            return st;
        }
        st.running = true;
        if let Ok(v) = self.ok("is_admin", None, Duration::from_secs(2)) {
            st.is_admin = v.as_bool().unwrap_or(false);
        }
        st
    }

    pub fn install(&self) -> bool {
        let bin = paths::service_binary();
        if let Err(e) = run_elevated(&bin.to_string_lossy(), "Prizrak-Box requires installing the TUN service", &["-install"]) {
            log::error!("service install failed: {e}");
            return false;
        }
        let ok = self.wait_for_service(Duration::from_secs(15), Duration::from_millis(300));
        if ok {
            // a fresh service deserves a fresh try
            SERVICE_PX_FAILED.store(false, Ordering::SeqCst);
        }
        ok
    }

    pub fn uninstall(&self) -> bool {
        if self.ping() {
            self.core.request_exit();
            let _ = self.ok("stop_px", None, Duration::from_secs(5));
        }
        let bin = paths::service_binary();
        if let Err(e) = run_elevated(&bin.to_string_lossy(), "", &["-uninstall"]) {
            log::error!("service uninstall failed: {e}");
            return false;
        }
        for _ in 0..20 {
            if !self.ping() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        self.core.mark_started_by_svc(false);
        true
    }

    fn start_via_service(&self) -> anyhow::Result<ConnInfo> {
        self.core.kill_px();
        let _ = self.ok("stop_px", None, Duration::from_secs(5));
        self.core.arm();
        self.ok(
            "start_px",
            Some(json!({
                "pxPath": self.core.px_path(),
                "addr": self.core.cb_addr(),
                "homeDir": self.core.home(),
            })),
            Duration::from_secs(10),
        )?;
        self.core.mark_started_by_svc(true);
        // px that cannot start under the service (SELinux lib_t on Fedora:
        // init_t may not write the data dir) dies within a second; the service
        // reports it, so stop waiting then instead of sitting out the timeout.
        let res = self.core.await_info_while(SERVICE_PX_TIMEOUT, || self.px_running_under_service());
        SERVICE_PX_FAILED.store(res.is_err(), Ordering::SeqCst);
        res
    }

    /// The service's `status`: is the px it started still running? A failed
    /// request counts as running, so only a definite "no" cuts the wait short.
    fn px_running_under_service(&self) -> bool {
        match self.ok("status", None, Duration::from_secs(2)) {
            Ok(v) => v.get("px_running").and_then(Value::as_bool).unwrap_or(true),
            Err(_) => true,
        }
    }

    /// The service could not bring px up in this run of the app.
    pub fn service_px_failed(&self) -> bool {
        SERVICE_PX_FAILED.load(Ordering::SeqCst)
    }

    fn wait_for_service(&self, total: Duration, step: Duration) -> bool {
        let deadline = Instant::now() + total;
        loop {
            if self.ping() {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(step);
        }
    }

    fn wait_for_service_startup(&self, total: Duration, step: Duration) -> bool {
        let deadline = Instant::now() + total;
        let mut next_check = Instant::now() + Duration::from_secs(1);
        loop {
            if self.ping() {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            if Instant::now() > next_check {
                if !query_state().may_come_up() {
                    return false;
                }
                next_check = Instant::now() + Duration::from_secs(1);
            }
            std::thread::sleep(step);
        }
    }

    /// Application startup: route px through the service when reachable.
    pub fn start_backend(&self) -> anyhow::Result<ConnInfo> {
        let mut reachable = self.ping();
        if !reachable && paths::tun_desired() && query_state().may_come_up() {
            reachable = self.wait_for_service_startup(Duration::from_secs(5), Duration::from_millis(250));
        }
        if reachable {
            match self.start_via_service() {
                Ok(info) => return Ok(info),
                Err(e) => log::warn!("start px via service failed; falling back to direct spawn: {e}"),
            }
        }
        self.core.restart_direct()
    }

    /// Restarts px (through the service when it runs) and returns fresh info.
    pub fn restart_backend(&self) -> anyhow::Result<ConnInfo> {
        // Once the service has failed to bring px up in this run, do not stop a
        // working px to ask it again (until the service is reinstalled).
        if !self.service_px_failed() && self.ping() {
            match self.start_via_service() {
                Ok(info) => return Ok(info),
                // The service answers but its px dies (e.g. SELinux lib_t: init_t
                // cannot write the data dir). start_via_service has already
                // stopped the running px, so without this the app was left with
                // no backend at all — the TUN restore after an autostart takes
                // this path. Same fallback as start_backend.
                Err(e) => log::warn!("restart px via service failed; falling back to direct spawn: {e}"),
            }
        }
        self.core.mark_started_by_svc(false);
        self.core.restart_direct()
    }
}

fn query_state() -> ServiceState {
    #[cfg(target_os = "linux")]
    {
        let unit = "PrizrakBoxService.service";
        let installed = ["/etc/systemd/system/", "/usr/lib/systemd/system/", "/lib/systemd/system/"]
            .iter()
            .any(|d| std::path::Path::new(&format!("{d}{unit}")).exists());
        if !installed {
            return ServiceState::default();
        }
        let mut st = ServiceState { installed: true, ..Default::default() };
        let out = |args: &[&str]| {
            Command::new("systemctl").args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
        };
        match out(&["is-active", unit]).as_str() {
            "active" => st.running = true,
            "activating" | "reloading" => st.starting = true,
            _ => {}
        }
        if matches!(out(&["is-enabled", unit]).as_str(), "disabled" | "masked") {
            st.disabled = true;
        }
        st
    }
    #[cfg(target_os = "macos")]
    {
        let id = "com.prizrak-box.service";
        if !std::path::Path::new(&format!("/Library/LaunchDaemons/{id}.plist")).exists() {
            return ServiceState::default();
        }
        let mut st = ServiceState { installed: true, ..Default::default() };
        if let Ok(o) = Command::new("launchctl").args(["print", &format!("system/{id}")]).output() {
            if o.status.success() {
                if String::from_utf8_lossy(&o.stdout).contains("state = running") {
                    st.running = true;
                } else {
                    st.starting = true;
                }
            }
        }
        st
    }
    #[cfg(windows)]
    {
        let mut cmd = Command::new("sc");
        cmd.args(["query", "PrizrakBoxService"]);
        crate::core::hide_window(&mut cmd);
        let Ok(o) = cmd.output() else { return ServiceState::default() };
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        if !o.status.success() || !text.contains("STATE") {
            return ServiceState::default();
        }
        let mut st = ServiceState { installed: true, ..Default::default() };
        st.running = text.contains("RUNNING");
        st.starting = text.contains("START_PENDING");
        let mut qc = Command::new("sc");
        qc.args(["qc", "PrizrakBoxService"]);
        crate::core::hide_window(&mut qc);
        if let Ok(c) = qc.output() {
            st.disabled = String::from_utf8_lossy(&c.stdout).contains("DISABLED");
        }
        st
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn osa_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Runs a binary elevated and waits (UAC / osascript / pkexec).
pub fn run_elevated(path: &str, prompt: &str, args: &[&str]) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        let arg_list = if args.is_empty() {
            String::new()
        } else {
            format!(" -ArgumentList {}", args.iter().map(|a| format!("'{a}'")).collect::<Vec<_>>().join(","))
        };
        let ps = format!("Start-Process -FilePath '{path}'{arg_list} -Verb RunAs -Wait");
        let mut cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-Command", &ps]);
        crate::core::hide_window(&mut cmd);
        let st = cmd.status()?;
        if !st.success() {
            anyhow::bail!("elevation failed: {st}");
        }
        let _ = prompt;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let mut inner = shell_quote(path);
        for a in args {
            inner.push(' ');
            inner.push_str(&shell_quote(a));
        }
        let script = if prompt.is_empty() {
            format!("do shell script {} with administrator privileges", osa_quote(&inner))
        } else {
            format!(
                "do shell script {} with administrator privileges with prompt {}",
                osa_quote(&inner),
                osa_quote(prompt)
            )
        };
        let st = Command::new("osascript").args(["-e", &script]).status()?;
        if !st.success() {
            anyhow::bail!("elevation failed: {st}");
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let _ = (prompt, osa_quote(""), shell_quote(""));
        let mut last: Option<anyhow::Error> = None;
        let mut tried = false;
        for helper in ["pkexec", "gksudo", "kdesudo"] {
            if which(helper).is_none() {
                continue;
            }
            tried = true;
            match Command::new(helper).arg(path).args(args).status() {
                Ok(s) if s.success() => return Ok(()),
                Ok(s) => last = Some(anyhow::anyhow!("{helper}: {s}")),
                Err(e) => last = Some(anyhow::anyhow!("{helper}: {e}")),
            }
        }
        if std::env::var("SUDO_ASKPASS").map(|v| !v.is_empty()).unwrap_or(false) && which("sudo").is_some() {
            tried = true;
            match Command::new("sudo").arg("-A").arg(path).args(args).status() {
                Ok(s) if s.success() => return Ok(()),
                Ok(s) => last = Some(anyhow::anyhow!("sudo -A: {s}")),
                Err(e) => last = Some(anyhow::anyhow!("sudo -A: {e}")),
            }
        }
        if !tried {
            anyhow::bail!("no graphical elevation helper found (pkexec/gksudo/kdesudo)");
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("elevation failed")))
    }
}

#[cfg(target_os = "linux")]
pub fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p).map(|d| d.join(name)).find(|c| c.is_file())
    })
}
