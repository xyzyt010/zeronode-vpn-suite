//! Outline / Shadowsocks access-key parsing (OS-agnostic).
//!
//! Outline clients typically use SIP002 `ss://` URIs or dynamic access keys
//! that resolve to Shadowsocks parameters. This module only parses and
//! validates — the platform layer owns the data plane (local SS client + TUN).

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD, STANDARD_NO_PAD}, Engine as _};
use serde::{Deserialize, Serialize};

/// Parsed Outline / Shadowsocks endpoint parameters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutlineEndpoint {
    pub method: String,
    pub password: String,
    pub host: String,
    pub port: u16,
    /// Optional human label (from fragment `#Name` or dynamic key name).
    pub name: Option<String>,
    /// Optional plugin (e.g. v2ray-plugin) — not yet executed by the client.
    pub plugin: Option<String>,
    pub plugin_opts: Option<String>,
}

impl OutlineEndpoint {
    pub fn endpoint_label(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn is_aead(&self) -> bool {
        let m = self.method.to_ascii_lowercase();
        m.contains("gcm")
            || m.contains("poly1305")
            || m.contains("2022")
            || m == "chacha20-ietf-poly1305"
            || m == "xchacha20-ietf-poly1305"
    }

    pub fn security_warning(&self) -> Option<&'static str> {
        if self.is_aead() {
            None
        } else {
            Some("This cipher is outdated (stream cipher). Prefer AEAD methods such as chacha20-ietf-poly1305 or aes-256-gcm.")
        }
    }
}

/// Parse an Outline access key or `ss://` URI.
///
/// Supported forms:
/// - `ss://BASE64(method:password@host:port)` (legacy)
/// - `ss://BASE64(method:password)@host:port` (SIP002 userinfo)
/// - `ss://method:password@host:port` (plain, uncommon)
/// - `ssconf://…` — returns an error directing the caller to fetch JSON first
pub fn parse_access_key(raw: &str) -> Result<OutlineEndpoint> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("access key is empty");
    }

    if raw.to_ascii_lowercase().starts_with("ssconf://") {
        bail!("ssconf:// dynamic keys must be fetched over HTTPS first; paste the resolved ss:// key or JSON");
    }

    if let Some(rest) = strip_scheme(raw, "ss://") {
        return parse_ss_body(rest);
    }

    // Bare base64 blob or method:password@host:port
    if raw.contains('@') || raw.contains(':') {
        if let Ok(ep) = parse_ss_body(raw) {
            return Ok(ep);
        }
    }

    // Try as full base64 of method:password@host:port
    if let Ok(decoded) = decode_b64(raw) {
        if let Ok(ep) = parse_userinfo_host(&decoded) {
            return Ok(ep);
        }
    }

    bail!("unrecognized Outline / Shadowsocks access key format")
}

/// Parse Outline dynamic-key JSON (subset used by Outline Manager).
///
/// Example:
/// ```json
/// {"server":"1.2.3.4","server_port":12345,"password":"x","method":"chacha20-ietf-poly1305","name":"MyServer"}
/// ```
pub fn parse_outline_json(json: &str) -> Result<OutlineEndpoint> {
    #[derive(Deserialize)]
    struct DynKey {
        server: String,
        #[serde(alias = "server_port", alias = "port")]
        server_port: u16,
        password: String,
        method: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        plugin: Option<String>,
        #[serde(default)]
        #[serde(alias = "plugin_opts")]
        plugin_opts: Option<String>,
    }
    let key: DynKey = serde_json::from_str(json.trim()).context("invalid Outline JSON")?;
    if key.server.trim().is_empty() {
        bail!("server host is empty");
    }
    if key.password.is_empty() {
        bail!("password is empty");
    }
    Ok(OutlineEndpoint {
        method: key.method,
        password: key.password,
        host: key.server.trim().to_string(),
        port: key.server_port,
        name: key.name,
        plugin: key.plugin,
        plugin_opts: key.plugin_opts,
    })
}

/// Try access key first, then JSON.
pub fn parse_outline_input(raw: &str) -> Result<OutlineEndpoint> {
    let raw = raw.trim();
    if raw.starts_with('{') {
        return parse_outline_json(raw);
    }
    parse_access_key(raw)
}

