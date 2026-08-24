use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use vpn_suite_core::model::TorExitInfo;

#[derive(Clone, Debug, Default)]
pub struct OvpnConfig {
    pub id: i64,
    pub name: String,
    pub content: String,
    pub country_code: Option<String>,
    pub fail_count: i32,
    /// Primary remote host from the profile (`remote` directive).
    pub remote_host: Option<String>,
    pub remote_port: Option<u16>,
    pub proto: Option<String>,
    /// Resolved A/AAAA for remote_host (may equal remote_host if already an IP).
    pub resolved_ip: Option<String>,
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    pub isp: Option<String>,
    pub org: Option<String>,
    pub as_name: Option<String>,
    pub timezone: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub cipher: Option<String>,
    pub auth: Option<String>,
}

impl OvpnConfig {
    /// Structured location view for the UI (mirrors Tor exit card fields).
    pub fn location_info(&self) -> TorExitInfo {
        TorExitInfo {
            ip: self
                .resolved_ip
                .clone()
                .or_else(|| self.remote_host.clone())
                .unwrap_or_default(),
            ipv6: String::new(),
            country_code: self.country_code.clone().unwrap_or_default(),
            country: self.country.clone().unwrap_or_default(),
            region_code: String::new(),
            region: self.region.clone().unwrap_or_default(),
            city: self.city.clone().unwrap_or_default(),
            zip: String::new(),
            lat: self.lat.unwrap_or(0.0),
            lon: self.lon.unwrap_or(0.0),
            timezone: self.timezone.clone().unwrap_or_default(),
            isp: self.isp.clone().unwrap_or_default(),
            org: self.org.clone().unwrap_or_default(),
            as_name: self.as_name.clone().unwrap_or_default(),
        }
    }

    pub fn endpoint_label(&self) -> String {
        match (&self.remote_host, self.remote_port) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.clone(),
            _ => String::from("OpenVPN profile"),
        }
    }
}

/// App paths isolated through Tor SOCKS5 (launch-through-proxy list).
#[derive(Clone, Debug)]
pub struct TorIsolatedApp {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub enabled: bool,
}

pub fn get_db_path() -> Result<PathBuf> {
    let mut path = vpn_suite_core::app_paths::client_paths()?.base_dir;
    path.push("client_db.sqlite");
    Ok(path)
}

pub fn init_db() -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS ovpn_configs (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            content TEXT NOT NULL,
            country_code TEXT,
            fail_count INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )?;
    // Additive migrations for structured OpenVPN server details.
    for (col, decl) in [
        ("remote_host", "TEXT"),
        ("remote_port", "INTEGER"),
        ("proto", "TEXT"),
        ("resolved_ip", "TEXT"),
        ("city", "TEXT"),
        ("region", "TEXT"),
        ("country", "TEXT"),
        ("isp", "TEXT"),
        ("org", "TEXT"),
        ("as_name", "TEXT"),
        ("timezone", "TEXT"),
        ("lat", "REAL"),
        ("lon", "REAL"),
        ("cipher", "TEXT"),
        ("auth", "TEXT"),
    ] {
        let _ = conn.execute(
            &format!("ALTER TABLE ovpn_configs ADD COLUMN {col} {decl}"),
            [],
        );
    }

    conn.execute(
        "CREATE TABLE IF NOT EXISTS tor_isolated_apps (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            path TEXT NOT NULL UNIQUE,
            enabled INTEGER NOT NULL DEFAULT 1
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS client_prefs (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )?;
    ensure_protocol_tables(&conn)?;
    Ok(())
}

fn map_ovpn_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OvpnConfig> {
    Ok(OvpnConfig {
        id: row.get(0)?,
        name: row.get(1)?,
        content: row.get(2)?,
        country_code: row.get(3)?,
        fail_count: row.get(4)?,
        remote_host: row.get(5)?,
        remote_port: row.get::<_, Option<i64>>(6)?.map(|v| v as u16),
        proto: row.get(7)?,
        resolved_ip: row.get(8)?,
        city: row.get(9)?,
        region: row.get(10)?,
        country: row.get(11)?,
        isp: row.get(12)?,
        org: row.get(13)?,
        as_name: row.get(14)?,
        timezone: row.get(15)?,
        lat: row.get(16)?,
        lon: row.get(17)?,
        cipher: row.get(18)?,
        auth: row.get(19)?,
    })
}

