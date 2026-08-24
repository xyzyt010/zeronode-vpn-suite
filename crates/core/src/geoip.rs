use arc_swap::ArcSwap;
use ipnet::Ipv4Net;
use iprange::IpRange;
use maxminddb::{geoip2, Reader};
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
    country: ArcSwap<Reader<Vec<u8>>>,
    asn: ArcSwap<Reader<Vec<u8>>>,
    country_path: PathBuf,
    asn_path: PathBuf,
}

impl GeoIpStack {
    pub fn open(country_path: PathBuf, asn_path: PathBuf) -> anyhow::Result<Self> {
        let country = Reader::open_readfile(&country_path)?;
        let asn = Reader::open_readfile(&asn_path)?;
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

    pub fn reload(&self) -> anyhow::Result<()> {
        let new_country = Reader::open_readfile(&self.country_path)?;
        let new_asn = Reader::open_readfile(&self.asn_path)?;
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

pub async fn refresh_once(stack: &GeoIpStack, data_dir: &Path) -> anyhow::Result<()> {
    let year_month = chrono::Utc::now().format("%Y-%m").to_string();
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
    let bytes = reqwest::get(url).await?.bytes().await?;
    let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut decompressed = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut decompressed)?;
    tokio::fs::write(tmp_path, &decompressed).await?;

    // Validate before promoting
    Reader::open_readfile(tmp_path)?;
    tokio::fs::rename(tmp_path, final_path).await?;
    Ok(())
}
