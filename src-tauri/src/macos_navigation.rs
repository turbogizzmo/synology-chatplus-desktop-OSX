//! wry hands every WKNavigationAction to Tauri's `on_navigation` with only its
//! URL, including loads inside embedded frames. The service view's top-level
//! origin rule then treated each cross-origin iframe (Synology's notification
//! frame, video embeds) as a followed link: it was cancelled and opened in the
//! default browser. WebView2 never reports subframes there, so Windows was
//! unaffected.
//!
//! This proxy sits in front of wry's navigation delegate. Subframe loads proceed
//! as in any browser; main-frame navigations, new windows and downloads still go
//! to wry and through the app's trust checks unchanged. Every other delegate
//! method is forwarded to wry untouched.
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject, Sel},
    sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_foundation::{NSObject, NSObjectProtocol};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKWebView,
};
use std::{cell::RefCell, collections::HashMap};

pub struct Ivars {
    inner: Retained<ProtocolObject<dyn WKNavigationDelegate>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ChatPlusFrameAwareNavigation"]
    #[ivars = Ivars]
    pub struct FrameAwareNavigation;

    unsafe impl NSObjectProtocol for FrameAwareNavigation {}

    unsafe impl WKNavigationDelegate for FrameAwareNavigation {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide(
            &self,
            webview: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            if subframe_load(action) {
                handler.call((WKNavigationActionPolicy::Allow,));
            } else {
                unsafe {
                    self.ivars()
                        .inner
                        .webView_decidePolicyForNavigationAction_decisionHandler(
                            webview, action, handler,
                        )
                }
            }
        }
    }

    impl FrameAwareNavigation {
        // WebKit caches which delegate methods exist when the delegate is set;
        // report wry's so its commit/finish/download callbacks keep arriving.
        #[unsafe(method(respondsToSelector:))]
        fn responds_to_selector(&self, selector: Sel) -> bool {
            let own: bool = unsafe { msg_send![super(self), respondsToSelector: selector] };
            own || self.ivars().inner.respondsToSelector(selector)
        }

        #[unsafe(method(forwardingTargetForSelector:))]
        fn forwarding_target_for_selector(&self, _selector: Sel) -> *mut AnyObject {
            Retained::as_ptr(&self.ivars().inner) as *mut AnyObject
        }
    }
);

/// A load inside an existing child frame that is not a download.
fn subframe_load(action: &WKNavigationAction) -> bool {
    unsafe {
        // No target frame means a new window: wry and the UI delegate decide that.
        let Some(frame) = action.targetFrame() else {
            return false;
        };
        if frame.isMainFrame() {
            return false;
        }
        // shouldPerformDownload exists from macOS 11.3; downloads stay with wry.
        let download = action.respondsToSelector(sel!(shouldPerformDownload))
            && action.shouldPerformDownload();
        !download
    }
}

thread_local! {
    // WebKit holds its navigation delegate weakly; keep one proxy per service view.
    static INSTALLED: RefCell<HashMap<String, Retained<FrameAwareNavigation>>> =
        RefCell::new(HashMap::new());
}

pub fn install(label: &str, view: &WKWebView, mtm: MainThreadMarker) {
    let Some(current) = (unsafe { view.navigationDelegate() }) else {
        return;
    };
    // Re-attaching the same view must not stack proxies.
    if current.isKindOfClass(FrameAwareNavigation::class()) {
        return;
    }
    let proxy = mtm
        .alloc::<FrameAwareNavigation>()
        .set_ivars(Ivars { inner: current });
    let proxy: Retained<FrameAwareNavigation> = unsafe { msg_send![super(proxy), init] };
    unsafe { view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*proxy))) };
    INSTALLED.with(|installed| {
        installed.borrow_mut().insert(label.to_string(), proxy);
    });
}
