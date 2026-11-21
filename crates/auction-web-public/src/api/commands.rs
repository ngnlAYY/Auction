//! Public 状态命令的 HTTP 请求。
//! 浏览器只生成稳定重试所需的 `command_id` 并提交契约 DTO；金额、资格、锁和幂等结果仍由
//! 服务端应用端口决定。

#[cfg(test)]
use std::{cell::Cell, future::Future, pin::Pin};

use auction_contracts::{
    CreateProxyAuthorizationRequest, LowerProxyAuthorizationRequest, PlaceBidRequest,
};
pub(crate) use auction_contracts::{
    PlaceBidResultResponse as BidResult,
    PublicProxyCommandAuthorizationResponse as ProxyCommandAuthorization,
};
use gloo_net::http::Request;
use serde::Serialize;
use uuid::Uuid;

use super::clock::send_response_observed;
#[cfg(test)]
use super::clock::{HttpFixture, HttpHeadFixtureFuture, HttpResponseFixture};
use super::session::identity::{
    SessionIdentityObservation, SessionRefreshReason, observe_public_session_failure_observed,
    request_public_session_check,
};
use super::{PublicSessionApiError, TransportError, decoder};

fn valid_path_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

fn invalid_path_request() -> CommandError {
    CommandError { code: "invalid_request".to_owned(), status: 400 }
}

const NOT_SENT: &str = "command_not_sent";

#[cfg(test)]
type WireReply = Pin<Box<dyn Future<Output = Result<(u16, Option<String>), TransportError>>>>;
#[cfg(test)]
type WireTransport = fn(&str, &str, &[u8]) -> Result<WireReply, TransportError>;

#[cfg(test)]
type WireTransportWithHead = fn(&str, &str, &[u8]) -> Result<HttpHeadFixtureFuture, TransportError>;

#[cfg(test)]
#[derive(Clone, Copy)]
enum WireFixture {
    Buffered(WireTransport),
    WithHead(WireTransportWithHead),
}

#[cfg(test)]
thread_local! {
    static WIRE_TRANSPORT: Cell<Option<WireFixture>> = const { Cell::new(None) };
}

/// Public 命令失败的稳定错误投影；不把服务端说明正文带入浏览器展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandError {
    pub code: String,
    pub status: u16,
}

impl CommandError {
    /// 本次尝试在调用 fetch 之前结束。
    pub(crate) fn before_send() -> Self {
        Self { code: NOT_SENT.to_owned(), status: 0 }
    }

    /// 缺少明确业务结果时只允许重放原命令；新输入仍须经过正常表单校验。
    pub(crate) fn outcome_uncertain(&self) -> bool {
        // 浏览器会话锁失败发生在 HTTP 发送前，不把首次拒绝误报为未知外写。
        !self.not_sent()
            && !matches!(self.code.as_str(), "session_lock_unavailable" | "session_lock_timeout")
            && (self.status == 0 || self.status >= 500 || self.code == "invalid_response")
    }

    /// 仅由竞争命令的本地传输边界产生，HTTP 响应不能冒充这个状态。
    pub(crate) fn not_sent(&self) -> bool {
        self.status == 0 && self.code == NOT_SENT
    }

    #[cfg(test)]
    pub(crate) fn intercept_test_requests(transport: WireTransport) {
        WIRE_TRANSPORT.set(Some(WireFixture::Buffered(transport)));
    }

    #[cfg(test)]
    pub(crate) fn intercept_test_requests_with_head(transport: WireTransportWithHead) {
        WIRE_TRANSPORT.set(Some(WireFixture::WithHead(transport)));
    }

    fn transport(error: TransportError, response_status: Option<u16>) -> Self {
        let unauthorized =
            response_status == Some(401) || matches!(error, TransportError::Network(Some(401)));
        // 未完成的正文不能了结命令。保留 401 供命令宿主解释尚未确定的结果。
        Self { code: error.code().to_owned(), status: if unauthorized { 401 } else { 0 } }
    }

    fn invalid_response(status: u16) -> Self {
        Self { code: "invalid_response".to_owned(), status }
    }
}

