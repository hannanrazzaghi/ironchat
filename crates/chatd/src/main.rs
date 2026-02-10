use anyhow::{Context, Result};
use chat_core::allowlist::AllowlistFiles;
use chat_core::history::{HistoryStore, InMemoryHistory};
use chat_core::identities::{FileIdentityStore, IdentityStore};
use chat_core::protocol::{clean_line, format_server_msg, parse_client_line, ClientMsg, ServerMsg};
use chat_core::MAX_NICK;
use clap::{Parser, Subcommand};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tracing::{error, info, warn};

mod state;
mod tls;

use state::{ClientId, HubState};

#[derive(Parser, Debug)]
#[command(name = "chatd", version, about = "IronChat server daemon")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(long, default_value = "0.0.0.0:5555")]
    bind: String,

    #[arg(long)]
    cert: Option<PathBuf>,

    #[arg(long)]
    key: Option<PathBuf>,

    #[arg(long)]
    motd: Option<String>,

    #[arg(long, default_value = "./allowed.toml")]
    allowlist: PathBuf,

    #[arg(long, default_value = "./pending.toml")]
    pending: PathBuf,

    #[arg(long, default_value = "./identities.toml")]
    identities: PathBuf,

    #[arg(long)]
    redis: Option<String>,

    #[arg(long, default_value_t = 20)]
    ip_rate: u32,

    #[arg(long, default_value_t = 5)]
    conn_rate: u32,

    #[arg(long)]
    idle_timeout: Option<u64>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Allow {
        #[command(subcommand)]
        command: AllowCommands,
    },
    Pending {
        #[command(subcommand)]
        command: PendingCommands,
    },
}

#[derive(Subcommand, Debug)]
enum AllowCommands {
    Add { entry: String },
    Remove { entry: String },
    List,
}

#[derive(Subcommand, Debug)]
enum PendingCommands {
    List,
    Remove { ip: String },
    Clear,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    if let Some(command) = &cli.command {
        return handle_admin(command, &cli).await;
    }

    let cert = cli.cert.context("--cert is required")?;
    let key = cli.key.context("--key is required")?;

    let tls_config = tls::load_server_config(&cert, &key)?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = TcpListener::bind(&cli.bind).await?;
    info!(bind = %cli.bind, "chatd listening");

    let allow_files = Arc::new(AllowlistFiles {
        allowlist: cli.allowlist.clone(),
        pending: cli.pending.clone(),
    });

    let identities: Arc<dyn IdentityStore> = if let Some(url) = cli.redis.clone() {
        #[cfg(feature = "redis")]
        {
            let client = redis::Client::open(url)?;
            let store = chat_core::identities::redis_store::RedisIdentityStore::new(client, "ironchat:identities");
            Arc::new(store)
        }
        #[cfg(not(feature = "redis"))]
        {
            let _ = url;
            warn!("redis feature not enabled, using file identities");
            Arc::new(FileIdentityStore::new(cli.identities.clone()))
        }
    } else {
        Arc::new(FileIdentityStore::new(cli.identities.clone()))
    };

    let history: Arc<dyn HistoryStore> = if let Some(url) = cli.redis.clone() {
        #[cfg(feature = "redis")]
        {
            let client = redis::Client::open(url)?;
            let store = chat_core::history::redis_history::RedisHistory::new(client, "ironchat:history", 100);
            Arc::new(store)
        }
        #[cfg(not(feature = "redis"))]
        {
            let _ = url;
            Arc::new(InMemoryHistory::new(100))
        }
    } else {
        Arc::new(InMemoryHistory::new(100))
    };

    let hub = Arc::new(tokio::sync::Mutex::new(HubState::new(
        cli.conn_rate,
        cli.ip_rate,
    )));

    loop {
        let (stream, addr) = listener.accept().await?;
        let ip = addr.ip();
        let allow = allow_files.check_or_note(ip)?;
        if !allow {
            tokio::spawn(deny_unapproved(stream, acceptor.clone()));
            continue;
        }

        let acceptor = acceptor.clone();
        let hub = hub.clone();
        let history = history.clone();
        let identities = identities.clone();
        let motd = cli.motd.clone();
        let idle = cli.idle_timeout;

        tokio::spawn(async move {
            if let Err(err) = handle_client(
                stream,
                ip,
                acceptor,
                hub,
                history,
                identities,
                motd,
                idle,
            )
            .await
            {
                error!(%err, "client error");
            }
        });
    }
}

