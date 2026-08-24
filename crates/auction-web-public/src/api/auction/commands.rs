//! Public 状态命令的 HTTP 请求。
//!
//! 浏览器只生成稳定重试所需的 `command_id` 并提交契约 DTO；金额、资格、锁和幂等结果仍由
//! 服务端应用端口决定。

use auction_contracts::{
    ApiErrorResponse, ApiResponse, CreateProxyAuthorizationRequest, LowerProxyAuthorizationRequest,
    PlaceBidRequest, PlaceBidResponse, PublicProxyCommandResponse,
};
pub(crate) use auction_contracts::{
    PlaceBidResultResponse as BidResult,
    PublicProxyCommandAuthorizationResponse as ProxyCommandAuthorization,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::observe_response_date;

/// Public 命令失败的稳定错误投影；不把服务端说明正文带入浏览器展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandError {
    pub code: String,
    pub status: u16,
}

impl CommandError {
    fn transport() -> Self {
        Self { code: "network_error".to_owned(), status: 0 }
    }

    fn invalid_response(status: u16) -> Self {
        Self { code: "invalid_response".to_owned(), status }
    }
}

pub(crate) async fn place_bid(
    lot_id: String,
    amount: i64,
    command_id: Uuid,
) -> Result<BidResult, CommandError> {
    let url = format!("/api/v1/lots/{lot_id}/bids");
    let request = Request::post(&url)
        .json(&PlaceBidRequest { command_id, amount })
        .map_err(|_| CommandError::transport())?;
    let response = request.send().await.map_err(|_| CommandError::transport())?;
    observe_response_date(&response);
    if !response.ok() {
        return Err(read_command_error(response).await);
    }
    let payload: ApiResponse<PlaceBidResponse> =
        response.json().await.map_err(|_| CommandError::invalid_response(response.status()))?;
    Ok(payload.data.bid)
}

/// 创建代理授权并解码旧契约要求的完整命令投影。
pub(crate) async fn create_proxy_authorization(
    lot_id: String,
    maximum_amount: i64,
    mode: String,
    command_id: Uuid,
) -> Result<ProxyCommandAuthorization, CommandError> {
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations");
    let request = Request::post(&url)
        .json(&CreateProxyAuthorizationRequest { command_id, maximum_amount, mode })
        .map_err(|_| CommandError::transport())?;
    let response = request.send().await.map_err(|_| CommandError::transport())?;
    observe_response_date(&response);
    if !response.ok() {
        return Err(read_command_error(response).await);
    }
    let payload: ApiResponse<PublicProxyCommandResponse> =
        response.json().await.map_err(|_| CommandError::invalid_response(response.status()))?;
    Ok(payload.data.authorization)
}

/// 降低代理授权上限并解码旧契约要求的完整命令投影。
pub(crate) async fn lower_proxy_authorization(
    lot_id: String,
    authorization_id: String,
    maximum_amount: i64,
    command_id: Uuid,
) -> Result<ProxyCommandAuthorization, CommandError> {
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower");
    let request = Request::post(&url)
        .json(&LowerProxyAuthorizationRequest { command_id, maximum_amount })
        .map_err(|_| CommandError::transport())?;
    let response = request.send().await.map_err(|_| CommandError::transport())?;
    observe_response_date(&response);
    if !response.ok() {
        return Err(read_command_error(response).await);
    }
    let payload: ApiResponse<PublicProxyCommandResponse> =
        response.json().await.map_err(|_| CommandError::invalid_response(response.status()))?;
    Ok(payload.data.authorization)
}

async fn read_command_error(response: gloo_net::http::Response) -> CommandError {
    let status = response.status();
    response
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
        .map(|error| CommandError { code: error.code, status })
        .unwrap_or_else(|| CommandError::invalid_response(status))
}
