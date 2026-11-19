//! 从回环连接检查静态资源再验证在线路上的边界。

use std::{
    borrow::Cow,
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
    response::Response,
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    service::TowerToHyperService,
};
use rust_embed::{EmbeddedFile, Metadata, RustEmbed};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};

use super::{serve_admin_asset, serve_admin_setup_asset, serve_public_asset, tests::TestAssets};
use crate::WebScriptPolicy;
use crate::server::serve_http;

async fn public_asset<A: RustEmbed + Send + Sync + 'static>(request: Request<Body>) -> Response {
    serve_public_asset::<A>(request, WebScriptPolicy::Dioxus).await
}

async fn admin_asset<A: RustEmbed + Send + Sync + 'static>(request: Request<Body>) -> Response {
    serve_admin_asset::<A>(request, WebScriptPolicy::Dioxus).await
}

async fn listen() -> (SocketAddr, oneshot::Sender<()>, JoinHandle<io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let app = Router::new().fallback(public_asset::<TestAssets>);
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
        let app = Router::new().fallback(public_asset::<TestAssets>);
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

const ENTRY_HTML: &[u8] = b"<link rel=\"stylesheet\" href=\"/assets/app.css\">";
const FAVICON_SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
// 对应原文的 Brotli 字节。发布构建不生成 HTML 副本，也跳过当前尺寸的图标。
const ENTRY_BR: &[u8] = b"\x1f\x2d\x00\xf8\xa5\x5f\x7c\xc2\xe0\xc0\x06\x1c\xb8\x94\x44\x30\
                         \xd1\x3d\x02\x2c\xc8\x60\xe4\xc1\x3c\x7e\xf4\x8b\x9e";
const FAVICON_BR: &[u8] = b"\x1f\x28\x00\xf8\x45\x4f\x96\xfa\xd7\xf9\xa8\x68\xd5\xba\xcd\x61\
                           \x03\x4e\x84\x92\xc0\xe2\x51\xee\x18\x74\xe9\x26\x26\x5f\xf7\x7b\
                           \xee\x69\x74\x8e\x24\x53\x0a\x05\x49\x47\xf8\x00";

struct NegotiationAssets<const FAVICON_BROTLI: bool = true>;

impl<const FAVICON_BROTLI: bool> RustEmbed for NegotiationAssets<FAVICON_BROTLI> {
    fn get(path: &str) -> Option<EmbeddedFile> {
        let (data, mime) = match path {
            "index.html" => (ENTRY_HTML, "text/html"),
            "index.html.br" => (ENTRY_BR, "application/octet-stream"),
            "assets/favicon-test.svg" => (FAVICON_SVG, "image/svg+xml"),
            "assets/favicon-test.svg.br" if FAVICON_BROTLI => {
                (FAVICON_BR, "application/octet-stream")
            }
            _ => return TestAssets::get(path),
        };
        Some(EmbeddedFile {
            data: Cow::Borrowed(data),
            metadata: Metadata::__rust_embed_new(Sha256::digest(data).into(), None, None, mime),
        })
    }

    fn iter() -> impl Iterator<Item = Cow<'static, str>> + 'static {
        TestAssets::iter().chain(
            ["index.html.br", "assets/favicon-test.svg"]
                .into_iter()
                .chain(FAVICON_BROTLI.then_some("assets/favicon-test.svg.br"))
                .map(Cow::Borrowed),
        )
    }
}

#[derive(rust_embed::Embed)]
#[folder = "tests/assets-fixture-missing"]
#[allow_missing = true]
struct MissingAssets;

async fn http1_asset(app: &Router, method: &str, path: &str, headers: &str) -> (String, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut client, server) = tokio::io::duplex(8192);
        let app = TowerToHyperService::new(app.clone());
        let server = tokio::spawn(async move {
            hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(server), app)
                .await
        });
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{headers}Connection: close\r\n\r\n"
        );
        client.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        server.await.unwrap().unwrap();
        let end = response.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap();
        let body = response.split_off(end + 4);
        response.truncate(end);
        (String::from_utf8(response).unwrap(), body)
    })
    .await
    .expect("HTTP/1.1 duplex")
}

