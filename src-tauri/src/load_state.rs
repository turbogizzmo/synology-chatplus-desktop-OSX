//! Whether a service view has shown its server's page yet. Until it has, the
//! native view stays hidden and the shell shows a loading or failure screen in
//! its place; a WebView that never reached its server is only a blank page.
//! Once a page has loaded, a later failed navigation leaves it on screen (WebKit
//! keeps the old document) and only the footer's connection state reports it.
use std::{collections::HashMap, sync::Mutex, time::Duration};
use tauri::{AppHandle, Emitter, Manager};

/// Long enough for a sleeping NAS or a slow relay, short enough that a first run
/// against a wrong address does not look like an endless spinner.
#[cfg(target_os = "macos")]
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    Offline,
    Host,
    Refused,
    Timeout,
    Certificate,
    Insecure,
    QuickConnect,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum Phase {
    Loading,
    Ready,
    Failed { reason: Reason },
}

#[derive(Default)]
pub struct LoadState(Mutex<HashMap<String, (Phase, u64)>>);

fn update(app: &AppHandle, id: &str, change: impl FnOnce(Phase) -> Option<Phase>) -> Option<u64> {
    let state = app.state::<LoadState>();
    let mut entries = state.0.lock().unwrap_or_else(|e| e.into_inner());
    let (phase, generation) = entries.get(id).copied().unwrap_or((Phase::Loading, 0));
    let next = change(phase)?;
    let generation = generation + 1;
    entries.insert(id.into(), (next, generation));
    drop(entries);
    if next != phase {
        crate::services::present(app);
        let _ = app.emit_to("main", "service-load-changed", snapshot(app));
    }
    Some(generation)
}

/// A view waiting for something other than its own load, such as QuickConnect.
pub fn waiting(app: &AppHandle, id: &str) {
    update(app, id, |phase| {
        (phase != Phase::Ready).then_some(Phase::Loading)
    });
}

/// A real navigation toward the service is starting, first or as a retry after
/// a failure; it must commit in time.
pub fn begin(app: &AppHandle, id: &str) {
    let Some(_generation) = update(app, id, |phase| {
        (phase != Phase::Ready).then_some(Phase::Loading)
    }) else {
        return;
    };
    // WebView2 times out its own navigations and reports them as failures.
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        let id = id.to_string();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(LOAD_TIMEOUT).await;
            let state = app.state::<LoadState>();
            let current = state
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&id)
                .copied();
            if current == Some((Phase::Loading, _generation)) {
                fail(&app, &id, Reason::Timeout);
            }
        });
    }
}

pub fn ready(app: &AppHandle, id: &str) {
    update(app, id, |phase| {
        (phase != Phase::Ready).then_some(Phase::Ready)
    });
}

/// Only a view that never showed a page swaps to the failure screen.
pub fn fail(app: &AppHandle, id: &str, reason: Reason) {
    let failed = update(app, id, |phase| {
        (phase == Phase::Loading).then_some(Phase::Failed { reason })
    });
    if failed.is_some() {
        crate::connection::set_for(
            app,
            id,
            if reason == Reason::Offline {
                crate::connection::Connection::Offline
            } else {
                crate::connection::Connection::Error
            },
        );
    }
}

pub fn remove(app: &AppHandle, id: &str) {
    app.state::<LoadState>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
}

pub fn visible(app: &AppHandle, id: &str) -> bool {
    phase(app, id) == Phase::Ready
}

pub fn phase(app: &AppHandle, id: &str) -> Phase {
    app.state::<LoadState>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map_or(Phase::Loading, |(phase, _)| *phase)
}

pub fn snapshot(app: &AppHandle) -> HashMap<String, Phase> {
    app.state::<LoadState>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(id, (phase, _))| (id.clone(), *phase))
        .collect()
}

/// NSURLErrorDomain codes from a failed provisional load, or None when the
/// navigation was cancelled on purpose (a replaced load, a download, a link
/// the app opened in the browser instead).
pub fn reason_for_url_error(domain: &str, code: isize) -> Option<Reason> {
    if domain == "WebKitErrorDomain" {
        // 102: interrupted by a policy change (downloads); 204: handled by a plug-in.
        return (!matches!(code, 102 | 204)).then_some(Reason::Other);
    }
    if domain != "NSURLErrorDomain" {
        return Some(Reason::Other);
    }
    Some(match code {
        -999 => return None,
        -1009 | -1018 | -1020 => Reason::Offline,
        -1003 | -1006 => Reason::Host,
        -1004 | -1005 => Reason::Refused,
        -1001 => Reason::Timeout,
        -1022 => Reason::Insecure,
        -1206..=-1200 | -2000 => Reason::Certificate,
        _ => Reason::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deliberate_cancellations_are_not_failures() {
        assert_eq!(reason_for_url_error("NSURLErrorDomain", -999), None);
        assert_eq!(reason_for_url_error("WebKitErrorDomain", 102), None);
        assert_eq!(reason_for_url_error("WebKitErrorDomain", 204), None);
    }
    #[test]
    fn first_run_mistakes_get_specific_reasons() {
        let reason = |code| reason_for_url_error("NSURLErrorDomain", code);
        assert_eq!(reason(-1003), Some(Reason::Host));
        assert_eq!(reason(-1004), Some(Reason::Refused));
        assert_eq!(reason(-1001), Some(Reason::Timeout));
        assert_eq!(reason(-1009), Some(Reason::Offline));
        assert_eq!(reason(-1202), Some(Reason::Certificate));
        assert_eq!(reason(-1022), Some(Reason::Insecure));
        assert_eq!(reason(-1), Some(Reason::Other));
        assert_eq!(
            reason_for_url_error("kCFErrorDomainCFNetwork", 1),
            Some(Reason::Other)
        );
    }
    #[test]
    fn phases_serialize_for_the_shell() {
        assert_eq!(
            serde_json::to_value(Phase::Failed {
                reason: Reason::QuickConnect
            })
            .unwrap(),
            serde_json::json!({ "phase": "failed", "reason": "quickConnect" })
        );
        assert_eq!(
            serde_json::to_value(Phase::Ready).unwrap(),
            serde_json::json!({ "phase": "ready" })
        );
    }
}
