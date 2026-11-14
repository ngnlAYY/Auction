//! Public/Admin 匿名浏览器错误信标。
//! 该端点只接受闭集类别与严格格式的构建号，不读取或记录原始错误正文，不触碰数据库；成功
//! 仅累加进程内计数。请求体必须为空，限流使用独立账本，避免与登录或业务 mutation 配额互相
//! 消耗。未通过校验或限流的请求不会增加类别计数。

use crate::mapping::{api_error, parse_query};
/// 构建号与响应头、实时首帧同源；只比较是否等于当前进程构建号，不把上报值写入指标标签。
use crate::server::build_identity::build_id;
use crate::session::rate_limited_response;
use crate::state::{AdminApiState, PublicApiState};
use auction_observability::{BrowserErrorClass, HttpRateLimitKind, ProcessMetrics};
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use std::net::{IpAddr, SocketAddr};

/// Public surface 的信标入口。
pub(crate) async fn public(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    handle(
        address,
        state.metrics(),
        |address| state.security().allow_browser_error(address),
        request,
    )
    .await
}

/// Admin surface 的信标入口；与 Public 同路径、同校验、独立进程计数。
pub(crate) async fn admin(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    handle(
        address,
        state.metrics(),
        |address| state.security().allow_browser_error(address),
        request,
    )
    .await
}