#[tokio::test]
async fn http1_asset_encoding_preserves_legacy_representations() {
    let app = Router::new().fallback(public_asset::<TestAssets>);
    for (accept, expected_encoding) in [
        (None, None),
        (Some(""), None),
        (Some("gzip, deflate, br, zstd"), Some("br")),
        (Some("br;q=0"), None),
        (Some("*;q=0.2"), None),
        (Some("gzip"), None),
        (Some("identity"), None),
        (Some("br;q=0.2, identity;q=0.3"), Some("br")),
    ] {
        let headers =
            accept.map_or_else(String::new, |value| format!("Accept-Encoding: {value}\r\n"));
        let (expected_body, expected_etag) = if expected_encoding.is_some() {
            (
                include_bytes!("../../tests/assets-fixture/assets/app.css.br").as_slice(),
                "\"4c109cb162d4ad2b4d17b1db6a60d22772a10cb4d86dbf7808abb362d913a8bb\"",
            )
        } else {
            (
                include_bytes!("../../tests/assets-fixture/assets/app.css").as_slice(),
                "\"74d94aede163ac74eb42fe7cac4066626820fa15001ddf02c3b2d25df8e6c771\"",
            )
        };
        let (head, body) = http1_asset(&app, "GET", "/assets/app.css", &headers).await;
        assert!(head.starts_with("HTTP/1.1 200 "), "{accept:?}: {head}");
        assert_eq!(body, expected_body, "{accept:?}");
        assert_eq!(http1_header(&head, "content-encoding"), expected_encoding, "{accept:?}");
        assert_eq!(http1_header(&head, "etag"), Some(expected_etag), "{accept:?}");
        assert_eq!(
            http1_header(&head, "content-length"),
            Some(expected_body.len().to_string().as_str()),
            "{accept:?}",
        );
        assert_eq!(http1_header(&head, "vary"), Some("Accept-Encoding"), "{accept:?}");
    }
}

#[tokio::test]
async fn http1_identity_only_favicon_varies_on_success_revalidation_and_rejection() {
    let app = Router::new().fallback(public_asset::<NegotiationAssets<false>>);
    let path = "/favicon.ico";
    let (head, body) = http1_asset(&app, "GET", path, "").await;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert_eq!(body, FAVICON_SVG);
    assert_eq!(http1_header(&head, "cache-control"), Some("public, max-age=86400"));
    assert_eq!(http1_header(&head, "vary"), Some("Accept-Encoding"));

    let (rejected, body) =
        http1_asset(&app, "GET", path, "Accept-Encoding: identity;q=0\r\n").await;
    assert!(rejected.starts_with("HTTP/1.1 406 "), "{rejected}");
    assert_eq!(http1_header(&rejected, "vary"), Some("Accept-Encoding"));
    assert_eq!(http1_header(&rejected, "cache-control"), Some("no-store"));
    assert!(body.is_empty());

    let etag = http1_header(&head, "etag").unwrap();
    let headers = format!("If-None-Match: {etag}\r\n");
    let (revalidated, body) = http1_asset(&app, "GET", path, &headers).await;
    assert!(revalidated.starts_with("HTTP/1.1 304 "), "{revalidated}");
    assert_eq!(http1_header(&revalidated, "vary"), Some("Accept-Encoding"));
    assert_eq!(http1_header(&revalidated, "etag"), Some(etag));
    assert!(body.is_empty());
}

#[tokio::test]
async fn http1_entry_aliases_and_favicon_negotiate_existing_copies() {
    let public = Router::new().fallback(public_asset::<NegotiationAssets>);
    let admin = Router::new().fallback(admin_asset::<NegotiationAssets>);
    let setup = Router::new().fallback(serve_admin_setup_asset::<NegotiationAssets>);
    let headers = "Accept-Encoding: *;q=1\r\nAccept-Encoding: identity;q=0\r\n";
    let preload = "</assets/app.css>; rel=preload; as=style";
    for (app, path, copy, mime, link) in [
        (&public, "/", ENTRY_BR, "text/html", Some(preload)),
        (&admin, "/dashboard", ENTRY_BR, "text/html", Some(preload)),
        (&setup, "/setup", ENTRY_BR, "text/html", Some(preload)),
        (&public, "/index.html", ENTRY_BR, "text/html", None),
        (&public, "/favicon.ico", FAVICON_BR, "image/svg+xml", None),
    ] {
        let (head, body) = http1_asset(app, "GET", path, headers).await;
        assert!(head.starts_with("HTTP/1.1 200 "), "{path}: {head}");
        assert_eq!(http1_header(&head, "content-encoding"), Some("br"), "{path}");
        assert_eq!(http1_header(&head, "vary"), Some("Accept-Encoding"), "{path}");
        assert_eq!(http1_header(&head, "content-type"), Some(mime), "{path}");
        assert_eq!(http1_header(&head, "link"), link, "{path}");
        assert_eq!(body, copy, "{path}");
    }
}

