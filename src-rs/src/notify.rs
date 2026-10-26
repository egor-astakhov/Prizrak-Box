//! Native notifications (window.pxNotify / pxNotifyAlert of the Wails shim).
//!
//! A click on a subscription reminder opens SubscriptionAlertModal, as the
//! Wails NotificationService does (OnNotificationResponse in
//! src-wails/main.go):
//! * Linux — D-Bus action "default" (notify-rust);
//! * macOS — NSUserNotificationCenter click response (mac-notification-sys);
//! * Windows — the toast launches `prizrak-box://subscription-alert?...`
//!   (protocol activation through the scheme the MSI registers), which also
//!   works from Action Center and after the app has quit, as the Wails COM
//!   activator did; its in-process `Activated` event covers a portable build
//!   with no scheme registered. Posted under the "Prizrak-Box"
//!   AppUserModelID, the same identity the Wails shell registers (go-toast).

use serde_json::Value;

/// Called once at startup, before the first notification.
pub fn init() {
    #[cfg(target_os = "macos")]
    {
        // Post as the installed app (its name and icon in Notification
        // Center); a dev build outside a bundle falls back like notify-rust.
        let bundle = mac_notification_sys::get_bundle_identifier("Prizrak-Box")
            .unwrap_or_else(|| mac_notification_sys::get_bundle_identifier_or_default("use_default"));
        let _ = mac_notification_sys::set_application(&bundle);
    }
    #[cfg(windows)]
    windows_aumid::register();
}

pub fn send(title: &str, body: &str) {
    let (t, b) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            let _ = windows_aumid::toast(&t, &b).show();
        }
        #[cfg(not(windows))]
        {
            let _ = notify_rust::Notification::new().appname("Prizrak-Box").summary(&t).body(&b).show();
        }
    });
}

/// Subscription reminder; a click opens SubscriptionAlertModal.
pub fn alert(title: &str, body: &str, profile_id: String, alert: Value) {
    let (t, b) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        #[cfg(windows)]
        let link = crate::deeplink::alert_link(&profile_id, &alert);
        let open = move || crate::app::show_subscription_alert(profile_id.clone(), alert.clone());
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let mut n = notify_rust::Notification::new();
            n.appname("Prizrak-Box").summary(&t).body(&b).action("default", "Open");
            if let Ok(h) = n.show() {
                h.wait_for_action(|a| {
                    if a == "default" {
                        open();
                    }
                });
            }
        }
        #[cfg(target_os = "macos")]
        {
            // Blocks this thread until the notification is clicked or closed.
            let res = mac_notification_sys::Notification::new()
                .title(&t)
                .message(&b)
                .wait_for_click(true)
                .send();
            log::info!("subscription notification: {res:?}");
            if matches!(res, Ok(mac_notification_sys::NotificationResponse::Click)) {
                open();
            }
        }
        #[cfg(windows)]
        {
            if let Err(e) = windows_aumid::show_alert(&t, &b, &link, open) {
                log::warn!("subscription toast: {e}");
            }
        }
    });
}

#[cfg(windows)]
mod windows_aumid {
    use tauri_winrt_notification::Toast;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    /// Same AppUserModelID as the Wails build (application Name), so toasts
    /// keep showing as "Prizrak-Box" with its icon after the upgrade.
    pub const APP_ID: &str = "Prizrak-Box";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// HKCU\Software\Classes\AppUserModelId\Prizrak-Box: DisplayName + IconUri
    /// (what go-toast's SetAppData writes). The Wails CustomActivator is
    /// removed: it names a COM server registered to the Wails executable,
    /// while this shell takes clicks in-process (Toast Activated event).
    pub fn register() {
        let icon = crate::paths::flag_file("notify-icon.png");
        if let Some(dir) = icon.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&icon, include_bytes!("../assets/appicon.png"));
        let path = wide(&format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
        let mut key: HKEY = std::ptr::null_mut();
        unsafe {
            if RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            ) != ERROR_SUCCESS
            {
                return;
            }
            let set = |name: &str, value: &str| {
                let n = wide(name);
                let v = wide(value);
                RegSetValueExW(key, n.as_ptr(), 0, REG_SZ, v.as_ptr() as *const u8, (v.len() * 2) as u32);
            };
            set("DisplayName", APP_ID);
            set("IconUri", &icon.to_string_lossy());
            let ca = wide("CustomActivator");
            RegDeleteValueW(key, ca.as_ptr());
            RegCloseKey(key);
        }
    }

    pub fn toast(title: &str, body: &str) -> Toast {
        Toast::new(APP_ID).title(title).text1(body)
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
    }

    /// A toast whose click launches `link` (activationType="protocol"), which
    /// tauri-winrt-notification's builder cannot express.
    pub fn show_alert(title: &str, body: &str, link: &str, open: impl Fn() + Send + 'static) -> windows::core::Result<()> {
        use windows::core::{IInspectable, HSTRING};
        use windows::Data::Xml::Dom::XmlDocument;
        use windows::Foundation::TypedEventHandler;
        use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
        let xml = format!(
            r#"<toast activationType="protocol" launch="{}"><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual></toast>"#,
            xml_escape(link),
            xml_escape(title),
            xml_escape(body)
        );
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, _| {
            open();
            Ok(())
        }))?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)
    }
}
