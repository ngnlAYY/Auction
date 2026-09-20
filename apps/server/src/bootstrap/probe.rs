//! 对当前子进程的配置入口执行有界 readiness 探针，不以 PID 存活代替就绪。

use super::{
    cli::ChildRole,
    configuration::{Endpoint, RolePlan},
};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpStream, UnixStream},
};

pub(super) async fn ready(plan: &RolePlan) -> bool {
    tokio::time::timeout(Duration::from_secs(1), probe(plan)).await.unwrap_or(false)
}

async fn probe(plan: &RolePlan) -> bool {
    let path = if plan.role == ChildRole::Worker { "/v1/health/ready" } else { "/health/ready" };
    match &plan.endpoint {
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
        Endpoint::Unix(pathname) => match UnixStream::connect(pathname).await {
            Ok(mut stream) => http(&mut stream, path).await,
            Err(_) => false,
        },
    }
}

async fn http(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), path: &str) -> bool {
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut buffer = [0u8; 512];
    let mut length = 0;
    while length < buffer.len() && !buffer[..length].contains(&b'\n') {
        match stream.read(&mut buffer[length..]).await {
            Ok(0) | Err(_) => return false,
            Ok(count) => length += count,
        }
    }
    let line = buffer[..length].split(|byte| *byte == b'\n').next().unwrap_or_default();
    line.starts_with(b"HTTP/1.1 200 ") || line.starts_with(b"HTTP/1.0 200 ")
}