#[tokio::test]
async fn http1_head_and_304_describe_the_negotiated_copy() {
    let app = Router::new().fallback(public_asset::<NegotiationAssets>);
    let (identity, body) = http1_asset(&app, "GET", "/", "").await;
    assert!(identity.starts_with("HTTP/1.1 200 "));
    assert_eq!(body, ENTRY_HTML);
    assert_eq!(http1_header(&identity, "content-encoding"), None);
    assert_eq!(http1_header(&identity, "vary"), Some("Accept-Encoding"));
    let identity_etag = http1_header(&identity, "etag").unwrap();

    let accept = "Accept-Encoding: *;q=0\r\nAccept-Encoding: br;q=0.8\r\n";
    let headers = format!("{accept}If-None-Match: {identity_etag}\r\n");
    let (compressed, body) = http1_asset(&app, "GET", "/", &headers).await;
    assert!(compressed.starts_with("HTTP/1.1 200 "), "{compressed}");
    assert_eq!(body, ENTRY_BR);
    let brotli_etag = http1_header(&compressed, "etag").unwrap();
    assert_ne!(brotli_etag, identity_etag);
    let length = body.len().to_string();
    assert_eq!(http1_header(&compressed, "content-length"), Some(length.as_str()));

    let (head, body) = http1_asset(&app, "HEAD", "/", accept).await;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert!(body.is_empty());
    assert_eq!(http1_header(&head, "content-length"), Some(length.as_str()));

    let headers = format!("{accept}If-None-Match: {brotli_etag}\r\n");
    let (not_modified, body) = http1_asset(&app, "GET", "/", &headers).await;
    assert!(not_modified.starts_with("HTTP/1.1 304 "), "{not_modified}");
    assert!(body.is_empty());
    if let Some(revalidated_length) = http1_header(&not_modified, "content-length") {
        assert_eq!(revalidated_length, length);
    }
    for response in [&head, &not_modified] {
        for header in ["etag", "content-encoding", "vary", "cache-control", "link"] {
            assert_eq!(http1_header(response, header), http1_header(&compressed, header));
        }
    }
}

#[tokio::test]
async fn http1_negotiates_only_after_route_and_method_checks() {
    let app = Router::new().fallback(public_asset::<NegotiationAssets>);
    let missing = Router::new().fallback(public_asset::<MissingAssets>);
    let accept = "Accept-Encoding: br;q=0, identity;q=0\r\nIf-None-Match: *\r\n";
    for (app, method, path, status) in [
        (&app, "GET", "/assets/app.css", 406),
        (&app, "HEAD", "/assets/app.js", 406),
        (&app, "GET", "/favicon.ico", 406),
        (&app, "GET", "/", 406),
        (&app, "GET", "/assets/app.css.br", 404),
        (&app, "GET", "/assets/missing.css", 404),
        (&app, "POST", "/api/missing", 404),
        (&app, "POST", "/assets/app.css", 405),
        (&app, "POST", "/", 405),
        (&missing, "GET", "/favicon.ico", 404),
        (&missing, "POST", "/favicon.ico", 405),
        (&missing, "GET", "/", 503),
    ] {
        let (head, body) = http1_asset(app, method, path, accept).await;
        assert!(head.starts_with(&format!("HTTP/1.1 {status} ")), "{method} {path}: {head}");
        assert_eq!(http1_header(&head, "cache-control"), Some("no-store"));
        if status == 406 {
            assert_eq!(http1_header(&head, "vary"), Some("Accept-Encoding"));
            assert_eq!(http1_header(&head, "etag"), None);
            assert_eq!(http1_header(&head, "link"), None);
            assert!(body.is_empty());
        }
    }
}
