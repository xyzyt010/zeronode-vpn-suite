//! Tor exit GeoIP resolution without relying on a single third-party API.
//!
//! Lookups go through the local Tor SOCKS5 proxy so we see the *exit node's*
//! public IP. Resolution order:
//!
//!   1. Rich JSON endpoints (ip-api, ipwho.is, ipapi.co) — city/ISP/lat/lon
//!   2. Plain IP echo endpoints + local DB-IP `.mmdb` (country + ASN)
//!   3. Plain IP alone with country-centroid fallback if ISO code is known
//!
//! Never discard a successfully-resolved exit IP just because enrichment
//! failed — the right pane and globe still need *something* to show.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tracing::{debug, info, warn};
use vpn_suite_core::geoip::{ensure_local_databases, GeoIpStack};
use vpn_suite_core::model::TorExitInfo;

#[derive(Clone)]
struct Centroid {
    name: String,
    lat: f64,
    lng: f64,
}

fn centroid_table() -> &'static HashMap<String, Centroid> {
    static TABLE: OnceLock<HashMap<String, Centroid>> = OnceLock::new();
    TABLE.get_or_init(|| {
        const RAW: &str = include_str!("../assets/globe/country_centroids.json");
        #[derive(serde::Deserialize)]
        struct RawCentroid {
            name: String,
            lat: f64,
            lng: f64,
        }
        let raw: HashMap<String, RawCentroid> = serde_json::from_str(RAW).unwrap_or_default();
        raw.into_iter()
            .map(|(k, v)| {
                (
                    k.to_uppercase(),
                    Centroid {
                        name: v.name,
                        lat: v.lat,
                        lng: v.lng,
                    },
                )
            })
            .collect()
    })
}

fn centroid_coords(iso: &str) -> Option<(f64, f64)> {
    centroid_table()
        .get(&iso.to_uppercase())
        .map(|c| (c.lat, c.lng))
}

fn centroid_name(iso: &str) -> Option<String> {
    centroid_table()
        .get(&iso.to_uppercase())
        .map(|c| c.name.clone())
}

/// Public lookup of the embedded country-centroid table.
pub fn country_name(iso: &str) -> Option<String> {
    centroid_name(iso)
}

pub fn try_open_geoip_stack(geo_dir: &Path) -> Option<Arc<GeoIpStack>> {
    let country = geo_dir.join("dbip-country-lite.mmdb");
    let asn = geo_dir.join("dbip-asn-lite.mmdb");
    if !country.exists() || !asn.exists() {
        warn!(
            "local GeoIP databases missing under {} (country={}, asn={})",
            geo_dir.display(),
            country.exists(),
            asn.exists()
        );
        return None;
    }
    match GeoIpStack::open(country, asn) {
        Ok(stack) => {
            info!("local GeoIP stack opened from {}", geo_dir.display());
            Some(Arc::new(stack))
        }
        Err(error) => {
            warn!("local GeoIP stack unavailable: {error:#}");
            None
        }
    }
}

pub async fn open_geoip_stack(geo_dir: &Path) -> Option<Arc<GeoIpStack>> {
    if let Err(error) = ensure_local_databases(geo_dir).await {
        warn!("could not provision local GeoIP databases: {error:#}");
    }
    try_open_geoip_stack(geo_dir)
}

/// Hardened reqwest client that routes every request through Tor SOCKS5
/// (`socks5h://` so DNS also goes through Tor).
///
/// Fixed SOCKS username/password (`zeronode`/`ipcheck`) so IsolateSOCKSAuth
/// reuses the same Tor circuit family for every "Your IP" probe — otherwise
/// each fresh SOCKS connection can exit a different node (normal Tor, but
/// confusing when the app and a browser disagree).
pub fn tor_proxy_client(socks_port: u16) -> Option<reqwest::Client> {
    let proxy_url = format!("socks5h://zeronode:ipcheck@127.0.0.1:{socks_port}");
    match reqwest::Proxy::all(&proxy_url) {
        Ok(proxy) => match reqwest::Client::builder()
            .proxy(proxy)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(12))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) ZeroNodeVPN/0.1")
            .pool_max_idle_per_host(2)
            .build()
        {
            Ok(client) => {
                debug!("Tor SOCKS5 client ready at {proxy_url}");
                Some(client)
            }
            Err(error) => {
                warn!("failed to build Tor-aware reqwest client: {error}");
                None
            }
        },
        Err(error) => {
            warn!("invalid Tor proxy URL {proxy_url}: {error}");
            None
        }
    }
}

/// Direct (non-Tor) client for the real public-IP card.
fn direct_client() -> Option<reqwest::Client> {
    match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) ZeroNodeVPN/0.1")
        .no_proxy()
        .build()
    {
        Ok(c) => Some(c),
        Err(error) => {
            warn!("failed to build direct HTTP client: {error}");
            None
        }
    }
}

