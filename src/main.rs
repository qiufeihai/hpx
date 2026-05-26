use std::{
    collections::HashSet,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use bytes::{Bytes, BytesMut};
use clap::Parser;
use futures_util::future::poll_fn;
use h2::server::{self, SendResponse};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinSet,
    time::timeout,
};
use tokio_rustls::TlsAcceptor;
use tracing::{error, info, warn};

static STREAM_SEQ: AtomicU64 = AtomicU64::new(1);

mod file_config;
mod tls;
mod vless;

#[derive(Clone)]
struct Config {
    hosts: HashSet<String>,
    path: String,
    users: HashSet<[u8; 16]>,
    connect_timeout: Duration,
    idle_timeout: Duration,
    sub_path: Option<String>,
    sub_token: Option<String>,
    public_host: String,
    public_port: u16,
}

#[derive(Parser)]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,

    #[arg(long, default_value = "0.0.0.0:443")]
    listen: SocketAddr,

    #[arg(long)]
    cert: Option<PathBuf>,

    #[arg(long)]
    key: Option<PathBuf>,

    #[arg(long = "host")]
    hosts: Vec<String>,

    #[arg(long, default_value = "/")]
    path: String,

    #[arg(long = "uuid")]
    uuids: Vec<String>,

    #[arg(long, default_value_t = 5000)]
    connect_timeout_ms: u64,

    #[arg(long, default_value_t = 300)]
    idle_timeout_s: u64,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .without_time()
        .init();

    let args = Args::parse();

    let file_cfg = if let Some(p) = args.config.as_deref() {
        Some(file_config::load(p)?)
    } else {
        None
    };

    let listen = file_cfg
        .as_ref()
        .and_then(|c| c.listen)
        .unwrap_or(args.listen);

    let cert = args
        .cert
        .clone()
        .or_else(|| file_cfg.as_ref().and_then(|c| c.cert.as_ref().map(PathBuf::from)))
        .context("missing --cert (or cert= in config file)")?;
    let key = args
        .key
        .clone()
        .or_else(|| file_cfg.as_ref().and_then(|c| c.key.as_ref().map(PathBuf::from)))
        .context("missing --key (or key= in config file)")?;

    let mut hosts = HashSet::new();
    let mut primary_host = None;
    for h in file_cfg
        .as_ref()
        .map(|c| c.hosts.as_slice())
        .unwrap_or(&[])
        .iter()
        .chain(args.hosts.iter())
    {
        if primary_host.is_none() && !h.is_empty() {
            primary_host = Some(h.to_string());
        }
        hosts.insert(h.to_ascii_lowercase());
    }
    if hosts.is_empty() {
        return Err(anyhow!("missing --host (or host= in config file)"));
    }

    let path = normalize_path(
        file_cfg
            .as_ref()
            .and_then(|c| c.path.as_deref())
            .unwrap_or(&args.path),
    );

    let mut users = HashSet::new();
    for u in file_cfg
        .as_ref()
        .map(|c| c.uuids.as_slice())
        .unwrap_or(&[])
        .iter()
        .chain(args.uuids.iter())
    {
        let id = uuid::Uuid::parse_str(u).with_context(|| format!("invalid uuid: {u}"))?;
        users.insert(id.into_bytes());
    }
    if users.is_empty() {
        return Err(anyhow!("missing --uuid (or uuid= in config file)"));
    }

    let connect_timeout_ms = file_cfg
        .as_ref()
        .and_then(|c| c.connect_timeout_ms)
        .unwrap_or(args.connect_timeout_ms);
    let idle_timeout_s = file_cfg
        .as_ref()
        .and_then(|c| c.idle_timeout_s)
        .unwrap_or(args.idle_timeout_s);

    let sub_path = file_cfg
        .as_ref()
        .and_then(|c| c.sub_path.as_deref())
        .map(normalize_path);
    let sub_token = file_cfg.as_ref().and_then(|c| c.sub_token.clone());

    let public_host = file_cfg
        .as_ref()
        .and_then(|c| c.public_host.as_deref())
        .map(|s| s.to_string())
        .or(primary_host)
        .unwrap();
    let public_port = file_cfg
        .as_ref()
        .and_then(|c| c.public_port)
        .unwrap_or(listen.port());

    let cfg = Arc::new(Config {
        hosts,
        path,
        users,
        connect_timeout: Duration::from_millis(connect_timeout_ms),
        idle_timeout: Duration::from_secs(idle_timeout_s),
        sub_path,
        sub_token,
        public_host,
        public_port,
    });

    let tls_cfg = tls::load_server_config(&cert, &key)?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_cfg));

    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("bind {}", listen))?;

    info!("listening on {}", listen);

    loop {
        let (tcp, peer) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let cfg = cfg.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_tcp_conn(cfg, acceptor, tcp, peer).await {
                warn!(%peer, "conn error: {e:#}");
            }
        });
    }
}

