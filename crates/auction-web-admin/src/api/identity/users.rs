//! Admin 用户管理 API 客户端。
//!
//! 浏览器只保存安全用户投影和当前表单内容；登录 key 的生成结果只在创建/重签响应里短暂
//! 展示，不参与前端状态推导。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ApiResponse;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminUserBlacklistHistory {
    pub public_reason: String,
    pub blacklisted_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminUser {
    pub id: String,
    pub username: String,
    pub game_player_id: Option<String>,
    pub status: String,
    pub key_status: String,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub blacklist_count: i64,
    #[serde(default)]
    pub blacklist_history: Vec<AdminUserBlacklistHistory>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminUserPagination {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminUserListData {
    users: Vec<AdminUser>,
    pagination: AdminUserPagination,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminUserPage {
    pub users: Vec<AdminUser>,
    pub pagination: AdminUserPagination,
}

impl From<AdminUserListData> for AdminUserPage {
    fn from(data: AdminUserListData) -> Self {
        Self { users: data.users, pagination: data.pagination }
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

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminUserMutationResult {
    pub user: AdminUser,
    pub generated_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AdminUserCreateRequest {
    command_id: Uuid,
    username: String,
    game_player_id: String,
    key_mode: &'static str,
    key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AdminUserMutationRequest {
    command_id: Uuid,
    user_id: Uuid,
    expected_state_version: i64,
    action: &'static str,
    username: Option<String>,
    game_player_id: Option<String>,
    public_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AdminUserBlacklistRequest {
    command_id: Uuid,
    expected_state_version: i64,
    public_reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct AdminUserUnblacklistRequest {
    command_id: Uuid,
    expected_state_version: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminUserMutationData {
    user: AdminUser,
    generated_key: Option<String>,
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
    let payload: ApiResponse<AdminUserListData> =
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
        .json(&AdminUserCreateRequest { command_id, username, game_player_id, key_mode, key })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminUserMutationData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(AdminUserMutationResult {
        user: payload.data.user,
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
            action,
            username,
            game_player_id,
            public_reason: None,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminUserMutationData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(AdminUserMutationResult {
        user: payload.data.user,
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
    let payload: ApiResponse<AdminUserMutationData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.user)
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
    let payload: ApiResponse<AdminUserMutationData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.user)
}

fn append_query(url: &mut String, name: &str, value: &str) {
    url.push('&');
    url.push_str(name);
    url.push('=');
    url.push_str(&url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>());
}