const OVPN_SELECT: &str = "SELECT id, name, content, country_code, fail_count,
    remote_host, remote_port, proto, resolved_ip, city, region, country,
    isp, org, as_name, timezone, lat, lon, cipher, auth
    FROM ovpn_configs";

pub fn add_ovpn_config(name: &str, content: &str, country_code: Option<&str>) -> Result<i64> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "INSERT INTO ovpn_configs (name, content, country_code, fail_count) VALUES (?1, ?2, ?3, 0)",
        params![name, content, country_code],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn add_ovpn_config_full(cfg: &OvpnConfig) -> Result<i64> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "INSERT INTO ovpn_configs (
            name, content, country_code, fail_count,
            remote_host, remote_port, proto, resolved_ip,
            city, region, country, isp, org, as_name, timezone, lat, lon,
            cipher, auth
        ) VALUES (
            ?1, ?2, ?3, 0,
            ?4, ?5, ?6, ?7,
            ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
            ?17, ?18
        )",
        params![
            cfg.name,
            cfg.content,
            cfg.country_code,
            cfg.remote_host,
            cfg.remote_port.map(|p| p as i64),
            cfg.proto,
            cfg.resolved_ip,
            cfg.city,
            cfg.region,
            cfg.country,
            cfg.isp,
            cfg.org,
            cfg.as_name,
            cfg.timezone,
            cfg.lat,
            cfg.lon,
            cfg.cipher,
            cfg.auth,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_ovpn_configs() -> Result<Vec<OvpnConfig>> {
    let conn = Connection::open(get_db_path()?)?;
    let mut stmt = conn.prepare(OVPN_SELECT)?;
    let config_iter = stmt.query_map([], map_ovpn_row)?;
    let mut configs = Vec::new();
    for config in config_iter {
        configs.push(config?);
    }
    Ok(configs)
}

pub fn get_ovpn_config(id: i64) -> Result<Option<OvpnConfig>> {
    let conn = Connection::open(get_db_path()?)?;
    let mut stmt = conn.prepare(&format!("{OVPN_SELECT} WHERE id = ?1"))?;
    let row = stmt
        .query_row(params![id], map_ovpn_row)
        .optional()?;
    Ok(row)
}

pub fn update_ovpn_country(id: i64, country_code: &str) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "UPDATE ovpn_configs SET country_code = ?1 WHERE id = ?2",
        params![country_code, id],
    )?;
    Ok(())
}

pub fn update_ovpn_location(id: i64, info: &TorExitInfo, remote_host: Option<&str>, remote_port: Option<u16>, proto: Option<&str>, cipher: Option<&str>, auth: Option<&str>) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "UPDATE ovpn_configs SET
            country_code = ?1,
            resolved_ip = ?2,
            city = ?3,
            region = ?4,
            country = ?5,
            isp = ?6,
            org = ?7,
            as_name = ?8,
            timezone = ?9,
            lat = ?10,
            lon = ?11,
            remote_host = COALESCE(?12, remote_host),
            remote_port = COALESCE(?13, remote_port),
            proto = COALESCE(?14, proto),
            cipher = COALESCE(?15, cipher),
            auth = COALESCE(?16, auth)
         WHERE id = ?17",
        params![
            if info.country_code.is_empty() {
                None
            } else {
                Some(info.country_code.as_str())
            },
            if info.ip.is_empty() { None } else { Some(info.ip.as_str()) },
            if info.city.is_empty() { None } else { Some(info.city.as_str()) },
            if info.region.is_empty() { None } else { Some(info.region.as_str()) },
            if info.country.is_empty() { None } else { Some(info.country.as_str()) },
            if info.isp.is_empty() { None } else { Some(info.isp.as_str()) },
            if info.org.is_empty() { None } else { Some(info.org.as_str()) },
            if info.as_name.is_empty() { None } else { Some(info.as_name.as_str()) },
            if info.timezone.is_empty() { None } else { Some(info.timezone.as_str()) },
            info.lat,
            info.lon,
            remote_host,
            remote_port.map(|p| p as i64),
            proto,
            cipher,
            auth,
            id,
        ],
    )?;
    Ok(())
}

pub fn increment_ovpn_fail_count(id: i64) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "UPDATE ovpn_configs SET fail_count = fail_count + 1 WHERE id = ?1",
        params![id],
    )?;
    let mut stmt = conn.prepare("SELECT fail_count FROM ovpn_configs WHERE id = ?1")?;
    let fail_count: i32 = stmt.query_row(params![id], |row| row.get(0))?;
    if fail_count > 5 {
        conn.execute("DELETE FROM ovpn_configs WHERE id = ?1", params![id])?;
    }
    Ok(())
}