fn normalize_path(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

async fn handle_tcp_conn(
    cfg: Arc<Config>,
    acceptor: TlsAcceptor,
    tcp: tokio::net::TcpStream,
    peer: SocketAddr,
) -> Result<()> {
    tcp.set_nodelay(true).ok();

    let tls = acceptor.accept(tcp).await?;
    let alpn = tls
        .get_ref()
        .1
        .alpn_protocol()
        .map(|p| String::from_utf8_lossy(p).to_string());
    if alpn.as_deref() != Some("h2") {
        warn!(%peer, alpn = ?alpn, "reject conn: unexpected alpn");
        return Err(anyhow!("unexpected alpn: {alpn:?}"));
    }

    let mut conn = server::handshake(tls).await?;
    let mut tasks = JoinSet::new();

    while let Some(res) = conn.accept().await {
        let (req, respond) = res?;
        let cfg = cfg.clone();
        let stream_id = STREAM_SEQ.fetch_add(1, Ordering::Relaxed);
        tasks.spawn(async move {
            if let Err(e) = handle_stream(cfg, req, respond, peer, stream_id).await {
                error!(%peer, stream_id, "stream error: {e:#}");
            }
        });
    }

    while tasks.join_next().await.is_some() {}
    Ok(())
}

async fn handle_stream(
    cfg: Arc<Config>,
    req: http::Request<h2::RecvStream>,
    mut respond: SendResponse<Bytes>,
    peer: SocketAddr,
    stream_id: u64,
) -> Result<()>
{
    if req.method() == http::Method::GET {
        if let Some(sub_path) = cfg.sub_path.as_deref() {
            if req.uri().path() == sub_path {
                return handle_subscription(cfg, req, respond, peer, stream_id).await;
            }
        }
    }

    let path = req.uri().path();
    if path != cfg.path {
        warn!(%peer, stream_id, got_path = path, expected_path = %cfg.path, "reject stream: path mismatch");
        let res = http::Response::builder()
            .status(404)
            .body(())
            .context("build response")?;
        respond.send_response(res, true)?;
        return Ok(());
    }

    let authority = req
        .uri()
        .authority()
        .map(|a| a.as_str().to_ascii_lowercase())
        .or_else(|| {
            req.headers()
                .get("host")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_ascii_lowercase())
        });

    let Some(authority) = authority else {
        warn!(%peer, stream_id, "reject stream: missing authority/host");
        let res = http::Response::builder()
            .status(421)
            .body(())
            .context("build response")?;
        respond.send_response(res, true)?;
        return Ok(());
    };

    if !cfg.hosts.contains(&authority) {
        warn!(%peer, stream_id, authority = %authority, "reject stream: authority not allowed");
        let res = http::Response::builder()
            .status(421)
            .body(())
            .context("build response")?;
        respond.send_response(res, true)?;
        return Ok(());
    }

    let res = http::Response::builder()
        .status(200)
        .body(())
        .context("build response")?;
    let mut send_stream = respond.send_response(res, false)?;
    let mut recv_stream = req.into_body();

    let mut buf = BytesMut::new();
    let (header, consumed, vless_version) =
        match read_and_parse_vless_header(cfg.clone(), &mut recv_stream, &mut buf).await {
            Ok(v) => v,
            Err(e) => {
                warn!(%peer, stream_id, authority = %authority, "reject stream: invalid vless header: {e:#}");
                send_stream.send_reset(h2::Reason::PROTOCOL_ERROR);
                return Ok(());
            }
        };

    let mut payload = buf.split_off(consumed);
    buf.clear();

    if header.command != 0x01 {
        warn!(%peer, stream_id, cmd = header.command, "reject stream: unsupported command");
        send_stream.send_reset(h2::Reason::PROTOCOL_ERROR);
        return Ok(());
    }

    if !cfg.users.contains(&header.user_id) {
        warn!(%peer, stream_id, "reject stream: uuid not allowed");
        send_stream.send_reset(h2::Reason::REFUSED_STREAM);
        return Ok(());
    }

    let dest = header.destination;
    let mut upstream = match connect_with_timeout(&cfg, dest.clone()).await {
        Ok(v) => v,
        Err(e) => {
            warn!(
                %peer,
                stream_id,
                dest = %dest_to_string(&dest),
                "connect failed: {e:#}"
            );
            send_stream.send_reset(h2::Reason::INTERNAL_ERROR);
            return Ok(());
        }
    };
    upstream.set_nodelay(true).ok();

    send_bytes(
        &mut send_stream,
        Bytes::copy_from_slice(&[vless_version, 0]),
        false,
    )
    .await?;

    if !payload.is_empty() {
        upstream.write_all(&payload).await?;
        payload.clear();
    }

    let (mut ur, mut uw) = upstream.into_split();

    let idle = cfg.idle_timeout;
    let mut send_task = tokio::spawn(async move {
        let mut read_buf = [0u8; 16 * 1024];
        loop {
            let n = match timeout(idle, ur.read(&mut read_buf)).await {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(anyhow!(e)),
                Err(_) => return Err(anyhow!("idle timeout (upstream->client)")),
            };
            if n == 0 {
                send_stream.send_data(Bytes::new(), true)?;
                return Ok(());
            }
            send_bytes(
                &mut send_stream,
                Bytes::copy_from_slice(&read_buf[..n]),
                false,
            )
            .await?;
        }
    });

    let mut recv_task = tokio::spawn(async move {
        loop {
            let chunk = match timeout(idle, recv_stream.data()).await {
                Ok(v) => v,
                Err(_) => return Err(anyhow!("idle timeout (client->upstream)")),
            };
            let Some(chunk) = chunk else {
                uw.shutdown().await.ok();
                return Ok(());
            };
            let chunk = chunk?;
            if !chunk.is_empty() {
                uw.write_all(&chunk).await?;
                recv_stream.flow_control().release_capacity(chunk.len())?;
            }
        }
    });

    tokio::select! {
        res = &mut send_task => {
            recv_task.abort();
            res??;
        }
        res = &mut recv_task => {
            send_task.abort();
            res??;
        }
    }

    Ok(())
}