pub(crate) async fn place_bid(
    user_id: Uuid,
    lot_id: String,
    amount: i64,
    command_id: Uuid,
) -> Result<BidResult, CommandError> {
    if !valid_path_uuid(&lot_id) {
        return Err(invalid_path_request());
    }
    let url = format!("/api/v1/lots/{lot_id}/bids");
    let command = PlaceBidRequest { command_id: command_id.to_string(), amount };
    send_command(
        user_id,
        async |invalidation| post_command(&url, user_id, &command, invalidation).await,
        |body| decoder::decode_bid_command(body, &lot_id),
    )
    .await
}

/// 创建代理授权并解码旧契约要求的完整命令投影。
pub(crate) async fn create_proxy_authorization(
    user_id: Uuid,
    lot_id: String,
    maximum_amount: i64,
    mode: String,
    command_id: Uuid,
) -> Result<ProxyCommandAuthorization, CommandError> {
    if !valid_path_uuid(&lot_id) {
        return Err(invalid_path_request());
    }
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations");
    let command = CreateProxyAuthorizationRequest {
        command_id: command_id.to_string(),
        maximum_amount,
        mode: mode.clone(),
    };
    send_command(
        user_id,
        async |invalidation| post_command(&url, user_id, &command, invalidation).await,
        |body| decoder::decode_create_proxy_command(body, &lot_id, maximum_amount, &mode),
    )
    .await
}

/// 降低代理授权上限并解码旧契约要求的完整命令投影。
pub(crate) async fn lower_proxy_authorization(
    user_id: Uuid,
    lot_id: String,
    authorization_id: String,
    maximum_amount: i64,
    command_id: Uuid,
) -> Result<ProxyCommandAuthorization, CommandError> {
    if !valid_path_uuid(&lot_id) || !valid_path_uuid(&authorization_id) {
        return Err(invalid_path_request());
    }
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower");
    let command =
        LowerProxyAuthorizationRequest { command_id: command_id.to_string(), maximum_amount };
    send_command(
        user_id,
        async |invalidation| post_command(&url, user_id, &command, invalidation).await,
        |body| {
            decoder::decode_lower_proxy_command(body, &lot_id, &authorization_id, maximum_amount)
        },
    )
    .await
}

#[derive(Default)]
struct CommandInvalidation {
    request_built: bool,
    response_status: Option<u16>,
}

impl CommandInvalidation {
    fn built<T>(&mut self, request: Result<T, TransportError>) -> Result<T, TransportError> {
        let request = request?;
        self.request_built = true;
        Ok(request)
    }
}

impl Drop for CommandInvalidation {
    fn drop(&mut self) {
        // 请求构建成功后可能已提交。无论传输返回或任务取消，权威重读都须越过命令前的快照。
        if self.request_built {
            super::swr_cache::invalidate_after_command();
        }
    }
}

struct CommandResponse {
    status: u16,
    body: Option<String>,
}

fn observe_command_head(response_status: &mut Option<u16>, status: u16) {
    *response_status = Some(status);
    // 401 发起复核并暂停新写入和重试。正文收口前仍保留原命令的归属。
    if status == 401 {
        super::session::broadcast_session_change();
        request_public_session_check(SessionRefreshReason::AuthenticationFailed);
    }
}

async fn post_command(
    url: &str,
    user_id: Uuid,
    command: &impl Serialize,
    invalidation: &mut CommandInvalidation,
) -> Result<CommandResponse, TransportError> {
    // 复用同一份序列化正文建立真实请求或交给传输替身；构造失败时还没有调用 fetch。
    let Ok(body) = serde_json::to_string(command) else {
        return Err(TransportError::Decode);
    };
    let expected_user = user_id.to_string();
    #[cfg(test)]
    let fixture = WIRE_TRANSPORT
        .get()
        .map(|fixture| {
            let response = match fixture {
                WireFixture::Buffered(transport) => transport(url, &expected_user, body.as_bytes())
                    .map(|response| {
                        HttpFixture::Buffered(Box::pin(async move {
                            let (status, body) = response.await?;
                            Ok(HttpResponseFixture { status, etag: None, body })
                        }))
                    }),
                WireFixture::WithHead(transport) => {
                    transport(url, &expected_user, body.as_bytes()).map(HttpFixture::WithHead)
                }
            };
            invalidation.built(response)
        })
        .transpose()?;

    let mut response_status = None;
    let sent = send_response_observed(
        |abort_signal| {
            invalidation.built(
                Request::post(url)
                    .abort_signal(abort_signal)
                    .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, &expected_user)
                    .header("Content-Type", "application/json")
                    .body(body)
                    .map_err(|_| TransportError::Network(None)),
            )
        },
        #[cfg(test)]
        fixture,
        |head| {
            // 超时可发生在响应头之后，共享超时错误本身不携带状态。
            observe_command_head(&mut response_status, head.status());
            head.observe_date();
        },
    )
    .await;
    invalidation.response_status = response_status;
    let response = sent?;
    Ok(CommandResponse { status: response.status(), body: response.text().await })
}

