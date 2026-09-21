use arc_swap::ArcSwap;
use ipnet::Ipv4Net;
use iprange::IpRange;
use maxminddb::{geoip2, Mmap, Reader};
use serde::{Deserialize, Serialize};
use std::{net::IpAddr, path::{Path, PathBuf}, sync::Arc, time::Duration};
use tokio::time::interval;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountryInfo {
    pub iso_code: String,
    pub name: String,
    pub continent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsnInfo {
    pub number: u32,
    pub org: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CityInfo {
    pub city: String,
    pub region: String,
    pub region_code: String,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ConnectionType {
    Residential,
    Datacenter,
    KnownVpnExit,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoSnapshot {
    pub ip: String,
    pub country: Option<CountryInfo>,
    pub asn: Option<AsnInfo>,
    pub connection_type: ConnectionType,
}

pub struct GeoIpStack {
    // Memory-mapped readers: the 17MB of .mmdb data lives in page cache
    // shared with the OS instead of private heap (open_readfile duplicated
    // both files into RSS). Lookups page in only the nodes they touch.
    country: ArcSwap<Reader<Mmap>>,
    asn: ArcSwap<Reader<Mmap>>,
    country_path: PathBuf,
    asn_path: PathBuf,
}

impl GeoIpStack {
    pub fn open(country_path: PathBuf, asn_path: PathBuf) -> anyhow::Result<Self> {
        let country = Reader::open_mmap(&country_path)?;
        let asn = Reader::open_mmap(&asn_path)?;
        Ok(Self {
            country: ArcSwap::from_pointee(country),
            asn: ArcSwap::from_pointee(asn),
            country_path,
            asn_path,
        })
    }

    pub fn lookup_country(&self, ip: IpAddr) -> Option<CountryInfo> {
        let reader = self.country.load();
        let rec: geoip2::Country = reader.lookup(ip).ok()?;
        // Prefer `country`, fall back to `registered_country` (some DB-IP
        // entries only populate one of the two).
        let country = rec
            .country
            .as_ref()
            .or(rec.registered_country.as_ref())?;
        let iso_code = country.iso_code.as_ref()?;
        let name = country
            .names
            .as_ref()
            .and_then(|names| names.get(&"en").copied())
            .unwrap_or(*iso_code);
        let continent = rec
            .continent
            .as_ref()
            .and_then(|c| c.code.as_ref())
            .map(|s| s.to_string())
            .unwrap_or_default();
        Some(CountryInfo {
            iso_code: iso_code.to_string(),
            name: name.to_string(),
            continent,
        })
    }

    pub fn lookup_asn(&self, ip: IpAddr) -> Option<AsnInfo> {
        let reader = self.asn.load();
        let rec: geoip2::Asn = reader.lookup(ip).ok()?;
        Some(AsnInfo {
            number: rec.autonomous_system_number?,
            org: rec.autonomous_system_organization.as_ref()?.to_string(),
        })
    }

    /// City-level detail (city/region/coords). Returns `None` on the
    /// country-only database — the same call works on a City edition file,
    /// where the record additionally carries `city` + `subdivisions`.
    pub fn lookup_city(&self, ip: IpAddr) -> Option<CityInfo> {
        let reader = self.country.load();
        let rec: geoip2::City = reader.lookup(ip).ok()?;
        let city = rec
            .city
            .as_ref()
            .and_then(|c| c.names.as_ref())
            .and_then(|n| n.get(&"en").copied())
            .unwrap_or("")
            .to_string();
        let (region, region_code) = rec
            .subdivisions
            .as_ref()
            .and_then(|s| s.first())
            .map(|sub| {
                (
                    sub.names
                        .as_ref()
                        .and_then(|n| n.get(&"en").copied())
                        .unwrap_or("")
                        .to_string(),
                    sub.iso_code.unwrap_or_default().to_string(),
                )
            })
            .unwrap_or_default();
        let (lat, lon) = rec
            .location
            .as_ref()
            .map(|l| (l.latitude.unwrap_or(0.0), l.longitude.unwrap_or(0.0)))
            .unwrap_or((0.0, 0.0));
        if city.is_empty() && lat == 0.0 && lon == 0.0 {
            return None;
        }
        Some(CityInfo {
            city,
            region,
            region_code,
            lat,
            lon,
        })
    }

    pub fn reload(&self) -> anyhow::Result<()> {
        let new_country = Reader::open_mmap(&self.country_path)?;
        let new_asn = Reader::open_mmap(&self.asn_path)?;
        self.country.store(Arc::new(new_country));
        self.asn.store(Arc::new(new_asn));
        Ok(())
    }
}

pub struct ConnTypeMatcher {
    vpn_ranges: IpRange<Ipv4Net>,
    datacenter_ranges: IpRange<Ipv4Net>,
}

impl ConnTypeMatcher {
    pub fn load(vpn_list_path: &Path, dc_list_path: &Path) -> anyhow::Result<Self> {
        let vpn_ranges = if vpn_list_path.exists() {
            parse_cidr_file(vpn_list_path)?
        } else {
            IpRange::new()
        };
        let datacenter_ranges = if dc_list_path.exists() {
            parse_cidr_file(dc_list_path)?
        } else {
            IpRange::new()
        };
        Ok(Self { vpn_ranges, datacenter_ranges })
    }

    pub fn classify(&self, ip: IpAddr) -> ConnectionType {
        let IpAddr::V4(v4) = ip else { return ConnectionType::Unknown };
        if self.vpn_ranges.contains(&v4) {
            ConnectionType::KnownVpnExit
        } else if self.datacenter_ranges.contains(&v4) {
            ConnectionType::Datacenter
        } else {
            ConnectionType::Residential
        }
    }
}

fn parse_cidr_file(path: &Path) -> anyhow::Result<IpRange<Ipv4Net>> {
    let mut range = IpRange::new();
    let contents = std::fs::read_to_string(path)?;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if let Ok(net) = line.parse::<Ipv4Net>() {
            range.add(net);
        }
    }
    range.simplify();
    Ok(range)
}

pub fn spawn_refresh_task(stack: Arc<GeoIpStack>, data_dir: PathBuf) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(7 * 24 * 3600));
        loop {
            ticker.tick().await;
            if let Err(e) = refresh_once(&stack, &data_dir).await {
                eprintln!("geoip refresh failed, keeping current databases: {e}");
            }
        }
    });
}