async fn handle_subscription(
    cfg: Arc<Config>,
    req: http::Request<h2::RecvStream>,
    mut respond: SendResponse<Bytes>,
    peer: SocketAddr,
    stream_id: u64,
) -> Result<()> {
    let authority = req
        .uri()
        .authority()
        .map(|a| a.as_str().to_ascii_lowercase())
        .or_else(|| {
            req.headers()
                .get("host")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_ascii_lowercase())
        });

    let Some(authority) = authority else {
        warn!(%peer, stream_id, "reject sub: missing authority/host");
        let res = http::Response::builder().status(404).body(())?;
        respond.send_response(res, true)?;
        return Ok(());
    };
    if !cfg.hosts.contains(&authority) {
        warn!(%peer, stream_id, authority = %authority, "reject sub: authority not allowed");
        let res = http::Response::builder().status(404).body(())?;
        respond.send_response(res, true)?;
        return Ok(());
    }

    let (fmt, token) = parse_sub_query(req.uri().query());
    if let Some(expected) = cfg.sub_token.as_deref() {
        if token.as_deref() != Some(expected) {
            warn!(%peer, stream_id, "reject sub: bad token");
            let res = http::Response::builder().status(404).body(())?;
            respond.send_response(res, true)?;
            return Ok(());
        }
    }

    let body = match fmt.as_deref() {
        Some("vless") | Some("uri") | Some("links") => build_vless_links(&cfg),
        _ => build_clash_yaml(&cfg),
    };

    let content_type = match fmt.as_deref() {
        Some("vless") | Some("uri") | Some("links") => "text/plain; charset=utf-8",
        _ => "text/yaml; charset=utf-8",
    };

    let res = http::Response::builder()
        .status(200)
        .header("content-type", content_type)
        .body(())?;
    let mut send = respond.send_response(res, false)?;
    send.send_data(Bytes::from(body), true)?;
    Ok(())
}

fn parse_sub_query(query: Option<&str>) -> (Option<String>, Option<String>) {
    let mut fmt = None;
    let mut token = None;
    let Some(q) = query else {
        return (fmt, token);
    };
    for part in q.split('&') {
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        match k {
            "fmt" | "format" => fmt = Some(v.to_string()),
            "token" => token = Some(v.to_string()),
            _ => {}
        }
    }
    (fmt, token)
}

