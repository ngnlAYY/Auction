//! 部署启动使用的应用 readiness 探针。
//!
//! TCP 与 Unix HTTP socket 访问 Public/Admin 的 `/health/ready`；Worker control 使用独立的
//! `/v1/health/ready`。响应正文不参与判断，HTTP 200 是应用 owner 对 readiness 的唯一权威信号。

use std::{
    fs,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpStream, UnixStream},
};

const PROBE_IO_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) enum ReadinessProbe {
    Tcp(SocketAddr),
    UnixHttp(PathBuf),
    UnixControl(PathBuf),
}

pub(super) async fn readiness_probe(probe: &ReadinessProbe, startup_deadline: Instant) -> bool {
    let now = Instant::now();
    if now >= startup_deadline {
        return false;
    }
    let Some(probe_deadline) = now.checked_add(PROBE_IO_TIMEOUT) else {
        return false;
    };
    // 非阻塞连接、写请求和读状态行共用一个预算；每轮探针不能延长启动总截止时刻。
    tokio::time::timeout_at(
        tokio::time::Instant::from_std(probe_deadline.min(startup_deadline)),
        async {
            match probe {
                ReadinessProbe::Tcp(address) => tcp_http_ready(*address).await,
                ReadinessProbe::UnixHttp(path) => unix_http_ready(path, "/health/ready").await,
                ReadinessProbe::UnixControl(path) => {
                    unix_http_ready(path, "/v1/health/ready").await
                }
            }
        },
    )
    .await
    .unwrap_or(false)
}

async fn tcp_http_ready(address: SocketAddr) -> bool {
    let probe_address = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), address.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), address.port())
        }
        _ => address,
    };
    let Ok(mut stream) = TcpStream::connect(probe_address).await else {
        return false;
    };
    http_ready(&mut stream, "/health/ready").await
}

async fn unix_http_ready(path: &Path, readiness_path: &str) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_socket() || metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(mut stream) = UnixStream::connect(path).await else {
        return false;
    };
    http_ready(&mut stream, readiness_path).await
}

async fn http_ready(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), path: &str) -> bool {
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nUser-Agent: auctionctl-deployment\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).await.is_err() || stream.flush().await.is_err() {
        return false;
    }
    let mut response = [0_u8; 512];
    let mut length = 0;
    while length < response.len() && !response[..length].contains(&b'\n') {
        match stream.read(&mut response[length..]).await {
            Ok(0) => break,
            Ok(read) => length += read,
            Err(_) => return false,
        }
    }
    if length == 0 {
        return false;
    }
    let first_line = response[..length].split(|byte| *byte == b'\n').next().unwrap_or_default();
    first_line.starts_with(b"HTTP/1.1 200 ") || first_line.starts_with(b"HTTP/1.0 200 ")
}
