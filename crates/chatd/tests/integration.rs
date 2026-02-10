use anyhow::{Context, Result};
use chat_core::protocol::{parse_server_line, ClientMsg, ServerMsg};
use rcgen::{CertificateParams, DistinguishedName, DnType, SanType};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use rustls_pemfile::certs;
use std::io::Cursor;
use std::net::IpAddr;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use std::sync::Arc;

struct TestServer {
    child: Child,
    port: u16,
    _dir: tempfile::TempDir,
    ca_cert: Vec<u8>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[tokio::test]
async fn broadcast_and_who() -> Result<()> {
    let server = start_server(5, 20).await?;

    let mut a = connect_client(server.port, &server.ca_cert).await?;
    ensure_nick(&mut a, "alice").await?;

    let mut b = connect_client(server.port, &server.ca_cert).await?;
    ensure_nick(&mut b, "bob").await?;
    wait_for_who(&mut b, 2).await?;

    a.send(ClientMsg::Say { text: "hello".into() }).await?;

    let msg = read_until(&mut b, |msg| matches!(msg, ServerMsg::Msg { .. })).await?;
    match msg {
        ServerMsg::Msg { nick, text } => {
            assert_eq!(nick, "alice");
            assert_eq!(text, "hello");
        }
        _ => unreachable!(),
    }

    b.send(ClientMsg::Who).await?;
    let who = read_until(&mut b, |msg| matches!(msg, ServerMsg::Who { .. })).await?;
    match who {
        ServerMsg::Who { count, nicks } => {
            assert_eq!(count, 2);
            assert!(nicks.contains(&"alice".to_string()));
            assert!(nicks.contains(&"bob".to_string()));
        }
        _ => unreachable!(),
    }

    Ok(())
}

#[tokio::test]
async fn nickname_uniqueness() -> Result<()> {
    let server = start_server(5, 20).await?;

    let mut a = connect_client(server.port, &server.ca_cert).await?;
    ensure_nick(&mut a, "alice").await?;
    wait_for_who(&mut a, 1).await?;

    let mut b = connect_client(server.port, &server.ca_cert).await?;
    ensure_nick(&mut b, "alice").await?;

    let sys = read_until(&mut b, |msg| matches!(msg, ServerMsg::Sys { .. })).await?;
    match sys {
        ServerMsg::Sys { text } => assert!(text.contains("nickname already taken")),
        _ => unreachable!(),
    }

    Ok(())
}

#[tokio::test]
async fn reconnect_prompts_for_saved_nick() -> Result<()> {
    let server = start_server(5, 20).await?;

    let mut a = connect_client(server.port, &server.ca_cert).await?;
    ensure_nick(&mut a, "alice").await?;
    wait_for_who(&mut a, 1).await?;
    a.send(ClientMsg::Quit).await?;

    let mut b = connect_client(server.port, &server.ca_cert).await?;
    let (id, prompt) = expect_prompt(&mut b).await?;
    assert_eq!(id, "keep_nick");
    assert!(prompt.contains("Your nickname is alice"));

    Ok(())
}

#[tokio::test]
async fn rate_limit_disconnects() -> Result<()> {
    let server = start_server(1, 1).await?;

    let mut a = connect_client(server.port, &server.ca_cert).await?;
    ensure_nick(&mut a, "alice").await?;
    wait_for_who(&mut a, 1).await?;

    a.send(ClientMsg::Say { text: "spam".into() }).await?;

    let sys_or_closed = read_until_allow_close(&mut a, |msg| {
        matches!(msg, ServerMsg::Sys { text } if text.contains("rate limit exceeded"))
    })
    .await?;
    if let Some(ServerMsg::Sys { text }) = sys_or_closed {
        assert!(text.contains("rate limit exceeded"));
    }

    Ok(())
}

async fn start_server(conn_rate: u32, ip_rate: u32) -> Result<TestServer> {
    let dir = tempdir()?;
    let port = pick_port()?;

    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    let allow_path = dir.path().join("allowed.toml");
    let pending_path = dir.path().join("pending.toml");
    let identities_path = dir.path().join("identities.toml");

    let (cert_pem, key_pem) = generate_cert()?;
    std::fs::write(&cert_path, &cert_pem)?;
    std::fs::write(&key_path, &key_pem)?;

    std::fs::write(&allow_path, "allow = [\"127.0.0.1\"]\n")?;

    let child = Command::new(env!("CARGO_BIN_EXE_chatd"))
        .arg("--bind")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--cert")
        .arg(&cert_path)
        .arg("--key")
        .arg(&key_path)
        .arg("--allowlist")
        .arg(&allow_path)
        .arg("--pending")
        .arg(&pending_path)
        .arg("--identities")
        .arg(&identities_path)
        .arg("--conn-rate")
        .arg(conn_rate.to_string())
        .arg("--ip-rate")
        .arg(ip_rate.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn chatd")?;

    wait_for_port(port).await?;

    Ok(TestServer {
        child,
        port,
        _dir: dir,
        ca_cert: cert_pem,
    })
}

fn generate_cert() -> Result<(Vec<u8>, Vec<u8>)> {
    let mut params = CertificateParams::new(vec!["localhost".into()]);
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "ironchat");
    params.subject_alt_names.push(SanType::IpAddress(IpAddr::from([127, 0, 0, 1])));
    let cert = rcgen::Certificate::from_params(params)?;
    let cert_pem = cert.serialize_pem()?;
    let key_pem = cert.serialize_private_key_pem();
    Ok((cert_pem.into_bytes(), key_pem.into_bytes()))
}

async fn connect_client(port: u16, ca_cert: &[u8]) -> Result<TestClient> {
    let mut root = RootCertStore::empty();
    let mut cursor = Cursor::new(ca_cert);
    let certs = certs(&mut cursor).collect::<Result<Vec<_>, _>>()?;
    for cert in certs {
        root.add(CertificateDer::from(cert))?;
    }
    let config = ClientConfig::builder()
        .with_root_certificates(root)
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(format!("127.0.0.1:{port}")).await?;
    let server_name = ServerName::try_from("localhost").context("server name")?;
    let tls = connector.connect(server_name, tcp).await?;
    let (reader, writer) = tokio::io::split(tls);
    Ok(TestClient {
        reader: BufReader::new(reader).lines(),
        writer,
    })
}

struct TestClient {
    reader: tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>>>,
    writer: tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
}

impl TestClient {
    async fn send(&mut self, msg: ClientMsg) -> Result<()> {
        let line = format_client_msg(msg);
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        Ok(())
    }