fn build_clash_yaml(cfg: &Config) -> String {
    let mut out = String::new();
    out.push_str("proxies:\n");
    for (idx, u) in cfg.users.iter().enumerate() {
        let uuid = uuid::Uuid::from_bytes(*u).to_string();
        let name = format!("hpx-{}", idx + 1);
        out.push_str(&format!("  - name: {name}\n"));
        out.push_str("    type: vless\n");
        out.push_str(&format!("    server: {}\n", cfg.public_host));
        out.push_str(&format!("    port: {}\n", cfg.public_port));
        out.push_str(&format!("    uuid: {uuid}\n"));
        out.push_str("    udp: false\n");
        out.push_str("    tls: true\n");
        out.push_str(&format!("    servername: {}\n", cfg.public_host));
        out.push_str("    network: h2\n");
        out.push_str("    h2-opts:\n");
        out.push_str("      host:\n");
        out.push_str(&format!("        - {}\n", cfg.public_host));
        out.push_str(&format!("      path: {}\n", cfg.path));
    }
    out
}

fn build_vless_links(cfg: &Config) -> String {
    let mut out = String::new();
    for (idx, u) in cfg.users.iter().enumerate() {
        let uuid = uuid::Uuid::from_bytes(*u).to_string();
        let name = format!("hpx-{}", idx + 1);
        let link = format!(
            "vless://{}@{}:{}?encryption=none&security=tls&type=http&host={}&path={}&sni={}#{}",
            uuid, cfg.public_host, cfg.public_port, cfg.public_host, cfg.path, cfg.public_host, name
        );
        out.push_str(&link);
        out.push('\n');
    }
    out
}

async fn read_and_parse_vless_header(
    cfg: Arc<Config>,
    recv: &mut h2::RecvStream,
    buf: &mut BytesMut,
) -> Result<(vless::Header, usize, u8)> {
    loop {
        if let Some((hdr, consumed)) = vless::try_parse(buf)? {
            let v = hdr.version;
            return Ok((hdr, consumed, v));
        }

        let next = timeout(cfg.idle_timeout, recv.data())
            .await
            .context("idle timeout")?;
        let Some(next) = next else {
            return Err(anyhow!("unexpected end of stream"));
        };
        let next = next?;
        if !next.is_empty() {
            buf.extend_from_slice(&next);
            recv.flow_control().release_capacity(next.len())?;
        }
    }
}

async fn connect_with_timeout(cfg: &Config, dest: vless::Destination) -> Result<tokio::net::TcpStream> {
    let fut = async {
        match dest.address {
            vless::DestAddr::Ip(ip) => tokio::net::TcpStream::connect(SocketAddr::new(ip, dest.port)).await.map_err(anyhow::Error::from),
            vless::DestAddr::Domain(name) => {
                let mut last = None;
                let addrs = tokio::net::lookup_host((name.as_str(), dest.port)).await?;
                for a in addrs {
                    match tokio::net::TcpStream::connect(a).await {
                        Ok(s) => return Ok(s),
                        Err(e) => last = Some(e),
                    }
                }
                Err(anyhow!(last.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no address"))))
            }
        }
    };

    timeout(cfg.connect_timeout, fut)
        .await
        .context("connect timeout")?
        .context("connect failed")
}

fn dest_to_string(dest: &vless::Destination) -> String {
    match &dest.address {
        vless::DestAddr::Ip(ip) => format!("{ip}:{}", dest.port),
        vless::DestAddr::Domain(name) => format!("{name}:{}", dest.port),
    }
}

async fn send_bytes(send: &mut h2::SendStream<Bytes>, mut data: Bytes, end: bool) -> Result<()> {
    while !data.is_empty() {
        let cap = send.capacity();
        if cap == 0 {
            send.reserve_capacity(data.len());
            poll_fn(|cx| match send.poll_capacity(cx) {
                std::task::Poll::Ready(Some(Ok(_))) => std::task::Poll::Ready(Ok(())),
                std::task::Poll::Ready(Some(Err(e))) => std::task::Poll::Ready(Err(anyhow!(e))),
                std::task::Poll::Ready(None) => std::task::Poll::Ready(Err(anyhow!("stream closed"))),
                std::task::Poll::Pending => std::task::Poll::Pending,
            })
            .await?;
            continue;
        }

        let n = cap.min(data.len());
        let chunk = data.split_to(n);
        send.send_data(chunk, end && data.is_empty())?;
    }
    Ok(())
}