pub fn delete_ovpn_config(id: i64) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute("DELETE FROM ovpn_configs WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn get_pref(key: &str) -> Result<Option<String>> {
    let conn = Connection::open(get_db_path()?)?;
    let mut stmt = conn.prepare("SELECT value FROM client_prefs WHERE key = ?1")?;
    let val = stmt
        .query_row(params![key], |row| row.get(0))
        .optional()?;
    Ok(val)
}

pub fn set_pref(key: &str, value: &str) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "INSERT INTO client_prefs (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_selected_ovpn_id() -> Option<i64> {
    get_pref("selected_ovpn_id")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
}

pub fn set_selected_ovpn_id(id: Option<i64>) {
    let _ = match id {
        Some(id) => set_pref("selected_ovpn_id", &id.to_string()),
        None => set_pref("selected_ovpn_id", ""),
    };
}

pub fn get_tor_isolation_mode() -> String {
    get_pref("tor_isolation_mode")
        .ok()
        .flatten()
        .unwrap_or_else(|| String::from("system"))
}

pub fn set_tor_isolation_mode(mode: &str) {
    let _ = set_pref("tor_isolation_mode", mode);
}

pub fn add_tor_isolated_app(name: &str, path: &str) -> Result<i64> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "INSERT INTO tor_isolated_apps (name, path, enabled) VALUES (?1, ?2, 1)
         ON CONFLICT(path) DO UPDATE SET name = excluded.name, enabled = 1",
        params![name, path],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_tor_isolated_apps() -> Result<Vec<TorIsolatedApp>> {
    let conn = Connection::open(get_db_path()?)?;
    let mut stmt =
        conn.prepare("SELECT id, name, path, enabled FROM tor_isolated_apps ORDER BY name")?;
    let iter = stmt.query_map([], |row| {
        Ok(TorIsolatedApp {
            id: row.get(0)?,
            name: row.get(1)?,
            path: row.get(2)?,
            enabled: row.get::<_, i64>(3)? != 0,
        })
    })?;
    let mut out = Vec::new();
    for a in iter {
        out.push(a?);
    }
    Ok(out)
}

pub fn set_tor_isolated_app_enabled(id: i64, enabled: bool) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "UPDATE tor_isolated_apps SET enabled = ?1 WHERE id = ?2",
        params![if enabled { 1 } else { 0 }, id],
    )?;
    Ok(())
}

pub fn delete_tor_isolated_app(id: i64) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute("DELETE FROM tor_isolated_apps WHERE id = ?1", params![id])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-protocol profile store (WireGuard / PPTP / Outline)
// ---------------------------------------------------------------------------

pub fn get_selected_vpn_protocol() -> vpn_suite_core::model::VpnUiProtocol {
    get_pref("selected_vpn_protocol")
        .ok()
        .flatten()
        .map(|s| vpn_suite_core::model::VpnUiProtocol::from_pref(&s))
        .unwrap_or_default()
}

pub fn set_selected_vpn_protocol(p: vpn_suite_core::model::VpnUiProtocol) {
    let _ = set_pref("selected_vpn_protocol", p.as_pref());
}

fn ensure_protocol_tables(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS wg_configs (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            content TEXT NOT NULL,
            endpoint TEXT,
            public_key TEXT,
            address TEXT,
            country_code TEXT,
            resolved_ip TEXT,
            city TEXT,
            region TEXT,
            country TEXT,
            isp TEXT,
            lat REAL,
            lon REAL
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS pptp_configs (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            host TEXT NOT NULL,
            port INTEGER NOT NULL DEFAULT 1723,
            username TEXT NOT NULL,
            password TEXT NOT NULL DEFAULT '',
            domain TEXT NOT NULL DEFAULT '',
            country_code TEXT,
            resolved_ip TEXT,
            city TEXT,
            region TEXT,
            country TEXT,
            lat REAL,
            lon REAL
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS outline_configs (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            access_key TEXT NOT NULL,
            method TEXT,
            host TEXT,
            port INTEGER,
            country_code TEXT,
            resolved_ip TEXT,
            city TEXT,
            region TEXT,
            country TEXT,
            lat REAL,
            lon REAL
        )",
        [],
    )?;
    Ok(())
}

/// Call from init_db (and safe to re-call).
pub fn migrate_protocol_tables() -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    ensure_protocol_tables(&conn)
}