fn strip_scheme<'a>(s: &'a str, scheme: &str) -> Option<&'a str> {
    if s.len() >= scheme.len() && s[..scheme.len()].eq_ignore_ascii_case(scheme) {
        Some(&s[scheme.len()..])
    } else {
        None
    }
}

fn parse_ss_body(body: &str) -> Result<OutlineEndpoint> {
    let (main, fragment) = match body.split_once('#') {
        Some((m, f)) => (m, Some(percent_decode(f))),
        None => (body, None),
    };

    // SIP002: base64(method:password)@host:port?plugin=...
    if let Some((userinfo, hostport)) = main.split_once('@') {
        let (method, password) = if looks_like_b64(userinfo) {
            let decoded = decode_b64(userinfo).context("decode ss userinfo")?;
            split_method_password(&decoded)?
        } else {
            split_method_password(userinfo)?
        };
        let (host, port, plugin, plugin_opts) = parse_hostport_query(hostport)?;
        return Ok(OutlineEndpoint {
            method,
            password,
            host,
            port,
            name: fragment,
            plugin,
            plugin_opts,
        });
    }

    // Legacy: entire body is base64(method:password@host:port)
    if looks_like_b64(main) {
        let decoded = decode_b64(main).context("decode legacy ss body")?;
        let mut ep = parse_userinfo_host(&decoded)?;
        if ep.name.is_none() {
            ep.name = fragment;
        }
        return Ok(ep);
    }

    parse_userinfo_host(main).map(|mut ep| {
        if ep.name.is_none() {
            ep.name = fragment;
        }
        ep
    })
}

fn parse_userinfo_host(s: &str) -> Result<OutlineEndpoint> {
    let (userinfo, hostport) = s
        .rsplit_once('@')
        .context("missing @host:port in Shadowsocks URI")?;
    let (method, password) = split_method_password(userinfo)?;
    let (host, port, plugin, plugin_opts) = parse_hostport_query(hostport)?;
    Ok(OutlineEndpoint {
        method,
        password,
        host,
        port,
        name: None,
        plugin,
        plugin_opts,
    })
}

fn split_method_password(userinfo: &str) -> Result<(String, String)> {
    let (method, password) = userinfo
        .split_once(':')
        .context("expected method:password")?;
    if method.is_empty() {
        bail!("cipher method is empty");
    }
    Ok((method.to_string(), password.to_string()))
}