async fn send_command<T>(
    expected_user: Uuid,
    send: impl AsyncFnOnce(&mut CommandInvalidation) -> Result<CommandResponse, TransportError>,
    decode: impl FnOnce(Option<&str>) -> Result<T, String>,
) -> Result<T, CommandError> {
    let observation = SessionIdentityObservation::user_owner();
    let expected_user_id = expected_user.to_string();
    let (response, response_status) = {
        let mut invalidation = CommandInvalidation::default();
        let response = send(&mut invalidation).await.map_err(|error| {
            if invalidation.request_built {
                let error = CommandError::transport(error, invalidation.response_status);
                if error.status == 401 && invalidation.response_status != Some(401) {
                    request_public_session_check(SessionRefreshReason::AuthenticationFailed);
                }
                error
            } else {
                CommandError::before_send()
            }
        })?;
        (response, invalidation.response_status)
    };
    let status = response.status;
    let body = response.body.as_deref();
    if !(200..300).contains(&status) {
        let error = read_command_error(status, body);
        if status != 401 || response_status != Some(401) {
            observe_public_session_failure_observed(
                status,
                &error.code,
                Some(&expected_user_id),
                &observation,
            );
        }
        return Err(error);
    }
    decode(body).map_err(|_| CommandError::invalid_response(status))
}

fn read_command_error(status: u16, body: Option<&str>) -> CommandError {
    super::session::transport::decode_identity_response::<()>(
        status,
        body,
        serde_json::Value::is_null,
    )
    .err()
    .map(PublicSessionApiError::into_read_code)
    .filter(|code| {
        !code.is_empty()
            && code.len() <= 64
            && code == code.trim()
            && code.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
            })
    })
    .map_or_else(|| CommandError::invalid_response(status), |code| CommandError { code, status })
}

#[cfg(test)]
mod tests {
    use std::future::{pending, ready};

    use futures_util::FutureExt;
    use serde_json::json;
    use uuid::Uuid;

    use super::{BidResult, CommandError, CommandResponse, send_command};
    use crate::api::swr_cache::{FetchOutcome, LIVE_TTL_MILLIS, current_revision, get_or_fetch};
    use crate::api::{TransportError, decoder};

    const LOT_ID: &str = "6f1c2a5e-1b2c-4d3e-8f4a-000000000003";

    fn decode_bid_receipt(body: Option<&str>) -> Result<BidResult, String> {
        decoder::decode_bid_command(body, LOT_ID)
    }

    fn read_snapshot(
        url: &str,
        fetch: impl FnOnce(Option<String>) -> FetchOutcome<&'static str, &'static str>,
    ) -> Result<&'static str, &'static str> {
        get_or_fetch(url, LIVE_TTL_MILLIS, || 0, |etag| ready(fetch(etag)))
            .now_or_never()
            .expect("替身读取应立即完成")
    }

    fn seed_snapshot(url: &str) {
        assert_eq!(
            read_snapshot(url, |_| FetchOutcome::Changed {
                value: "before",
                etag: Some("etag-before".to_owned()),
            }),
            Ok("before")
        );
    }

    fn assert_snapshot_invalidated(url: &str) {
        assert_eq!(
            read_snapshot(url, |etag| {
                assert_eq!(etag.as_deref(), Some("etag-before"));
                FetchOutcome::Transport("offline")
            }),
            Err("offline")
        );
    }

