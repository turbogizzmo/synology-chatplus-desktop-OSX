//! macOS transport for the provider bridge. WKWebView has neither WebView2's
//! WebMessageReceived nor its NotificationReceived event, so one
//! WKScriptMessageHandler carries unread observations, notification arrivals
//! (from the injected `webkit-notifications.js` shim) and viewing-query results.
//! The trust rules match Windows: main frame only, exact configured origin,
//! current bridge only, and no Tauri IPC capability for the remote page.
use super::{
    current_bridge, parse_diagnostic, parse_unread, trusted_origin, viewing_query, EventHistory,
    PendingNotification, PendingQueries, EVENT_HISTORY_LIMIT,
};
use objc2::{
    define_class, msg_send, rc::Retained, runtime::ProtocolObject, DefinedClass, MainThreadOnly,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};
use objc2_web_kit::{WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView};
use std::cell::RefCell;
use std::sync::{LazyLock, Mutex};
use tauri::Manager;

const HANDLER: &str = "chatplusBridge";
// Unread proofs stay under 8 KiB; notification content is truncated by the shim.
const MESSAGE_LIMIT: usize = 16 * 1024;
const TITLE_LIMIT: usize = 512;
const BODY_LIMIT: usize = 4096;
const TAG_LIMIT: usize = 512;

// Viewing queries complete through a later script message or a timeout, both on
// the main thread, so pending arrivals live in one bounded shared table.
static QUERIES: LazyLock<Mutex<PendingQueries<PendingNotification>>> =
    LazyLock::new(Default::default);

pub struct Ivars {
    app: tauri::AppHandle,
    service: crate::providers::ServiceConfig,
    bridge_id: u64,
    seen: RefCell<EventHistory<(String, u64)>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ChatPlusBridgeHandler"]
    #[ivars = Ivars]
    pub struct BridgeHandler;

    unsafe impl NSObjectProtocol for BridgeHandler {}

    unsafe impl WKScriptMessageHandler for BridgeHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            let (Some(origin), Some(json)) = (main_frame_origin(message), body(message)) else {
                return;
            };
            self.ivars().receive(&origin, &json);
        }
    }
);

impl BridgeHandler {
    fn new(mtm: MainThreadMarker, ivars: Ivars) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }
}

fn main_frame_origin(message: &WKScriptMessage) -> Option<String> {
    let frame = unsafe { message.frameInfo() };
    if !unsafe { frame.isMainFrame() } {
        return None;
    }
    let origin = unsafe { frame.securityOrigin() };
    let (scheme, host, port) = unsafe {
        (
            origin.protocol().to_string(),
            origin.host().to_string(),
            origin.port(),
        )
    };
    Some(web_origin(&scheme, &host, port))
}

fn body(message: &WKScriptMessage) -> Option<String> {
    let body = unsafe { message.body() };
    let text = body.downcast::<NSString>().ok()?.to_string();
    (text.len() <= MESSAGE_LIMIT).then_some(text)
}

