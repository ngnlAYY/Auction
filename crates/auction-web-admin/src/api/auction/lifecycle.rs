//! Admin 拍卖生命周期命令 API 客户端。
//!
//! 浏览器为一次动作输入生成并复用 `command_id`，再提交到版本化 REST 端点；状态迁移、权限、
//! 幂等和审计仍由服务端应用端口负责。

use auction_contracts::AdminAuctionLifecycleRequest;
pub(crate) use auction_contracts::AdminAuctionLifecycleResponse as AdminAuctionLifecycleResult;
use gloo_net::http::Request;
use uuid::Uuid;

use super::{ApiResponse, fetch_csrf_token};

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
    let request = Request::post(&url)
        .header("x-csrf-token", &csrf_token)
        .json(&AdminAuctionLifecycleRequest { command_id, expected_state_version, reason })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminAuctionLifecycleResult> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.command_id != command_id || payload.data.auction_id != auction_id {
        return Err("lifecycle response does not match command".to_owned());
    }
    Ok(payload.data)
}
