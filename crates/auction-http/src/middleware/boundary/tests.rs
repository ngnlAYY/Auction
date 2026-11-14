use super::*;

fn headers(value: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(FORWARDED_FOR_HEADER, HeaderValue::from_static(value));
    // CDN 专用头不能自行授予信任；可信上游应把核验后的链写入 XFF。
    headers.insert("cf-connecting-ip", HeaderValue::from_static("192.0.2.99"));
    headers
}

#[test]
fn cdn_chain_ignores_spoofed_left_prefix_and_untrusted_peers() {
    let boundary = HttpBoundary::new(
        Duration::from_secs(15),
        &["127.0.0.1/32".to_owned(), "173.245.48.0/20".to_owned()],
        false,
    )
    .unwrap();
    let chain = headers("192.0.2.123, 198.51.100.7, 173.245.48.10");
    assert_eq!(
        resolve_client_address(Some("127.0.0.1:1234".parse().unwrap()), &chain, &boundary),
        "198.51.100.7".parse::<IpAddr>().unwrap()
    );
    assert_eq!(
        resolve_client_address(Some("203.0.113.10:1234".parse().unwrap()), &chain, &boundary),
        "203.0.113.10".parse::<IpAddr>().unwrap()
    );
}

#[test]
fn unix_proxy_requires_explicit_trust_and_cdn_cidrs() {
    let chain = headers("198.51.100.7, 173.245.48.10");
    let no_trust = HttpBoundary::default();
    assert_eq!(resolve_client_address(None, &chain, &no_trust), Ipv4Addr::UNSPECIFIED);
    let trusted_unix = HttpBoundary::new(Duration::from_secs(15), &[], true).unwrap();
    assert_eq!(
        resolve_client_address(None, &chain, &trusted_unix),
        "173.245.48.10".parse::<IpAddr>().unwrap()
    );
    let with_cdn =
        HttpBoundary::new(Duration::from_secs(15), &["173.245.48.0/20".to_owned()], true).unwrap();
    assert_eq!(
        resolve_client_address(None, &chain, &with_cdn),
        "198.51.100.7".parse::<IpAddr>().unwrap()
    );
    assert_eq!(
        resolve_client_address(None, &headers("198.51.100.7, bad"), &with_cdn),
        Ipv4Addr::UNSPECIFIED
    );
}

#[test]
fn universal_proxy_trust_is_rejected_at_http_boundary() {
    for network in ["0.0.0.0/0", "::/0"] {
        assert!(HttpBoundary::new(Duration::from_secs(15), &[network.to_owned()], false).is_err());
    }
}

#[tokio::test]
async fn boundary_sets_hsts_only_for_proxied_https_requests() {
    use axum::{Router, middleware, routing::get};
    use tower::ServiceExt;

    let boundary =
        HttpBoundary::new(Duration::from_secs(15), &["127.0.0.1/32".to_owned()], false).unwrap();
    let app = Router::new()
        .route("/ping", get(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(boundary, enforce));

    let build = |peer: &str, proto: Option<&'static str>| {
        let mut builder = Request::builder().uri("/ping");
        if let Some(proto) = proto {
            builder = builder.header(FORWARDED_PROTO_HEADER, proto);
        }
        let mut request = builder.body(Body::empty()).expect("request");
        request.extensions_mut().insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        request
    };

    let secure = app.clone().oneshot(build("127.0.0.1:5000", Some("https"))).await.unwrap();
    assert_eq!(secure.status(), StatusCode::OK);
    assert!(secure.headers().contains_key(header::STRICT_TRANSPORT_SECURITY));
    assert!(secure.headers().contains_key(header::CONTENT_SECURITY_POLICY));

    // 开发/测试与未经代理的明文请求没有协议证明，不能下发 HSTS。
    let plain = app.clone().oneshot(build("127.0.0.1:5000", None)).await.unwrap();
    assert_eq!(plain.status(), StatusCode::OK);
    assert!(!plain.headers().contains_key(header::STRICT_TRANSPORT_SECURITY));
    assert!(plain.headers().contains_key(header::CONTENT_SECURITY_POLICY));

    let untrusted = app.oneshot(build("203.0.113.9:5000", Some("https"))).await.unwrap();
    assert_eq!(untrusted.status(), StatusCode::OK);
    assert!(!untrusted.headers().contains_key(header::STRICT_TRANSPORT_SECURITY));
}

