//! QuickConnect setup. A Synology service may be configured with its shareable
//! QuickConnect link, stored canonically as `https://quickconnect.to/<id>/<path>/`.
//! The link itself redirects across hosts, which the exact-origin navigation rules
//! would block, so the app resolves the ID the way Synology's clients do: ask the
//! QuickConnect directory for the NAS's addresses, probe each over HTTPS with
//! `/webman/pingpong.cgi`, and keep only an address whose `ezid` proves it is the
//! same NAS (MD5 of its server ID). The verified address becomes the service's
//! effective origin for loading and for every trust check; until one exists the
//! service loads nothing rather than following an unverified redirect.
//!
//! Plain HTTP candidates are never used, so traffic is not sent unencrypted.
use crate::providers::{ProviderId, ServiceConfig};
use md5::{Digest, Md5};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use url::Url;

const SERVICE_ID: &str = "dsm_portal_https";
// The DSM root: the service view adds `?launchApp=` (see ProviderId::dsm_launch).
const DEFAULT_PATH: &str = "/";
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const CACHE_FILE: &str = "quickconnect.json";
const WATCH_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    domain: &'static str,
    pub path: String,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 63
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Parses the canonical stored form. Anything else is an ordinary server URL.
pub fn target(configured: &str) -> Option<Target> {
    let url = Url::parse(configured).ok()?;
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let domain = match url.host_str()? {
        "quickconnect.to" => "quickconnect.to",
        "quickconnect.cn" => "quickconnect.cn",
        _ => return None,
    };
    let mut segments = url.path_segments()?.filter(|s| !s.is_empty());
    let id = segments.next()?.to_ascii_lowercase();
    if !valid_id(&id) {
        return None;
    }
    let rest: Vec<&str> = segments.collect();
    let path = if rest.is_empty() {
        DEFAULT_PATH.to_string()
    } else {
        format!("/{}/", rest.join("/"))
    };
    Some(Target { id, domain, path })
}

/// Whether this service is set up by QuickConnect ID (Synology providers only).
pub fn uses_quickconnect(service: &ServiceConfig) -> bool {
    applies(service).is_some()
}

/// QuickConnect serves Synology DSM; other providers keep their URL literally.
fn applies(service: &ServiceConfig) -> Option<Target> {
    matches!(
        service.provider,
        ProviderId::SynologyChatplus | ProviderId::SynologyChat
    )
    .then(|| target(&service.url))
    .flatten()
}

#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq, Debug)]
struct Resolved {
    configured: String,
    effective: String,
    /// Lets the watcher re-verify the address with a single pingpong.
    #[serde(default)]
    server_id: String,
}

#[derive(Default)]
pub struct Cache(Mutex<HashMap<String, Resolved>>);

fn cache_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_local_data_dir()
        .ok()
        .map(|dir| dir.join(CACHE_FILE))
}

pub fn load(app: &AppHandle) {
    let stored = cache_path(app)
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<HashMap<String, Resolved>>(&bytes).ok())
        .unwrap_or_default();
    *app.state::<Cache>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = stored;
}

fn store(app: &AppHandle, service_id: &str, resolved: Resolved) {
    let snapshot = {
        let state = app.state::<Cache>();
        let mut cache = state.0.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(service_id.to_string(), resolved);
        cache.clone()
    };
    if let (Some(path), Ok(json)) = (cache_path(app), serde_json::to_vec_pretty(&snapshot)) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, json);
    }
}

fn cached(app: &AppHandle, service: &ServiceConfig) -> Option<String> {
    app.state::<Cache>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&service.id)
        .filter(|resolved| resolved.configured == service.url)
        .map(|resolved| resolved.effective.clone())
}

/// The URL the service actually loads and is trusted for. For an unresolved
/// QuickConnect service this stays the directory link, which no NAS page matches.
pub fn effective(app: &AppHandle, service: &ServiceConfig) -> String {
    if applies(service).is_some() {
        cached(app, service).unwrap_or_else(|| service.url.clone())
    } else {
        service.url.clone()
    }
}