// --- WireGuard profiles ----------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct WgConfig {
    pub id: i64,
    pub name: String,
    pub content: String,
    pub endpoint: Option<String>,
    pub public_key: Option<String>,
    pub address: Option<String>,
    pub country_code: Option<String>,
    pub resolved_ip: Option<String>,
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    pub isp: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

impl WgConfig {
    pub fn location_info(&self) -> TorExitInfo {
        TorExitInfo {
            ip: self
                .resolved_ip
                .clone()
                .or_else(|| {
                    self.endpoint
                        .as_ref()
                        .and_then(|e| e.rsplit_once(':').map(|(h, _)| h.to_string()))
                })
                .unwrap_or_default(),
            ipv6: String::new(),
            country_code: self.country_code.clone().unwrap_or_default(),
            country: self.country.clone().unwrap_or_default(),
            region_code: String::new(),
            region: self.region.clone().unwrap_or_default(),
            city: self.city.clone().unwrap_or_default(),
            zip: String::new(),
            lat: self.lat.unwrap_or(0.0),
            lon: self.lon.unwrap_or(0.0),
            timezone: String::new(),
            isp: self.isp.clone().unwrap_or_default(),
            org: String::new(),
            as_name: String::new(),
        }
    }

    pub fn endpoint_label(&self) -> String {
        self.endpoint
            .clone()
            .unwrap_or_else(|| String::from("WireGuard profile"))
    }
}

fn map_wg_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WgConfig> {
    Ok(WgConfig {
        id: row.get(0)?,
        name: row.get(1)?,
        content: row.get(2)?,
        endpoint: row.get(3)?,
        public_key: row.get(4)?,
        address: row.get(5)?,
        country_code: row.get(6)?,
        resolved_ip: row.get(7)?,
        city: row.get(8)?,
        region: row.get(9)?,
        country: row.get(10)?,
        isp: row.get(11)?,
        lat: row.get(12)?,
        lon: row.get(13)?,
    })
}

const WG_SELECT: &str = "SELECT id, name, content, endpoint, public_key, address,
    country_code, resolved_ip, city, region, country, isp, lat, lon FROM wg_configs";

pub fn add_wg_config(cfg: &WgConfig) -> Result<i64> {
    let conn = Connection::open(get_db_path()?)?;
    ensure_protocol_tables(&conn)?;
    conn.execute(
        "INSERT INTO wg_configs (name, content, endpoint, public_key, address,
            country_code, resolved_ip, city, region, country, isp, lat, lon)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![
            cfg.name,
            cfg.content,
            cfg.endpoint,
            cfg.public_key,
            cfg.address,
            cfg.country_code,
            cfg.resolved_ip,
            cfg.city,
            cfg.region,
            cfg.country,
            cfg.isp,
            cfg.lat,
            cfg.lon,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_wg_configs() -> Result<Vec<WgConfig>> {
    let conn = Connection::open(get_db_path()?)?;
    ensure_protocol_tables(&conn)?;
    let mut stmt = conn.prepare(WG_SELECT)?;
    let iter = stmt.query_map([], map_wg_row)?;
    let mut out = Vec::new();
    for r in iter {
        out.push(r?);
    }
    Ok(out)
}

pub fn get_wg_config(id: i64) -> Result<Option<WgConfig>> {
    let conn = Connection::open(get_db_path()?)?;
    ensure_protocol_tables(&conn)?;
    let mut stmt = conn.prepare(&format!("{WG_SELECT} WHERE id = ?1"))?;
    Ok(stmt.query_row(params![id], map_wg_row).optional()?)
}

pub fn update_wg_location(id: i64, info: &TorExitInfo, resolved_ip: Option<&str>) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "UPDATE wg_configs SET country_code=?1, resolved_ip=COALESCE(?2, resolved_ip),
            city=?3, region=?4, country=?5, isp=?6, lat=?7, lon=?8 WHERE id=?9",
        params![
            if info.country_code.is_empty() {
                None
            } else {
                Some(info.country_code.as_str())
            },
            resolved_ip,
            if info.city.is_empty() {
                None
            } else {
                Some(info.city.as_str())
            },
            if info.region.is_empty() {
                None
            } else {
                Some(info.region.as_str())
            },
            if info.country.is_empty() {
                None
            } else {
                Some(info.country.as_str())
            },
            if info.isp.is_empty() {
                None
            } else {
                Some(info.isp.as_str())
            },
            info.lat,
            info.lon,
            id,
        ],
    )?;
    Ok(())
}

