//! Admin 拍卖生命周期命令 API 客户端。
//!
//! 浏览器为一次动作输入生成并复用 `command_id`，再提交到版本化 REST 端点；状态迁移、权限、
//! 幂等和审计仍由服务端应用端口负责。

use auction_contracts::AdminAuctionLifecycleRequest;
pub(crate) use auction_contracts::AdminAuctionLifecycleResponse as AdminAuctionLifecycleResult;
use gloo_net::http::Request;
use uuid::Uuid;

use super::auctions::decode_auction_command_response;
use super::{fetch_csrf_token, invalid_request_error, response_error, send_request};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminAuctionLifecycleAction {
    Preview,
    Open,
    Cancel,
    FinalPhase,
}

impl AdminAuctionLifecycleAction {
    const fn path(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Open => "open",
            Self::Cancel => "cancel",
            Self::FinalPhase => "final-phase",
        }
    }
}

pub(crate) async fn execute_admin_auction_lifecycle(
    command_id: Uuid,
    auction_id: String,
    action: AdminAuctionLifecycleAction,
    expected_state_version: i64,
    reason: Option<String>,
) -> Result<AdminAuctionLifecycleResult, String> {
    let csrf_token = fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/auctions/{auction_id}/{}", action.path());
    let command = AdminAuctionLifecycleRequest {
        command_id: command_id.to_string(),
        expected_state_version,
        reason,
    };
    let response = send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let auction = decode_auction_command_response(response).await?;
    if auction.id != auction_id {
        return Err("invalid_response".to_owned());
    }
    Ok(AdminAuctionLifecycleResult { auction })
}
