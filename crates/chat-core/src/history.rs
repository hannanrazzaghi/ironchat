use crate::util::now_ts;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub nick: String,
    pub text: String,
    pub ts: u64,
}

#[async_trait]
pub trait HistoryStore: Send + Sync {
    async fn push(&self, nick: String, text: String) -> anyhow::Result<()>;
    async fn list(&self) -> anyhow::Result<Vec<HistoryItem>>;
}

#[derive(Debug)]
pub struct InMemoryHistory {
    max: usize,
    items: Mutex<VecDeque<HistoryItem>>,
}

impl InMemoryHistory {
    pub fn new(max: usize) -> Self {
        Self {
            max,
            items: Mutex::new(VecDeque::new()),
        }
    }
}

#[async_trait]
impl HistoryStore for InMemoryHistory {
    async fn push(&self, nick: String, text: String) -> anyhow::Result<()> {
        let mut items = self.items.lock().await;
        items.push_back(HistoryItem {
            nick,
            text,
            ts: now_ts(),
        });
        while items.len() > self.max {
            items.pop_front();
        }
        Ok(())
    }

    async fn list(&self) -> anyhow::Result<Vec<HistoryItem>> {
        let items = self.items.lock().await;
        Ok(items.iter().cloned().collect())
    }
}

#[cfg(feature = "redis")]
pub mod redis_history {
    use super::*;
    use redis::AsyncCommands;

    #[derive(Clone)]
    pub struct RedisHistory {
        client: redis::Client,
        key: String,
        max: usize,
    }

    impl RedisHistory {
        pub fn new(client: redis::Client, key: impl Into<String>, max: usize) -> Self {
            Self {
                client,
                key: key.into(),
                max,
            }
        }
    }

    #[async_trait]
    impl HistoryStore for RedisHistory {
        async fn push(&self, nick: String, text: String) -> anyhow::Result<()> {
            let mut conn = self.client.get_async_connection().await?;
            let item = HistoryItem {
                nick,
                text,
                ts: now_ts(),
            };
            let raw = serde_json::to_string(&item)?;
            let _: () = conn.lpush(&self.key, raw).await?;
            let _: () = conn.ltrim(&self.key, 0, (self.max as isize) - 1).await?;
            Ok(())
        }

        async fn list(&self) -> anyhow::Result<Vec<HistoryItem>> {
            let mut conn = self.client.get_async_connection().await?;
            let raws: Vec<String> = conn.lrange(&self.key, 0, -1).await?;
            let mut out = Vec::new();
            for raw in raws {
                if let Ok(item) = serde_json::from_str::<HistoryItem>(&raw) {
                    out.push(item);
                }
            }
            out.reverse();
            Ok(out)
        }
    }
}