async fn handle(
    address: SocketAddr,
    metrics: Option<&ProcessMetrics>,
    allow: impl FnOnce(IpAddr) -> Option<u64>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    // 显式声明任何非零 Content-Length 都直接拒绝，不等待正文；chunked 或缺失声明的正文由
    // 零预算读取再次确认。这里不缓冲、不解析、也不记录正文。
    if parts.headers.get(header::CONTENT_LENGTH).is_some_and(|value| value.as_bytes() != b"0") {
        return unexpected_body();
    }
    match to_bytes(body, 1).await {
        Ok(bytes) if bytes.is_empty() => {}
        _ => return unexpected_body(),
    }

    let Ok(report) = parse_report(&parts.uri, build_id()) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    };

    if let Some(retry_after) = allow(address.ip()) {
        if let Some(metrics) = metrics {
            metrics.http().observe_rate_limit_rejection(HttpRateLimitKind::BrowserError);
        }
        return rate_limited_response(retry_after);
    }

    if let Some(metrics) = metrics {
        let http = metrics.http();
        http.observe_browser_error(report.class);
        if !report.build_matches {
            http.observe_browser_error_build_mismatch();
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

struct Report {
    class: BrowserErrorClass,
    build_matches: bool,
}

fn parse_report(uri: &Uri, expected_build: &str) -> Result<Report, ()> {
    let mut query = parse_query(uri, &["class", "build"])?;
    let class = match query.remove("class").ok_or(())?.as_str() {
        "wasm_panic" => BrowserErrorClass::WasmPanic,
        "dto_decode" => BrowserErrorClass::DtoDecode,
        "realtime_protocol" => BrowserErrorClass::RealtimeProtocol,
        "i18n_format" => BrowserErrorClass::I18nFormat,
        _ => return Err(()),
    };
    let build = query.remove("build").ok_or(())?;
    if !valid_build_id(&build) {
        return Err(());
    }
    Ok(Report { class, build_matches: build == expected_build })
}

/// 与 `auction-web-shared::build_id_is_valid` 保持同一闭集；只允许 URL 安全字符。
fn valid_build_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn unexpected_body() -> Response {
    api_error(StatusCode::BAD_REQUEST, "unexpected_body", "该接口不接受请求体").into_response()
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::public::browser_errors as parent;
    use auction_observability::ProcessMetrics;
    use axum::body::Body;
    use std::sync::Arc;

    const ADDRESS: SocketAddr = SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 41234);

    fn state() -> PublicApiState {
        PublicApiState::default().with_metrics(Arc::new(
            ProcessMetrics::new(true, "auction-public-api", "public", "0.1.0")
                .expect("metrics registry"),
        ))
    }

    async fn send(state: &PublicApiState, uri: &str, body: Body) -> Response {
        let request = Request::builder().method("POST").uri(uri).body(body).unwrap();
        public(State(state.clone()), ConnectInfo(ADDRESS), request).await
    }

    fn metrics_text(state: &PublicApiState) -> String {
        let metrics = state.metrics().expect("metrics state");
        String::from_utf8(metrics.encode().expect("encode")).expect("utf8")
    }

    fn counter_is(state: &PublicApiState, name: &str, labels: &[&str], value: &str) -> bool {
        metrics_text(state).lines().any(|line| {
            line.starts_with(name)
                && labels.iter().all(|label| line.contains(label))
                && line.ends_with(&format!(" {value}"))
        })
    }

    #[test]
    fn build_id_validation_is_closed() {
        assert!(valid_build_id("0.1.0"));
        assert!(valid_build_id("build_2026-11-04.abc"));
        assert!(!valid_build_id(""));
        assert!(!valid_build_id("with space"));
        assert!(!valid_build_id("bad/slash"));
        assert!(!valid_build_id("bad?query"));
        assert!(!valid_build_id(&"a".repeat(65)));
    }

    #[test]
    fn query_accepts_only_closed_classes_and_required_build() {
        assert_eq!(
            parse_report(
                &"/api/v1/browser-errors?class=wasm_panic&build=abc".parse().unwrap(),
                "abc"
            )
            .unwrap()
            .class,
            BrowserErrorClass::WasmPanic
        );
        assert_eq!(
            parse_report(
                &"/api/v1/browser-errors?class=dto_decode&build=abc".parse().unwrap(),
                "abc"
            )
            .unwrap()
            .class,
            BrowserErrorClass::DtoDecode
        );
        assert_eq!(
            parse_report(
                &"/api/v1/browser-errors?class=realtime_protocol&build=abc".parse().unwrap(),
                "abc"
            )
            .unwrap()
            .class,
            BrowserErrorClass::RealtimeProtocol
        );
        for uri in [
            "/api/v1/browser-errors",
            "/api/v1/browser-errors?class=wasm_panic",
            "/api/v1/browser-errors?build=abc",
            "/api/v1/browser-errors?class=unknown&build=abc",
            "/api/v1/browser-errors?class=wasm_panic&build=abc&extra=1",
            "/api/v1/browser-errors?class=wasm_panic&class=dto_decode&build=abc",
            "/api/v1/browser-errors?class=wasm_panic&build=",
            "/api/v1/browser-errors?class=wasm_panic&build=bad%20value",
        ] {
            assert!(parse_report(&uri.parse().unwrap(), "abc").is_err(), "{uri}");
        }
    }

    #[test]
    fn build_mismatch_is_derived_without_echoing_value() {
        let matched = parse_report(
            &"/api/v1/browser-errors?class=dto_decode&build=abc".parse().unwrap(),
            "abc",
        )
        .unwrap();
        assert!(matched.build_matches);
        let stale = parse_report(
            &"/api/v1/browser-errors?class=dto_decode&build=old-build".parse().unwrap(),
            "abc",
        )
        .unwrap();
        assert!(!stale.build_matches);
    }

    #[tokio::test]
    async fn accepted_classes_return_no_content_and_increment_one_counter() {
        let state = state();
        for class in ["wasm_panic", "dto_decode", "realtime_protocol", "i18n_format"] {
            let uri = format!("/api/v1/browser-errors?class={class}&build={}", build_id());
            let response = send(&state, &uri, Body::empty()).await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT, "{class}");
            assert!(counter_is(
                &state,
                "auction_browser_errors_total",
                &[&format!("class=\"{class}\""), "role=\"public\""],
                "1"
            ));
        }
        assert!(counter_is(
            &state,
            "auction_browser_error_build_mismatches_total",
            &["role=\"public\""],
            "0"
        ));
    }

    #[tokio::test]
    async fn stale_build_increments_only_the_mismatch_counter() {
        let state = state();
        let response =
            send(&state, "/api/v1/browser-errors?class=dto_decode&build=old-build", Body::empty())
                .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(counter_is(
            &state,
            "auction_browser_error_build_mismatches_total",
            &["role=\"public\""],
            "1"
        ));
        assert!(!metrics_text(&state).contains("old-build"));
    }

    #[tokio::test]
    async fn any_declared_or_actual_body_is_rejected_without_counting() {
        let state = state();
        let uri = format!("/api/v1/browser-errors?class=dto_decode&build={}", build_id());
        let response = send(&state, &uri, Body::from("error text")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let request = Request::builder()
            .method("POST")
            .uri(&uri)
            .header(header::CONTENT_LENGTH, "12")
            .body(Body::empty())
            .unwrap();
        let response = public(State(state.clone()), ConnectInfo(ADDRESS), request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        assert!(!metrics_text(&state).contains("auction_browser_errors_total{"));
    }

    #[tokio::test]
    async fn invalid_query_is_rejected_without_counting() {
        let state = state();
        let response =
            send(&state, "/api/v1/browser-errors?class=unknown&build=abc", Body::empty()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!metrics_text(&state).contains("auction_browser_errors_total{"));
    }

    #[tokio::test]
    async fn per_source_budget_is_enforced_and_observable() {
        let state = state();
        let uri = format!("/api/v1/browser-errors?class=wasm_panic&build={}", build_id());
        for _ in 0..30 {
            assert_eq!(send(&state, &uri, Body::empty()).await.status(), StatusCode::NO_CONTENT);
        }
        let rejected = send(&state, &uri, Body::empty()).await;
        assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(rejected.headers().contains_key(header::RETRY_AFTER));
        assert!(counter_is(
            &state,
            "auction_http_rate_limit_rejections_total",
            &["kind=\"browser_error\"", "role=\"public\""],
            "1"
        ));
        assert!(counter_is(
            &state,
            "auction_browser_errors_total",
            &["class=\"wasm_panic\"", "role=\"public\""],
            "30"
        ));
    }
}
