use crate::util::{atomic_write, now_ts};
use anyhow::Context;
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AllowedList {
    pub allow: Vec<String>,
}

impl AllowedList {
    pub fn to_nets(&self) -> Vec<IpNet> {
        self.allow
            .iter()
            .filter_map(|entry| {
                if entry.contains('/') {
                    entry.parse::<IpNet>().ok()
                } else {
                    entry
                        .parse::<IpAddr>()
                        .ok()
                        .map(|ip| IpNet::from(ip))
                }
            })
            .collect()
    }

    pub fn allows(&self, ip: IpAddr) -> bool {
        let nets = self.to_nets();
        nets.iter().any(|net| net.contains(&ip))
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path).context("read allowlist")?;
        let parsed = toml::from_str::<AllowedList>(&raw).unwrap_or_default();
        Ok(parsed)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let data = toml::to_string_pretty(self)?;
        atomic_write(path, data.as_bytes())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEntry {
    pub first_seen: u64,
    pub last_seen: u64,
    pub attempts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PendingList {
    pub pending: BTreeMap<String, PendingEntry>,
}

impl PendingList {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path).context("read pending list")?;
        let parsed = toml::from_str::<PendingList>(&raw).unwrap_or_default();
        Ok(parsed)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let data = toml::to_string_pretty(self)?;
        atomic_write(path, data.as_bytes())
    }

    pub fn note_attempt(&mut self, ip: IpAddr) {
        let key = ip.to_string();
        let now = now_ts();
        self.pending
            .entry(key)
            .and_modify(|entry| {
                entry.last_seen = now;
                entry.attempts += 1;
            })
            .or_insert(PendingEntry {
                first_seen: now,
                last_seen: now,
                attempts: 1,
            });
    }

    pub fn remove(&mut self, ip: &str) {
        self.pending.remove(ip);
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

#[derive(Debug, Clone)]
pub struct AllowlistFiles {
    pub allowlist: PathBuf,
    pub pending: PathBuf,
}

impl AllowlistFiles {
    pub fn check_or_note(&self, ip: IpAddr) -> anyhow::Result<bool> {
        let allowed = AllowedList::load(&self.allowlist)?.allows(ip);
        if allowed {
            return Ok(true);
        }
        let mut pending = PendingList::load(&self.pending)?;
        pending.note_attempt(ip);
        pending.save(&self.pending)?;
        info!(%ip, "ip not approved - added to pending");
        Ok(false)
    }

    pub fn add_allow(&self, entry: &str) -> anyhow::Result<()> {
        let mut allow = AllowedList::load(&self.allowlist)?;
        if !allow.allow.iter().any(|e| e == entry) {
            allow.allow.push(entry.to_string());
            allow.allow.sort();
            allow.save(&self.allowlist)?;
        }
        Ok(())
    }

    pub fn remove_allow(&self, entry: &str) -> anyhow::Result<()> {
        let mut allow = AllowedList::load(&self.allowlist)?;
        allow.allow.retain(|e| e != entry);
        allow.save(&self.allowlist)?;
        Ok(())
    }

    pub fn list_allow(&self) -> anyhow::Result<Vec<String>> {
        let allow = AllowedList::load(&self.allowlist)?;
        Ok(allow.allow)
    }

    pub fn list_pending(&self) -> anyhow::Result<Vec<(String, PendingEntry)>> {
        let pending = PendingList::load(&self.pending)?;
        Ok(pending.pending.into_iter().collect())
    }

    pub fn remove_pending(&self, ip: &str) -> anyhow::Result<()> {
        let mut pending = PendingList::load(&self.pending)?;
        if pending.pending.remove(ip).is_some() {
            pending.save(&self.pending)?;
        } else {
            warn!(%ip, "pending ip not found");
        }
        Ok(())
    }

    pub fn clear_pending(&self) -> anyhow::Result<()> {
        let mut pending = PendingList::load(&self.pending)?;
        pending.clear();
        pending.save(&self.pending)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::str::FromStr;

    #[test]
    fn allowlist_basic() {
        let list = AllowedList {
            allow: vec!["127.0.0.1".into(), "10.0.0.0/8".into()],
        };
        assert!(list.allows(IpAddr::from_str("127.0.0.1").unwrap()));
        assert!(list.allows(IpAddr::from_str("10.1.2.3").unwrap()));
        assert!(!list.allows(IpAddr::from_str("192.168.0.1").unwrap()));
    }
}
