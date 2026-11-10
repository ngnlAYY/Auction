// 匿名公开读取的独立闸门：每来源固定窗口限流 + 进程内并发上限。
// 本层只挂在匿名读路由组；出价、会话与身份读不经过这里，因此读突发既不能消耗写入配额，
// 也不能同时占用全部服务池连接。拒绝立即返回，不把读请求堆积在等待队列中。

use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::security::AnonymousReadGate;

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
    if let Some(retry_after) = gate.allow(address) {
        return busy_response("rate_limited", "读取请求过于频繁", retry_after);
    }
    let Some(_permit) = gate.try_acquire() else {
        return busy_response("read_busy", "读取并发已达上限", 1);
    };
    // 许可覆盖 handler 的完整生命周期（含数据库读取与响应映射），响应生成后自动归还。
    next.run(request).await
}

fn busy_response(code: &'static str, message: &'static str, retry_after: u64) -> Response {
    let mut response =
        crate::mapping::api_error(StatusCode::TOO_MANY_REQUESTS, code, message).into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after.max(1).to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use axum::{
        Router,
        body::Body,
        extract::ConnectInfo,
        http::{Request, StatusCode, header},
        middleware,
        routing::get,
    };
    use tower::ServiceExt;

    use crate::middleware::security::{HttpSecurityOptions, PublicHttpSecurity};

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
        let mut request = Request::builder().uri(uri).body(Body::empty()).expect("request");
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 10], 4000))));
        request
    }
}