/// True while a QuickConnect service has no verified address to load yet.
pub fn pending(app: &AppHandle, service: &ServiceConfig) -> bool {
    applies(service).is_some() && cached(app, service).is_none()
}

fn text<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut node = value;
    for key in path {
        node = node.get(key)?;
    }
    node.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("NULL"))
}

/// Ports arrive as numbers or numeric strings; 0 means "not forwarded".
fn port(value: &Value, path: &[&str]) -> Option<u16> {
    let mut node = value;
    for key in path {
        node = node.get(key)?;
    }
    let port = match node {
        Value::Number(n) => n.as_u64()?,
        Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    u16::try_from(port).ok().filter(|p| *p != 0)
}

fn private_ipv4(ip: &str) -> bool {
    ip.parse::<std::net::Ipv4Addr>()
        .is_ok_and(|ip| ip.is_private() || ip.is_loopback())
}

/// HTTPS candidates in the order Synology's clients prefer them: LAN first,
/// then names that work from anywhere, then raw WAN addresses, then the relay.
fn candidates(info: &Value, target: &Target) -> Vec<Url> {
    let mut hosts: Vec<(String, u16)> = Vec::new();
    let lan = port(info, &["service", "port"]);
    let wan = port(info, &["service", "ext_port"]);
    let both = |host: &str, hosts: &mut Vec<(String, u16)>| {
        for p in [lan, wan].into_iter().flatten() {
            hosts.push((host.to_string(), p));
        }
    };
    let smart_lan: Vec<&str> = info
        .pointer("/smartdns/lan")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if let Some(p) = lan {
        for host in smart_lan.iter().filter(|h| !h.starts_with("syn4-")) {
            hosts.push((host.to_string(), p));
        }
        for ip in info
            .pointer("/server/interface")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|i| text(i, &["ip"]))
            .filter(|ip| private_ipv4(ip))
        {
            hosts.push((ip.to_string(), p));
        }
    }
    for key in ["fqdn", "ddns"] {
        if let Some(host) = text(info, &["server", key]) {
            both(host, &mut hosts);
        }
    }
    if let Some(host) = text(info, &["smartdns", "host"]) {
        both(host, &mut hosts);
    }
    for host in smart_lan.iter().filter(|h| h.starts_with("syn4-")) {
        both(host, &mut hosts);
    }
    if let Some(ip) = text(info, &["server", "external", "ip"]) {
        both(ip, &mut hosts);
    }
    hosts.extend(relay_hosts(info, target));
    let mut seen = std::collections::HashSet::new();
    hosts
        .into_iter()
        .filter(|entry| seen.insert(entry.clone()))
        .filter_map(|(host, p)| {
            let base = Url::parse(&format!("https://{host}:{p}/")).ok()?;
            // A directory answer must not smuggle paths, credentials or other schemes.
            (base.host_str().is_some() && base.path() == "/" && base.username().is_empty())
                .then_some(base)
        })
        .collect()
}

fn relay_hosts(info: &Value, target: &Target) -> Vec<(String, u16)> {
    let mut hosts = Vec::new();
    if let (Some(host), Some(p)) = (
        text(info, &["service", "relay_dn"]),
        port(info, &["service", "relay_port"]),
    ) {
        hosts.push((host.to_string(), p));
    }
    // The browser relay portal: <id>.<region>.quickconnect.to on 443.
    if let Some(region) = text(info, &["env", "relay_region"]).filter(|r| valid_id(r)) {
        hosts.push((format!("{}.{region}.{}", target.id, target.domain), 443));
    }
    hosts
}

