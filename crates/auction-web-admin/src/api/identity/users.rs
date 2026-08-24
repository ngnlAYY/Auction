//! Admin 用户管理 API 客户端。
//!
//! 浏览器只保存安全用户投影和当前表单内容；登录 key 的生成结果只在创建/重签响应里短暂
//! 展示，不参与前端状态推导。

use auction_contracts::AdminUserBlacklistHistoryResponse as AdminUserBlacklistHistory;
use auction_contracts::{
    AdminUserBlacklistRequest, AdminUserCreateRequest, AdminUserListItemResponse,
    AdminUserListResponse, AdminUserMutationRequest, AdminUserMutationResponse, AdminUserResponse,
    AdminUserUnblacklistRequest,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::ApiResponse;

/// 用户列表行的浏览器投影；它把基础用户响应与列表专属黑名单摘要合并，避免 UI 传播 wire 嵌套。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminUser {
    pub id: String,
    pub username: String,
    pub game_player_id: Option<String>,
    pub status: String,
    pub key_status: String,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub blacklist_count: i64,
    pub blacklist_history: Vec<AdminUserBlacklistHistory>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminUserPage {
    pub users: Vec<AdminUser>,
    pub pagination: auction_contracts::AdminUserPaginationResponse,
}

impl From<AdminUserResponse> for AdminUser {
    fn from(user: AdminUserResponse) -> Self {
        Self {
            id: user.id,
            username: user.username,
            game_player_id: user.game_player_id,
            status: user.status,
            key_status: user.key_status,
            state_version: user.state_version,
            created_at: user.created_at,
            updated_at: user.updated_at,
            blacklist_count: 0,
            blacklist_history: Vec::new(),
        }
    }
}

impl From<AdminUserListItemResponse> for AdminUser {
    fn from(item: AdminUserListItemResponse) -> Self {
        let mut user = Self::from(item.user);
        user.blacklist_count = item.blacklist_count;
        user.blacklist_history = item.blacklist_history;
        user
    }
}

impl From<AdminUserListResponse> for AdminUserPage {
    fn from(data: AdminUserListResponse) -> Self {
        Self {
            users: data.users.into_iter().map(Into::into).collect(),
            pagination: data.pagination,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminUserStatusFilter {
    All,
    Active,
    Blacklisted,
}

impl AdminUserStatusFilter {
    const fn as_query(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Active => Some("active"),
            Self::Blacklisted => Some("blacklisted"),
        }
    }
}

/// 用户写命令的浏览器结果；基础用户 wire 响应在边界转换为列表行投影。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminUserMutationResult {
    pub user: AdminUser,
    pub generated_key: Option<String>,
}

/// 获取 Admin 用户列表。
pub(crate) async fn fetch_admin_users(
    name: String,
    status: AdminUserStatusFilter,
    page: i64,
) -> Result<AdminUserPage, String> {
    let mut url = format!("/api/v1/admin/users?page={page}&page_size=50");
    if !name.is_empty() {
        append_query(&mut url, "name", &name);
    }
    if let Some(status) = status.as_query() {
        append_query(&mut url, "status", status);
    }
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminUserListResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.pagination.page != page {
        return Err("user list response does not match page".to_owned());
    }
    Ok(payload.data.into())
}

/// 创建用户；`key_mode = generated` 时返回一次性 key。
pub(crate) async fn create_admin_user(
    command_id: Uuid,
    username: String,
    game_player_id: String,
    key_mode: &'static str,
    key: Option<String>,
) -> Result<AdminUserMutationResult, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::post("/api/v1/admin/users")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminUserCreateRequest {
            command_id,
            username,
            game_player_id,
            key_mode: key_mode.to_owned(),
            key,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminUserMutationResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(AdminUserMutationResult {
        user: payload.data.user.into(),
        generated_key: payload.data.generated_key,
    })
}

/// 修改用户名、游戏身份或登录 key 状态。
pub(crate) async fn mutate_admin_user(
    command_id: Uuid,
    user: &AdminUser,
    action: &'static str,
    username: Option<String>,
    game_player_id: Option<String>,
) -> Result<AdminUserMutationResult, String> {
    let user_id = Uuid::parse_str(&user.id).map_err(|_| "invalid user id".to_owned())?;
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::patch("/api/v1/admin/users")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminUserMutationRequest {
            command_id,
            user_id,
            expected_state_version: user.state_version,
            action: action.to_owned(),
            username,
            game_player_id,
            public_reason: None,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminUserMutationResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(AdminUserMutationResult {
        user: payload.data.user.into(),
        generated_key: payload.data.generated_key,
    })
}

/// 将用户加入黑名单。
pub(crate) async fn blacklist_admin_user(
    command_id: Uuid,
    user: &AdminUser,
    public_reason: String,
) -> Result<AdminUser, String> {
    let user_id = Uuid::parse_str(&user.id).map_err(|_| "invalid user id".to_owned())?;
    let csrf_token = super::fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/users/{user_id}/blacklist");
    let request = Request::put(&url)
        .header("x-csrf-token", &csrf_token)
        .json(&AdminUserBlacklistRequest {
            command_id,
            expected_state_version: user.state_version,
            public_reason,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminUserMutationResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.user.into())
}

/// 将用户移出黑名单。
pub(crate) async fn unblacklist_admin_user(
    command_id: Uuid,
    user: &AdminUser,
) -> Result<AdminUser, String> {
    let user_id = Uuid::parse_str(&user.id).map_err(|_| "invalid user id".to_owned())?;
    let csrf_token = super::fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/users/{user_id}/blacklist");
    let request = Request::delete(&url)
        .header("x-csrf-token", &csrf_token)
        .json(&AdminUserUnblacklistRequest {
            command_id,
            expected_state_version: user.state_version,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminUserMutationResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.user.into())
}

fn append_query(url: &mut String, name: &str, value: &str) {
    url.push('&');
    url.push_str(name);
    url.push('=');
    url.push_str(&url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>());
}
