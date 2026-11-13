//! 本机角色进程的 readiness 探针：只有以 CRLF 结尾的 HTTP/1.x 200 状态行才视为就绪，不以 PID 存活代替。

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::Path,
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpStream, UnixStream},
};

/// Public/Admin HTTP 的 readiness 路径。
pub const HTTP_READY_PATH: &str = "/health/ready";
/// Worker control socket 的 readiness 路径。
pub const CONTROL_READY_PATH: &str = "/v1/health/ready";

/// 连接、写请求与读状态行共用的单次探测预算。
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
/// 状态行读取上限；读满仍未见行尾即判未就绪。
const STATUS_LINE_LIMIT: usize = 512;

/// 被探测角色的本机入口；调用方从自己的配置推导，本模块不读配置。
#[derive(Debug, Clone, Copy)]
pub enum ReadinessTarget<'a> {
    Tcp(SocketAddr),
    Unix(&'a Path),
}

/// 单次有界探测；连接、读写失败或超时一律返回 `false`。
pub async fn probe_ready(target: ReadinessTarget<'_>, path: &str) -> bool {
    tokio::time::timeout(PROBE_TIMEOUT, probe(target, path)).await.unwrap_or(false)
}

async fn probe(target: ReadinessTarget<'_>, path: &str) -> bool {
    match target {
        ReadinessTarget::Tcp(mut address) => {
            // 通配监听地址不是可连接目标，改连同族回环。
            if address.ip().is_unspecified() {
                address.set_ip(if address.is_ipv4() {
                    IpAddr::V4(Ipv4Addr::LOCALHOST)
                } else {
                    IpAddr::V6(Ipv6Addr::LOCALHOST)
                });
            }
            match TcpStream::connect(address).await {
                Ok(mut stream) => status_is_ok(&mut stream, path).await,
                Err(_) => false,
            }
        }
        ReadinessTarget::Unix(socket) => match UnixStream::connect(socket).await {
            Ok(mut stream) => status_is_ok(&mut stream, path).await,
            Err(_) => false,
        },
    }
}

async fn status_is_ok(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), path: &str) -> bool {
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut buffer = [0_u8; STATUS_LINE_LIMIT];
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

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::{HTTP_READY_PATH, ReadinessTarget, STATUS_LINE_LIMIT, probe_ready, status_is_ok};

    /// 以给定分片应答一次探测，返回探测结论与服务端实际收到的请求。
    async fn exchange(chunks: &[&[u8]]) -> (bool, String) {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let chunks = chunks.iter().map(|chunk| chunk.to_vec()).collect::<Vec<_>>();
        let peer = tokio::spawn(async move {
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if server.read(&mut byte).await.expect("read request") == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            for chunk in chunks {
                if server.write_all(&chunk).await.is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            String::from_utf8(request).expect("ascii request")
        });
        let ready = status_is_ok(&mut client, HTTP_READY_PATH).await;
        drop(client);
        (ready, peer.await.expect("peer task"))
    }

    #[tokio::test]
    async fn only_a_crlf_terminated_200_status_line_is_ready() {
        let cases: [(&[u8], bool); 8] = [
            (b"HTTP/1.1 200 OK\r\n\r\n", true),
            (b"HTTP/1.0 200 OK\r\n", true),
            (b"HTTP/1.1 503 Service Unavailable\r\n", false),
            (b"HTTP/1.1 204 No Content\r\n", false),
            (b"HTTP/2 200 \r\n", false),
            (b"HTTP/1.1 200 OK\n", false),
            (b"\r\n", false),
            (b"HTTP/1.1 200 OK", false),
        ];
        for (response, expected) in cases {
            let (ready, _) = exchange(&[response]).await;
            assert_eq!(ready, expected, "response={:?}", String::from_utf8_lossy(response));
        }
    }

    #[tokio::test]
    async fn status_line_may_arrive_in_pieces() {
        let (ready, request) = exchange(&[b"HTTP/1.1 2", b"00 OK\r", b"\nServer: x\r\n"]).await;
        assert!(ready);
        assert_eq!(
            request,
            "GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        );
    }

    #[tokio::test]
    async fn unterminated_status_line_is_bounded() {
        let filler = vec![b'a'; STATUS_LINE_LIMIT];
        let (ready, _) = exchange(&[&filler, b"\r\n"]).await;
        assert!(!ready);
    }

    #[tokio::test]
    async fn wildcard_listen_address_is_probed_through_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let mut address = listener.local_addr().expect("local address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buffer = [0_u8; 256];
            let _ = stream.read(&mut buffer).await.expect("read request");
            stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.expect("write response");
        });
        address.set_ip([0, 0, 0, 0].into());
        assert!(probe_ready(ReadinessTarget::Tcp(address), HTTP_READY_PATH).await);
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn unreachable_targets_are_not_ready() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let address = listener.local_addr().expect("local address");
        drop(listener);
        assert!(!probe_ready(ReadinessTarget::Tcp(address), HTTP_READY_PATH).await);
        let missing = std::path::Path::new("/nonexistent/auction-readiness.sock");
        assert!(!probe_ready(ReadinessTarget::Unix(missing), HTTP_READY_PATH).await);
    }
}
