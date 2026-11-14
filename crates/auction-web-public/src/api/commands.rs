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
    let response = send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, &user_id.to_string())
            .json(&command)
            .map_err(|_| TransportError::Network(None))
    })
    .await
    .map_err(CommandError::transport)?;
    if !response.ok() {
        return Err(read_command_error(response).await);
    }
    let status = response.status();
    let result = decoder::decode_bid_command(response, &lot_id)
        .await
        .map_err(|_| CommandError::invalid_response(status))?;
    // 竞价已提交：目录价格、领先者、详情与竞价日志都可能有新权威快照，下一次读取不得命中 TTL 缓存。
    super::swr_cache::invalidate_all();
    Ok(result)
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
    let response = send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, &user_id.to_string())
            .json(&command)
            .map_err(|_| TransportError::Network(None))
    })
    .await
    .map_err(CommandError::transport)?;
    if !response.ok() {
        return Err(read_command_error(response).await);
    }
    let status = response.status();
    let result = decoder::decode_create_proxy_command(response, &lot_id, maximum_amount, &mode)
        .await
        .map_err(|_| CommandError::invalid_response(status))?;
    // 代理授权成功会改变 lot 的权威投影；公开读取缓存随之失效。
    super::swr_cache::invalidate_all();
    Ok(result)
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
    let response = send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, &user_id.to_string())
            .json(&command)
            .map_err(|_| TransportError::Network(None))
    })
    .await
    .map_err(CommandError::transport)?;
    if !response.ok() {
        return Err(read_command_error(response).await);
    }
    let status = response.status();
    let result =
        decoder::decode_lower_proxy_command(response, &lot_id, &authorization_id, maximum_amount)
            .await
            .map_err(|_| CommandError::invalid_response(status))?;
    // 降低上限同样改变 lot 的权威投影；公开读取缓存随之失效。
    super::swr_cache::invalidate_all();
    Ok(result)
}

async fn read_command_error(response: gloo_net::http::Response) -> CommandError {
    let status = response.status();
    // 服务端已对命令作答：被拒绝通常正是因为价格、领先者或拍品状态已经变化。表单随后会重读
    // 权威快照并据此提示下一口价，这次重读不得命中 TTL 内的旧条目。传输失败不经过这里，
    // 断网时仍可回退到最近一次已验证的快照。
    super::swr_cache::invalidate_all();
    let error = response
        .json::<ApiErrorResponse>()
        .await
        .ok()
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
