//! Launch at login — same registrations as the Wails shell so a toggle set
//! there keeps working (and is re-applied on every start, like MyEvent.vue):
//!   * Windows: Task Scheduler logon task "Prizrak-Box Startup" (15 s delay,
//!     interactive token, never elevated); legacy Run-key entries are removed;
//!   * macOS:   SMAppService (Login Items) when running from the .app on
//!     macOS 13+, as Wails v3's Autostart does; otherwise (dev build, older
//!     macOS) ~/Library/LaunchAgents/com.legiz-ru.prizrak-box.plist;
//!   * Linux:   ~/.config/autostart/prizrak-box.desktop.

#[cfg(any(windows, target_os = "linux"))]
use std::path::PathBuf;

fn exe() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
}

pub fn apply(enabled: bool) {
    std::thread::spawn(move || {
        if let Err(e) = set(enabled) {
            log::error!("autostart set to {enabled} failed: {e}");
            crate::shell_log(&format!("autostart set to {enabled} failed: {e}"));
        }
    });
}

#[cfg(target_os = "linux")]
fn set(enabled: bool) -> anyhow::Result<()> {
    let dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".config")).join("autostart");
    let file = dir.join("prizrak-box.desktop");
    if !enabled {
        let _ = std::fs::remove_file(&file);
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    let exe = exe();
    // Packaged installs run through the /usr/bin wrapper.
    let cmd = if std::path::Path::new("/usr/bin/prizrak-box").exists() && exe.starts_with("/usr/") {
        "/usr/bin/prizrak-box".to_string()
    } else {
        exe
    };
    std::fs::write(
        &file,
        format!(
            "[Desktop Entry]\nType=Application\nName=Prizrak-Box\nExec={cmd}\nIcon=com.legiz-ru.prizrak-box\nX-GNOME-Autostart-enabled=true\nStartupWMClass=com.legiz-ru.prizrak-box\n"
        ),
    )?;
    Ok(())
}

/// SMAppService.mainAppService (ServiceManagement, macOS 13+), looked up at
/// run time like Wails' autostart_darwin_smappservice.go.
#[cfg(target_os = "macos")]
mod sm_app_service {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    #[link(name = "ServiceManagement", kind = "framework")]
    unsafe extern "C" {}

    pub enum Error {
        /// No SMAppService (macOS < 13) — use the LaunchAgent instead.
        Unavailable,
        Failed(String),
    }

    fn service() -> Option<*mut AnyObject> {
        let cls = AnyClass::get(c"SMAppService")?;
        let svc: *mut AnyObject = unsafe { msg_send![cls, mainAppService] };
        (!svc.is_null()).then_some(svc)
    }

    fn describe(err: *mut AnyObject) -> String {
        if err.is_null() {
            return "unknown error".into();
        }
        let desc: *mut AnyObject = unsafe { msg_send![err, localizedDescription] };
        if desc.is_null() {
            return "unknown error".into();
        }
        let utf8: *const std::ffi::c_char = unsafe { msg_send![desc, UTF8String] };
        if utf8.is_null() {
            return "unknown error".into();
        }
        unsafe { std::ffi::CStr::from_ptr(utf8) }.to_string_lossy().into_owned()
    }

    pub fn register() -> Result<(), Error> {
        let svc = service().ok_or(Error::Unavailable)?;
        let mut err: *mut AnyObject = std::ptr::null_mut();
        let ok: Bool = unsafe { msg_send![svc, registerAndReturnError: &mut err] };
        if ok.as_bool() { Ok(()) } else { Err(Error::Failed(describe(err))) }
    }

    pub fn unregister() -> Result<(), Error> {
        let svc = service().ok_or(Error::Unavailable)?;
        // SMAppServiceStatusNotRegistered = 0, SMAppServiceStatusNotFound = 3
        let status: isize = unsafe { msg_send![svc, status] };
        if status == 0 || status == 3 {
            return Ok(());
        }
        let mut err: *mut AnyObject = std::ptr::null_mut();
        let ok: Bool = unsafe { msg_send![svc, unregisterAndReturnError: &mut err] };
        if ok.as_bool() { Ok(()) } else { Err(Error::Failed(describe(err))) }
    }
}

/// Wails' darwinAutostart.strategy(): SMAppService only for the bundled app.
#[cfg(target_os = "macos")]
fn in_app_bundle() -> bool {
    let exe = exe();
    exe.contains(".app/Contents/MacOS/")
}

#[cfg(target_os = "macos")]
fn set(enabled: bool) -> anyhow::Result<()> {
    if in_app_bundle() {
        let res = if enabled { sm_app_service::register() } else { sm_app_service::unregister() };
        match res {
            Ok(()) if enabled => return Ok(()),
            Ok(()) | Err(sm_app_service::Error::Unavailable) => {}
            Err(sm_app_service::Error::Failed(e)) if enabled => anyhow::bail!("SMAppService register: {e}"),
            Err(sm_app_service::Error::Failed(e)) => log::warn!("SMAppService unregister: {e}"),
        }
    }
    let dir = dirs::home_dir().unwrap_or_default().join("Library").join("LaunchAgents");
    let file = dir.join("com.legiz-ru.prizrak-box.plist");
    if !enabled {
        let _ = std::fs::remove_file(&file);
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    let exe = exe();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.legiz-ru.prizrak-box</string>
    <key>ProgramArguments</key>
    <array><string>{}</string></array>
    <key>RunAtLoad</key><true/>
    <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#,
        exe.replace('&', "&amp;").replace('<', "&lt;")
    );
    std::fs::write(&file, plist)?;
    Ok(())
}

#[cfg(windows)]
fn set(enabled: bool) -> anyhow::Result<()> {
    use std::process::Command;
    const TASK: &str = "Prizrak-Box Startup";
    let exe = exe();
    // Clean legacy Run-key entries (Electron auto-launch / old Wails builds).
    for name in ["Prizrak-Box", "prizrak-box", "Prizrak Box"] {
        let mut c = Command::new("reg");
        c.args(["delete", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", name, "/f"]);
        crate::core::hide_window(&mut c);
        let _ = c.output();
    }
    if !enabled {
        let mut c = Command::new("schtasks");
        c.args(["/Delete", "/TN", TASK, "/F"]);
        crate::core::hide_window(&mut c);
        let _ = c.output();
        return Ok(());
    }
    let user = std::env::var("USERDOMAIN")
        .map(|d| format!("{d}\\{}", crate::app::username()))
        .unwrap_or_else(|_| crate::app::username());
    let workdir = std::path::Path::new(&exe).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Start Prizrak-Box at logon</Description></RegistrationInfo>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId><Delay>PT15S</Delay></LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author">
    <Exec><Command>{}</Command><WorkingDirectory>{}</WorkingDirectory></Exec>
  </Actions>
</Task>
"#,
        esc(&exe),
        esc(&workdir),
        user = esc(&user)
    );
    let path: PathBuf = std::env::temp_dir().join("prizrak-box-task.xml");
    let mut bytes: Vec<u8> = vec![0xFF, 0xFE];
    for u in xml.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&path, bytes)?;
    let mut c = Command::new("schtasks");
    c.args(["/Create", "/TN", TASK, "/XML", &path.to_string_lossy(), "/F"]);
    crate::core::hide_window(&mut c);
    let out = c.output()?;
    let _ = std::fs::remove_file(&path);
    if !out.status.success() {
        anyhow::bail!("schtasks: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}