pub fn delete_wg_config(id: i64) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute("DELETE FROM wg_configs WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn get_selected_wg_id() -> Option<i64> {
    get_pref("selected_wg_id")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
}

pub fn set_selected_wg_id(id: Option<i64>) {
    let _ = match id {
        Some(id) => set_pref("selected_wg_id", &id.to_string()),
        None => set_pref("selected_wg_id", ""),
    };
}

// --- PPTP profiles ---------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct PptpConfig {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub domain: String,
    pub country_code: Option<String>,
    pub resolved_ip: Option<String>,
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

impl PptpConfig {
    pub fn location_info(&self) -> TorExitInfo {
        TorExitInfo {
            ip: self
                .resolved_ip
                .clone()
                .unwrap_or_else(|| self.host.clone()),
            ipv6: String::new(),
            country_code: self.country_code.clone().unwrap_or_default(),
            country: self.country.clone().unwrap_or_default(),
            region_code: String::new(),
            region: self.region.clone().unwrap_or_default(),
            city: self.city.clone().unwrap_or_default(),
            zip: String::new(),
            lat: self.lat.unwrap_or(0.0),
            lon: self.lon.unwrap_or(0.0),
            timezone: String::new(),
            isp: String::new(),
            org: String::new(),
            as_name: String::new(),
        }
    }