fn md5_hex(value: &str) -> String {
    Md5::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A candidate is accepted only when it proves it is the NAS the directory named.
fn verified(pingpong: &Value, server_id: &str) -> bool {
    !server_id.is_empty()
        && pingpong.get("success").and_then(Value::as_bool) == Some(true)
        && pingpong
            .get("ezid")
            .and_then(Value::as_str)
            .is_some_and(|ezid| ezid.eq_ignore_ascii_case(&md5_hex(server_id)))
}

#[derive(Debug, PartialEq)]
pub enum Failure {
    NotFound,
    Lookup(i64),
    Network,
    Unreachable,
}

impl Failure {
    pub fn message(&self, id: &str) -> String {
        match self {
            Self::NotFound => format!(
                "QuickConnect ID \"{id}\" was not found. Check it in DSM > Control Panel > External Access > QuickConnect, or enter a direct https:// address."
            ),
            Self::Lookup(errno) => format!("QuickConnect lookup for \"{id}\" failed (error {errno})."),
            Self::Network => "Could not reach Synology QuickConnect. Check your internet connection.".into(),
            Self::Unreachable => format!(
                "QuickConnect found \"{id}\", but none of its addresses answered securely from this Mac. Make sure the NAS is online and HTTPS is enabled in DSM."
            ),
        }
    }
}

fn lookup_result(response: &Value) -> Result<&Value, Failure> {
    let entry = match response {
        Value::Array(items) => items.first().ok_or(Failure::Lookup(-1))?,
        other => other,
    };
    match entry.get("errno").and_then(Value::as_i64).unwrap_or(-1) {
        0 => Ok(entry),
        4 => Err(Failure::NotFound),
        errno => Err(Failure::Lookup(errno)),
    }
}

fn client(timeout: Duration) -> Result<reqwest::Client, Failure> {
    reqwest::Client::builder()
        .https_only(true)
        .timeout(timeout)
        .user_agent("ChatPlus-Desktop")
        .build()
        .map_err(|_| Failure::Network)
}

async fn directory(host: &str, command: &str, target: &Target) -> Result<Value, Failure> {
    let body = serde_json::json!([{
        "version": 1,
        "command": command,
        "stop_when_error": false,
        "stop_when_success": false,
        "id": SERVICE_ID,
        "serverID": target.id,
        "is_gofile": false,
    }]);
    let response = client(LOOKUP_TIMEOUT)?
        .post(format!("https://{host}/Serv.php"))
        .json(&body)
        .send()
        .await
        .map_err(|_| Failure::Network)?;
    let value: Value = response.json().await.map_err(|_| Failure::Network)?;
    lookup_result(&value).cloned()
}

async fn probe(candidates: Vec<Url>, server_id: &str) -> Vec<Url> {
    let Ok(client) = client(PROBE_TIMEOUT) else {
        return Vec::new();
    };
    let mut tasks = tokio::task::JoinSet::new();
    for (rank, base) in candidates.into_iter().enumerate() {
        let client = client.clone();
        let expected = server_id.to_string();
        tasks.spawn(async move {
            let ping = base
                .join("webman/pingpong.cgi?action=cors&quickconnect=true")
                .ok()?;
            let value: Value = client.get(ping).send().await.ok()?.json().await.ok()?;
            verified(&value, &expected).then_some((rank, base))
        });
    }
    let mut found = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(Some(hit)) = result {
            found.push(hit);
        }
    }
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().map(|(_, base)| base).collect()
}

/// Prefer the address already in use when it still verifies: switching between
/// equivalent addresses would change the origin and sign the user out.
fn choose(verified: &[Url], previous: Option<&Url>) -> Option<Url> {
    previous
        .and_then(|prev| verified.iter().find(|base| base.origin() == prev.origin()))
        .or_else(|| verified.first())
        .cloned()
}

pub struct Resolution {
    pub effective: String,
    server_id: String,
}

