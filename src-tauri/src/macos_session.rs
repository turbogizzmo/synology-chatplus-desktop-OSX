//! Keeps a DSM sign-in across app restarts when the user chose "Stay signed in".
//!
//! DSM issues its login as a session cookie and records the user's choice in a
//! separate persistent `stay_login` cookie. A browser that restores its session
//! keeps the login; WKWebView drops session cookies when the app quits, so every
//! launch showed the DSM sign-in again. While that opt-in is present, this module
//! re-stores the service host's session cookies as persistent ones that expire
//! no later than the opt-in itself (and at most 30 days ahead).
//!
//! Scope is deliberately narrow: Synology services only, cookies for the
//! service's own trusted host only, and nothing without the opt-in. A copy that
//! would lose its Secure or HttpOnly flag is skipped rather than weakened. When
//! DSM signs the user out it deletes the session cookie, which also removes the
//! persistent copy (same name, domain and path).
use crate::providers::{ProviderId, ServiceConfig};
use block2::RcBlock;
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_foundation::{
    NSArray, NSDate, NSHTTPCookie, NSHTTPCookieDiscard, NSHTTPCookieExpires, NSMutableDictionary,
    NSString,
};
use objc2_web_kit::{WKHTTPCookieStore, WKWebView};
use std::ptr::NonNull;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const OPT_IN: &str = "stay_login";
const MAX_KEEP: f64 = 30. * 24. * 60. * 60.;
// Short enough that quitting soon after signing in still keeps the session.
const INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq)]
struct Meta {
    domain: String,
    name: String,
    value_opts_in: bool,
    session_only: bool,
    expires: Option<f64>,
}

/// Cookie domains may carry a leading dot and cover subdomains of that host.
fn host_matches(domain: &str, host: &str) -> bool {
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
}

fn opts_in(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no"
    )
}

/// When the user opted in for this host: keep sessions until the opt-in ends,
/// but never more than 30 days ahead. `None` means do nothing.
fn keep_until(cookies: &[Meta], host: &str, now: f64) -> Option<f64> {
    cookies
        .iter()
        .filter(|c| c.name == OPT_IN && c.value_opts_in && host_matches(&c.domain, host))
        .filter_map(|c| c.expires)
        .filter(|expires| *expires > now)
        .reduce(f64::max)
        .map(|expires| expires.min(now + MAX_KEEP))
}

fn should_persist(cookie: &Meta, host: &str) -> bool {
    cookie.session_only && cookie.name != OPT_IN && host_matches(&cookie.domain, host)
}

/// The host whose session may be kept: the trusted address the view loads.
pub fn host(app: &AppHandle, service: &ServiceConfig) -> Option<String> {
    if !matches!(
        service.provider,
        ProviderId::SynologyChatplus | ProviderId::SynologyChat
    ) {
        return None;
    }
    url::Url::parse(&crate::quickconnect::effective(app, service))
        .ok()?
        .host_str()
        .map(str::to_string)
}

fn meta(cookie: &NSHTTPCookie) -> Meta {
    let name = cookie.name().to_string();
    Meta {
        domain: cookie.domain().to_string(),
        value_opts_in: name == OPT_IN && opts_in(&cookie.value().to_string()),
        name,
        session_only: cookie.isSessionOnly(),
        expires: cookie.expiresDate().map(|d| d.timeIntervalSince1970()),
    }
}

/// Same cookie with an expiry date, or `None` if any security property would change.
fn persistent_copy(cookie: &NSHTTPCookie, until: f64) -> Option<Retained<NSHTTPCookie>> {
    let properties = cookie.properties()?;
    let copy = NSMutableDictionary::dictionaryWithDictionary(&properties);
    let expires = NSDate::dateWithTimeIntervalSince1970(until);
    unsafe {
        copy.insert(NSHTTPCookieExpires, AsRef::<AnyObject>::as_ref(&*expires));
        copy.removeObjectForKey(NSHTTPCookieDiscard);
    }
    if cookie.isHTTPOnly() {
        // The flag is not always present in `properties`; state it explicitly.
        let flag = NSString::from_str("TRUE");
        copy.insert(
            &*NSString::from_str("HttpOnly"),
            AsRef::<AnyObject>::as_ref(&*flag),
        );
    }
    let persistent = unsafe { NSHTTPCookie::cookieWithProperties(&copy) }?;
    let unchanged = persistent.name() == cookie.name()
        && persistent.value() == cookie.value()
        && persistent.domain() == cookie.domain()
        && persistent.path() == cookie.path()
        && persistent.isSecure() == cookie.isSecure()
        && persistent.isHTTPOnly() == cookie.isHTTPOnly()
        && !persistent.isSessionOnly();
    unchanged.then_some(persistent)
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0., |d| d.as_secs_f64())
}

/// One pass over the view's cookie store. Must run on the main thread.
fn keep(view: &WKWebView, host: String) {
    let store: Retained<WKHTTPCookieStore> =
        unsafe { view.configuration().websiteDataStore().httpCookieStore() };
    let writer = store.clone();
    let block = RcBlock::new(move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
        let cookies: Vec<Retained<NSHTTPCookie>> = unsafe { cookies.as_ref() }.to_vec();
        let metas: Vec<Meta> = cookies.iter().map(|c| meta(c)).collect();
        let Some(until) = keep_until(&metas, &host, now()) else {
            return;
        };
        for (cookie, meta) in cookies.iter().zip(&metas) {
            if !should_persist(meta, &host) {
                continue;
            }
            if let Some(persistent) = persistent_copy(cookie, until) {
                unsafe { writer.setCookie_completionHandler(&persistent, None) };
            }
        }
    });
    unsafe { store.getAllCookies(&block) };
}

