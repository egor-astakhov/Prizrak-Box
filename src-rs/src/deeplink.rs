//! `prizrak-box://install-config?url=...&name=...` — port of main.ts
//! setupDeepLinkHandler / parseDeepLinkUrl.
//!
//! `prizrak-box://subscription-alert?profile=...&alert=...` is what a Windows
//! subscription-reminder toast launches on click (see notify.rs), so a click
//! from Action Center or after the app has quit still opens the alert modal.

use std::time::Duration;

use serde_json::{json, Value};

use crate::app::{self, ctx, st};
use crate::i18n::tr;
use crate::util::{self, str_of};
use crate::App;
use slint::ComponentHandle;

pub const SCHEME: &str = "prizrak-box";

pub fn find(args: &[String]) -> Option<String> {
    args.iter().find(|a| a.starts_with(&format!("{SCHEME}://"))).cloned()
}

fn safe_decode(v: &str) -> String {
    percent_encoding::percent_decode_str(v).decode_utf8().map(|s| s.to_string()).unwrap_or_else(|_| v.to_string())
}

/// Returns (subscription url, optional name).
pub fn parse(link: &str) -> Option<(String, Option<String>)> {
    let rest = link.strip_prefix(&format!("{SCHEME}://"))?;
    let (host, query) = rest.split_once('?')?;
    let host = host.trim_end_matches('/');
    if !host.is_empty() && !host.eq_ignore_ascii_case("install-config") {
        return None;
    }
    let mut url: Option<String> = None;
    let mut name: Option<String> = None;
    for seg in query.split('&') {
        if seg.is_empty() {
            continue;
        }
        let (k, v) = seg.split_once('=').unwrap_or((seg, ""));
        if k == "url" && url.is_none() {
            url = Some(v.to_string());
            continue;
        }
        if url.is_some() && k == "name" {
            name = Some(safe_decode(v));
            continue;
        }
        if let Some(u) = url.as_mut() {
            u.push('&');
            u.push_str(seg);
        }
    }
    let u = safe_decode(&url?);
    if u.is_empty() {
        return None;
    }
    Some((u, name))
}

const ALERT_HOST: &str = "subscription-alert";

/// The link a subscription-reminder toast carries (Windows protocol activation).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn alert_link(profile_id: &str, alert: &Value) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
    format!("{SCHEME}://{ALERT_HOST}?profile={}&alert={}", enc(profile_id), enc(&alert.to_string()))
}

fn parse_alert(link: &str) -> Option<(String, Value)> {
    let rest = link.strip_prefix(&format!("{SCHEME}://"))?;
    let (host, query) = rest.split_once('?')?;
    if !host.trim_end_matches('/').eq_ignore_ascii_case(ALERT_HOST) {
        return None;
    }
    let (mut id, mut alert) = (None, None);
    for seg in query.split('&') {
        match seg.split_once('=') {
            Some(("profile", v)) => id = Some(safe_decode(v)),
            Some(("alert", v)) => alert = serde_json::from_str(&safe_decode(v)).ok(),
            _ => {}
        }
    }
    Some((id?, alert?))
}

async fn wait_backend() {
    // A link can launch the app: wait for px before talking to it.
    for _ in 0..300 {
        if st().backend_ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub fn handle(link: String) {
    crate::window::show();
    if let Some((id, alert)) = parse_alert(&link) {
        app::spawn(async move {
            wait_backend().await;
            app::show_subscription_alert(id, alert);
        });
        return;
    }
    app::spawn(async move { import(link).await });
}

async fn import(link: String) {
    wait_backend().await;
    let Some((url, name)) = parse(&link) else {
        app::error(tr("profiles.deeplink.invalid-url"));
        return;
    };
    if !util::is_http_url(&url) {
        app::error(tr("profiles.deeplink.invalid-url-format"));
        return;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    st().deeplink_cancel = Some(tx);
    app::ui(|w| {
        let a = w.global::<App>();
        a.set_deeplink_message(tr("profiles.deeplink.importing").into());
        a.set_deeplink_cancel(tr("profiles.deeplink.cancel-import").into());
        a.set_deeplink_importing(true);
    });
    let api = ctx().api.clone();
    let mut body = json!({ "content": url });
    if let Some(n) = name {
        body["title"] = json!(n);
    }
    let work = async {
        let list = api.add_profile(body).await?;
        if let Some(first) = list.first() {
            let id = str_of(first, "id");
            let _ = api.switch_profile(&id, true, true).await;
            api.wait_running(Duration::from_secs(5)).await;
        }
        Ok::<_, crate::api::ApiError>(list)
    };
    tokio::select! {
        res = work => {
            match res {
                Ok(_) => {
                    app::load_profiles().await;
                    if let Some(a) = app::active_profile() { st().f_profile = Some(a); }
                    crate::pages::on_profile_switched();
                    app::success(tr("profiles.deeplink.import-success"));
                }
                Err(e) => {
                    if !app::hwid_from_error(&e.body) {
                        app::error(if e.message.is_empty() { tr("profiles.deeplink.import-failed") } else { e.message });
                    }
                }
            }
        }
        _ = rx => {
            app::warning(tr("profiles.deeplink.import-cancelled"));
        }
    }
    st().deeplink_cancel = None;
    app::ui(|w| w.global::<App>().set_deeplink_importing(false));
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_install_config() {
        let (u, n) = parse("prizrak-box://install-config?url=https%3A%2F%2Fa.b%2Fsub%3Fx%3D1&name=My%20VPN").unwrap();
        assert_eq!(u, "https://a.b/sub?x=1");
        assert_eq!(n.as_deref(), Some("My VPN"));
        let (u, _) = parse("prizrak-box://install-config?url=https://a.b/s?x=1&y=2").unwrap();
        assert_eq!(u, "https://a.b/s?x=1&y=2");
        assert!(parse("prizrak-box://other?url=x").is_none());
    }
}
