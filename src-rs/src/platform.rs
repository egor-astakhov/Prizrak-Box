//! Platform glue that winit does not cover.

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{define_class, msg_send, sel, AllocAnyThread};
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSAppleEventDescriptor, NSAppleEventManager, NSString};

    const GURL: u32 = 0x4755_524c; // 'GURL' (kInternetEventClass / kAEGetURL)
    const KEY_DIRECT_OBJECT: u32 = 0x2d2d_2d2d; // '----'

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "PrizrakBoxURLHandler"]
        struct UrlHandler;

        impl UrlHandler {
            #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
            fn handle_url(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                let url: Option<String> = unsafe {
                    let d: Option<Retained<NSAppleEventDescriptor>> = msg_send![event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT];
                    d.and_then(|d| {
                        let s: Option<Retained<NSString>> = msg_send![&*d, stringValue];
                        s.map(|s| s.to_string())
                    })
                };
                if let Some(u) = url {
                    crate::deeplink::handle(u);
                }
            }
        }
    );

    /// Registers the `prizrak-box://` Apple Event handler (the URL scheme is
    /// declared in Info.plist; launch and running-instance URLs both arrive here).
    pub fn register_url_handler() {
        unsafe {
            let handler: Retained<UrlHandler> = msg_send![UrlHandler::alloc(), init];
            let mgr = NSAppleEventManager::sharedAppleEventManager();
            let _: () = msg_send![
                &*mgr,
                setEventHandler: &*handler,
                andSelector: sel!(handleGetURLEvent:withReplyEvent:),
                forEventClass: GURL,
                andEventID: GURL
            ];
            std::mem::forget(handler);
        }
    }

    pub fn activate() {
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::register_url_handler;

#[cfg(target_os = "macos")]
pub fn macos_activate() {
    mac::activate();
}
