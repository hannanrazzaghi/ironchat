use crate::util::{atomic_write, now_ts};
use anyhow::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use tokio::sync::Mutex;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityRecord {
    pub nick: String,
    pub updated: u64,
}

#[async_trait]
pub trait IdentityStore: Send + Sync {
    async fn get(&self, ip: IpAddr) -> anyhow::Result<Option<IdentityRecord>>;
    async fn set(&self, ip: IpAddr, nick: String) -> anyhow::Result<()>;
    async fn remove(&self, ip: IpAddr) -> anyhow::Result<()>;
    async fn list(&self) -> anyhow::Result<Vec<(IpAddr, IdentityRecord)>>;
}

#[derive(Debug)]
pub struct FileIdentityStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileIdentityStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    fn load_inner(path: &PathBuf) -> anyhow::Result<BTreeMap<String, IdentityRecord>> {
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let raw = std::fs::read_to_string(path).context("read identities")?;
        let parsed = toml::from_str::<BTreeMap<String, IdentityRecord>>(&raw)
            .unwrap_or_else(|_| BTreeMap::new());
        Ok(parsed)
    }

    fn save_inner(path: &PathBuf, map: BTreeMap<String, IdentityRecord>) -> anyhow::Result<()> {
        let mut nick_index: BTreeMap<String, (String, IdentityRecord)> = BTreeMap::new();
        for (ip, rec) in map.into_iter() {
            let key = rec.nick.to_lowercase();
            match nick_index.get(&key) {
                Some((_, existing)) if existing.updated >= rec.updated => {
                    continue;
                }
                _ => {
                    nick_index.insert(key, (ip, rec));
                }
            }
        }
        let mut cleaned = BTreeMap::new();
        for (_nick, (ip, rec)) in nick_index {
            cleaned.insert(ip, rec);
        }
        let data = toml::to_string_pretty(&cleaned)?;
        atomic_write(path, data.as_bytes())
    }
}

#[async_trait]
impl IdentityStore for FileIdentityStore {
    async fn get(&self, ip: IpAddr) -> anyhow::Result<Option<IdentityRecord>> {
        let _guard = self.lock.lock().await;
        let map = Self::load_inner(&self.path)?;
        Ok(map.get(&ip.to_string()).cloned())
    }

    async fn set(&self, ip: IpAddr, nick: String) -> anyhow::Result<()> {
        let _guard = self.lock.lock().await;
        let mut map = Self::load_inner(&self.path)?;
        map.insert(
            ip.to_string(),
            IdentityRecord {
                nick,
                updated: now_ts(),
            },
        );
        Self::save_inner(&self.path, map)
    }

    async fn remove(&self, ip: IpAddr) -> anyhow::Result<()> {
        let _guard = self.lock.lock().await;
        let mut map = Self::load_inner(&self.path)?;
        map.remove(&ip.to_string());
        Self::save_inner(&self.path, map)
    }

    async fn list(&self) -> anyhow::Result<Vec<(IpAddr, IdentityRecord)>> {
        let _guard = self.lock.lock().await;
        let map = Self::load_inner(&self.path)?;
        let mut out = Vec::new();
        for (ip, rec) in map {
            match ip.parse::<IpAddr>() {
                Ok(addr) => out.push((addr, rec)),
                Err(_) => warn!(%ip, "invalid ip in identities"),
            }
        }
        Ok(out)
    }
}

#[cfg(feature = "redis")]
pub mod redis_store {
    use super::*;
    use redis::AsyncCommands;

    #[derive(Clone)]
    pub struct RedisIdentityStore {
        client: redis::Client,
        key: String,
    }

    impl RedisIdentityStore {
        pub fn new(client: redis::Client, key: impl Into<String>) -> Self {
            Self {
                client,
                key: key.into(),
            }
        }
    }

    #[async_trait]
    impl IdentityStore for RedisIdentityStore {
        async fn get(&self, ip: IpAddr) -> anyhow::Result<Option<IdentityRecord>> {
            let mut conn = self.client.get_async_connection().await?;
            let raw: Option<String> = conn.hget(&self.key, ip.to_string()).await?;
            Ok(raw.map(|s| serde_json::from_str(&s).unwrap_or(IdentityRecord {
                nick: String::new(),
                updated: 0,
            })))
        }

        async fn set(&self, ip: IpAddr, nick: String) -> anyhow::Result<()> {
            let mut conn = self.client.get_async_connection().await?;
            let rec = IdentityRecord {
                nick,
                updated: now_ts(),
            };
            let raw = serde_json::to_string(&rec)?;
            let _: () = conn.hset(&self.key, ip.to_string(), raw).await?;
            Ok(())
        }

        async fn remove(&self, ip: IpAddr) -> anyhow::Result<()> {
            let mut conn = self.client.get_async_connection().await?;
            let _: () = conn.hdel(&self.key, ip.to_string()).await?;
            Ok(())
        }

        async fn list(&self) -> anyhow::Result<Vec<(IpAddr, IdentityRecord)>> {
            let mut conn = self.client.get_async_connection().await?;
            let map: BTreeMap<String, String> = conn.hgetall(&self.key).await?;
            let mut out = Vec::new();
            for (ip, raw) in map {
                if let Ok(addr) = ip.parse::<IpAddr>() {
                    if let Ok(rec) = serde_json::from_str::<IdentityRecord>(&raw) {
                        out.push((addr, rec));
                    }
                }
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::str::FromStr;
    use tempfile::tempdir;

    #[tokio::test]
    async fn file_identity_store_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identities.toml");
        let store = FileIdentityStore::new(path);
        let ip = IpAddr::from_str("127.0.0.1").unwrap();
        store.set(ip, "alice".into()).await.unwrap();
        let rec = store.get(ip).await.unwrap().unwrap();
        assert_eq!(rec.nick, "alice");
    }
}