pub async fn resolve(target: &Target, previous: Option<&str>) -> Result<Resolution, Failure> {
    let global = format!("global.{}", target.domain);
    let info = directory(&global, "get_server_info", target).await?;
    let server_id = text(&info, &["server", "serverID"])
        .unwrap_or_default()
        .to_string();
    let previous = previous.and_then(|p| Url::parse(p).ok());
    let mut server_id = server_id;
    let mut verified = probe(candidates(&info, target), &server_id).await;
    if verified.is_empty() {
        // No direct path: ask the regional control host for a relay tunnel.
        let control = text(&info, &["env", "control_host"])
            .filter(|h| h.ends_with(target.domain))
            .map(str::to_string)
            .unwrap_or(global);
        if let Ok(tunnel) = directory(&control, "request_tunnel", target).await {
            if let Some(id) = text(&tunnel, &["server", "serverID"]) {
                server_id = id.to_string();
            }
            let relays = relay_hosts(&tunnel, target)
                .into_iter()
                .filter_map(|(h, p)| Url::parse(&format!("https://{h}:{p}/")).ok())
                .collect();
            verified = probe(relays, &server_id).await;
        }
    }
    let base = choose(&verified, previous.as_ref()).ok_or(Failure::Unreachable)?;
    let mut effective = base;
    effective.set_path(&target.path);
    Ok(Resolution {
        effective: effective.into(),
        server_id,
    })
}

/// Resolves every new or changed QuickConnect service before Settings persists it,
/// so a wrong ID is reported in Settings instead of as a blank service view.
pub async fn prepare(app: &AppHandle, services: &[ServiceConfig]) -> Result<(), String> {
    for service in services.iter().filter(|s| s.enabled) {
        let Some(target) = applies(service) else {
            continue;
        };
        if cached(app, service).is_some() {
            continue;
        }
        let resolution = resolve(&target, None)
            .await
            .map_err(|failure| failure.message(&target.id))?;
        store(
            app,
            &service.id,
            Resolved {
                configured: service.url.clone(),
                effective: resolution.effective,
                server_id: resolution.server_id,
            },
        );
    }
    Ok(())
}

/// Re-resolves in the background (startup, Reload) and moves the view when the
/// best verified address changed, e.g. after a laptop left the home network.
pub fn refresh(app: &AppHandle, service: &ServiceConfig, reload: bool) {
    let Some(target) = applies(service) else {
        return;
    };
    let app = app.clone();
    let service = service.clone();
    tauri::async_runtime::spawn(async move {
        let previous = cached(&app, &service);
        let label = crate::services::label(&service.id);
        match resolve(&target, previous.as_deref()).await {
            Ok(Resolution {
                effective,
                server_id,
            }) => {
                let changed = previous.as_deref() != Some(effective.as_str());
                store(
                    &app,
                    &service.id,
                    Resolved {
                        configured: service.url.clone(),
                        effective: effective.clone(),
                        server_id,
                    },
                );
                if let Some(view) = app.get_webview(&label) {
                    if changed {
                        if let Ok(url) = effective.parse() {
                            crate::load_state::begin(&app, &service.id);
                            let _ = view.navigate(service.provider.dsm_launch(url));
                        }
                    } else if reload {
                        let _ = view.reload();
                    }
                }
            }
            Err(failure) => {
                if reload || previous.is_none() {
                    crate::shell::report(&app, failure.message(&target.id));
                }
                if previous.is_none() {
                    crate::load_state::fail(
                        &app,
                        &service.id,
                        crate::load_state::Reason::QuickConnect,
                    );
                    crate::connection::set_for(
                        &app,
                        &service.id,
                        crate::connection::Connection::Error,
                    );
                }
                // Keep a cached address on failure: the directory may be down while the NAS is fine.
                if reload && previous.is_some() {
                    if let Some(view) = app.get_webview(&label) {
                        let _ = view.reload();
                    }
                }
            }
        }
    });
}