    async fn send_prompt(&mut self, id: &str, answer: &str) -> Result<()> {
        self.send(ClientMsg::Prompt {
            id: id.into(),
            answer: answer.into(),
        })
        .await
    }
}

fn format_client_msg(msg: ClientMsg) -> String {
    match msg {
        ClientMsg::Nick { nick } => format!("NICK {}", nick),
        ClientMsg::Say { text } => format!("SAY {}", text),
        ClientMsg::Who => "WHO".into(),
        ClientMsg::Quit => "QUIT".into(),
        ClientMsg::Prompt { id, answer } => format!("PROMPT {} {}", id, answer),
    }
}

async fn expect_prompt(client: &mut TestClient) -> Result<(String, String)> {
    let msg = read_until(client, |msg| matches!(msg, ServerMsg::Prompt { .. })).await?;
    match msg {
        ServerMsg::Prompt { id, text } => Ok((id, text)),
        _ => unreachable!(),
    }
}

async fn wait_for_who(client: &mut TestClient, expected: usize) -> Result<()> {
    client.send(ClientMsg::Who).await?;
    let msg = read_until(client, |msg| matches!(msg, ServerMsg::Who { .. })).await?;
    match msg {
        ServerMsg::Who { count, .. } => {
            if count >= expected {
                Ok(())
            } else {
                anyhow::bail!("unexpected who count")
            }
        }
        _ => unreachable!(),
    }
}

async fn ensure_nick(client: &mut TestClient, desired: &str) -> Result<()> {
    let (id, _text) = expect_prompt(client).await?;
    if id == "keep_nick" {
        client.send_prompt("keep_nick", "y").await?;
        let (id2, _text2) = expect_prompt(client).await?;
        if id2 != "nick" {
            anyhow::bail!("expected nick prompt");
        }
        client.send_prompt("nick", desired).await?;
        return Ok(());
    }
    if id != "nick" {
        anyhow::bail!("expected nick prompt");
    }
    client.send_prompt("nick", desired).await?;
    Ok(())
}

async fn read_until<F>(client: &mut TestClient, pred: F) -> Result<ServerMsg>
where
    F: Fn(&ServerMsg) -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            anyhow::bail!("timeout waiting for server msg");
        }
        match tokio::time::timeout(Duration::from_millis(200), client.reader.next_line()).await {
            Ok(Ok(Some(line))) => {
                if let Ok(msg) = parse_server_line(&line) {
                    if pred(&msg) {
                        return Ok(msg);
                    }
                }
            }
            Ok(Ok(None)) => anyhow::bail!("connection closed"),
            _ => {}
        }
    }
}

async fn read_until_allow_close<F>(client: &mut TestClient, pred: F) -> Result<Option<ServerMsg>>
where
    F: Fn(&ServerMsg) -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            anyhow::bail!("timeout waiting for server msg");
        }
        match tokio::time::timeout(Duration::from_millis(200), client.reader.next_line()).await {
            Ok(Ok(Some(line))) => {
                if let Ok(msg) = parse_server_line(&line) {
                    if pred(&msg) {
                        return Ok(Some(msg));
                    }
                }
            }
            Ok(Ok(None)) => return Ok(None),
            _ => {}
        }
    }
}

fn pick_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok(port)
}

async fn wait_for_port(port: u16) -> Result<()> {
    let addr = format!("127.0.0.1:{port}");
    for _ in 0..20 {
        if TcpStream::connect(&addr).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("server did not start");
}
