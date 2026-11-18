//! Public 状态命令的 HTTP 请求。
//! 浏览器只生成稳定重试所需的 `command_id` 并提交契约 DTO；金额、资格、锁和幂等结果仍由
//! 服务端应用端口决定。

use auction_contracts::{
    ApiErrorResponse, CreateProxyAuthorizationRequest, LowerProxyAuthorizationRequest,
    PlaceBidRequest,
};
pub(crate) use auction_contracts::{
    PlaceBidResultResponse as BidResult,
    PublicProxyCommandAuthorizationResponse as ProxyCommandAuthorization,
};
use gloo_net::http::Request;
use serde::Serialize;
use uuid::Uuid;

use super::{TransportError, decoder, send_request};

/// Public 命令失败的稳定错误投影；不把服务端说明正文带入浏览器展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandError {
    pub code: String,
    pub status: u16,
}

impl CommandError {
    /// 缺少明确业务结果时只允许重放原命令；新输入仍须经过正常表单校验。
    pub(crate) fn outcome_uncertain(&self) -> bool {
        // 浏览器会话锁失败发生在 HTTP 发送前，不把首次拒绝误报为未知外写。
        !matches!(self.code.as_str(), "session_lock_unavailable" | "session_lock_timeout")
            && (self.status == 0 || self.status >= 500 || self.code == "invalid_response")
    }

    fn transport(error: TransportError) -> Self {
        Self { code: error.code().to_owned(), status: 0 }
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
    let url = format!("/api/v1/lots/{lot_id}/bids");
    let command = PlaceBidRequest { command_id: command_id.to_string(), amount };
    send_command(
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
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations");
    let command = CreateProxyAuthorizationRequest {
        command_id: command_id.to_string(),
        maximum_amount,
        mode: mode.clone(),
    };
    send_command(
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
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower");
    let command =
        LowerProxyAuthorizationRequest { command_id: command_id.to_string(), maximum_amount };
    send_command(
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

async fn post_command(
    url: &str,
    user_id: Uuid,
    command: &impl Serialize,
    invalidation: &mut CommandInvalidation,
) -> Result<CommandResponse, TransportError> {
    let response = send_request(|abort_signal| {
        invalidation.built(
            Request::post(url)
                .abort_signal(abort_signal)
                .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, &user_id.to_string())
                .json(command)
                .map_err(|_| TransportError::Network(None)),
        )
    })
    .await?;
    Ok(CommandResponse { status: response.status(), body: response.text().await.ok() })
}

async fn send_command<T>(
    send: impl AsyncFnOnce(&mut CommandInvalidation) -> Result<CommandResponse, TransportError>,
    decode: impl FnOnce(Option<&str>) -> Result<T, String>,
) -> Result<T, CommandError> {
    let response = {
        let mut invalidation = CommandInvalidation::default();
        send(&mut invalidation).await
    }
    .map_err(CommandError::transport)?;
    let status = response.status;
    let body = response.body.as_deref();
    if !(200..300).contains(&status) {
        return Err(read_command_error(status, body));
    }
    decode(body).map_err(|_| CommandError::invalid_response(status))
}

fn read_command_error(status: u16, body: Option<&str>) -> CommandError {
    let error = body
        .and_then(|body| serde_json::from_str::<ApiErrorResponse>(body).ok())
        .filter(|error| {
            !error.code.is_empty()
                && error.code.len() <= 64
                && error.code == error.code.trim()
                && error.code.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        })
        .map_or_else(
            || CommandError::invalid_response(status),
            |error| CommandError { code: error.code, status },
        );
    super::session::observe_public_session_failure(status, &error.code);
    error
}

#[cfg(test)]
mod tests {
    use std::future::{pending, ready};

    use futures_util::FutureExt;
    use serde_json::json;

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
            async |invalidation| invalidation.built(Err(TransportError::Network(None))),
            decode_bid_receipt,
        )
        .now_or_never()
        .unwrap();
        assert!(result.is_err());
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
}
