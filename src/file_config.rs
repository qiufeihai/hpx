use std::{fs, net::SocketAddr, path::Path};

use anyhow::{anyhow, Context, Result};

#[derive(Default, Debug, Clone)]
pub struct FileConfig {
    pub listen: Option<SocketAddr>,
    pub cert: Option<String>,
    pub key: Option<String>,
    pub hosts: Vec<String>,
    pub path: Option<String>,
    pub uuids: Vec<String>,
    pub connect_timeout_ms: Option<u64>,
    pub idle_timeout_s: Option<u64>,
    pub sub_path: Option<String>,
    pub sub_token: Option<String>,
    pub public_host: Option<String>,
    pub public_port: Option<u16>,
}

pub fn load(path: &Path) -> Result<FileConfig> {
    let content = fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    parse_str(&content)
}

fn parse_str(s: &str) -> Result<FileConfig> {
    let mut cfg = FileConfig::default();

    for (idx, raw) in s.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid line {}: expected KEY=VALUE", idx + 1))?;

        let key = k.trim().to_ascii_lowercase();
        let mut val = v.trim();
        if let Some(stripped) = val.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
            val = stripped;
        }
        if let Some(stripped) = val.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
            val = stripped;
        }

        match key.as_str() {
            "listen" => cfg.listen = Some(val.parse().context("parse listen")?),
            "cert" => cfg.cert = Some(val.to_string()),
            "key" => cfg.key = Some(val.to_string()),
            "host" | "hosts" => cfg.hosts.extend(split_list(val)),
            "path" => cfg.path = Some(val.to_string()),
            "uuid" | "uuids" => cfg.uuids.extend(split_list(val)),
            "connect_timeout_ms" => cfg.connect_timeout_ms = Some(val.parse().context("parse connect_timeout_ms")?),
            "idle_timeout_s" => cfg.idle_timeout_s = Some(val.parse().context("parse idle_timeout_s")?),
            "sub_path" => cfg.sub_path = Some(val.to_string()),
            "sub_token" => cfg.sub_token = Some(val.to_string()),
            "public_host" => cfg.public_host = Some(val.to_string()),
            "public_port" => cfg.public_port = Some(val.parse().context("parse public_port")?),
            _ => return Err(anyhow!("unknown key '{}' at line {}", key, idx + 1)),
        }
    }

    Ok(cfg)
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect()
}
