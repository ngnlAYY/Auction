//! 取回客户端的传输边界：重定向、状态、表示、大小与源站地址。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use url::Url;

use super::{WebFetchClient, WebFetchError, WebFetchPolicy, WebRepresentation};

/// 回环替身：每个连接读一个请求，按 `(路径, Accept-Encoding)` 交给处理函数得到整段原始响应。
fn serve(handler: impl Fn(&str, &str) -> Vec<u8> + Send + 'static) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
            let accept = head
                .lines()
                .find_map(|line| line.strip_prefix("accept-encoding: "))
                .unwrap_or("")
                .to_owned();
            let _ = stream.write_all(&handler(&path, &accept));
        }
    });
    origin
}

fn response(status: &str, headers: &[&str], body: &[u8]) -> Vec<u8> {
    let mut out =
        format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n", body.len());
    for header in headers {
        out.push_str(header);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn client(origin: &Url, max_file_bytes: u64) -> WebFetchClient {
    let policy = WebFetchPolicy { max_file_bytes, ..WebFetchPolicy::default() };
    WebFetchClient::new(origin, policy).unwrap()
}

#[tokio::test]
async fn web_fetch_fetch_returns_exact_entity_bytes_per_representation() {
    let origin = serve(|path, accept| match (path, accept) {
        ("/a.js", "identity") => response("200 OK", &[], b"plain"),
        ("/a.js", "br") => response("200 OK", &["content-encoding: br"], b"\x01\x02raw-br"),
        _ => response("404 Not Found", &[], b""),
    });
    let client = client(&origin, 1024);
    assert_eq!(client.get("a.js", WebRepresentation::Identity).await.unwrap().body, b"plain");
    assert_eq!(
        client.get("a.js", WebRepresentation::Brotli).await.unwrap().body,
        b"\x01\x02raw-br"
    );
}

#[tokio::test]
async fn web_fetch_fetch_rejects_redirect_status_and_wrong_encoding() {
    let origin = serve(|path, _| match path {
        "/moved.js" => response("302 Found", &["location: /elsewhere.js"], b""),
        "/missing.js" => response("404 Not Found", &[], b"x"),
        // 请求 identity 却得到 br 实体：不能当作原文。
        "/encoded.js" => response("200 OK", &["content-encoding: br"], b"zz"),
        _ => response("200 OK", &[], b"ok"),
    });
    let client = client(&origin, 1024);
    for path in ["moved.js", "missing.js"] {
        let error = client.get(path, WebRepresentation::Identity).await.unwrap_err();
        assert!(matches!(error, WebFetchError::Status { .. }), "{path}: {error}");
    }
    let error = client.get("encoded.js", WebRepresentation::Identity).await.unwrap_err();
    assert!(matches!(error, WebFetchError::Encoding { .. }));
    // 请求 br 却得到无编码的响应：同样拒绝。
    let error = client.get("plain.js", WebRepresentation::Brotli).await.unwrap_err();
    assert!(matches!(error, WebFetchError::Encoding { .. }));
}

#[tokio::test]
async fn web_fetch_fetch_enforces_size_limit() {
    let origin = serve(|path, _| match path {
        "/big.js" => response("200 OK", &[], &[7_u8; 100]),
        _ => response("200 OK", &[], b"ok"),
    });
    let client = client(&origin, 10);
    let error = client.get("big.js", WebRepresentation::Identity).await.unwrap_err();
    assert!(matches!(error, WebFetchError::TooLarge { .. }));
    assert_eq!(client.get("small.js", WebRepresentation::Identity).await.unwrap().body, b"ok");
}

#[test]
fn web_fetch_origin_must_be_bare_loopback() {
    for bad in [
        "http://example.com/",
        "http://192.0.2.1:8080/",
        "http://user:pw@127.0.0.1:1/",
        "http://127.0.0.1:1/base/",
        "http://127.0.0.1:1/?a=b",
        "ftp://127.0.0.1/",
    ] {
        let origin = Url::parse(bad).unwrap();
        assert!(
            matches!(
                WebFetchClient::new(&origin, WebFetchPolicy::default()),
                Err(WebFetchError::InvalidOrigin(_))
            ),
            "{bad}"
        );
    }
    for good in ["http://127.0.0.1:9/", "http://localhost:9/", "http://[::1]:9/"] {
        assert!(WebFetchClient::new(&Url::parse(good).unwrap(), WebFetchPolicy::default()).is_ok());
    }
}

#[tokio::test]
async fn web_fetch_fetch_rejects_unsafe_paths_before_any_request() {
    let origin = Url::parse("http://127.0.0.1:9/").unwrap();
    let client = client(&origin, 1024);
    for path in [
        "",
        "/abs.js",
        "../x.js",
        "a/../x.js",
        "a//b.js",
        "a\\b.js",
        "a.js?x=1",
        "a#b",
        "a%2e.js",
        "a.js.br",
    ] {
        let error = client.get(path, WebRepresentation::Identity).await.unwrap_err();
        assert!(matches!(error, WebFetchError::InvalidPath(_)), "{path:?}");
    }
}

/// 辅助用例的源站地址经环境变量传入；平时被 `#[ignore]` 跳过，只由下面的用例在子进程里点名运行。
const PROXY_CHILD_ORIGIN: &str = "AUCTION_TEST_WEB_FETCH_PROXY_ORIGIN";

#[tokio::test]
#[ignore = "只作为 web_fetch_ignores_proxy_environment 的子进程辅助用例"]
async fn web_fetch_proxy_child_fetches_once() {
    let Ok(origin) = std::env::var(PROXY_CHILD_ORIGIN) else { return };
    let client = client(&Url::parse(&origin).unwrap(), 1024);
    let fetched = client.get("a.js", WebRepresentation::Identity).await.unwrap();
    assert_eq!(fetched.body, b"origin-bytes");
}

/// 保护的安全边界：取旧字节不得经代理（代理可改写字节）。2024 版次不在测试进程里 `set_var`，
/// 所以重新执行当前测试二进制，在子进程的环境里设置各个代理变量。
#[test]
fn web_fetch_ignores_proxy_environment() {
    let origin_hits = Arc::new(AtomicUsize::new(0));
    let origin = serve({
        let origin_hits = Arc::clone(&origin_hits);
        move |_, _| {
            origin_hits.fetch_add(1, Ordering::SeqCst);
            response("200 OK", &[], b"origin-bytes")
        }
    });
    let proxy_hits = Arc::new(AtomicUsize::new(0));
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_address = format!("http://{}", proxy.local_addr().unwrap());
    {
        let proxy_hits = Arc::clone(&proxy_hits);
        // 计数的代理：只数连接并立即关闭；监听线程随测试进程结束。
        thread::spawn(move || {
            for _connection in proxy.incoming() {
                proxy_hits.fetch_add(1, Ordering::SeqCst);
            }
        });
    }

    // 子进程按路径精确点名辅助用例；名字不符时辅助用例不会运行，源站计数断言会失败。
    let helper = format!(
        "{}::web_fetch_proxy_child_fetches_once",
        module_path!().split_once("::").unwrap().1
    );
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", helper.as_str(), "--ignored", "--test-threads=1"]);
    command.env(PROXY_CHILD_ORIGIN, origin.as_str());
    for name in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        command.env(name, &proxy_address);
    }
    for name in ["NO_PROXY", "no_proxy"] {
        command.env_remove(name);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(proxy_hits.load(Ordering::SeqCst), 0, "请求经过了代理");
    assert!(origin_hits.load(Ordering::SeqCst) >= 1, "源站没有收到请求");
}