    #[test]
    fn sent_transport_failures_forbid_falling_back_to_pre_command_cache() {
        for (index, failure) in [
            TransportError::Network(None),
            TransportError::Timeout,
            TransportError::Network(Some(200)),
            TransportError::Network(Some(409)),
            TransportError::Network(Some(503)),
            TransportError::Decode,
        ]
        .into_iter()
        .enumerate()
        {
            let url = format!("/api/v1/lots/lot?command_transport={index}");
            seed_snapshot(&url);
            let before = current_revision();
            let error = send_command(
                Uuid::from_u128(1),
                async |invalidation| {
                    invalidation.built(Ok(()))?;
                    Err(failure)
                },
                decode_bid_receipt,
            )
            .now_or_never()
            .unwrap()
            .unwrap_err();
            assert_eq!(error, CommandError { code: failure.code().to_owned(), status: 0 });
            assert!(error.outcome_uncertain());
            assert_eq!(current_revision(), before + 1, "{failure:?}");
            assert_snapshot_invalidated(&url);
        }
    }

    #[test]
    fn disconnected_unauthorized_response_keeps_its_header_status() {
        let error = send_command(
            Uuid::from_u128(1),
            async |invalidation| {
                invalidation.built(Ok(()))?;
                Err(TransportError::Network(Some(401)))
            },
            decode_bid_receipt,
        )
        .now_or_never()
        .unwrap()
        .unwrap_err();
        assert_eq!(error, CommandError { code: "network_error".to_owned(), status: 401 });
    }

