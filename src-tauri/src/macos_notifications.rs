//! Native macOS delivery through UNUserNotificationCenter. The notification
//! plugin's desktop path uses the deprecated NSUserNotificationCenter, discards
//! delivery errors and always reports permission as granted; this keeps real
//! authorization state, reports failures and routes clicks to their service.
//!
//! macOS refuses UNUserNotificationCenter to ad-hoc signed apps (no team ID)
//! with UNErrorDomain code 1. Such builds, and unbundled development runs, fall
//! back to AppleScript `display notification`, which shows under Script Editor
//! and cannot route clicks. Signing with an Apple Development identity avoids it.
use block2::RcBlock;
use objc2::{
    define_class, msg_send, rc::Retained, runtime::ProtocolObject, AllocAnyThread, DefinedClass,
};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use std::process::{Command, Stdio};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;
use tauri::Emitter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authorization {
    /// Not running from an .app bundle (tauri dev, tests); UN would throw.
    Unavailable,
    /// macOS rejected this build's signature for UserNotifications.
    NotAllowed,
    NotDetermined,
    Denied,
    Authorized,
}

const UNKNOWN: u8 = u8::MAX;
/// UNErrorCodeNotificationsNotAllowed.
const NOT_ALLOWED: isize = 1;
static STATUS: AtomicU8 = AtomicU8::new(UNKNOWN);
static REFUSED: AtomicBool = AtomicBool::new(false);
static BUNDLED: OnceLock<bool> = OnceLock::new();

/// UNUserNotificationCenter raises an Objective-C exception for a process
/// without a bundle proxy, so only use it from a real `.app`.
fn bundled() -> bool {
    *BUNDLED.get_or_init(|| {
        std::env::current_exe().is_ok_and(|exe| {
            exe.ancestors()
                .any(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        })
    })
}

fn authorization_from(status: UNAuthorizationStatus) -> Authorization {
    match status {
        UNAuthorizationStatus::NotDetermined => Authorization::NotDetermined,
        UNAuthorizationStatus::Denied => Authorization::Denied,
        // Provisional and ephemeral authorizations deliver quietly; treat as allowed.
        _ => Authorization::Authorized,
    }
}

fn store(authorization: Authorization) {
    STATUS.store(authorization as u8, Ordering::Release);
}

/// Last known authorization; refreshed at startup and after each request.
pub fn authorization() -> Authorization {
    if !bundled() {
        return Authorization::Unavailable;
    }
    // A signature refusal also reads back as Denied; keep the more precise cause.
    if REFUSED.load(Ordering::Acquire) {
        return Authorization::NotAllowed;
    }
    match STATUS.load(Ordering::Acquire) {
        s if s == Authorization::NotDetermined as u8 => Authorization::NotDetermined,
        s if s == Authorization::Denied as u8 => Authorization::Denied,
        s if s == Authorization::Authorized as u8 => Authorization::Authorized,
        // Startup has not heard back yet; do not report a denial that never happened.
        _ => Authorization::NotDetermined,
    }
}

pub fn available() -> bool {
    bundled()
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ChatPlusNotificationDelegate"]
    #[ivars = tauri::AppHandle]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            // deliver_for already applied mute and viewed-conversation suppression;
            // without this macOS hides banners whenever the app is frontmost.
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let service = response
                .notification()
                .request()
                .content()
                .threadIdentifier()
                .to_string();
            let app = self.ivars().clone();
            // Restore off the delegate callback, as single-instance activation does.
            std::thread::spawn(move || {
                let _ = crate::window::activate_notification(
                    &app,
                    (!service.is_empty()).then_some(service.as_str()),
                );
            });
            handler.call(());
        }
    }
);

impl Delegate {
    fn new(app: tauri::AppHandle) -> Retained<Self> {
        let this = Self::alloc().set_ivars(app);
        unsafe { msg_send![super(this), init] }
    }
}

fn refresh(center: &UNUserNotificationCenter) {
    let block = RcBlock::new(|settings: NonNull<UNNotificationSettings>| {
        store(authorization_from(
            unsafe { settings.as_ref() }.authorizationStatus(),
        ));
    });
    center.getNotificationSettingsWithCompletionHandler(&block);
}

