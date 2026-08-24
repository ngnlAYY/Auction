//! 部署启动使用的应用 readiness 探针。
//!
//! TCP 只访问 Public/Admin 的本机配置地址；Unix socket 只访问 Worker control。响应正文不参与
//! 判断，HTTP 200 是应用 owner 对 readiness 的唯一权威信号。

use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    os::unix::{fs::FileTypeExt, net::UnixStream},
    path::{Path, PathBuf},
    time::Duration,
};

const PROBE_IO_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) enum ReadinessProbe {
    Tcp(SocketAddr),
    Unix(PathBuf),
}

pub(super) fn readiness_probe(probe: &ReadinessProbe) -> bool {
    match probe {
        ReadinessProbe::Tcp(address) => tcp_http_ready(*address),
        ReadinessProbe::Unix(path) => unix_http_ready(path),
    }
}

fn tcp_http_ready(address: SocketAddr) -> bool {
    let probe_address = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), address.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), address.port())
        }
        _ => address,
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&probe_address, PROBE_IO_TIMEOUT) else {
        return false;
    };
    if stream.set_read_timeout(Some(PROBE_IO_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(PROBE_IO_TIMEOUT)).is_err()
    {
        return false;
    }
    http_ready(&mut stream, "/health/ready")
}

fn unix_http_ready(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_socket() || metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(mut stream) = UnixStream::connect(path) else {
        return false;
    };
    if stream.set_read_timeout(Some(PROBE_IO_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(PROBE_IO_TIMEOUT)).is_err()
    {
        return false;
    }
    http_ready(&mut stream, "/v1/health/ready")
}

fn http_ready(stream: &mut (impl Read + Write), path: &str) -> bool {
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nUser-Agent: auctionctl-deployment\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() || stream.flush().is_err() {
        return false;
    }
    let mut response = [0_u8; 512];
    let mut length = 0;
    while length < response.len() && !response[..length].contains(&b'\n') {
        match stream.read(&mut response[length..]) {
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