    #[test]
    fn complete_error_and_malformed_receipts_go_through_command_decoding() {
        for (index, (status, body, code, uncertain)) in [
            (
                503,
                r#"{"code":"storage_unavailable","message":"fixture","data":null}"#,
                "storage_unavailable",
                true,
            ),
            (
                409,
                r#"{"code":"bid_too_low","message":"fixture","data":null}"#,
                "bid_too_low",
                false,
            ),
            (
                409,
                r#"{"code":"internal","c\u006fde":"bid_too_low","message":"","data":null}"#,
                "invalid_response",
                true,
            ),
            (
                409,
                r#"{"code":"bid_too_low","message":"first","message":"last","data":null}"#,
                "invalid_response",
                true,
            ),
            (
                409,
                r#"{"code":"bid_too_low","message":"","data":[],"data":null}"#,
                "invalid_response",
                true,
            ),
            (200, "{", "invalid_response", true),
            (
                200,
                r#"{"code":"ok","message":"fixture","data":{"bid":{}}}"#,
                "invalid_response",
                true,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let url = format!("/api/v1/lots/lot?command_receipt={index}");
            seed_snapshot(&url);
            let before = current_revision();
            let error = send_command(
                Uuid::from_u128(1),
                async |invalidation| {
                    invalidation.built(Ok(()))?;
                    Ok(CommandResponse { status, body: Some(body.to_owned()) })
                },
                decode_bid_receipt,
            )
            .now_or_never()
            .unwrap()
            .unwrap_err();
            assert_eq!(error, CommandError { code: code.to_owned(), status });
            assert_eq!(error.outcome_uncertain(), uncertain);
            assert_eq!(current_revision(), before + 1);
            assert_snapshot_invalidated(&url);
        }
    }

    #[test]
    fn valid_bid_receipt_passes_the_production_decoder_and_invalidates_once() {
        let url = "/api/v1/lots/lot?command_success";
        seed_snapshot(url);
        let before = current_revision();
        let body = json!({
            "code": "ok",
            "message": "fixture",
            "data": {
                "bid": {
                    "lot_id": LOT_ID,
                    "current_price": 1500,
                    "effective_end_at": "2026-11-18T12:00:00Z",
                    "state_version": 2,
                    "leading": true,
                }
            }
        });
        let result = send_command(
            Uuid::from_u128(1),
            async |invalidation| {
                invalidation.built(Ok(()))?;
                Ok(CommandResponse { status: 200, body: Some(body.to_string()) })
            },
            decode_bid_receipt,
        )
        .now_or_never()
        .unwrap()
        .unwrap();
        assert_eq!(result.current_price, 1500);
        assert!(result.leading);
        assert_eq!(current_revision(), before + 1);
        assert_snapshot_invalidated(url);
    }

    #[test]
    fn request_build_failure_keeps_the_pre_command_cache_fresh() {
        let url = "/api/v1/lots/lot?command_build_failed";
        seed_snapshot(url);
        let before = current_revision();
        let result = send_command(
            Uuid::from_u128(1),
            async |invalidation| invalidation.built(Err(TransportError::Network(None))),
            decode_bid_receipt,
        )
        .now_or_never()
        .unwrap();
        assert!(!result.unwrap_err().outcome_uncertain());
        assert_eq!(current_revision(), before);
        assert_eq!(read_snapshot(url, |_| panic!("发送前失败不应触发重读")), Ok("before"));
    }

    #[test]
    fn dropping_a_command_future_invalidates_once_only_after_request_build() {
        for request_built in [false, true] {
            let url = format!("/api/v1/lots/lot?command_cancel_built={request_built}");
            seed_snapshot(&url);
            let before = current_revision();
            let mut command = Box::pin(send_command(
                Uuid::from_u128(1),
                async |invalidation| {
                    if request_built {
                        invalidation.built(Ok(()))?;
                    }
                    pending().await
                },
                decode_bid_receipt,
            ));
            assert!(command.as_mut().now_or_never().is_none());
            assert_eq!(current_revision(), before);
            drop(command);
            assert_eq!(current_revision(), before + u64::from(request_built));
            if request_built {
                assert_snapshot_invalidated(&url);
            } else {
                assert_eq!(
                    read_snapshot(&url, |_| panic!("请求构建前取消应保留缓存")),
                    Ok("before")
                );
            }
        }
    }

    #[test]
    fn competition_rejections_decode_backend_contract_fixtures() {
        use super::read_command_error;

        for (status, code, body) in [
            (
                400,
                "invalid_maximum_amount",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-invalid-maximum-amount.json"
                ),
            ),
            (
                400,
                "invalid_proxy_mode",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-invalid-proxy-mode.json"
                ),
            ),
            (
                400,
                "invalid_amount",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-invalid-amount.json"
                ),
            ),
            (
                409,
                "bid_too_low",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-bid-too-low.json"
                ),
            ),
            (
                409,
                "already_leading",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-already-leading.json"
                ),
            ),
            (
                409,
                "active_proxy_exists",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-active-proxy-exists.json"
                ),
            ),
            (
                409,
                "auction_not_open",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-auction-not-open.json"
                ),
            ),
            (
                403,
                "bidder_ineligible",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-bidder-ineligible.json"
                ),
            ),
            (
                409,
                "proxy_disabled",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-proxy-disabled.json"
                ),
            ),
            (
                409,
                "proxy_slow_final",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-proxy-slow-final.json"
                ),
            ),
            (
                409,
                "proxy_outbid_limit",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-proxy-outbid-limit.json"
                ),
            ),
            (
                409,
                "proxy_reentry_maximum_not_higher",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-proxy-reentry-maximum-not-higher.json"
                ),
            ),
            (
                409,
                "proxy_maximum_not_lower",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-proxy-maximum-not-lower.json"
                ),
            ),
            (
                409,
                "proxy_maximum_below_binding",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-proxy-maximum-below-binding.json"
                ),
            ),
            (
                409,
                "proxy_authorization_not_active",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-proxy-authorization-not-active.json"
                ),
            ),
            (
                409,
                "invalid_policy",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-invalid-policy.json"
                ),
            ),
            (
                503,
                "temporarily_unavailable",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-temporarily-unavailable.json"
                ),
            ),
            (
                409,
                "command_conflict",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-command-conflict.json"
                ),
            ),
            (
                409,
                "session_changed",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-session-changed.json"
                ),
            ),
            (
                401,
                "unauthenticated",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-unauthenticated.json"
                ),
            ),
        ] {
            let error = read_command_error(status, Some(body));
            assert_eq!((error.code.as_str(), error.status), (code, status));
            assert_eq!(error.outcome_uncertain(), status == 503);
        }
    }

    #[test]
    fn watch_rejections_decode_backend_contract_fixtures() {
        use crate::api::session::decode_identity_response;

        for (status, code, body) in [
            (
                422,
                "invalid_watch",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-invalid-watch.json"
                ),
            ),
            (
                409,
                "watch_conflict",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-watch-conflict.json"
                ),
            ),
            (
                404,
                "not_found",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-not-found.json"
                ),
            ),
            (
                422,
                "invalid_watch_preference",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-invalid-watch-preference.json"
                ),
            ),
            (
                409,
                "watch_preference_conflict",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-watch-preference-conflict.json"
                ),
            ),
            (
                422,
                "pm_channel_unavailable",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-pm-channel-unavailable.json"
                ),
            ),
        ] {
            let error =
                decode_identity_response::<()>(status, Some(body), serde_json::Value::is_null)
                    .expect_err("标记命令已被拒绝");
            assert_eq!(error.into_read_code(), code);
        }
    }
}