/// WKSecurityOrigin reports a bare IPv6 host and port 0 for the scheme default.
fn web_origin(scheme: &str, host: &str, port: isize) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port > 0 {
        format!("{scheme}://{host}:{port}")
    } else {
        format!("{scheme}://{host}")
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct IncomingNotification {
    chatplus_notification: u8,
    session: String,
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    silent: bool,
}

fn parse_notification(json: &str) -> Option<IncomingNotification> {
    let value: IncomingNotification = serde_json::from_str(json).ok()?;
    if value.chatplus_notification != 1
        || value.id == 0
        || value.session.len() > 64
        || !value.session.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let bound = |text: String, limit: usize| text.chars().take(limit).collect::<String>();
    Some(IncomingNotification {
        title: bound(value.title, TITLE_LIMIT),
        body: bound(value.body, BODY_LIMIT),
        tag: bound(value.tag, TAG_LIMIT),
        ..value
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewingResult {
    chatplus_viewing: u8,
    token: u64,
    viewed: bool,
}

fn parse_viewing(json: &str) -> Option<ViewingResult> {
    let value: ViewingResult = serde_json::from_str(json).ok()?;
    (value.chatplus_viewing == 1).then_some(value)
}

/// Same gate WebView2's PermissionRequested applies on Windows. The shim always
/// reports "granted" to the page, so a denied state is enforced here instead.
fn notifications_allowed(
    app: &tauri::AppHandle,
    service: &crate::providers::ServiceConfig,
) -> bool {
    let settings = crate::state::current(app);
    settings.desktop_notifications
        && service.supports_notifications()
        && settings
            .services
            .iter()
            .any(|s| s.id == service.id && s.notifications)
        && crate::desktop_notifications::permission(app).granted
}

impl Ivars {
    fn receive(&self, origin: &str, json: &str) {
        let (app, service) = (&self.app, &self.service);
        if !crate::services::configured(app, service)
            || !current_bridge(app, &service.id, self.bridge_id)
            || !trusted_origin(&crate::quickconnect::effective(app, service), origin)
        {
            return;
        }
        if let Some(result) = parse_viewing(json) {
            finish_query(app, &service.id, result.token, result.viewed);
            return;
        }
        if let Some(notification) = parse_notification(json) {
            if service.supports_notifications() && notifications_allowed(app, service) {
                self.notification(notification);
            }
            return;
        }
        if !service.supports_unread() {
            return;
        }
        if let Some(diagnostic) = parse_diagnostic(json) {
            if diagnostic.source.accepts(service.unread_source_provider()) {
                crate::diagnostics::notification(
                    app,
                    &service.id,
                    diagnostic.event,
                    diagnostic.reason,
                    diagnostic.generation,
                );
            }
        } else if let Some(unread) = parse_unread(json) {
            crate::unread::publish_for(
                app,
                &service.id,
                unread.count,
                unread.has_unread,
                unread.source,
                &unread.reason,
                unread.acknowledgement,
                &unread.read_arrivals,
                unread.proof_only,
                unread.provider_zero,
            );
        }
    }

    fn notification(&self, incoming: IncomingNotification) {
        use crate::diagnostics::{NotificationEvent as Event, NotificationReason as Reason};
        let (app, service) = (&self.app, &self.service);
        let (token, first_observation) = {
            let mut seen = self.seen.borrow_mut();
            let (token, first) = seen.observe((incoming.session, incoming.id));
            (token, first)
        };
        // Mirrors the WebView2 NotificationReceived path: a replayed event must not
        // resurrect acknowledged unread, and each arrival gets its own generation.
        let read_generation = self.seen.borrow().read_generation(token);
        let generation =
            crate::unread::begin_incoming_for(app, &service.id, first_observation, read_generation);
        self.seen
            .borrow_mut()
            .remember_read_generation(token, generation.read);
        if !first_observation {
            crate::diagnostics::notification(
                app,
                &service.id,
                Event::DuplicateSuppressed,
                Reason::DuplicateEvent,
                generation.read,
            );
        }
        crate::diagnostics::notification(
            app,
            &service.id,
            Event::Normalized,
            if incoming.silent {
                Reason::ProviderSilent
            } else {
                Reason::BrowserEvent
            },
            generation.read,
        );
        query_view(
            app,
            PendingNotification {
                service: service.clone(),
                title: incoming.title,
                body: incoming.body,
                tag: incoming.tag,
                identity: format!("{}:{token}", self.bridge_id),
                first_observation,
                generation: generation.read,
                visibility_generation: generation.visibility,
                bridge_id: self.bridge_id,
                provider_silent: incoming.silent,
            },
        );
    }
}

/// Ask the provider whether the arrival's conversation is on screen, as the
/// Windows path does with ExecuteScript. The answer returns as a script message;
/// a missing, failed or slow renderer falls back to conservative delivery.
fn query_view(app: &tauri::AppHandle, pending: PendingNotification) {
    let Some(view) = app.get_webview(&crate::services::label(&pending.service.id)) else {
        pending.finish(app, false);
        return;
    };
    let script_query = viewing_query(&pending.tag, &pending.title, pending.generation);
    let service_id = pending.service.id.clone();
    let token = {
        let mut queries = QUERIES.lock().unwrap_or_else(|e| e.into_inner());
        if queries.values.len() >= EVENT_HISTORY_LIMIT {
            None
        } else {
            Some(queries.insert(pending.clone()))
        }
    };
    let Some(token) = token else {
        pending.finish(app, false);
        return;
    };
    let script = format!(
        "(()=>{{const viewed={script_query};try{{window.webkit.messageHandlers.{HANDLER}.postMessage(JSON.stringify({{chatplusViewing:1,token:{token},viewed}}))}}catch{{}}}})()"
    );
    if view.eval(&script).is_err() {
        finish_query(app, &service_id, token, false);
        return;
    }
    let timer_app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let dispatcher = timer_app.clone();
        let _ = dispatcher.run_on_main_thread(move || {
            finish_query(&timer_app, &service_id, token, false);
        });
    });
}

/// Callback and timeout race for one record; only the first may deliver.
fn finish_query(app: &tauri::AppHandle, service_id: &str, token: u64, viewed: bool) {
    let pending = {
        let mut queries = QUERIES.lock().unwrap_or_else(|e| e.into_inner());
        // A page may only answer queries issued for its own service.
        match queries.values.get(&token) {
            Some(pending) if pending.service.id == service_id => queries.take(token),
            _ => None,
        }
    };
    if let Some(pending) = pending {
        pending.finish(app, viewed);
    }
}

pub fn attach(
    app: &tauri::AppHandle,
    window: &tauri::Webview,
    service: &crate::providers::ServiceConfig,
) -> tauri::Result<()> {
    use crate::desktop_notifications as notifications;
    static NEXT_BRIDGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let app = app.clone();
    let service = service.clone();
    window.with_webview(move |webview| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let bridge_id = NEXT_BRIDGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let settings = crate::state::current(&app);
            let state = app.state::<notifications::Service>();
            let mut tracking = state.0.lock().unwrap_or_else(|e| e.into_inner());
            tracking
                .bridges
                .retain(|id, _| settings.services.iter().any(|s| s.id == *id && s.enabled));
            tracking.bridges.insert(
                service.id.clone(),
                (bridge_id, service.supports_notifications()),
            );
        }
        let handler = BridgeHandler::new(
            mtm,
            Ivars {
                app: app.clone(),
                service: service.clone(),
                bridge_id,
                seen: RefCell::new(EventHistory::default()),
            },
        );
        // SAFETY: on macOS Tauri's platform webview is a live WKWebView, and
        // with_webview runs this closure on the main thread.
        unsafe {
            let view: &WKWebView = &*webview.inner().cast();
            let controller = view.configuration().userContentController();
            let name = NSString::from_str(HANDLER);
            // WebKit throws on a duplicate handler name; replace any earlier bridge.
            controller.removeScriptMessageHandlerForName(&name);
            controller.addScriptMessageHandler_name(ProtocolObject::from_ref(&*handler), &name);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_origin_parts_match_configured_server_origins() {
        assert!(trusted_origin(
            "https://nas.example.com/chat/",
            &web_origin("https", "nas.example.com", 0)
        ));
        assert!(trusted_origin(
            "https://nas.example.com:5001/",
            &web_origin("https", "nas.example.com", 5001)
        ));
        assert!(trusted_origin(
            "http://[fd00::1]:5000/",
            &web_origin("http", "fd00::1", 5000)
        ));
        assert!(!trusted_origin(
            "https://nas.example.com/",
            &web_origin("https", "nas.example.com", 5001)
        ));
        assert!(!trusted_origin(
            "https://nas.example.com/",
            &web_origin("https", "evil.example.com", 0)
        ));
    }

    #[test]
    fn notification_messages_are_validated_and_bounded() {
        let parsed = parse_notification(&format!(
            r#"{{"chatplusNotification":1,"session":"0a0b","id":3,"title":"{}","body":"b","tag":"t","silent":true}}"#,
            "x".repeat(600)
        ))
        .unwrap();
        assert_eq!(parsed.title.chars().count(), TITLE_LIMIT);
        assert_eq!((parsed.id, parsed.silent), (3, true));
        for json in [
            r#"{"chatplusNotification":2,"session":"0a","id":1}"#,
            r#"{"chatplusNotification":1,"session":"zz","id":1}"#,
            r#"{"chatplusNotification":1,"session":"0a","id":0}"#,
            r#"{"chatplusUnread":1,"hasUnread":true}"#,
        ] {
            assert!(parse_notification(json).is_none(), "{json}");
        }
        let viewed = parse_viewing(r#"{"chatplusViewing":1,"token":9,"viewed":true}"#).unwrap();
        assert_eq!((viewed.token, viewed.viewed), (9, true));
        assert!(parse_viewing(r#"{"chatplusViewing":0,"token":9,"viewed":true}"#).is_none());
        assert!(parse_viewing(r#"{"chatplusNotification":1,"session":"0a","id":1}"#).is_none());
    }

    #[test]
    fn viewing_answers_are_scoped_to_their_service_and_consumed_once() {
        let mut queries = PendingQueries::<&str>::default();
        let token = queries.insert("service-a");
        assert_eq!(queries.values.get(&token), Some(&"service-a"));
        assert_eq!(queries.take(token), Some("service-a"));
        assert_eq!(queries.take(token), None);
    }
}