async fn handle_admin(command: &Commands, cli: &Cli) -> Result<()> {
    let files = AllowlistFiles {
        allowlist: cli.allowlist.clone(),
        pending: cli.pending.clone(),
    };
    match command {
        Commands::Allow { command } => match command {
            AllowCommands::Add { entry } => {
                files.add_allow(&entry)?;
                println!("added {entry}");
            }
            AllowCommands::Remove { entry } => {
                files.remove_allow(&entry)?;
                println!("removed {entry}");
            }
            AllowCommands::List => {
                for entry in files.list_allow()? {
                    println!("{entry}");
                }
            }
        },
        Commands::Pending { command } => match command {
            PendingCommands::List => {
                for (ip, entry) in files.list_pending()? {
                    println!("{ip} attempts={} last_seen={}", entry.attempts, entry.last_seen);
                }
            }
            PendingCommands::Remove { ip } => {
                files.remove_pending(&ip)?;
                println!("removed {ip}");
            }
            PendingCommands::Clear => {
                files.clear_pending()?;
                println!("cleared pending list");
            }
        },
    }
    Ok(())
}

async fn deny_unapproved(stream: TcpStream, acceptor: TlsAcceptor) {
    if let Ok(mut tls) = acceptor.accept(stream).await {
        let _ = tls
            .write_all(format_server_msg(&ServerMsg::Sys {
                text: "Not approved. Ask admin.".into(),
            })
            .as_bytes())
            .await;
        let _ = tls.write_all(b"\n").await;
    }
}

async fn handle_client(
    stream: TcpStream,
    ip: IpAddr,
    acceptor: TlsAcceptor,
    hub: Arc<tokio::sync::Mutex<HubState>>,
    history: Arc<dyn HistoryStore>,
    identities: Arc<dyn IdentityStore>,
    motd: Option<String>,
    idle_timeout: Option<u64>,
) -> Result<()> {
    let tls = acceptor.accept(stream).await?;
    let (reader, mut writer) = tokio::io::split(tls);
    let mut lines = BufReader::new(reader).lines();

    let (tx, mut rx) = mpsc::channel::<ServerMsg>(64);

    let writer_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let line = format_server_msg(&msg);
            if writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if writer.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    if let Some(m) = motd {
        let _ = tx.send(ServerMsg::Sys { text: m }).await;
    }

    let mut nick = init_identity(&tx, &mut lines, ip, &hub, identities.clone()).await?;

    let mut state = hub.lock().await;
    let client_id = state.add_client(nick.clone(), ip, tx.clone());
    drop(state);
    info!(%ip, nick = %nick, "client joined");

    let hist = history.list().await?;
    for item in hist {
        let _ = tx
            .send(ServerMsg::Hist {
                nick: item.nick,
                text: item.text,
            })
            .await;
    }

    broadcast_sys(&hub, &format!("{nick} joined"));

    let idle_duration = idle_timeout.map(Duration::from_secs);

    loop {
        let next_line = if let Some(idle) = idle_duration {
            tokio::time::timeout(idle, lines.next_line()).await
        } else {
            Ok(lines.next_line().await)
        };

        let line = match next_line {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => break,
            Ok(Err(err)) => {
                warn!(%err, "read error");
                break;
            }
            Err(_) => {
                warn!(%ip, "idle timeout");
                break;
            }
        };

        let Some(clean) = clean_line(&line) else { continue; };

        let msg = match parse_client_line(&clean) {
            Ok(m) => m,
            Err(_) => {
                let _ = tx.send(ServerMsg::Sys { text: "invalid command".into() }).await;
                continue;
            }
        };

        let mut state = hub.lock().await;
        let conn_ok = state.conn_rate_ok(client_id);
        let ip_ok = state.ip_rate_ok(ip);
        if !conn_ok || !ip_ok {
            let mut should_disconnect = false;
            if !conn_ok {
                if state.conn_warned(client_id) {
                    should_disconnect = true;
                } else {
                    state.mark_conn_warned(client_id);
                }
            }
            if !ip_ok {
                if state.ip_warned(ip) {
                    should_disconnect = true;
                } else {
                    state.mark_ip_warned(ip);
                }
            }
            drop(state);
            if should_disconnect {
                warn!(%ip, nick = %nick, "rate limit disconnect");
                break;
            }
            let _ = tx
                .send(ServerMsg::Sys {
                    text: "rate limit exceeded".into(),
                })
                .await;
            continue;
        }
        drop(state);

        match msg {
            ClientMsg::Nick { nick: new } => {
                if new.len() > MAX_NICK {
                    let _ = tx
                        .send(ServerMsg::Sys {
                            text: "nickname too long".into(),
                        })
                        .await;
                } else {
                    let mut state = hub.lock().await;
                    let taken = state.nicks.contains(&new.to_lowercase());
                    if taken {
                        drop(state);
                        let _ = tx
                            .send(ServerMsg::Sys {
                                text: "nickname already taken".into(),
                            })
                            .await;
                    } else {
                        let old = nick.clone();
                        if let Err(err) = state.rename(client_id, new.clone()) {
                            drop(state);
                            let _ = tx
                                .send(ServerMsg::Sys { text: err })
                                .await;
                            continue;
                        }
                        drop(state);
                        let _ = identities.set(ip, new.clone()).await;
                        nick = new.clone();
                        info!(%ip, nick = %nick, "nickname changed");
                        broadcast_sys(&hub, &format!("{old} is now {new}"));
                    }
                }
            }
            ClientMsg::Say { text } => {
                history.push(nick.clone(), text.clone()).await?;
                let msg = ServerMsg::Msg {
                    nick: nick.clone(),
                    text,
                };
                let mut state = hub.lock().await;
                let drop_ids = state.broadcast_with_disconnects(&msg);
                drop(state);
                for id in drop_ids {
                    disconnect_client(&hub, id, "slow consumer").await;
                }
                continue;
            }
            ClientMsg::Who => {
                let state = hub.lock().await;
                let nicks = state.list_nicks();
                let _ = tx
                    .send(ServerMsg::Who {
                        count: nicks.len(),
                        nicks,
                    })
                    .await;
            }
            ClientMsg::Quit => {
                break;
            }
            ClientMsg::Prompt { .. } => {
                let _ = tx
                    .send(ServerMsg::Sys {
                        text: "unexpected prompt".into(),
                    })
                    .await;
            }
        }
    }

    writer_task.abort();
    disconnect_client(&hub, client_id, "client left").await;

    Ok(())
}

