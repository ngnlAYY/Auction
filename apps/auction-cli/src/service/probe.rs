//! 应用 readiness 探针：只有完整 CRLF 结尾的 HTTP/1.x 200 状态行才视为就绪，不以 PID 存活代替。

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::Path,
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpStream, UnixStream},
};

use crate::config_dir::Endpoint;

const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Public/Admin HTTP 的 readiness 路径。
pub(crate) const HTTP_READY_PATH: &str = "/health/ready";
/// Worker control socket 的 readiness 路径。
pub(crate) const CONTROL_READY_PATH: &str = "/v1/health/ready";

/// 单次探测，连接、写请求与读状态行共用 1 秒预算。
pub(crate) async fn ready(endpoint: &Endpoint, path: &str) -> bool {
    tokio::time::timeout(PROBE_TIMEOUT, probe(endpoint, path)).await.unwrap_or(false)
}

async fn probe(endpoint: &Endpoint, path: &str) -> bool {
    match endpoint {
        Endpoint::Tcp(address) => {
            let mut address = *address;
            if address.ip().is_unspecified() {
                address.set_ip(if address.is_ipv4() {
                    IpAddr::V4(Ipv4Addr::LOCALHOST)
                } else {
                    IpAddr::V6(Ipv6Addr::LOCALHOST)
                });
            }
            match TcpStream::connect(address).await {
                Ok(mut stream) => http(&mut stream, path).await,
                Err(_) => false,
            }
        }
        Endpoint::Unix(socket) => unix(socket, path).await,
    }
}

async fn unix(socket: &Path, path: &str) -> bool {
    match UnixStream::connect(socket).await {
        Ok(mut stream) => http(&mut stream, path).await,
        Err(_) => false,
    }
}

async fn http(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), path: &str) -> bool {
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nUser-Agent: auction-cli\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut buffer = [0_u8; 512];
    let mut length = 0;
    while length < buffer.len() {
        let count = match stream.read(&mut buffer[length..]).await {
            Ok(0) | Err(_) => return false,
            Ok(count) => count,
        };
        let start = length;
        length += count;
        // 只扫描本次新增字节；未终止或仅 LF 的状态行不能证明服务就绪。
        if let Some(offset) = buffer[start..length].iter().position(|byte| *byte == b'\n') {
            let end = start + offset;
            if end == 0 || buffer[end - 1] != b'\r' {
                return false;
            }
            let line = &buffer[..end - 1];
            return line.starts_with(b"HTTP/1.1 200 ") || line.starts_with(b"HTTP/1.0 200 ");
        }
    }
    false
}