/// Probe Tor connectivity and resolve exit IP + full location details.
pub async fn resolve_tor_exit(
    client: &reqwest::Client,
    geo: Option<&GeoIpStack>,
) -> Option<TorExitInfo> {
    // 1) Rich multi-field APIs first (call separately — distinct async fn types).
    let mut rich = fetch_ip_api(client).await;
    if rich.is_none() {
        rich = fetch_ipwho(client).await;
    }
    if rich.is_none() {
        rich = fetch_ipapi_co(client).await;
    }
    if let Some(mut info) = rich {
        fill_missing_from_geo(&mut info, geo);
        fill_missing_from_centroid(&mut info);
        enrich_ipv6(client, &mut info).await;
        if !info.ip.is_empty() {
            info!(
                "Tor exit resolved via rich API: {} ({}) — {}",
                info.ip, info.country_code, info.country
            );
            return Some(info);
        }
    }
    warn!("all rich GeoIP APIs failed through Tor; falling back to plain IP echoes");

    // 2) Plain IP echoes + local mmdb enrichment.
    for url in IP_ECHO_URLS {
        if let Some(ip) = fetch_plain_ip(client, url).await {
            match build_exit_info(&ip, geo) {
                Some(mut info) => {
                    enrich_ipv6(client, &mut info).await;
                    info!(
                        "Tor exit resolved via {url}: {} ({})",
                        info.ip, info.country_code
                    );
                    return Some(info);
                }
                None => {
                    // Never throw away a working exit IP — return partial
                    // details so the UI still shows the address and can pan
                    // once country enrichment succeeds later.
                    warn!(
                        "got IP {ip} from {url} without full GeoIP enrichment; returning partial"
                    );
                    let mut info = partial_exit_info(&ip);
                    enrich_ipv6(client, &mut info).await;
                    return Some(info);
                }
            }
        } else {
            debug!("plain IP echo {url} failed");
        }
    }

    warn!("all Tor GeoIP lookups exhausted — returning None");
    None
}

const IP_ECHO_URLS: &[&str] = &[
    "https://check.torproject.org/api/ip",
    "https://api.ipify.org?format=json",
    "https://icanhazip.com",
    "https://ifconfig.me/ip",
    "https://api64.ipify.org?format=json",
    "https://ident.me",
];

/// Resolve the user's real (non-Tor) public IP + GeoIP details.
///
/// Uses a cache-buster query so CDN/ISP caches don't return a pre-tunnel IP
/// right after WireGuard/Outline connect.
pub async fn resolve_local_ip() -> Option<TorExitInfo> {
    let client = direct_client()?;
    let bust = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut rich = fetch_ip_api_bust(&client, bust).await;
    if rich.is_none() {
        rich = fetch_ipwho_bust(&client, bust).await;
    }
    if rich.is_none() {
        rich = fetch_ipapi_co(&client).await;
    }
    if let Some(mut info) = rich {
        fill_missing_from_centroid(&mut info);
        enrich_ipv6(&client, &mut info).await;
        if !info.ip.is_empty() {
            return Some(info);
        }
    }
    // Plain IP fallback without Tor (append bust where query strings work).
    for url in IP_ECHO_URLS {
        let url = if url.contains('?') {
            format!("{url}&_={bust}")
        } else {
            format!("{url}?_={bust}")
        };
        if let Some(ip) = fetch_plain_ip(&client, &url).await {
            let mut info = partial_exit_info(&ip);
            enrich_ipv6(&client, &mut info).await;
            return Some(info);
        }
    }
    None
}

/// Best-effort public IPv6 (dual-stack). Empty when the path is IPv4-only
/// (common under Tor/Wintun IPv4 tunnels).
async fn enrich_ipv6(client: &reqwest::Client, info: &mut TorExitInfo) {
    if !info.ipv6.is_empty() {
        return;
    }
    // If the primary address is already IPv6, mirror it.
    if let Ok(IpAddr::V6(_)) = info.ip.parse::<IpAddr>() {
        info.ipv6 = info.ip.clone();
        return;
    }
    for url in [
        "https://api6.ipify.org",
        "https://v6.ident.me",
        "https://ipv6.icanhazip.com",
    ] {
        if let Some(ip) = fetch_plain_ip(client, url).await {
            if let Ok(IpAddr::V6(_)) = ip.parse::<IpAddr>() {
                info.ipv6 = ip;
                return;
            }
        }
    }
}

fn partial_exit_info(ip: &str) -> TorExitInfo {
    TorExitInfo {
        ip: ip.to_string(),
        ipv6: String::new(),
        country_code: String::new(),
        country: String::from("Resolving…"),
        region_code: String::new(),
        region: String::new(),
        city: String::new(),
        zip: String::new(),
        lat: 0.0,
        lon: 0.0,
        timezone: String::new(),
        isp: String::new(),
        org: String::new(),
        as_name: String::new(),
    }
}