async fn init_identity(
    tx: &mpsc::Sender<ServerMsg>,
    lines: &mut tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio_rustls::server::TlsStream<TcpStream>>>>,
    ip: IpAddr,
    hub: &Arc<tokio::sync::Mutex<HubState>>,
    identities: Arc<dyn IdentityStore>,
) -> Result<String> {
    if let Some(record) = identities.get(ip).await? {
        let prompt_id = "keep_nick".to_string();
        let _ = tx
            .send(ServerMsg::Prompt {
                id: prompt_id.clone(),
                text: format!("Your nickname is {}. Change it? (y/N)", record.nick),
            })
            .await;
        if let Some(answer) = read_prompt(lines, &prompt_id).await? {
            if answer.to_lowercase().starts_with('y') {
                return prompt_for_nick(tx, lines, hub, identities, ip).await;
            }
            let state = hub.lock().await;
            if state.nicks.contains(&record.nick.to_lowercase()) {
                drop(state);
                let _ = tx
                    .send(ServerMsg::Sys {
                        text: "nickname already taken".into(),
                    })
                    .await;
                return prompt_for_nick(tx, lines, hub, identities, ip).await;
            }
            return Ok(record.nick);
        }
    }
    prompt_for_nick(tx, lines, hub, identities, ip).await
}

async fn prompt_for_nick(
    tx: &mpsc::Sender<ServerMsg>,
    lines: &mut tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio_rustls::server::TlsStream<TcpStream>>>>,
    hub: &Arc<tokio::sync::Mutex<HubState>>,
    identities: Arc<dyn IdentityStore>,
    ip: IpAddr,
) -> Result<String> {
    loop {
        let prompt_id = "nick".to_string();
        let _ = tx
            .send(ServerMsg::Prompt {
                id: prompt_id.clone(),
                text: "Choose nickname".into(),
            })
            .await;
        if let Some(answer) = read_prompt(lines, &prompt_id).await? {
            let nick = answer.trim().to_string();
            if nick.is_empty() || nick.len() > MAX_NICK {
                let _ = tx
                    .send(ServerMsg::Sys {
                        text: "invalid nickname".into(),
                    })
                    .await;
                continue;
            }
            let state = hub.lock().await;
            if state.nicks.contains(&nick.to_lowercase()) {
                drop(state);
                let _ = tx
                    .send(ServerMsg::Sys {
                        text: "nickname already taken".into(),
                    })
                    .await;
                continue;
            }
            drop(state);
            identities.set(ip, nick.clone()).await?;
            return Ok(nick);
        }
    }
}

async fn read_prompt(
    lines: &mut tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio_rustls::server::TlsStream<TcpStream>>>>,
    prompt_id: &str,
) -> Result<Option<String>> {
    while let Some(line) = lines.next_line().await? {
        let Some(clean) = clean_line(&line) else { continue; };
        if let Ok(ClientMsg::Prompt { id, answer }) = parse_client_line(&clean) {
            if id == prompt_id {
                return Ok(Some(answer));
            }
        }
    }
    Ok(None)
}

fn broadcast_sys(hub: &Arc<tokio::sync::Mutex<HubState>>, text: &str) {
    let hub = hub.clone();
    let text = text.to_string();
    tokio::spawn(async move {
        let state = hub.lock().await;
        state.broadcast(&ServerMsg::Sys { text });
    });
}

async fn disconnect_client(hub: &Arc<tokio::sync::Mutex<HubState>>, id: ClientId, reason: &str) {
    let mut state = hub.lock().await;
    if let Some(handle) = state.remove_client(id) {
        info!(ip = %handle.ip, nick = %handle.nick, "client left");
        state.broadcast(&ServerMsg::Sys {
            text: format!("{} left ({})", handle.nick, reason),
        });
    }
}