/// Follows a laptop between networks without user action: every minute, one
/// pingpong confirms each service's current address is still the same NAS; when
/// it stops answering (home LAN gone, office network, VPN), resolve again.
pub fn watch(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(WATCH_INTERVAL).await;
            let services = crate::state::current(&app).services;
            for service in services
                .iter()
                .filter(|s| s.enabled && uses_quickconnect(s))
            {
                let entry = app
                    .state::<Cache>()
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&service.id)
                    .filter(|resolved| resolved.configured == service.url)
                    .cloned();
                let Some(entry) = entry else {
                    continue;
                };
                let healthy = match Url::parse(&entry.effective) {
                    Ok(mut base) if !entry.server_id.is_empty() => {
                        base.set_path("/");
                        !probe(vec![base], &entry.server_id).await.is_empty()
                    }
                    _ => false,
                };
                if !healthy {
                    refresh(&app, service, false);
                }
            }
        }
    });
}

/// Startup pass over enabled QuickConnect services that already have an address;
/// pending ones are resolved by the service view that needs them.
pub fn refresh_all(app: &AppHandle) {
    for service in crate::state::current(app)
        .services
        .iter()
        .filter(|s| s.enabled && !pending(app, s))
    {
        refresh(app, service, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, path: &str) -> Target {
        Target {
            id: id.into(),
            domain: "quickconnect.to",
            path: path.into(),
        }
    }

    #[test]
    fn canonical_links_parse_and_other_urls_do_not() {
        assert_eq!(
            target("https://quickconnect.to/MyNas/"),
            Some(t("mynas", "/"))
        );
        assert_eq!(
            target("https://quickconnect.to/mynas/chatplus/"),
            Some(t("mynas", "/chatplus/"))
        );
        assert_eq!(
            target("https://quickconnect.cn/mynas/chat/").map(|t| t.domain),
            Some("quickconnect.cn")
        );
        for other in [
            "https://nas.example.com/chat/",
            "http://quickconnect.to/mynas/",
            "https://quickconnect.to/",
            "https://quickconnect.to:8443/mynas/",
            "https://quickconnect.to/-bad/",
            "https://quickconnect.to/my_nas/",
            "https://mynas.direct.quickconnect.to/chat/",
        ] {
            assert_eq!(target(other), None, "{other}");
        }
    }

    fn sample() -> Value {
        serde_json::json!({
            "command": "get_server_info",
            "errno": 0,
            "env": {"control_host": "ukc1.quickconnect.to", "relay_region": "uk1"},
            "server": {
                "serverID": "123456789",
                "ddns": "mynas.synology.me",
                "fqdn": "NULL",
                "external": {"ip": "203.0.113.7"},
                "interface": [{"ip": "192.168.1.20"}, {"ip": "203.0.113.99"}]
            },
            "service": {"port": 5001, "ext_port": "50001", "relay_dn": "", "relay_port": 0},
            "smartdns": {
                "host": "mynas.direct.quickconnect.to",
                "lan": ["192-168-1-20.mynas.direct.quickconnect.to", "syn4-203-0-113-7.mynas.direct.quickconnect.to"]
            }
        })
    }

    #[test]
    fn candidates_prefer_lan_then_names_then_wan_then_relay() {
        let urls: Vec<String> = candidates(&sample(), &t("mynas", "/chat/"))
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(
            urls,
            [
                "https://192-168-1-20.mynas.direct.quickconnect.to:5001/",
                "https://192.168.1.20:5001/",
                "https://mynas.synology.me:5001/",
                "https://mynas.synology.me:50001/",
                "https://mynas.direct.quickconnect.to:5001/",
                "https://mynas.direct.quickconnect.to:50001/",
                "https://syn4-203-0-113-7.mynas.direct.quickconnect.to:5001/",
                "https://syn4-203-0-113-7.mynas.direct.quickconnect.to:50001/",
                "https://203.0.113.7:5001/",
                "https://203.0.113.7:50001/",
                "https://mynas.uk1.quickconnect.to/",
            ]
        );
    }

    #[test]
    fn hostile_directory_entries_cannot_inject_paths_or_credentials() {
        let info = serde_json::json!({
            "server": {"ddns": "evil.example/steal?x=", "fqdn": "user:pw@evil.example"},
            "service": {"port": 5001},
            "smartdns": {"lan": ["a.example/x"]}
        });
        for url in candidates(&info, &t("mynas", "/chat/")) {
            assert_eq!(url.path(), "/");
            assert!(url.username().is_empty() && url.password().is_none());
            assert!(url.query().is_none());
        }
    }

    #[test]
    fn only_the_named_nas_verifies() {
        let id = "123456789";
        let good = serde_json::json!({"success": true, "ezid": md5_hex(id)});
        assert!(verified(&good, id));
        assert!(!verified(
            &serde_json::json!({"success": true, "ezid": md5_hex("other")}),
            id
        ));
        assert!(!verified(&serde_json::json!({"success": true}), id));
        assert!(!verified(
            &serde_json::json!({"success": false, "ezid": md5_hex(id)}),
            id
        ));
        assert!(!verified(&good, ""));
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn directory_errors_map_to_actionable_failures() {
        let missing =
            serde_json::json!([{"command": "get_server_info", "errno": 4, "suberrno": 1}]);
        assert_eq!(lookup_result(&missing).unwrap_err(), Failure::NotFound);
        assert_eq!(
            lookup_result(&serde_json::json!({"errno": 9})).unwrap_err(),
            Failure::Lookup(9)
        );
        assert!(lookup_result(&serde_json::json!([{"errno": 0, "server": {}}])).is_ok());
        assert!(Failure::NotFound.message("mynas").contains("\"mynas\""));
    }

    #[test]
    fn the_address_in_use_wins_while_it_still_verifies() {
        let lan = Url::parse("https://192.168.1.20:5001/").unwrap();
        let ddns = Url::parse("https://mynas.synology.me:5001/").unwrap();
        let found = vec![lan.clone(), ddns.clone()];
        assert_eq!(choose(&found, Some(&ddns)), Some(ddns.clone()));
        assert_eq!(choose(&found, None), Some(lan.clone()));
        assert_eq!(choose(&[lan.clone()], Some(&ddns)), Some(lan));
        assert_eq!(choose(&[], Some(&ddns)), None);
    }

    #[tokio::test]
    #[ignore = "contacts Synology's public QuickConnect directory"]
    async fn live_directory_reports_an_unknown_id() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let missing = t("zz-not-a-real-id-9137", "/chat/");
        assert!(matches!(
            resolve(&missing, None).await,
            Err(Failure::NotFound)
        ));
    }

    #[test]
    fn only_synology_providers_use_quickconnect() {
        let mut service = ServiceConfig {
            id: "s".into(),
            provider: ProviderId::SynologyChatplus,
            name: "n".into(),
            url: "https://quickconnect.to/mynas/chat/".into(),
            enabled: true,
            notifications: true,
            notification_color: Default::default(),
        };
        assert!(uses_quickconnect(&service));
        // A QuickConnect-shaped URL on another provider stays an ordinary URL,
        // so Reload must take the normal path for it.
        for provider in [
            ProviderId::Discord,
            ProviderId::Mattermost,
            ProviderId::Slack,
        ] {
            service.provider = provider;
            assert!(!uses_quickconnect(&service));
        }
    }

    #[test]
    fn caches_written_before_server_ids_still_load() {
        let old = r#"{"s":{"configured":"https://quickconnect.to/mynas/","effective":"https://10-0-2-254.mynas.direct.quickconnect.to:5001/"}}"#;
        let cache: HashMap<String, Resolved> = serde_json::from_str(old).unwrap();
        // An empty ID makes the watcher re-resolve once, which backfills it.
        assert_eq!(cache["s"].server_id, "");
    }
}
