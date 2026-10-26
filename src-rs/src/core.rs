//! px backend lifecycle — port of `src-wails/services/core.go`.
//!
//! Contract with px (src-go/prizrak/core.go, src-go/api/job/alive.go):
//!   * px is spawned as `px -addr=127.0.0.1:<cbPort> -home=<dir>`;
//!   * px picks its own control port + secret and calls
//!     `GET /pxStore?port=<p>&secret=<s>` on the callback server until "ok";
//!   * px polls `GET /pxAlive` every 3 s and exits once the shell stops
//!     answering — that is how px shuts down (and restores the system proxy)
//!     even when the GUI is killed.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ConnInfo {
    pub host: String,
    pub port: u16,
    pub secret: String,
}

struct Inner {
    child: Option<Child>,
    started_by_svc: bool,
    cb_addr: String,
    info: ConnInfo,
    pulse: Option<mpsc::SyncSender<ConnInfo>>,
    pulse_rx: Option<mpsc::Receiver<ConnInfo>>,
}

pub struct Core {
    inner: Mutex<Inner>,
    ready: (std::sync::Mutex<bool>, std::sync::Condvar),
}

const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

impl Core {
    pub fn new() -> Self {
        Core {
            inner: Mutex::new(Inner {
                child: None,
                started_by_svc: false,
                cb_addr: String::new(),
                info: ConnInfo::default(),
                pulse: None,
                pulse_rx: None,
            }),
            ready: (std::sync::Mutex::new(false), std::sync::Condvar::new()),
        }
    }

    pub fn cb_addr(&self) -> String {
        self.inner.lock().cb_addr.clone()
    }

    pub fn info(&self) -> ConnInfo {
        self.inner.lock().info.clone()
    }

    pub fn px_path(&self) -> String {
        paths::px_binary().to_string_lossy().to_string()
    }

    pub fn home(&self) -> String {
        paths::home_dir().to_string_lossy().to_string()
    }

    /// Blocks until px reported its port/secret at least once (or timeout).
    pub fn wait_conn_info(&self, timeout: Duration) -> Option<ConnInfo> {
        let (m, cv) = &self.ready;
        let guard = m.lock().unwrap();
        let (guard, _) = cv.wait_timeout_while(guard, timeout, |r| !*r).unwrap();
        if *guard {
            Some(self.info())
        } else {
            None
        }
    }

    fn set_info(&self, info: ConnInfo) {
        self.inner.lock().info = info;
        let (m, cv) = &self.ready;
        *m.lock().unwrap() = true;
        cv.notify_all();
    }

    /// Starts the loopback callback server (idempotent).
    pub fn ensure_callback_server(self: &std::sync::Arc<Self>) -> std::io::Result<()> {
        if !self.inner.lock().cb_addr.is_empty() {
            return Ok(());
        }
        let ln = TcpListener::bind("127.0.0.1:0")?;
        let addr = ln.local_addr()?.to_string();
        self.inner.lock().cb_addr = addr;
        let me = self.clone();
        std::thread::Builder::new()
            .name("px-callback".into())
            .spawn(move || {
                for stream in ln.incoming().flatten() {
                    let me = me.clone();
                    std::thread::spawn(move || me.handle_cb(stream));
                }
            })?;
        Ok(())
    }

