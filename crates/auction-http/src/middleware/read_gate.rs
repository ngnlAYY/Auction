//! 匿名公开读取的独立闸门：每来源固定窗口限流 + 进程内并发上限。
//! 本层只挂在匿名读路由组；出价、会话与身份读不经过这里，因此读突发既不能消耗写入配额，
//! 也不能同时占用全部服务池连接。拒绝立即返回，不把读请求堆积在等待队列中；来源超额与许可
//! 耗尽分别计入 `kind=anonymous_read` 的频率拒绝计数与读闸繁忙计数。

use std::net::SocketAddr;

use auction_observability::HttpRateLimitKind;
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::security::AnonymousReadGate;
use crate::errors::api_error;

/// 进入匿名读闸门：先核对每来源额度，再获取进程内并发许可。
pub(crate) async fn enforce(
    State(gate): State<AnonymousReadGate>,
    request: Request,
    next: Next,
) -> Response {
    // 边界中间件已把可信代理链归一化进 ConnectInfo；缺失时退回后续 handler 的原有行为。
    let Some(address) =
        request.extensions().get::<ConnectInfo<SocketAddr>>().map(|value| value.0.ip())
    else {
        return next.run(request).await;
    };
    // 计数只记录已作出的拒绝，判定、许可数、窗口与 Retry-After 都与未注入记录器时相同。
    if let Some(retry_after) = gate.allow(address) {
        gate.metrics().observe_rate_limit_rejection(HttpRateLimitKind::AnonymousRead);
        return busy_response("rate_limited", "读取请求过于频繁", retry_after);
    }
    let Some(_permit) = gate.try_acquire() else {
        gate.metrics().observe_read_gate_busy();
        return busy_response("read_busy", "读取并发已达上限", 1);
    };
    // 许可覆盖 handler 的完整生命周期（含数据库读取与响应映射），响应生成后自动归还。
    next.run(request).await
}