fn fill_missing_from_geo(info: &mut TorExitInfo, geo: Option<&GeoIpStack>) {
    let Some(stack) = geo else { return };
    let Ok(parsed) = info.ip.parse::<IpAddr>() else {
        return;
    };
    if info.country_code.is_empty() {
        if let Some(c) = stack.lookup_country(parsed) {
            info.country_code = c.iso_code.clone();
            if info.country.is_empty() || info.country == "Resolving…" {
                info.country = c.name.clone();
            }
        }
    }
    if info.as_name.is_empty() {
        if let Some(asn) = stack.lookup_asn(parsed) {
            info.as_name = format!("AS{} {}", asn.number, asn.org);
            if info.org.is_empty() {
                info.org = asn.org.clone();
            }
            if info.isp.is_empty() {
                info.isp = asn.org;
            }
        }
    }
}

fn fill_missing_from_centroid(info: &mut TorExitInfo) {
    if info.country_code.is_empty() {
        return;
    }
    let cc = info.country_code.to_uppercase();
    info.country_code = cc.clone();
    if info.country.is_empty() || info.country == "Resolving…" {
        if let Some(name) = centroid_name(&cc) {
            info.country = name;
        }
    }
    if info.lat == 0.0 && info.lon == 0.0 {
        if let Some((lat, lon)) = centroid_coords(&cc) {
            info.lat = lat;
            info.lon = lon;
        }
    }
}

async fn fetch_ip_api(client: &reqwest::Client) -> Option<TorExitInfo> {
    fetch_ip_api_bust(client, 0).await
}

async fn fetch_ip_api_bust(client: &reqwest::Client, bust: u128) -> Option<TorExitInfo> {
    let url = format!(
        "http://ip-api.com/json/?fields=status,message,query,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as&_={bust}"
    );
    let json = fetch_json(client, &url).await?;
    if json["status"].as_str() != Some("success") {
        warn!(
            "ip-api reported status={:?} message={:?}",
            json["status"].as_str(),
            json["message"].as_str()
        );
        return None;
    }
    let ip = json["query"].as_str()?.to_owned();
    let cc = json["countryCode"].as_str().unwrap_or("").to_owned();
    Some(TorExitInfo {
        ip,
        ipv6: String::new(),
        country_code: cc,
        country: json["country"].as_str().unwrap_or("").to_string(),
        region_code: json["region"].as_str().unwrap_or("").to_string(),
        region: json["regionName"].as_str().unwrap_or("").to_string(),
        city: json["city"].as_str().unwrap_or("").to_string(),
        zip: json["zip"].as_str().unwrap_or("").to_string(),
        lat: json["lat"].as_f64().unwrap_or(0.0),
        lon: json["lon"].as_f64().unwrap_or(0.0),
        timezone: json["timezone"].as_str().unwrap_or("").to_string(),
        isp: json["isp"].as_str().unwrap_or("").to_string(),
        org: json["org"].as_str().unwrap_or("").to_string(),
        as_name: json["as"].as_str().unwrap_or("").to_string(),
    })
}

async fn fetch_ipwho(client: &reqwest::Client) -> Option<TorExitInfo> {
    fetch_ipwho_bust(client, 0).await
}

