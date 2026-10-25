/// WebView2 provides reliable transport completion; generic Tauri page-finished
/// events on other platforms cannot distinguish a server document from an error page.
#[cfg(windows)]
pub fn observe_connection(
    app: &tauri::AppHandle,
    window: &tauri::Webview,
    service_id: &str,
) -> tauri::Result<()> {
    use crate::connection::{self, Connection};
    use crate::load_state::{self, Reason};
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT,
            COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT,
            COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED,
            COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID,
            COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED,
            COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED,
            COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED,
            COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
            COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE,
            COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT,
        },
        NavigationCompletedEventHandler,
    };
    let app = app.clone();
    let service_id = service_id.to_string();
    window.with_webview(move |webview| unsafe {
        let Ok(core) = webview.controller().CoreWebView2() else {
            load_state::ready(&app, &service_id);
            connection::set_for(&app, &service_id, Connection::Unknown);
            return;
        };
        let handler_app = app.clone();
        let handler_id = service_id.clone();
        let result = core.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut success = Default::default();
                    args.IsSuccess(&mut success)?;
                    let state = if success.as_bool() {
                        crate::unread::sync_presentation(&handler_app);
                        load_state::ready(&handler_app, &handler_id);
                        Connection::Connected
                    } else {
                        let mut status = Default::default();
                        args.WebErrorStatus(&mut status)?;
                        if status == COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
                            return Ok(());
                        }
                        let reason = match status {
                            COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED => Reason::Offline,
                            COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED => Reason::Host,
                            COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT
                            | COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE => Reason::Refused,
                            COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT => Reason::Timeout,
                            COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT
                            | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED
                            | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED
                            | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID => {
                                Reason::Certificate
                            }
                            _ => Reason::Other,
                        };
                        load_state::fail(&handler_app, &handler_id, reason);
                        if reason == Reason::Offline {
                            Connection::Offline
                        } else {
                            Connection::Error
                        }
                    };
                    connection::set_for(&handler_app, &handler_id, state);
                }
                Ok(())
            })),
            &mut 0,
        );
        if result.is_err() {
            // Without completion events the view could never leave the loading screen.
            load_state::ready(&app, &service_id);
            connection::set_for(&app, &service_id, Connection::Unknown);
        }
    })
}
/// macOS has no transport-completion hook here (see `page_finished`), but this is
/// where each service view gets its frame-aware navigation delegate.
#[cfg(target_os = "macos")]
pub fn observe_connection(
    app: &tauri::AppHandle,
    window: &tauri::Webview,
    service_id: &str,
) -> tauri::Result<()> {
    let label = window.label().to_string();
    let app = app.clone();
    let service_id = service_id.to_string();
    window.with_webview(move |webview| {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        // SAFETY: on macOS the platform webview is a live WKWebView and
        // with_webview runs this on the main thread.
        let view: &objc2_web_kit::WKWebView = unsafe { &*webview.inner().cast() };
        crate::macos_navigation::install(&app, &label, &service_id, view, mtm);
    })
}
#[cfg(not(any(windows, target_os = "macos")))]
pub fn observe_connection(
    app: &tauri::AppHandle,
    _: &tauri::Webview,
    service_id: &str,
) -> tauri::Result<()> {
    // No load-failure signal here, so never hold the view behind the loading screen.
    crate::load_state::ready(app, service_id);
    Ok(())
}

/// Windows learns completion from NavigationCompleted above.
#[cfg(windows)]
pub fn page_finished(_: &tauri::AppHandle, _: &str) {}

/// WKWebView (via wry) reports Finished only from didFinishNavigation; failed
/// provisional loads emit nothing, so Finished means the server document loaded.
/// Re-project unread here, as NavigationCompleted does, or the presentation
/// invalidated on Started stays stale until the provider's next observation.
#[cfg(not(windows))]
pub fn page_finished(app: &tauri::AppHandle, service_id: &str) {
    crate::unread::sync_presentation(app);
    crate::connection::set_for(app, service_id, crate::connection::Connection::Connected);
}