/// Runs a pass for every Synology service view; safe to call from any thread.
pub fn keep_all(app: &AppHandle) {
    for service in crate::state::current(app)
        .services
        .iter()
        .filter(|s| s.enabled)
    {
        let (Some(host), Some(view)) = (
            host(app, service),
            app.get_webview(&crate::services::label(&service.id)),
        ) else {
            continue;
        };
        let _ = view.with_webview(move |webview| {
            // SAFETY: on macOS the platform webview is a live WKWebView, and
            // with_webview runs this closure on the main thread.
            let view: &WKWebView = unsafe { &*webview.inner().cast() };
            keep(view, host);
        });
    }
}

pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(INTERVAL).await;
            keep_all(&app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "synr-us2.mynas.direct.quickconnect.to";
    const NOW: f64 = 1_000_000.;
    const DAY: f64 = 24. * 60. * 60.;

    fn cookie(domain: &str, name: &str, session_only: bool, expires: Option<f64>) -> Meta {
        Meta {
            domain: domain.into(),
            name: name.into(),
            value_opts_in: name == OPT_IN,
            session_only,
            expires,
        }
    }

    #[test]
    fn hosts_match_exactly_or_through_a_parent_domain_cookie() {
        assert!(host_matches(HOST, HOST));
        assert!(host_matches(".mynas.direct.quickconnect.to", HOST));
        assert!(host_matches("SYNR-US2.mynas.direct.quickconnect.to", HOST));
        assert!(!host_matches("evil.example", HOST));
        assert!(!host_matches("nas.direct.quickconnect.to", HOST));
        assert!(!host_matches("", HOST));
        assert!(!host_matches(".", HOST));
    }

    #[test]
    fn nothing_is_kept_without_a_live_opt_in_for_this_host() {
        let session = cookie(HOST, "id", true, None);
        assert_eq!(keep_until(&[session.clone()], HOST, NOW), None);
        let other_host = cookie("other.example", OPT_IN, false, Some(NOW + DAY));
        assert_eq!(keep_until(&[session.clone(), other_host], HOST, NOW), None);
        let expired = cookie(HOST, OPT_IN, false, Some(NOW - 1.));
        assert_eq!(keep_until(&[session.clone(), expired], HOST, NOW), None);
        let mut declined = cookie(HOST, OPT_IN, false, Some(NOW + DAY));
        declined.value_opts_in = false;
        assert_eq!(keep_until(&[session, declined], HOST, NOW), None);
    }

    #[test]
    fn kept_sessions_end_with_the_opt_in_and_within_thirty_days() {
        let soon = cookie(HOST, OPT_IN, false, Some(NOW + 2. * DAY));
        assert_eq!(keep_until(&[soon], HOST, NOW), Some(NOW + 2. * DAY));
        let far = cookie(HOST, OPT_IN, false, Some(NOW + 365. * DAY));
        assert_eq!(keep_until(&[far], HOST, NOW), Some(NOW + MAX_KEEP));
    }

    #[test]
    fn only_this_hosts_session_cookies_are_persisted() {
        assert!(should_persist(&cookie(HOST, "id", true, None), HOST));
        assert!(!should_persist(
            &cookie(HOST, "did", false, Some(NOW + DAY)),
            HOST
        ));
        assert!(!should_persist(
            &cookie("tracker.example", "id", true, None),
            HOST
        ));
        assert!(!should_persist(&cookie(HOST, OPT_IN, true, None), HOST));
    }

    fn session_cookie(http_only: bool) -> Retained<NSHTTPCookie> {
        let properties = NSMutableDictionary::<NSString, AnyObject>::new();
        let set = |key: &str, value: &str| {
            let value = NSString::from_str(value);
            properties.insert(
                &*NSString::from_str(key),
                AsRef::<AnyObject>::as_ref(&*value),
            );
        };
        set("Name", "id");
        set("Value", "session-token");
        set("Domain", HOST);
        set("Path", "/");
        set("Secure", "TRUE");
        set("Discard", "TRUE");
        if http_only {
            set("HttpOnly", "TRUE");
        }
        let cookie = unsafe { NSHTTPCookie::cookieWithProperties(&properties) }.unwrap();
        assert!(cookie.isSessionOnly());
        assert_eq!(cookie.isHTTPOnly(), http_only);
        cookie
    }

    #[test]
    fn real_foundation_copy_keeps_every_security_flag_and_gains_an_expiry() {
        let until = now() + DAY;
        for http_only in [true, false] {
            let session = session_cookie(http_only);
            let kept = persistent_copy(&session, until).expect("copy accepted");
            assert!(!kept.isSessionOnly());
            assert_eq!(kept.isHTTPOnly(), http_only);
            assert!(kept.isSecure());
            assert_eq!(kept.name(), session.name());
            assert_eq!(kept.value(), session.value());
            assert_eq!(kept.domain(), session.domain());
            assert_eq!(kept.path(), session.path());
            let expires = kept.expiresDate().unwrap().timeIntervalSince1970();
            assert!((expires - until).abs() < 1.);
            assert_eq!(
                meta(&kept).session_only,
                false,
                "a kept cookie is not picked up again"
            );
        }
    }

    #[test]
    fn opt_in_values_that_mean_no_are_respected() {
        for value in ["", "0", "false", "FALSE", " no "] {
            assert!(!opts_in(value), "{value:?}");
        }
        for value in ["1", "true", "yes"] {
            assert!(opts_in(value), "{value:?}");
        }
    }
}