async fn fetch_ipwho_bust(client: &reqwest::Client, bust: u128) -> Option<TorExitInfo> {
    let url = format!("https://ipwho.is/?_={bust}");
    let json = fetch_json(client, &url).await?;
    if json["success"].as_bool() == Some(false) {
        warn!("ipwho.is reported failure: {:?}", json["message"]);
        return None;
    }
    let ip = json["ip"].as_str()?.to_owned();
    let cc = json["country_code"].as_str().unwrap_or("").to_owned();
    let connection = json.get("connection");
    let isp = connection
        .and_then(|c| c.get("isp"))
        .and_then(|v| v.as_str())
        .or_else(|| json["isp"].as_str())
        .unwrap_or("")
        .to_string();
    let org = connection
        .and_then(|c| c.get("org"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let asn = connection
        .and_then(|c| c.get("asn"))
        .map(|v| match v {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s.clone(),
            _ => String::new(),
        })
        .unwrap_or_default();
    let as_name = if asn.is_empty() {
        String::new()
    } else if asn.starts_with("AS") {
        format!("{asn} {org}").trim().to_string()
    } else {
        format!("AS{asn} {org}").trim().to_string()
    };
    Some(TorExitInfo {
        ip,
        ipv6: String::new(),
        country_code: cc,
        country: json["country"].as_str().unwrap_or("").to_string(),
        region_code: json["region_code"].as_str().unwrap_or("").to_string(),
        region: json["region"].as_str().unwrap_or("").to_string(),
        city: json["city"].as_str().unwrap_or("").to_string(),
        zip: json["postal"].as_str().unwrap_or("").to_string(),
        lat: json["latitude"].as_f64().unwrap_or(0.0),
        lon: json["longitude"].as_f64().unwrap_or(0.0),
        timezone: json
            .pointer("/timezone/id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        isp,
        org,
        as_name,
    })
}

async fn fetch_ipapi_co(client: &reqwest::Client) -> Option<TorExitInfo> {
    let json = fetch_json(client, "https://ipapi.co/json/").await?;
    if json.get("error").and_then(|v| v.as_bool()) == Some(true) {
        warn!("ipapi.co reported error: {:?}", json["reason"]);
        return None;
    }
    let ip = json["ip"].as_str()?.to_owned();
    let cc = json["country_code"].as_str().unwrap_or("").to_owned();
    let asn = json["asn"].as_str().unwrap_or("").to_string();
    let org = json["org"].as_str().unwrap_or("").to_string();
    let as_name = if asn.is_empty() {
        String::new()
    } else {
        format!("{asn} {org}").trim().to_string()
    };
    Some(TorExitInfo {
        ip,
        ipv6: String::new(),
        country_code: cc,
        country: json["country_name"].as_str().unwrap_or("").to_string(),
        region_code: json["region_code"].as_str().unwrap_or("").to_string(),
        region: json["region"].as_str().unwrap_or("").to_string(),
        city: json["city"].as_str().unwrap_or("").to_string(),
        zip: json["postal"].as_str().unwrap_or("").to_string(),
        lat: json["latitude"].as_f64().unwrap_or(0.0),
        lon: json["longitude"].as_f64().unwrap_or(0.0),
        timezone: json["timezone"].as_str().unwrap_or("").to_string(),
        isp: org.clone(),
        org,
        as_name,
    })
}

async fn fetch_json(client: &reqwest::Client, url: &str) -> Option<serde_json::Value> {
    let response = match client.get(url).send().await {
        Ok(response) => response,
        Err(error) => {
            warn!("GeoIP request to {url} failed: {error}");
            return None;
        }
    };
    if !response.status().is_success() {
        warn!(
            "GeoIP endpoint {url} returned non-success status {}",
            response.status()
        );
        return None;
    }
    match response.json::<serde_json::Value>().await {
        Ok(json) => Some(json),
        Err(error) => {
            warn!("GeoIP endpoint {url} returned invalid JSON: {error}");
            None
        }
    }
}

async fn fetch_plain_ip(client: &reqwest::Client, url: &str) -> Option<String> {
    let body = match client.get(url).send().await {
        Ok(response) => response.text().await,
        Err(error) => {
            debug!("GET {url} failed: {error}");
            return None;
        }
    };
    let body = match body {
        Ok(b) => b,
        Err(error) => {
            debug!("read body of {url} failed: {error}");
            return None;
        }
    };
    if url.contains("check.torproject.org") || url.contains("ipify") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(ip) = json
                .get("IP")
                .or_else(|| json.get("ip"))
                .and_then(|v| v.as_str())
            {
                return Some(ip.trim().to_string());
            }
        }
    }
    let ip = body.trim();
    if ip.parse::<IpAddr>().is_ok() {
        Some(ip.to_string())
    } else {
        debug!("body of {url} was not a parseable IP: {ip:?}");
        None
    }
}

fn build_exit_info(ip: &str, geo: Option<&GeoIpStack>) -> Option<TorExitInfo> {
    let parsed: IpAddr = ip.parse().ok()?;
    let mut country_code = String::new();
    let mut country = String::new();
    let mut lat = 0.0;
    let mut lon = 0.0;
    let mut as_name = String::new();
    let mut org = String::new();

    if let Some(stack) = geo {
        if let Some(c) = stack.lookup_country(parsed) {
            country_code = c.iso_code.clone();
            country = c.name.clone();
            if let Some((clat, clng)) = centroid_coords(&country_code) {
                lat = clat;
                lon = clng;
            }
        } else {
            warn!(
                "local DB-IP country database has no entry for Tor exit IP {ip}; \
                 right pane will show partial details"
            );
        }
        if let Some(asn) = stack.lookup_asn(parsed) {
            as_name = format!("AS{} {}", asn.number, asn.org);
            org = asn.org.clone();
        }
    }

    if country_code.is_empty() {
        return None;
    }

    Some(TorExitInfo {
        ip: ip.to_string(),
        ipv6: String::new(),
        country_code: country_code.clone(),
        country: if country.is_empty() {
            centroid_name(&country_code).unwrap_or(country_code)
        } else {
            country
        },
        region_code: String::new(),
        region: String::new(),
        city: String::new(),
        zip: String::new(),
        lat,
        lon,
        timezone: String::new(),
        isp: org.clone(),
        org,
        as_name,
    })
}