/// Download country + ASN databases if missing (no-op when already present).
pub async fn ensure_local_databases(data_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let country = data_dir.join("dbip-country-lite.mmdb");
    let asn = data_dir.join("dbip-asn-lite.mmdb");
    if country.exists() && asn.exists() {
        return Ok(());
    }
    let year_month = chrono::Utc::now().format("%Y-%m").to_string();
    if !country.exists() {
        download_and_validate(
            &format!("https://download.db-ip.com/free/dbip-country-lite-{year_month}.mmdb.gz"),
            &data_dir.join("dbip-country-lite.mmdb.tmp"),
            &country,
        )
        .await?;
    }
    if !asn.exists() {
        download_and_validate(
            &format!("https://download.db-ip.com/free/dbip-asn-lite-{year_month}.mmdb.gz"),
            &data_dir.join("dbip-asn-lite.mmdb.tmp"),
            &asn,
        )
        .await?;
    }
    Ok(())
}

/// --- Offline IP-database addon (DB-IP City Lite) ---------------------------
/// The addon is the same DB-IP source family as the auto-provisioned
/// country/ASN files, but the City edition (~60–90MB) carries city, region
/// and coordinates for fully-offline enrichment. Installed on demand from
/// inside the app; the light country+ASN pair keeps working untouched.

pub fn ipdb_addon_city_path(data_dir: &Path) -> PathBuf {
    data_dir.join("dbip-city-lite.mmdb")
}

/// Installed size in bytes, if the addon database is present.
pub fn ipdb_addon_size(data_dir: &Path) -> Option<u64> {
    std::fs::metadata(ipdb_addon_city_path(data_dir))
        .ok()
        .map(|m| m.len())
}

/// Download + validate the City edition into the data dir. Returns bytes.
pub async fn download_ipdb_addon(data_dir: &Path) -> anyhow::Result<u64> {
    download_ipdb_addon_with_progress(data_dir, |_, _| {}).await
}

