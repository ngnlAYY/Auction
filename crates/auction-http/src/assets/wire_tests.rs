//! 从回环连接检查静态资源再验证在线路上的边界。

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    service::TowerToHyperService,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};

use super::{serve_public_asset, tests::TestAssets};
use crate::server::serve_http;

async fn listen() -> (SocketAddr, oneshot::Sender<()>, JoinHandle<io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let app = Router::new().fallback(serve_public_asset::<TestAssets>);
    let server = tokio::spawn(serve_http(listener, app, async move {
        stopped.await.expect("stop listener");
    }));
    (address, stop, server)
}

#[tokio::test]
async fn http2_not_modified_ends_in_headers() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (address, stop, server) = listen().await;
        assert_http2_revalidation(TcpStream::connect(address).await.unwrap()).await;
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    })
    .await
    .expect("HTTP/2 loopback");
}

#[tokio::test]
async fn http2_not_modified_has_the_same_framing_over_duplex() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (client, server) = tokio::io::duplex(8192);
        let app = Router::new().fallback(serve_public_asset::<TestAssets>);
        let server = tokio::spawn(async move {
            hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(server), TowerToHyperService::new(app))
                .await
        });
        assert_http2_revalidation(client).await;
        server.abort();
    })
    .await
    .expect("HTTP/2 duplex");
}

async fn assert_http2_revalidation(stream: impl AsyncRead + AsyncWrite + Unpin + Send + 'static) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let stream = RecordingStream { stream, received: Arc::clone(&received) };
    let (mut client, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
            .await
            .unwrap();
    let connection = tokio::spawn(connection);
    let request = || Request::builder().uri("http://localhost/");
    let response = client.send_request(request().body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG].clone();
    let length = response.headers()[header::CONTENT_LENGTH].clone();
    let bytes = to_bytes(Body::new(response.into_body()), 4096).await.unwrap();
    assert!(!bytes.is_empty());
    assert_eq!(length, bytes.len().to_string());
    let response = client
        .send_request(request().header(header::IF_NONE_MATCH, etag).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    if let Some(revalidated_length) = response.headers().get(header::CONTENT_LENGTH) {
        assert_eq!(revalidated_length, &length);
    }
    assert!(to_bytes(Body::new(response.into_body()), 4096).await.unwrap().is_empty());
    assert_headers_end_stream(&received.lock().unwrap(), 3);
    connection.abort();
}

fn assert_headers_end_stream(mut bytes: &[u8], expected_stream: u32) {
    const DATA: u8 = 0;
    const HEADERS: u8 = 1;
    const END_STREAM: u8 = 1;

    let mut headers = None;
    while let Some((frame, remaining)) = bytes.split_first_chunk::<9>() {
        let length = u32::from_be_bytes([0, frame[0], frame[1], frame[2]]) as usize;
        let stream = u32::from_be_bytes([frame[5], frame[6], frame[7], frame[8]]) & 0x7fff_ffff;
        // 客户端可能已读入下一帧的前缀，帧头本身足以检查类型和标志。
        if stream == expected_stream {
            assert_ne!(frame[3], DATA, "304 sent a DATA frame");
            if frame[3] == HEADERS {
                assert!(headers.replace(frame[4]).is_none(), "304 sent extra headers");
            }
        }
        let Some(next) = remaining.get(length..) else { break };
        bytes = next;
    }
    assert_eq!(headers.expect("304 HEADERS") & END_STREAM, END_STREAM);
}

struct RecordingStream<S> {
    stream: S,
    received: Arc<Mutex<Vec<u8>>>,
}

impl<S: AsyncRead + Unpin> AsyncRead for RecordingStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let start = buffer.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buffer);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.received.lock().unwrap().extend_from_slice(&buffer.filled()[start..]);
        }
        result
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for RecordingStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

async fn http1_get(address: SocketAddr, etag: Option<&str>) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let validator = etag.map_or_else(String::new, |etag| format!("If-None-Match: {etag}\r\n"));
    let request =
        format!("GET / HTTP/1.1\r\nHost: localhost\r\n{validator}Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

fn http1_header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then(|| value.trim()))
}

#[tokio::test]
async fn http1_not_modified_ends_after_headers() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (address, stop, server) = listen().await;
        let response = http1_get(address, None).await;
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
        assert!(!body.is_empty());
        let length = http1_header(head, "content-length").unwrap();
        assert_eq!(length.parse::<usize>().unwrap(), body.len());
        let etag = http1_header(head, "etag").unwrap();
        let response = http1_get(address, Some(etag)).await;
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("HTTP/1.1 304 "), "{head}");
        assert!(body.is_empty());
        assert_eq!(http1_header(head, "etag"), Some(etag));
        if let Some(revalidated_length) = http1_header(head, "content-length") {
            assert_eq!(revalidated_length, length);
        }
        assert!(http1_header(head, "transfer-encoding").is_none());

        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    })
    .await
    .expect("HTTP/1.1 loopback");
}
