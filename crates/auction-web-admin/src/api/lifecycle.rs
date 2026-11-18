//! Admin 拍卖生命周期命令 API 客户端。
//! 浏览器为一次动作输入生成并复用 `command_id`，再提交到版本化 REST 端点；状态迁移、权限、
//! 幂等和审计仍由服务端应用端口负责。

pub(crate) use auction_contracts::AdminAuctionLifecycleResponse as AdminAuctionLifecycleResult;
use auction_contracts::{AdminAuctionLifecycleRequest, ApiResponse};
use gloo_net::http::Method;
use uuid::Uuid;

use super::auctions::{decode_auction_command_response, send_auction_edit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminAuctionLifecycleAction {
    Preview,
    Open,
    Cancel,
    FinalPhase,
    EarlySettlement,
}

impl AdminAuctionLifecycleAction {
    const fn path(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Open => "open",
            Self::Cancel => "cancel",
            Self::FinalPhase => "final-phase",
            Self::EarlySettlement => "early-settlement",
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
    let url = format!("/api/v1/admin/auctions/{auction_id}/{}", action.path());
    let command = AdminAuctionLifecycleRequest {
        command_id: command_id.to_string(),
        expected_state_version,
        reason,
    };
    let payload: ApiResponse<AdminAuctionLifecycleResult> =
        send_auction_edit(Method::POST, &url, &command).await?;
    let auction = decode_auction_command_response(payload)?;
    if auction.id != auction_id {
        return Err("invalid_response".to_owned());
    }
    Ok(AdminAuctionLifecycleResult { auction })
}
