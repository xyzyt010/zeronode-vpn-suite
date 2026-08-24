use anyhow::{Context, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Algorithm, Argon2, Params, Version,
};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use zeroize::Zeroizing;

pub const MAX_FAILED_ATTEMPTS_PER_IP: u32 = 5;
pub const COOLDOWN_SECS: u64 = 12 * 60 * 60;
pub const GLOBAL_LOCKDOWN_THRESHOLD: usize = 100;
pub const GLOBAL_LOCKDOWN_WINDOW_SECS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RateLimitJournal {
    pub failures: BTreeMap<String, IpFailureRecord>,
    pub recent_failures: VecDeque<FailureEvent>,
    pub banned_ips: BTreeSet<String>,
    pub lockdown: Option<LockdownRecord>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IpFailureRecord {
    pub count: u32,
    pub last_failure_unix: u64,
    pub cooldown_until_unix: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailureEvent {
    pub ip: String,
    pub at_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LockdownRecord {
    pub triggered_at_unix: u64,
    pub offender_ips: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AccessDecision {
    pub allowed: bool,
    pub cooldown_until_unix: Option<u64>,
    pub locked_down: bool,
    pub silently_drop: bool,
}

impl RateLimitJournal {
    pub fn check(&mut self, ip: &str, now_unix: u64) -> AccessDecision {
        self.trim_recent(now_unix);

        if self.lockdown.is_some() || self.banned_ips.contains(ip) {
            return AccessDecision {
                allowed: false,
                cooldown_until_unix: None,
                locked_down: self.lockdown.is_some(),
                silently_drop: true,
            };
        }

        if let Some(record) = self.failures.get(ip) {
            if let Some(until) = record.cooldown_until_unix {
                if until > now_unix {
                    return AccessDecision {
                        allowed: false,
                        cooldown_until_unix: Some(until),
                        locked_down: false,
                        silently_drop: true,
                    };
                }
            }
        }

        AccessDecision {
            allowed: true,
            cooldown_until_unix: None,
            locked_down: false,
            silently_drop: false,
        }
    }

    pub fn register_failure(&mut self, ip: &str, now_unix: u64) -> AccessDecision {
        self.trim_recent(now_unix);
        self.recent_failures.push_back(FailureEvent {
            ip: ip.to_owned(),
            at_unix: now_unix,
        });

        let record = self.failures.entry(ip.to_owned()).or_default();
        record.count += 1;
        record.last_failure_unix = now_unix;

        let mut cooldown_until_unix = None;
        if record.count >= MAX_FAILED_ATTEMPTS_PER_IP {
            let cooldown = now_unix + COOLDOWN_SECS;
            record.cooldown_until_unix = Some(cooldown);
            record.count = 0;
            cooldown_until_unix = Some(cooldown);
        }

        let recent_total = self.recent_failures.len();
        if recent_total > GLOBAL_LOCKDOWN_THRESHOLD {
            let offender_ips = self
                .recent_failures
                .iter()
                .map(|entry| entry.ip.clone())
                .collect::<BTreeSet<_>>();

            self.banned_ips.extend(offender_ips.iter().cloned());
            self.lockdown = Some(LockdownRecord {
                triggered_at_unix: now_unix,
                offender_ips: offender_ips.into_iter().collect(),
            });

            return AccessDecision {
                allowed: false,
                cooldown_until_unix,
                locked_down: true,
                silently_drop: false,
            };
        }

        AccessDecision {
            allowed: false,
            cooldown_until_unix,
            locked_down: false,
            silently_drop: false,
        }
    }

    pub fn register_success(&mut self, ip: &str) {
        if let Some(record) = self.failures.get_mut(ip) {
            record.count = 0;
            record.cooldown_until_unix = None;
        }
    }

    pub fn is_locked_down(&self) -> bool {
        self.lockdown.is_some()
    }

    pub fn unlock(&mut self) {
        self.lockdown = None;
    }

    pub fn list_bans(&self) -> Vec<String> {
        self.banned_ips.iter().cloned().collect()
    }

    pub fn unban(&mut self, ip: &str) -> bool {
        self.banned_ips.remove(ip)
    }

    pub fn trim_recent(&mut self, now_unix: u64) {
        while let Some(front) = self.recent_failures.front() {
            if now_unix.saturating_sub(front.at_unix) > GLOBAL_LOCKDOWN_WINDOW_SECS {
                self.recent_failures.pop_front();
            } else {
                break;
            }
        }
    }
}

pub fn hash_password(password: Zeroizing<String>) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let params =
        Params::new(64 * 1024, 3, 4, Some(32)).context("argon2 parameter construction failed")?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    argon
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .context("argon2 hashing failed")
}

pub fn verify_password(password: Zeroizing<String>, encoded_hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(encoded_hash).context("invalid password hash format")?;
    let params =
        Params::new(64 * 1024, 3, 4, Some(32)).context("argon2 parameter construction failed")?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    Ok(argon.verify_password(password.as_bytes(), &parsed).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_verifies_passwords() {
        let hash = hash_password(Zeroizing::new(String::from("top-secret"))).unwrap();
        assert!(verify_password(Zeroizing::new(String::from("top-secret")), &hash).unwrap());
        assert!(!verify_password(Zeroizing::new(String::from("wrong")), &hash).unwrap());
    }

    #[test]
    fn enters_cooldown_after_five_failures() {
        let mut journal = RateLimitJournal::default();
        let mut decision = AccessDecision {
            allowed: true,
            cooldown_until_unix: None,
            locked_down: false,
            silently_drop: false,
        };

        for step in 0..5 {
            decision = journal.register_failure("10.0.0.2", 1000 + step);
        }

        assert!(decision.cooldown_until_unix.is_some());
        let gate = journal.check("10.0.0.2", 1010);
        assert!(!gate.allowed);
        assert!(gate.silently_drop);
    }
}
