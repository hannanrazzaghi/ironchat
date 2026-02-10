use anyhow::{Context, Result};
use chat_core::protocol::{clean_line, parse_server_line, ClientMsg, ServerMsg, MAX_LINE};
use clap::Parser;
use chrono::Local;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use rustls_pemfile::certs;
use std::fs::File;
use std::io::BufReader;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "chatctl", version, about = "IronChat client")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:5555")]
    connect: String,

    #[arg(long)]
    nick: Option<String>,

    #[arg(long)]
    ca: Option<PathBuf>,

    #[arg(long)]
    insecure: bool,
}

#[derive(Debug)]
struct InsecureVerifier;

impl ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
        ]
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    if cli.insecure {
        eprintln!("WARNING: --insecure disables TLS verification. This is unsafe.");
    }

    let addr = cli
        .connect
        .to_socket_addrs()?
        .next()
        .context("resolve address")?;

    let host = cli
        .connect
        .split(':')
        .next()
        .unwrap_or("localhost")
        .to_string();

    let root = build_root_store(cli.ca.as_ref(), cli.insecure)?;
    let config = if cli.insecure {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureVerifier))
            .with_no_client_auth()
    } else {
        ClientConfig::builder()
            .with_root_certificates(root)
            .with_no_client_auth()
    };

    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(addr).await?;
    let server_name = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        ServerName::IpAddress(ip.into())
    } else {
        ServerName::try_from(host.as_str())
            .context("invalid dns name")?
            .to_owned()
    };
    let tls = connector.connect(server_name, tcp).await?;
    let (reader, mut writer) = tokio::io::split(tls);
    let mut lines = TokioBufReader::new(reader).lines();

    let pending_prompt: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let initial_nick = cli.nick.clone();

    let pending_clone = pending_prompt.clone();
    let reader_task = tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(msg) = parse_server_line(&line) {
                match msg {
                    ServerMsg::Prompt { id, text } => {
                        println!("{}", text);
                        let mut pending = pending_clone.lock().await;
                        *pending = Some(id);
                    }
                    ServerMsg::Msg { nick, text } => {
                        println!("{} {}: {}", ts(), nick, text);
                    }
                    ServerMsg::Hist { nick, text } => {
                        println!("{} {}: {}", ts(), nick, text);
                    }
                    ServerMsg::Who { count, nicks } => {
                        println!("{} online: {}", count, nicks.join(", "));
                    }
                    ServerMsg::Sys { text } => {
                        println!("{} [sys] {}", ts(), text);
                    }
                }
            }
        }
    });

    let pending_clone = pending_prompt.clone();
    let writer_task = tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut input = TokioBufReader::new(stdin).lines();
        let mut used_initial = false;

        loop {
            if let Some(line) = input.next_line().await.ok().flatten() {
                let Some(clean) = clean_line(&line) else { continue; };
                if clean.len() > MAX_LINE {
                    eprintln!("input too long");
                    continue;
                }

                let mut pending = pending_clone.lock().await;
                if let Some(prompt_id) = pending.take() {
                    if let Some(nick) = initial_nick.as_ref() {
                        if !used_initial && prompt_id == "nick" {
                            used_initial = true;
                            let msg = ClientMsg::Prompt {
                                id: prompt_id,
                                answer: nick.clone(),
                            };
                            let line = format_client_msg(msg);
                            if writer.write_all(line.as_bytes()).await.is_err() {
                                break;
                            }
                            if writer.write_all(b"\n").await.is_err() {
                                break;
                            }
                            continue;
                        }
                        if !used_initial && prompt_id == "keep_nick" {
                            let msg = ClientMsg::Prompt {
                                id: prompt_id,
                                answer: "y".into(),
                            };
                            let line = format_client_msg(msg);
                            if writer.write_all(line.as_bytes()).await.is_err() {
                                break;
                            }
                            if writer.write_all(b"\n").await.is_err() {
                                break;
                            }
                            continue;
                        }
                    }

                    let msg = ClientMsg::Prompt {
                        id: prompt_id,
                        answer: clean.clone(),
                    };
                    let line = format_client_msg(msg);
                    if writer.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    if writer.write_all(b"\n").await.is_err() {
                        break;
                    }
                    continue;
                }

                if clean.starts_with('/') {
                    if handle_local_command(&clean, &mut writer).await? {
                        break;
                    }
                    continue;
                }

                let msg = ClientMsg::Say { text: clean };
                let line = format_client_msg(msg);
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if writer.write_all(b"\n").await.is_err() {
                    break;
                }
            } else {
                break;
            }
        }

        Result::<()>::Ok(())
    });

    let _ = tokio::join!(reader_task, writer_task);
    info!("client exited");
    Ok(())
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

fn ts() -> String {
    Local::now().format("%H:%M:%S").to_string()
}

async fn handle_local_command(line: &str, writer: &mut tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>) -> Result<bool> {
    let mut parts = line.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    match cmd {
        "/help" => {
            println!("/help /nick <name> /who /quit");
        }
        "/nick" => {
            let nick = rest.trim();
            if nick.is_empty() {
                eprintln!("usage: /nick <name>");
            } else {
                let msg = ClientMsg::Nick {
                    nick: nick.to_string(),
                };
                let line = format_client_msg(msg);
                writer.write_all(line.as_bytes()).await?;
                writer.write_all(b"\n").await?;
            }
        }
        "/who" => {
            let line = format_client_msg(ClientMsg::Who);
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
        }
        "/quit" => {
            let line = format_client_msg(ClientMsg::Quit);
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            return Ok(true);
        }
        _ => {
            eprintln!("unknown command, try /help");
        }
    }
    Ok(false)
}

fn build_root_store(ca: Option<&PathBuf>, insecure: bool) -> Result<RootCertStore> {
    let mut root = RootCertStore::empty();
    if !insecure {
        let native = rustls_native_certs::load_native_certs().context("load native certs")?;
        for cert in native {
            root.add(cert).ok();
        }
    }

    if let Some(path) = ca {
        let mut reader = BufReader::new(File::open(path).context("open ca file")?);
        let certs = certs(&mut reader)
            .collect::<Result<Vec<_>, _>>()
            .context("read ca certs")?;
        for cert in certs {
            root.add(cert).context("add cert")?;
        }
    }

    Ok(root)
}