    fn handle_cb(&self, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        });
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        // drain headers
        loop {
            let mut h = String::new();
            match reader.read_line(&mut h) {
                Ok(0) => break,
                Ok(_) if h == "\r\n" || h == "\n" => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        let target = line.split_whitespace().nth(1).unwrap_or("/");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let body = match path {
            "/pxStore" => {
                let mut port = 0u16;
                let mut secret = String::new();
                for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
                    match k.as_ref() {
                        "port" => port = v.parse().unwrap_or(0),
                        "secret" => secret = v.to_string(),
                        _ => {}
                    }
                }
                if port > 0 {
                    let info = ConnInfo { host: "127.0.0.1".into(), port, secret };
                    let pulse = self.inner.lock().pulse.clone();
                    self.set_info(info.clone());
                    if let Some(p) = pulse {
                        let _ = p.try_send(info);
                    }
                }
                "ok"
            }
            "/pxAlive" => "alive",
            _ => "",
        };
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(resp.as_bytes());
    }

    /// Prepares a fresh pulse channel right before px (re)starts.
    pub fn arm(&self) {
        let (tx, rx) = mpsc::sync_channel(1);
        let mut g = self.inner.lock();
        g.pulse = Some(tx);
        g.pulse_rx = Some(rx);
    }

    /// Waits for the next /pxStore callback after `arm`.
    pub fn await_info(&self, timeout: Duration) -> anyhow::Result<ConnInfo> {
        let rx = self
            .inner
            .lock()
            .pulse_rx
            .take()
            .ok_or_else(|| anyhow::anyhow!("await without arm"))?;
        match rx.recv_timeout(timeout) {
            Ok(info) => {
                self.set_info(info.clone());
                Ok(info)
            }
            Err(_) => Err(anyhow::anyhow!("backend did not call back in time")),
        }
    }

    pub fn mark_started_by_svc(&self, v: bool) {
        self.inner.lock().started_by_svc = v;
    }

    /// Whether the running px was started by the privileged service.
    pub fn started_by_svc(&self) -> bool {
        self.inner.lock().started_by_svc
    }

    /// Like `await_info`, but gives up as soon as `alive()` reports that px has
    /// gone (checked every half second) instead of sitting out the timeout.
    pub fn await_info_while(&self, timeout: Duration, mut alive: impl FnMut() -> bool) -> anyhow::Result<ConnInfo> {
        let rx = self
            .inner
            .lock()
            .pulse_rx
            .take()
            .ok_or_else(|| anyhow::anyhow!("await without arm"))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(anyhow::anyhow!("backend did not call back in time"));
            }
            match rx.recv_timeout(left.min(Duration::from_millis(500))) {
                Ok(info) => {
                    self.set_info(info.clone());
                    return Ok(info);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !alive() {
                        return Err(anyhow::anyhow!("px exited before calling back"));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(anyhow::anyhow!("callback channel closed")),
            }
        }
    }

    /// Kills any running px and spawns a fresh one directly.
    pub fn restart_direct(&self) -> anyhow::Result<ConnInfo> {
        self.kill_px();
        self.arm();
        let mut cmd = Command::new(self.px_path());
        cmd.arg(format!("-addr={}", self.cb_addr()))
            .arg(format!("-home={}", self.home()))
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        hide_window(&mut cmd);
        let child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("spawn px ({}): {e}", self.px_path()))?;
        {
            let mut g = self.inner.lock();
            g.child = Some(child);
            g.started_by_svc = false;
        }
        self.await_info(Duration::from_secs(60))
    }

    /// Terminates the locally spawned px, asking it to clean up first.
    pub fn kill_px(&self) {
        let (child, info) = {
            let mut g = self.inner.lock();
            (g.child.take(), g.info.clone())
        };
        request_px_exit(&info);
        let Some(mut child) = child else { return };
        #[cfg(unix)]
        unsafe {
            libc_kill(child.id() as i32, 2);
        }
        let deadline = std::time::Instant::now() + SHUTDOWN_GRACE;
        while std::time::Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Asks the running px (whoever spawned it) to shut down cleanly.
    pub fn request_exit(&self) {
        let info = self.info();
        request_px_exit(&info);
    }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

pub fn request_px_exit(info: &ConnInfo) {
    if info.port == 0 {
        return;
    }
    let host = if info.host.is_empty() { "127.0.0.1" } else { &info.host };
    let addr = format!("{host}:{}", info.port);
    let Ok(mut s) = TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| "127.0.0.1:9686".parse().unwrap()),
        Duration::from_secs(2),
    ) else {
        return;
    };
    let _ = s.set_read_timeout(Some(SHUTDOWN_GRACE));
    let req = format!(
        "GET /prizrak/exit HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        info.secret
    );
    if s.write_all(req.as_bytes()).is_ok() {
        let mut buf = [0u8; 256];
        let _ = std::io::Read::read(&mut s, &mut buf);
    }
}

#[cfg(windows)]
pub fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn hide_window(_cmd: &mut Command) {}
