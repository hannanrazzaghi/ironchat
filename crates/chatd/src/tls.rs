use anyhow::Context;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use rustls_pemfile::{certs, private_key};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub fn load_server_config(cert_path: &Path, key_path: &Path) -> anyhow::Result<ServerConfig> {
    let mut cert_reader = BufReader::new(File::open(cert_path).context("open cert")?);
    let mut key_reader = BufReader::new(File::open(key_path).context("open key")?);

    let cert_chain: Vec<CertificateDer<'static>> = certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .context("read certs")?;

    let key: PrivateKeyDer<'static> =
        private_key(&mut key_reader).context("read private key")?
            .context("no private key found")?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .context("build tls config")?;

    Ok(config)
}
