//! PPTP profile model (OS-agnostic).
//!
//! PPTP uses weak crypto (MS-CHAPv2). Profiles are stored and validated here;
//! dialing is always OS-native (Windows RAS, Linux pppd).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PPTP_PORT: u16 = 1723;

pub const SECURITY_WARNING: &str =
    "PPTP is a legacy protocol with known cryptographic weaknesses (MS-CHAPv2). \
     Use only for compatibility with older routers/servers. Prefer WireGuard, OpenVPN, or Outline for privacy.";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PptpEndpoint {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Password may be empty when the UI prompts at connect time.
    pub password: String,
    pub domain: String,
}

impl PptpEndpoint {
    pub fn endpoint_label(&self) -> String {
        if self.port == DEFAULT_PPTP_PORT {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty() {
            bail!("PPTP server host is required");
        }
        if self.username.trim().is_empty() {
            bail!("PPTP username is required");
        }
        if self.port == 0 {
            bail!("PPTP port is invalid");
        }
        Ok(())
    }

    pub fn dial_target(&self) -> String {
        // Windows RAS PhoneNumber is typically just the host for PPTP.
        self.host.trim().to_string()
    }

    pub fn user_for_dial(&self) -> String {
        let user = self.username.trim();
        let domain = self.domain.trim();
        if domain.is_empty() {
            user.to_string()
        } else {
            format!("{domain}\\{user}")
        }
    }
}

pub fn default_endpoint() -> PptpEndpoint {
    PptpEndpoint {
        host: String::new(),
        port: DEFAULT_PPTP_PORT,
        username: String::new(),
        password: String::new(),
        domain: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_required_fields() {
        let mut ep = default_endpoint();
        assert!(ep.validate().is_err());
        ep.host = "vpn.example.com".into();
        assert!(ep.validate().is_err());
        ep.username = "alice".into();
        assert!(ep.validate().is_ok());
        assert_eq!(ep.user_for_dial(), "alice");
        ep.domain = "CORP".into();
        assert_eq!(ep.user_for_dial(), "CORP\\alice");
    }
}