    pub fn endpoint_label(&self) -> String {
        if self.port == 1723 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn to_endpoint(&self) -> vpn_suite_core::pptp::PptpEndpoint {
        vpn_suite_core::pptp::PptpEndpoint {
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password: self.password.clone(),
            domain: self.domain.clone(),
        }
    }
}

fn map_pptp_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PptpConfig> {
    Ok(PptpConfig {
        id: row.get(0)?,
        name: row.get(1)?,
        host: row.get(2)?,
        port: row.get::<_, i64>(3)? as u16,
        username: row.get(4)?,
        password: row.get(5)?,
        domain: row.get(6)?,
        country_code: row.get(7)?,
        resolved_ip: row.get(8)?,
        city: row.get(9)?,
        region: row.get(10)?,
        country: row.get(11)?,
        lat: row.get(12)?,
        lon: row.get(13)?,
    })
}

const PPTP_SELECT: &str = "SELECT id, name, host, port, username, password, domain,
    country_code, resolved_ip, city, region, country, lat, lon FROM pptp_configs";

pub fn add_pptp_config(cfg: &PptpConfig) -> Result<i64> {
    let conn = Connection::open(get_db_path()?)?;
    ensure_protocol_tables(&conn)?;
    conn.execute(
        "INSERT INTO pptp_configs (name, host, port, username, password, domain,
            country_code, resolved_ip, city, region, country, lat, lon)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![
            cfg.name,
            cfg.host,
            cfg.port as i64,
            cfg.username,
            cfg.password,
            cfg.domain,
            cfg.country_code,
            cfg.resolved_ip,
            cfg.city,
            cfg.region,
            cfg.country,
            cfg.lat,
            cfg.lon,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_pptp_configs() -> Result<Vec<PptpConfig>> {
    let conn = Connection::open(get_db_path()?)?;
    ensure_protocol_tables(&conn)?;
    let mut stmt = conn.prepare(PPTP_SELECT)?;
    let iter = stmt.query_map([], map_pptp_row)?;
    let mut out = Vec::new();
    for r in iter {
        out.push(r?);
    }
    Ok(out)
}

pub fn get_pptp_config(id: i64) -> Result<Option<PptpConfig>> {
    let conn = Connection::open(get_db_path()?)?;
    ensure_protocol_tables(&conn)?;
    let mut stmt = conn.prepare(&format!("{PPTP_SELECT} WHERE id = ?1"))?;
    Ok(stmt.query_row(params![id], map_pptp_row).optional()?)
}

pub fn delete_pptp_config(id: i64) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute("DELETE FROM pptp_configs WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn get_selected_pptp_id() -> Option<i64> {
    get_pref("selected_pptp_id")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
}

pub fn set_selected_pptp_id(id: Option<i64>) {
    let _ = match id {
        Some(id) => set_pref("selected_pptp_id", &id.to_string()),
        None => set_pref("selected_pptp_id", ""),
    };
}

// --- Outline profiles ------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct OutlineConfig {
    pub id: i64,
    pub name: String,
    pub access_key: String,
    pub method: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub country_code: Option<String>,
    pub resolved_ip: Option<String>,
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

impl OutlineConfig {
    pub fn location_info(&self) -> TorExitInfo {
        TorExitInfo {
            ip: self
                .resolved_ip
                .clone()
                .or_else(|| self.host.clone())
                .unwrap_or_default(),
            ipv6: String::new(),
            country_code: self.country_code.clone().unwrap_or_default(),
            country: self.country.clone().unwrap_or_default(),
            region_code: String::new(),
            region: self.region.clone().unwrap_or_default(),
            city: self.city.clone().unwrap_or_default(),
            zip: String::new(),
            lat: self.lat.unwrap_or(0.0),
            lon: self.lon.unwrap_or(0.0),
            timezone: String::new(),
            isp: String::new(),
            org: String::new(),
            as_name: String::new(),
        }
    }

    pub fn endpoint_label(&self) -> String {
        match (&self.host, self.port) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.clone(),
            _ => String::from("Outline access key"),
        }
    }
}

fn map_outline_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutlineConfig> {
    Ok(OutlineConfig {
        id: row.get(0)?,
        name: row.get(1)?,
        access_key: row.get(2)?,
        method: row.get(3)?,
        host: row.get(4)?,
        port: row.get::<_, Option<i64>>(5)?.map(|p| p as u16),
        country_code: row.get(6)?,
        resolved_ip: row.get(7)?,
        city: row.get(8)?,
        region: row.get(9)?,
        country: row.get(10)?,
        lat: row.get(11)?,
        lon: row.get(12)?,
    })
}

const OUTLINE_SELECT: &str = "SELECT id, name, access_key, method, host, port,
    country_code, resolved_ip, city, region, country, lat, lon FROM outline_configs";

pub fn add_outline_config(cfg: &OutlineConfig) -> Result<i64> {
    let conn = Connection::open(get_db_path()?)?;
    ensure_protocol_tables(&conn)?;
    conn.execute(
        "INSERT INTO outline_configs (name, access_key, method, host, port,
            country_code, resolved_ip, city, region, country, lat, lon)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            cfg.name,
            cfg.access_key,
            cfg.method,
            cfg.host,
            cfg.port.map(|p| p as i64),
            cfg.country_code,
            cfg.resolved_ip,
            cfg.city,
            cfg.region,
            cfg.country,
            cfg.lat,
            cfg.lon,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_outline_configs() -> Result<Vec<OutlineConfig>> {
    let conn = Connection::open(get_db_path()?)?;
    ensure_protocol_tables(&conn)?;
    let mut stmt = conn.prepare(OUTLINE_SELECT)?;
    let iter = stmt.query_map([], map_outline_row)?;
    let mut out = Vec::new();
    for r in iter {
        out.push(r?);
    }
    Ok(out)
}

pub fn get_outline_config(id: i64) -> Result<Option<OutlineConfig>> {
    let conn = Connection::open(get_db_path()?)?;
    ensure_protocol_tables(&conn)?;
    let mut stmt = conn.prepare(&format!("{OUTLINE_SELECT} WHERE id = ?1"))?;
    Ok(stmt.query_row(params![id], map_outline_row).optional()?)
}

pub fn update_outline_location(id: i64, info: &TorExitInfo) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute(
        "UPDATE outline_configs SET country_code=?1, resolved_ip=?2,
            city=?3, region=?4, country=?5, lat=?6, lon=?7 WHERE id=?8",
        params![
            if info.country_code.is_empty() {
                None
            } else {
                Some(info.country_code.as_str())
            },
            if info.ip.is_empty() {
                None
            } else {
                Some(info.ip.as_str())
            },
            if info.city.is_empty() {
                None
            } else {
                Some(info.city.as_str())
            },
            if info.region.is_empty() {
                None
            } else {
                Some(info.region.as_str())
            },
            if info.country.is_empty() {
                None
            } else {
                Some(info.country.as_str())
            },
            info.lat,
            info.lon,
            id,
        ],
    )?;
    Ok(())
}

pub fn delete_outline_config(id: i64) -> Result<()> {
    let conn = Connection::open(get_db_path()?)?;
    conn.execute("DELETE FROM outline_configs WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn get_selected_outline_id() -> Option<i64> {
    get_pref("selected_outline_id")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
}

pub fn set_selected_outline_id(id: Option<i64>) {
    let _ = match id {
        Some(id) => set_pref("selected_outline_id", &id.to_string()),
        None => set_pref("selected_outline_id", ""),
    };
}