#[test]
fn replayable_responses_do_not_report_the_process_build_id() {
    // 共享缓存可保存的响应会在部署后被重放；它们带旧构建号会让新页面误判版本过期。
    for cache_control in ["public, max-age=300, s-maxage=300", "no-cache"] {
        let mut response = StatusCode::OK.into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control));
        let response = finish_response(response, "request-1", false);
        assert!(
            !response.headers().contains_key(crate::server::build_identity::BUILD_ID_HEADER),
            "cache_control={cache_control}"
        );
    }
    // 只给浏览器自己保存的进行中读取仍然报告构建号。
    let mut response = StatusCode::OK.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=2"));
    let response = finish_response(response, "request-1", false);
    assert!(response.headers().contains_key(crate::server::build_identity::BUILD_ID_HEADER));
}

#[test]
fn non_replayable_responses_report_the_process_build_id() {
    let response = finish_response(StatusCode::NO_CONTENT.into_response(), "request-1", false);
    let reported = response
        .headers()
        .get(crate::server::build_identity::BUILD_ID_HEADER)
        .and_then(|value| value.to_str().ok());
    assert_eq!(reported, Some(crate::server::build_identity::build_id()));
    // 安全头是每个响应的固定部分；构建号随所有不可重放的响应出现，不限于成功正文。
    assert!(response.headers().contains_key(header::CONTENT_SECURITY_POLICY));
    // 开发与测试默认是明文 HTTP：没有协议证明时不能写 HSTS。
    assert!(!response.headers().contains_key(header::STRICT_TRANSPORT_SECURITY));
}

#[test]
fn https_responses_receive_hsts() {
    let response = finish_response(StatusCode::NO_CONTENT.into_response(), "request-1", true);
    assert_eq!(
        response
            .headers()
            .get(header::STRICT_TRANSPORT_SECURITY)
            .and_then(|value| value.to_str().ok()),
        Some("max-age=31536000")
    );
}

fn proto_request(peer: Option<&str>, values: &[&str]) -> Request {
    let mut builder = Request::builder().uri("/api/v1/auctions");
    for value in values {
        builder = builder.header(FORWARDED_PROTO_HEADER, *value);
    }
    let mut request = builder.body(Body::empty()).expect("request");
    if let Some(peer) = peer {
        request.extensions_mut().insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    }
    request
}

#[test]
fn proxy_protocol_only_counts_from_trusted_peers() {
    let trusted =
        HttpBoundary::new(Duration::from_secs(15), &["127.0.0.1/32".to_owned()], false).unwrap();
    // 可信回环代理转发 https：浏览器确实经 HTTPS 访问。
    assert!(request_is_https(&proto_request(Some("127.0.0.1:5000"), &["https"]), &trusted));
    // 未配置的直连来源自报 https 不能授予 HSTS。
    assert!(!request_is_https(&proto_request(Some("203.0.113.5:5000"), &["https"]), &trusted));
    // 代理声明明文时不发送。
    assert!(!request_is_https(&proto_request(Some("127.0.0.1:5000"), &["http"]), &trusted));
    // 缺失头部与无 peer 的直连同样是明文。
    assert!(!request_is_https(&proto_request(Some("127.0.0.1:5000"), &[]), &trusted));
    assert!(!request_is_https(&proto_request(None, &["https"]), &trusted));
    // 只有显式信任 Unix 上游时，无 peer 的转发证明才生效。
    let trusted_unix = HttpBoundary::new(Duration::from_secs(15), &[], true).unwrap();
    assert!(request_is_https(&proto_request(None, &["https"]), &trusted_unix));
}

#[test]
fn absolute_https_request_target_is_secure() {
    let boundary = HttpBoundary::default();
    let request = Request::builder()
        .uri("https://auction.example.test/api/v1/auctions")
        .body(Body::empty())
        .expect("request");
    assert!(request_is_https(&request, &boundary));
}

#[test]
fn ambiguous_proxy_protocol_never_yields_hsts() {
    let trusted =
        HttpBoundary::new(Duration::from_secs(15), &["127.0.0.1/32".to_owned()], false).unwrap();
    // 代理链中任一跳是明文，或出现重复/非法值，都退回不发 HSTS。
    assert!(!request_is_https(&proto_request(Some("127.0.0.1:5000"), &["https, http"]), &trusted));
    assert!(!request_is_https(
        &proto_request(Some("127.0.0.1:5000"), &["https", "http"]),
        &trusted
    ));
    assert!(!request_is_https(&proto_request(Some("127.0.0.1:5000"), &["wss"]), &trusted));
    assert!(!request_is_https(&proto_request(Some("127.0.0.1:5000"), &[""]), &trusted));
    // 全链路都声明 https 的常见 CDN → 自建代理场景仍然发送；大小写与空白按 token 处理。
    assert!(request_is_https(&proto_request(Some("127.0.0.1:5000"), &["https, https"]), &trusted));
    assert!(request_is_https(&proto_request(Some("127.0.0.1:5000"), &["HTTPS"]), &trusted));
}