fn parse_hostport_query(hostport: &str) -> Result<(String, u16, Option<String>, Option<String>)> {
    // Outline Manager keys often look like:
    //   ss://…@host:12345/?outline=1
    //   ss://…@host:12345?outline=1
    //   ss://…@host:12345/
    // Strip the query first, then any trailing slash on the host:port part.
    let (hp, query) = match hostport.split_once('?') {
        Some((h, q)) => (h, Some(q)),
        None => (hostport, None),
    };
    let hp = hp.trim().trim_end_matches('/').trim();

    let (host, port) = if let Some(rest) = hp.strip_prefix('[') {
        // IPv6 [addr]:port
        let (addr, after) = rest
            .split_once(']')
            .context("invalid IPv6 host in ss URI")?;
        let port_str = after
            .strip_prefix(':')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .context("missing port after IPv6 host")?;
        let port = parse_port_digits(port_str)?;
        (addr.to_string(), port)
    } else {
        let (h, p) = hp.rsplit_once(':').context("expected host:port")?;
        let port = parse_port_digits(p.trim())?;
        (h.trim().to_string(), port)
    };

    let mut plugin = None;
    let mut plugin_opts = None;
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                match k {
                    "plugin" => {
                        // plugin is often "name;opt=val;opt2=val"
                        let decoded = percent_decode(v);
                        if let Some((name, opts)) = decoded.split_once(';') {
                            plugin = Some(name.to_string());
                            plugin_opts = Some(opts.to_string());
                        } else {
                            plugin = Some(decoded);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if host.is_empty() {
        bail!("host is empty");
    }
    Ok((host, port, plugin, plugin_opts))
}

/// Parse a TCP/UDP port, tolerating trailing junk that sometimes appears in pasted keys.
fn parse_port_digits(raw: &str) -> Result<u16> {
    let s = raw.trim().trim_end_matches('/').trim();
    // Take leading digits only (handles "8388/", "8388 ", "8388#frag" edge cases).
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        bail!("invalid port: no digits in `{raw}`");
    }
    digits
        .parse::<u16>()
        .with_context(|| format!("invalid port: `{raw}`"))
}

fn looks_like_b64(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_'))
}

fn decode_b64(s: &str) -> Result<String> {
    let s = s.trim().trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .or_else(|_| URL_SAFE.decode(s))
        .or_else(|_| STANDARD_NO_PAD.decode(s))
        .or_else(|_| STANDARD.decode(s))
        .or_else(|_| {
            // pad and retry
            let mut padded = s.to_string();
            while padded.len() % 4 != 0 {
                padded.push('=');
            }
            STANDARD
                .decode(&padded)
                .or_else(|_| URL_SAFE.decode(&padded))
        })
        .context("base64 decode failed")?;
    String::from_utf8(bytes).context("ss credentials are not UTF-8")
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push(char::from((h * 16 + l) as u8));
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(' ');
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sip002_plain() {
        let ep = parse_access_key("ss://chacha20-ietf-poly1305:secret@1.2.3.4:8388#MyNode")
            .unwrap();
        assert_eq!(ep.method, "chacha20-ietf-poly1305");
        assert_eq!(ep.password, "secret");
        assert_eq!(ep.host, "1.2.3.4");
        assert_eq!(ep.port, 8388);
        assert_eq!(ep.name.as_deref(), Some("MyNode"));
        assert!(ep.is_aead());
    }

    #[test]
    fn parses_legacy_base64() {
        let raw = STANDARD.encode("aes-256-gcm:pass@example.com:443");
        let ep = parse_access_key(&format!("ss://{raw}")).unwrap();
        assert_eq!(ep.host, "example.com");
        assert_eq!(ep.port, 443);
        assert_eq!(ep.method, "aes-256-gcm");
    }

    #[test]
    fn parses_json() {
        let ep = parse_outline_json(
            r#"{"server":"10.0.0.1","server_port":9000,"password":"p","method":"aes-256-gcm","name":"x"}"#,
        )
        .unwrap();
        assert_eq!(ep.host, "10.0.0.1");
        assert_eq!(ep.port, 9000);
        assert_eq!(ep.name.as_deref(), Some("x"));
    }

    /// Outline Manager always appends `/?outline=1` — the trailing slash before
    /// `?` used to make port parse as `8388/` → "invalid digit found in string".
    #[test]
    fn parses_outline_manager_key_with_slash_query() {
        let userinfo = STANDARD_NO_PAD.encode("chacha20-ietf-poly1305:secret");
        let key = format!("ss://{userinfo}@1.2.3.4:8388/?outline=1");
        let ep = parse_access_key(&key).expect("outline manager key");
        assert_eq!(ep.host, "1.2.3.4");
        assert_eq!(ep.port, 8388);
        assert_eq!(ep.method, "chacha20-ietf-poly1305");
        assert_eq!(ep.password, "secret");
    }

    #[test]
    fn parses_trailing_slash_without_query() {
        let userinfo = STANDARD_NO_PAD.encode("aes-256-gcm:pw");
        let key = format!("ss://{userinfo}@example.com:443/");
        let ep = parse_access_key(&key).unwrap();
        assert_eq!(ep.port, 443);
        assert_eq!(ep.host, "example.com");
    }

    #[test]
    fn parses_query_without_slash() {
        let userinfo = STANDARD_NO_PAD.encode("aes-256-gcm:pw");
        let key = format!("ss://{userinfo}@10.0.0.1:9000?outline=1");
        let ep = parse_access_key(&key).unwrap();
        assert_eq!(ep.port, 9000);
    }

    #[test]
    fn parses_sip002_userinfo_base64() {
        let userinfo = URL_SAFE_NO_PAD.encode("chacha20-ietf-poly1305:s3cret");
        let key = format!("ss://{userinfo}@vpn.example:12345#Label");
        let ep = parse_access_key(&key).unwrap();
        assert_eq!(ep.method, "chacha20-ietf-poly1305");
        assert_eq!(ep.password, "s3cret");
        assert_eq!(ep.host, "vpn.example");
        assert_eq!(ep.port, 12345);
        assert_eq!(ep.name.as_deref(), Some("Label"));
    }
}
