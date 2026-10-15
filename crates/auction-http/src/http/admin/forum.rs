//! Admin 论坛预览与本地同步状态 HTTP 适配器。
//!
//! 处理器只负责 Admin session/CSRF、请求体边界、稳定 DTO 映射和错误状态；论坛语法解析由
//! `auction-integrations::ForumPreviewAdapter` 完成，预览成功不代表已经发布远程论坛。

#[path = "forum/status.rs"]
mod status;

pub(crate) use status::{get, sync_status};

use std::net::SocketAddr;

use auction_application::{AdminForumPreviewError, ApplicationError};
use auction_contracts::{
    AdminForumBidPostResponse, AdminForumCatalogResponse, AdminForumCommandResponse,
    AdminForumLotResponse, AdminForumPostResponse, AdminForumPreviewRequest,
    AdminForumPreviewResponse, AdminForumWarningResponse, ApiResponse,
};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::AdminApiState;

/// 生成 Admin 论坛拍卖预览。
pub(crate) async fn preview(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Err(response) = super::authenticated_admin_mutation(&state, &headers, address).await {
        return *response;
    }
    if let Some(response) = mutation_boundary(&uri) {
        return response;
    }
    let request: AdminForumPreviewRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let Some(port) = state.admin_forum_preview() else {
        return super::api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "论坛拍卖预览能力未装配",
        )
        .into_response();
    };
    let result = match port
        .preview(auction_application::AdminForumPreviewCommand {
            html: request.html,
            text: request.text,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok("论坛拍卖预览已生成", map_preview(result))),
    )
        .into_response()
}

fn mutation_boundary(uri: &Uri) -> Option<Response> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Some(
            super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "论坛预览不接受查询参数")
                .into_response(),
        );
    }
    None
}

fn map_preview(result: auction_application::AdminForumPreviewResult) -> AdminForumPreviewResponse {
    AdminForumPreviewResponse {
        catalog: AdminForumCatalogResponse {
            categories: result.catalog.categories,
            lots: result.catalog.lots.into_iter().map(map_lot).collect(),
            warnings: result.catalog.warnings.into_iter().map(map_warning).collect(),
        },
        posts: result.posts.into_iter().map(map_post).collect(),
        warnings: result.warnings.into_iter().map(map_warning).collect(),
    }
}

fn map_lot(lot: auction_application::AdminForumLot) -> AdminForumLotResponse {
    AdminForumLotResponse {
        code: lot.code,
        category: lot.category,
        description: lot.description,
        seller: lot.seller,
        start_price: lot.start_price,
        material: lot.material,
        current_bidder: lot.current_bidder,
        current_price: lot.current_price,
    }
}

fn map_post(post: auction_application::AdminForumBidPost) -> AdminForumBidPostResponse {
    AdminForumBidPostResponse {
        post: AdminForumPostResponse {
            id: post.post.id,
            author_id: post.post.author_id,
            author: post.post.author,
            text: post.post.text,
            edited: post.post.edited,
        },
        commands: post.commands.into_iter().map(map_command).collect(),
        warnings: post.warnings.into_iter().map(map_warning).collect(),
        ignored: post.ignored,
    }
}

fn map_command(command: auction_application::AdminForumCommand) -> AdminForumCommandResponse {
    AdminForumCommandResponse {
        line: command.line,
        text: command.text,
        actor: command.actor,
        lot_codes: command.lot_codes,
        kind: command.kind,
        amount: command.amount,
        proxy: command.proxy,
        proxy_lower: command.proxy_lower,
        notify: command.notify,
    }
}

fn map_warning(warning: auction_application::AdminForumWarning) -> AdminForumWarningResponse {
    AdminForumWarningResponse {
        line: warning.line,
        code: warning.code,
        message: warning.message,
        text: warning.text,
        post_id: warning.post_id,
    }
}

fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminForumPreview(AdminForumPreviewError::InvalidInput)
        | ApplicationError::AdminForumPreview(AdminForumPreviewError::ParseFailed) => {
            super::api_error(StatusCode::BAD_REQUEST, "invalid_forum_auction", "论坛拍卖内容无效")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => super::api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "论坛拍卖预览能力未装配",
        )
        .into_response(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "forum_preview_unavailable",
            "论坛拍卖预览暂不可用",
        )
        .into_response(),
    }
}