fn busy_response(code: &'static str, message: &'static str, retry_after: u64) -> Response {
    let mut response = api_error(StatusCode::TOO_MANY_REQUESTS, code, message).into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after.max(1).to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, sync::Arc, time::Duration};

    use auction_observability::ProcessMetrics;
    use axum::{
        Router,
        body::{Body, to_bytes},
        extract::ConnectInfo,
        http::{Request, StatusCode, header},
        middleware,
        routing::get,
    };
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::middleware::security::{AnonymousReadGate, HttpSecurityOptions, PublicHttpSecurity};
    use crate::state::PublicApiState;

    fn security(read_limit: u32, concurrency: u32) -> PublicHttpSecurity {
        PublicHttpSecurity::new(HttpSecurityOptions {
            cookie_secure: false,
            origins_enabled: false,
            allowed_origins: &[],
            login_limit: 10,
            mutation_limit: 60,
            anonymous_read_limit: read_limit,
            anonymous_read_concurrency: concurrency,
            rate_limit_window: Duration::from_secs(60),
            request_timeout: Duration::from_secs(15),
            trusted_proxy_cidrs: &[],
            trusted_unix_proxy: false,
        })
        .expect("valid security options")
    }

    async fn ok() -> &'static str {
        "ok"
    }

    /// 读组与写组共享同一进程；读闸门耗尽时写路由仍可直接通过。
    #[tokio::test]
    async fn saturated_read_gate_does_not_block_write_route() {
        let security = security(100, 1);
        let held = security.anonymous_read_gate().try_acquire().expect("first read permit");
        let reads = Router::new().route("/read", get(ok)).route_layer(
            middleware::from_fn_with_state(security.anonymous_read_gate(), super::enforce),
        );
        // 写路由不挂载读闸门，等同于出价等 mutation 路径。
        let app = reads.route("/write", get(ok));

        let read = app.clone().oneshot(request("/read")).await.expect("read response");
        assert_eq!(read.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(read.headers().contains_key(header::RETRY_AFTER));

        let write = app.oneshot(request("/write")).await.expect("write response");
        assert_eq!(write.status(), StatusCode::OK);

        // 写路由不受读许可影响；释放读许可后读组恢复。
        assert_eq!(held.num_permits(), 1);
    }

    #[tokio::test]
    async fn read_and_write_rate_limit_ledgers_are_independent() {
        let security = security(1, 4);
        let address = "192.0.2.10".parse().unwrap();
        assert_eq!(security.anonymous_read_gate().allow(address), None);
        assert!(security.anonymous_read_gate().allow(address).is_some());
        // 读额度耗尽不能消耗 mutation 或登录额度。
        assert_eq!(security.allow_mutation(address), None);
        assert_eq!(security.allow_login(address), None);
    }

    /// IPv6 同一 /64 共用读额度，跨 /64 使用新窗口。
    #[test]
    fn ipv6_reads_share_a_64_bit_key() {
        let security = security(1, 4);
        assert_eq!(security.anonymous_read_gate().allow("2001:db8:1:2::1".parse().unwrap()), None);
        assert!(
            security.anonymous_read_gate().allow("2001:db8:1:2:ffff::2".parse().unwrap()).is_some()
        );
        assert_eq!(security.anonymous_read_gate().allow("2001:db8:1:3::1".parse().unwrap()), None);
        // IPv4-mapped IPv6 与 IPv4 共用额度：先耗尽 IPv4 额度后，映射地址同样被拒。
        assert_eq!(security.anonymous_read_gate().allow("192.0.2.10".parse().unwrap()), None);
        assert!(
            security.anonymous_read_gate().allow("::ffff:192.0.2.10".parse().unwrap()).is_some()
        );
    }

    fn request(uri: &str) -> Request<Body> {
        request_from(uri, [192, 0, 2, 10])
    }

    fn request_from(uri: &str, address: [u8; 4]) -> Request<Body> {
        let mut request = Request::builder().uri(uri).body(Body::empty()).expect("request");
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from((address, 4000))));
        request
    }

    fn gated(gate: AnonymousReadGate) -> Router {
        Router::new()
            .route("/read", get(ok))
            .route_layer(middleware::from_fn_with_state(gate, super::enforce))
    }

    /// 一次读取的可观察结果：状态码、Retry-After 与正文。
    type Outcome = (StatusCode, Option<String>, Vec<u8>);

    async fn read_from(app: &Router, address: [u8; 4]) -> Outcome {
        let response = app.clone().oneshot(request_from("/read", address)).await.expect("response");
        let status = response.status();
        let retry_after = response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        (status, retry_after, body.to_vec())
    }

    fn error_code(outcome: &Outcome) -> Option<String> {
        let body = serde_json::from_slice::<Value>(&outcome.2).ok()?;
        body.get("code").and_then(Value::as_str).map(str::to_owned)
    }

    fn metrics() -> ProcessMetrics {
        ProcessMetrics::new(true, "auction-public-api", "public", "0.1.0").expect("metrics")
    }

    fn sample(metrics: &ProcessMetrics, name: &str, label: &str) -> Option<f64> {
        let text = String::from_utf8(metrics.encode().expect("encode")).expect("utf8");
        text.lines()
            .find(|line| line.starts_with(name) && line.contains(label))
            .and_then(|line| line.rsplit(' ').next())
            .and_then(|value| value.parse().ok())
    }

    /// 两类拒绝各自只累加自己的计数；放行路径不计数。
    #[tokio::test]
    async fn each_read_gate_rejection_increments_only_its_own_counter() {
        let metrics = metrics();
        let gate = security(1, 1).anonymous_read_gate().with_metrics(metrics.http());
        let app = gated(gate.clone());
        let counts = || {
            (
                sample(&metrics, "auction_http_read_gate_busy_total", "role=\"public\""),
                sample(
                    &metrics,
                    "auction_http_rate_limit_rejections_total",
                    "kind=\"anonymous_read\"",
                ),
            )
        };

        assert_eq!(read_from(&app, [192, 0, 2, 10]).await.0, StatusCode::OK);
        assert_eq!(counts(), (Some(0.0), Some(0.0)));
        // 另一来源仍有额度，但唯一许可被占用：容量拒绝只记读闸繁忙。
        let held = gate.try_acquire().expect("held permit");
        let busy = read_from(&app, [192, 0, 2, 11]).await;
        assert_eq!(error_code(&busy).as_deref(), Some("read_busy"));
        assert_eq!(counts(), (Some(1.0), Some(0.0)));
        drop(held);
        // 首个来源的窗口额度已用完：只记 anonymous_read 频率拒绝。
        let limited = read_from(&app, [192, 0, 2, 10]).await;
        assert_eq!(error_code(&limited).as_deref(), Some("rate_limited"));
        assert_eq!(counts(), (Some(1.0), Some(1.0)));
    }

    /// 装配根从已注入指标的 state 取闸门句柄：拒绝计数进入该进程的同一注册表。
    #[tokio::test]
    async fn state_gate_handle_carries_the_injected_recorder() {
        let metrics = Arc::new(metrics());
        let gate =
            PublicApiState::default().with_metrics(Arc::clone(&metrics)).anonymous_read_gate();
        let mut held = Vec::new();
        while let Some(permit) = gate.try_acquire() {
            held.push(permit);
        }
        let busy = read_from(&gated(gate), [192, 0, 2, 10]).await;
        assert_eq!(error_code(&busy).as_deref(), Some("read_busy"));
        assert_eq!(
            sample(&metrics, "auction_http_read_gate_busy_total", "role=\"public\""),
            Some(1.0)
        );
    }

    /// 记录器只观测：同一序列的状态码、Retry-After 与正文与未注入记录器时完全相同。
    #[tokio::test]
    async fn recorder_does_not_change_read_gate_decisions() {
        let metrics = metrics();
        let mut runs = Vec::new();
        for gate in [
            security(1, 1).anonymous_read_gate(),
            security(1, 1).anonymous_read_gate().with_metrics(metrics.http()),
        ] {
            let app = gated(gate.clone());
            let allowed = read_from(&app, [192, 0, 2, 10]).await;
            let held = gate.try_acquire().expect("held permit");
            let busy = read_from(&app, [192, 0, 2, 11]).await;
            drop(held);
            let limited = read_from(&app, [192, 0, 2, 10]).await;
            runs.push([allowed, busy, limited]);
        }
        assert_eq!(runs[0], runs[1]);
        let [allowed, busy, limited] = &runs[0];
        assert_eq!((allowed.0, allowed.1.as_deref()), (StatusCode::OK, None));
        assert_eq!((busy.0, busy.1.as_deref()), (StatusCode::TOO_MANY_REQUESTS, Some("1")));
        assert_eq!(limited.0, StatusCode::TOO_MANY_REQUESTS);
        assert!(limited.1.is_some());
    }
}