pub fn init(app: &tauri::AppHandle, request: bool) {
    if !bundled() {
        return;
    }
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let delegate = Delegate::new(app.clone());
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // The center keeps only a weak delegate reference; it must live for the process.
    let _ = Retained::into_raw(delegate);
    if request {
        request_authorization();
    } else {
        refresh(&center);
    }
}

/// Shows the system prompt the first time; later calls just report the stored choice.
pub fn request_authorization() {
    if !bundled() {
        return;
    }
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let block = RcBlock::new(|_granted: objc2::runtime::Bool, error: *mut NSError| {
        note_refusal(error);
        refresh(&UNUserNotificationCenter::currentNotificationCenter());
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &block,
    );
}

pub fn show(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    sound: bool,
    service_id: Option<&str>,
) -> Result<(), String> {
    if !bundled() || REFUSED.load(Ordering::Acquire) {
        return show_with_script(title, body, sound);
    }
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    if sound {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    // Groups banners per service and tells a click which service to open.
    if let Some(id) = service_id {
        content.setThreadIdentifier(&NSString::from_str(id));
    }
    let identifier = format!("chatplus-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&identifier),
        &content,
        None,
    );
    let app = app.clone();
    let (title, body) = (title.to_owned(), body.to_owned());
    let completion = RcBlock::new(move |error: *mut NSError| {
        if error.is_null() {
            return;
        }
        // Notifications enabled after startup reach the signature refusal here first.
        if note_refusal(error) && show_with_script(&title, &body, sound).is_ok() {
            return;
        }
        // Same signal Windows raises when a toast cannot be submitted.
        let _ = app.emit_to("main", "notification-issue", ());
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, Some(&completion));
    Ok(())
}

/// Records UNErrorDomain code 1 so later deliveries go straight to the fallback.
fn note_refusal(error: *mut NSError) -> bool {
    let Some(error) = (unsafe { error.as_ref() }) else {
        return false;
    };
    let refused = error.code() == NOT_ALLOWED && error.domain().to_string() == "UNErrorDomain";
    if refused {
        REFUSED.store(true, Ordering::Release);
    }
    refused
}

fn script_command(title: &str, body: &str, sound: bool) -> Command {
    // Provider text travels as argv and is never spliced into AppleScript source.
    let script = if sound {
        "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv) sound name \"Glass\"\nend run"
    } else {
        "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run"
    };
    let mut command = Command::new("/usr/bin/osascript");
    command
        .args(["-e", script, title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn show_with_script(title: &str, body: &str, sound: bool) -> Result<(), String> {
    let mut command = script_command(title, body, sound);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    // Reap the short-lived osascript process without blocking delivery.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_maps_every_status_conservatively() {
        assert_eq!(
            authorization_from(UNAuthorizationStatus::NotDetermined),
            Authorization::NotDetermined
        );
        assert_eq!(
            authorization_from(UNAuthorizationStatus::Denied),
            Authorization::Denied
        );
        for allowed in [
            UNAuthorizationStatus::Authorized,
            UNAuthorizationStatus::Provisional,
            UNAuthorizationStatus::Ephemeral,
        ] {
            assert_eq!(authorization_from(allowed), Authorization::Authorized);
        }
    }

    #[test]
    fn script_fallback_passes_provider_text_only_as_arguments() {
        let hostile = "\" & (do shell script \"id\") & \"";
        let command = script_command(hostile, "body \"quoted\"", true);
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(command.get_program(), "/usr/bin/osascript");
        assert_eq!(args.len(), 4);
        assert_eq!(args[0], "-e");
        assert!(!args[1].contains(hostile) && !args[1].contains("quoted"));
        assert_eq!(args[2], hostile);
        assert_eq!(args[3], "body \"quoted\"");
        assert!(args[1].contains("sound name"));
        assert!(!script_command("t", "b", false)
            .get_args()
            .any(|arg| arg.to_string_lossy().contains("sound name")));
    }

    #[test]
    fn test_binaries_never_touch_the_notification_center() {
        // cargo test runs outside any .app; UN would raise instead of failing softly.
        assert!(!available());
        assert_eq!(authorization(), Authorization::Unavailable);
    }
}
