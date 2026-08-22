//! Admin 拍卖生命周期命令 API 客户端。
//!
//! 浏览器只生成新的 `command_id` 并提交到版本化 REST 端点；状态迁移、权限、幂等和审计
//! 仍由服务端应用端口负责。

use gloo_net::http::Request;
use serde::Deserialize;
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

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminAuctionLifecycleResult {
    pub command_id: Uuid,
    pub auction_id: String,
    pub status: String,
    pub state_version: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AdminAuctionLifecycleRequest {
    command_id: Uuid,
}

pub(crate) async fn execute_admin_auction_lifecycle(
    auction_id: String,
    action: AdminAuctionLifecycleAction,
) -> Result<AdminAuctionLifecycleResult, String> {
    let command_id = Uuid::new_v4();
    let csrf_token = fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/auctions/{auction_id}/{}", action.path());
    let request = Request::post(&url)
        .header("x-csrf-token", &csrf_token)
        .json(&AdminAuctionLifecycleRequest { command_id })
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
