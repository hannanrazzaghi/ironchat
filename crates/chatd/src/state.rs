use chat_core::rate::RateLimiter;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::warn;

use chat_core::protocol::ServerMsg;

pub type ClientId = u64;

#[derive(Clone, Debug)]
pub struct ClientHandle {
    pub nick: String,
    pub ip: IpAddr,
    pub tx: mpsc::Sender<ServerMsg>,
}

#[derive(Debug)]
pub struct IpRate {
    limiter: RateLimiter,
    warned: bool,
}

impl IpRate {
    pub fn new(limit: u32) -> Self {
        Self {
            limiter: RateLimiter::new(limit, Duration::from_secs(1)),
            warned: false,
        }
    }

    pub fn check(&mut self) -> bool {
        let ok = self.limiter.check();
        if ok {
            self.warned = false;
        }
        ok
    }
}

#[derive(Debug)]
pub struct HubState {
    pub clients: HashMap<ClientId, ClientHandle>,
    pub nicks: HashSet<String>,
    pub next_id: ClientId,
    pub ip_rates: HashMap<IpAddr, IpRate>,
    pub conn_rates: HashMap<ClientId, (RateLimiter, bool)>,
    pub conn_limit: u32,
    pub ip_limit: u32,
}

impl HubState {
    pub fn new(conn_limit: u32, ip_limit: u32) -> Self {
        Self {
            clients: HashMap::new(),
            nicks: HashSet::new(),
            next_id: 1,
            ip_rates: HashMap::new(),
            conn_rates: HashMap::new(),
            conn_limit,
            ip_limit,
        }
    }

    pub fn add_client(&mut self, nick: String, ip: IpAddr, tx: mpsc::Sender<ServerMsg>) -> ClientId {
        let id = self.next_id;
        self.next_id += 1;
        self.nicks.insert(nick.to_lowercase());
        self.clients.insert(id, ClientHandle { nick, ip, tx });
        self.conn_rates.insert(
            id,
            (RateLimiter::new(self.conn_limit, Duration::from_secs(1)), false),
        );
        let ip_limit = self.ip_limit;
        self.ip_rates.entry(ip).or_insert_with(|| IpRate::new(ip_limit));
        id
    }

    pub fn remove_client(&mut self, id: ClientId) -> Option<ClientHandle> {
        if let Some(handle) = self.clients.remove(&id) {
            self.nicks.remove(&handle.nick.to_lowercase());
            self.conn_rates.remove(&id);
            return Some(handle);
        }
        None
    }

    pub fn rename(&mut self, id: ClientId, new_nick: String) -> Result<(), String> {
        let norm = new_nick.to_lowercase();
        if let Some(handle) = self.clients.get(&id) {
            if handle.nick.eq_ignore_ascii_case(&new_nick) {
                return Ok(());
            }
        }
        if self.nicks.contains(&norm) {
            return Err("nickname already taken".into());
        }
        if let Some(handle) = self.clients.get_mut(&id) {
            self.nicks.remove(&handle.nick.to_lowercase());
            handle.nick = new_nick.clone();
            self.nicks.insert(norm);
            Ok(())
        } else {
            Err("unknown client".into())
        }
    }

    pub fn list_nicks(&self) -> Vec<String> {
        self.clients.values().map(|c| c.nick.clone()).collect()
    }

    pub fn conn_rate_ok(&mut self, id: ClientId) -> bool {
        let Some((limiter, warned)) = self.conn_rates.get_mut(&id) else {
            return true;
        };
        let ok = limiter.check();
        if ok {
            *warned = false;
        }
        ok
    }

    pub fn conn_warned(&mut self, id: ClientId) -> bool {
        let Some((_, warned)) = self.conn_rates.get(&id) else {
            return false;
        };
        *warned
    }

    pub fn mark_conn_warned(&mut self, id: ClientId) {
        if let Some((_, warned)) = self.conn_rates.get_mut(&id) {
            *warned = true;
        }
    }

    pub fn ip_rate_ok(&mut self, ip: IpAddr) -> bool {
        let ip_limit = self.ip_limit;
        let entry = self.ip_rates.entry(ip).or_insert_with(|| IpRate::new(ip_limit));
        entry.check()
    }

    pub fn ip_warned(&mut self, ip: IpAddr) -> bool {
        self.ip_rates.get(&ip).map(|r| r.warned).unwrap_or(false)
    }

    pub fn mark_ip_warned(&mut self, ip: IpAddr) {
        if let Some(r) = self.ip_rates.get_mut(&ip) {
            r.warned = true;
        }
    }

    pub fn broadcast(&self, msg: &ServerMsg) {
        for (id, handle) in &self.clients {
            if handle.tx.try_send(msg.clone()).is_err() {
                warn!(client_id = *id, nick = %handle.nick, "client queue full, dropping");
            }
        }
    }

    pub fn broadcast_with_disconnects(&mut self, msg: &ServerMsg) -> Vec<ClientId> {
        let mut drop = VecDeque::new();
        for (id, handle) in &self.clients {
            if handle.tx.try_send(msg.clone()).is_err() {
                drop.push_back(*id);
            }
        }
        drop.into_iter().collect()
    }

}
