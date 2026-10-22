/// WebView2 provides reliable transport completion; generic Tauri page-finished
/// events on other platforms cannot distinguish a server document from an error page.
#[cfg(windows)]
pub fn observe_connection(
    app: &tauri::AppHandle,
    window: &tauri::Webview,
    service_id: &str,
) -> tauri::Result<()> {
    use crate::connection::{self, Connection};
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED,
            COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
        },
        NavigationCompletedEventHandler,
    };
    let app = app.clone();
    let service_id = service_id.to_string();
    window.with_webview(move |webview| unsafe {
        let Ok(core) = webview.controller().CoreWebView2() else {
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
                        Connection::Connected
                    } else {
                        let mut status = Default::default();
                        args.WebErrorStatus(&mut status)?;
                        if status == COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
                            return Ok(());
                        }
                        if status == COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED {
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
            connection::set_for(&app, &service_id, Connection::Unknown);
        }
    })
}
/// macOS has no transport-completion hook here (see `page_finished`), but this is
/// where each service view gets its frame-aware navigation delegate.
#[cfg(target_os = "macos")]
pub fn observe_connection(
    _: &tauri::AppHandle,
    window: &tauri::Webview,
    _: &str,
) -> tauri::Result<()> {
    let label = window.label().to_string();
    window.with_webview(move |webview| {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        // SAFETY: on macOS the platform webview is a live WKWebView and
        // with_webview runs this on the main thread.
        let view: &objc2_web_kit::WKWebView = unsafe { &*webview.inner().cast() };
        crate::macos_navigation::install(&label, view, mtm);
    })
}
#[cfg(not(any(windows, target_os = "macos")))]
pub fn observe_connection(_: &tauri::AppHandle, _: &tauri::Webview, _: &str) -> tauri::Result<()> {
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
