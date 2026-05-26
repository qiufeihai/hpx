use std::{fs, io::BufReader, path::Path};

use anyhow::{anyhow, Context, Result};
use tokio_rustls::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer},
};

pub fn load_server_config(cert: &Path, key: &Path) -> Result<rustls::ServerConfig> {
    let cert_pem = fs::read(cert).with_context(|| format!("read cert {}", cert.display()))?;
    let key_pem = fs::read(key).with_context(|| format!("read key {}", key.display()))?;

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut BufReader::new(&cert_pem[..]))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse cert")?;

    let key = rustls_pemfile::private_key(&mut BufReader::new(&key_pem[..]))
        .context("parse key")?
        .ok_or_else(|| anyhow!("no private key found"))?;
    let key: PrivateKeyDer<'static> = key;

    let mut cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("build rustls config")?;

    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Ok(cfg)
}