/// Streaming variant with accurate progress: download 0–85% (compressed
/// bytes vs Content-Length), decompress 85–100%.
pub async fn download_ipdb_addon_with_progress(
    data_dir: &Path,
    on_progress: impl Fn(u64, Option<u64>) + Send + 'static,
) -> anyhow::Result<u64> {
    std::fs::create_dir_all(data_dir)?;
    let year_month = chrono::Utc::now().format("%Y-%m").to_string();
    let tmp = data_dir.join("dbip-city-lite.mmdb.tmp");
    let final_path = ipdb_addon_city_path(data_dir);
    download_and_validate_with_progress(
        &format!("https://download.db-ip.com/free/dbip-city-lite-{year_month}.mmdb.gz"),
        &tmp,
        &final_path,
        &on_progress,
    )
    .await?;
    Ok(ipdb_addon_size(data_dir).unwrap_or(0))
}

pub fn remove_ipdb_addon(data_dir: &Path) -> anyhow::Result<()> {
    let p = ipdb_addon_city_path(data_dir);
    if p.exists() {
        std::fs::remove_file(&p)?;
    }
    let tmp = data_dir.join("dbip-city-lite.mmdb.tmp");
    if tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(())
}

pub async fn refresh_once(stack: &GeoIpStack, data_dir: &Path) -> anyhow::Result<()> {    let year_month = chrono::Utc::now().format("%Y-%m").to_string();
    download_and_validate(
        &format!("https://download.db-ip.com/free/dbip-country-lite-{year_month}.mmdb.gz"),
        &data_dir.join("dbip-country-lite.mmdb.tmp"),
        &data_dir.join("dbip-country-lite.mmdb"),
    ).await?;
    download_and_validate(
        &format!("https://download.db-ip.com/free/dbip-asn-lite-{year_month}.mmdb.gz"),
        &data_dir.join("dbip-asn-lite.mmdb.tmp"),
        &data_dir.join("dbip-asn-lite.mmdb"),
    ).await?;
    stack.reload()
}

async fn download_and_validate(url: &str, tmp_path: &Path, final_path: &Path) -> anyhow::Result<()> {
    download_and_validate_with_progress(url, tmp_path, final_path, &|_, _| {}).await
}

async fn download_and_validate_with_progress(
    url: &str,
    tmp_path: &Path,
    final_path: &Path,
    on_progress: &(impl Fn(u64, Option<u64>) + Send),
) -> anyhow::Result<()> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let resp = reqwest::get(url).await?.error_for_status()?;
    let total = resp.content_length();
    // Scale compressed download to 0–85% of the bar; decompress fills rest.
    let mut stream = resp.bytes_stream();
    let gz_tmp = tmp_path.with_extension("gz.tmp");
    let mut out = tokio::fs::File::create(&gz_tmp).await?;
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        out.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        // Map compressed progress into 0..85% of decompressed-equivalent.
        // Total decompressed unknown until inflate; report compressed scale
        // and let the UI show MB + % of download phase accurately.
        on_progress(downloaded, total);
    }
    out.flush().await?;
    drop(out);
    // Decompress with a second progress sweep (bytes read).
    let comp_bytes = tokio::fs::read(&gz_tmp).await?;
    let comp_len = comp_bytes.len() as u64;
    let mut decoder = flate2::read::GzDecoder::new(&comp_bytes[..]);
    let mut decompressed = Vec::new();
    // Chunked read so large DBs don't appear stuck at 85%.
    let mut buf = [0u8; 262_144];
    let mut done: u64 = 0;
    loop {
        use std::io::Read;
        let n = decoder.read(&mut buf)?;
        if n == 0 {
            break;
        }
        decompressed.extend_from_slice(&buf[..n]);
        done += n as u64;
        // Report 85% + up to 15% for inflate (total = comp + decomp estimate).
        let base = total.unwrap_or(comp_len);
        on_progress(base.saturating_add(done / 4), Some(base.saturating_add(base / 3)));
    }
    let _ = tokio::fs::remove_file(&gz_tmp).await;
    tokio::fs::write(tmp_path, &decompressed).await?;

    // Validate before promoting
    Reader::open_mmap(tmp_path)?;
    tokio::fs::rename(tmp_path, final_path).await?;
    Ok(())
}
