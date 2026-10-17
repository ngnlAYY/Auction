//! Admin 用户管理 API 客户端。
//!
//! 浏览器只保存安全用户投影和当前表单内容；登录 key 的生成结果只在创建/重签响应里短暂
//! 展示，不参与前端状态推导。

use auction_contracts::AdminUserBlacklistHistoryResponse as AdminUserBlacklistHistory;
use auction_contracts::{
    AdminUserBlacklistRequest, AdminUserCreateRequest, AdminUserMutationRequest,
    AdminUserPaginationResponse, AdminUserResponse, AdminUserUnblacklistRequest,
};
use gloo_net::http::Request;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, invalid_request_error, invalid_response_error,
    parse_admin_time, response_error, send_get, send_request, valid_admin_time, valid_safe_text,
    valid_uuid,
};

// 保留镜像的理由：contracts 的 `AdminUserResponse` 被 `AdminUserListItemResponse` 以
// `#[serde(flatten)]` 内嵌，serde 不支持 flatten 与 `deny_unknown_fields` 组合，因此用户
// 列表与写命令 wire 无法由 contracts DTO 承担闭集；下方 Strict 镜像继续保持 fail-closed。
// 同步义务：`AdminUserResponse`/`AdminUserListItemResponse`/`AdminUserBlacklistHistoryResponse`/
// `AdminUserPaginationResponse`/`AdminUserMutationResponse` 字段集变化时必须同步本文件；
// decode 函数中构造 DTO 的穷举字段列表会在编译期捕捉漂移。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUser {
    id: String,
    username: String,
    forum_account_id: Option<String>,
    status: String,
    key_status: String,
    state_version: i64,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictBlacklistHistory {
    public_reason: String,
    blacklisted_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictListedUser {
    id: String,
    username: String,
    forum_account_id: Option<String>,
    status: String,
    key_status: String,
    state_version: i64,
    created_at: String,
    updated_at: String,
    blacklist_count: i64,
    blacklist_history: Vec<StrictBlacklistHistory>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUserPagination {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUserList {
    users: Vec<StrictListedUser>,
    pagination: StrictUserPagination,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUserData {
    user: StrictUser,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUserDataWithKey {
    user: StrictUser,
    generated_key: String,
}

/// 用户列表行的浏览器投影；它把基础用户响应与列表专属黑名单摘要合并，避免 UI 传播 wire 嵌套。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminUser {
    pub id: String,
    pub username: String,
    pub forum_account_id: Option<String>,
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
            forum_account_id: user.forum_account_id,
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
    page_size: i64,
) -> Result<AdminUserPage, String> {
    super::pagination::validate(page, page_size, 200, i64::from(i32::MAX))?;
    let mut url = format!("/api/v1/admin/users?page={page}&page_size={page_size}");
    if !name.is_empty() {
        append_query(&mut url, "name", &name);
    }
    if let Some(status) = status.as_query() {
        append_query(&mut url, "status", status);
    }
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictUserList> =
        response.json().await.map_err(invalid_response_error)?;
    let result = decode_user_list(payload.data).map_err(invalid_response_error)?;
    if result.pagination.page != page
        || result.pagination.page_size != page_size
        || i64::try_from(result.users.len())
            .map_or(true, |count| count > page_size || count > result.pagination.total)
    {
        return Err(invalid_response_error("user_pagination"));
    }
    Ok(result)
}

/// 创建用户；`key_mode = generated` 时返回一次性 key。
pub(crate) async fn create_admin_user(
    command_id: Uuid,
    username: String,
    forum_account_id: Option<String>,
    key_mode: &'static str,
    key: Option<String>,
) -> Result<AdminUserMutationResult, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let command = AdminUserCreateRequest {
        command_id: command_id.to_string(),
        username,
        forum_account_id,
        key_mode: key_mode.to_owned(),
        key,
    };
    let response = send_request(|abort_signal| {
        Request::post("/api/v1/admin/users")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    decode_user_mutation(response, key_mode == "generated").await
}

/// 修改用户名、论坛账号 ID 或登录 key 状态。
pub(crate) async fn mutate_admin_user(
    command_id: Uuid,
    user: &AdminUser,
    action: &'static str,
    username: Option<String>,
    forum_account_id: Option<String>,
) -> Result<AdminUserMutationResult, String> {
    let user_id = Uuid::parse_str(&user.id).map_err(|_| "invalid_request".to_owned())?;
    let csrf_token = super::fetch_csrf_token().await?;
    let command = AdminUserMutationRequest {
        command_id: command_id.to_string(),
        user_id: user_id.to_string(),
        expected_state_version: user.state_version,
        action: action.to_owned(),
        username,
        forum_account_id,
    };
    let response = send_request(|abort_signal| {
        Request::patch("/api/v1/admin/users")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    decode_user_mutation(response, action == "reissue_key").await
}

/// 将用户加入黑名单。
pub(crate) async fn blacklist_admin_user(
    command_id: Uuid,
    user: &AdminUser,
    public_reason: String,
) -> Result<AdminUser, String> {
    let user_id = Uuid::parse_str(&user.id).map_err(|_| "invalid_request".to_owned())?;
    let csrf_token = super::fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/users/{user_id}/blacklist");
    let command = AdminUserBlacklistRequest {
        command_id: command_id.to_string(),
        expected_state_version: user.state_version,
        public_reason,
    };
    let response = send_request(|abort_signal| {
        Request::put(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    decode_user_mutation(response, false).await.map(|result| result.user)
}

/// 将用户移出黑名单。
pub(crate) async fn unblacklist_admin_user(
    command_id: Uuid,
    user: &AdminUser,
) -> Result<AdminUser, String> {
    let user_id = Uuid::parse_str(&user.id).map_err(|_| "invalid_request".to_owned())?;
    let csrf_token = super::fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/users/{user_id}/blacklist");
    let command = AdminUserUnblacklistRequest {
        command_id: command_id.to_string(),
        expected_state_version: user.state_version,
    };
    let response = send_request(|abort_signal| {
        Request::delete(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    decode_user_mutation(response, false).await.map(|result| result.user)
}

fn decode_user_list(data: StrictUserList) -> Result<AdminUserPage, &'static str> {
    let users = data.users.into_iter().map(decode_listed_user).collect::<Result<Vec<_>, _>>()?;
    let pagination = decode_pagination(data.pagination)?;
    Ok(AdminUserPage { users, pagination })
}

pub(super) async fn decode_user_mutation(
    response: gloo_net::http::Response,
    want_key: bool,
) -> Result<AdminUserMutationResult, String> {
    let payload: ApiResponse<serde_json::Value> =
        response.json().await.map_err(invalid_response_error)?;
    let (user, generated_key) = if want_key {
        let data: StrictUserDataWithKey =
            serde_json::from_value(payload.data).map_err(invalid_response_error)?;
        if !valid_generated_key(&data.generated_key) {
            return Err("invalid_response".to_owned());
        }
        (data.user, Some(data.generated_key))
    } else {
        let data: StrictUserData =
            serde_json::from_value(payload.data).map_err(invalid_response_error)?;
        (data.user, None)
    };
    Ok(AdminUserMutationResult {
        user: decode_user(user).map_err(invalid_response_error)?.into(),
        generated_key,
    })
}

fn decode_listed_user(value: StrictListedUser) -> Result<AdminUser, &'static str> {
    let history_length = usize::try_from(value.blacklist_count.min(10)).map_err(|_| "user")?;
    if !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.blacklist_count)
        || value.blacklist_history.len() != history_length
    {
        return Err("user");
    }
    let history = value
        .blacklist_history
        .into_iter()
        .map(decode_blacklist_history)
        .collect::<Result<Vec<_>, _>>()?;
    if history.windows(2).any(|pair| {
        parse_admin_time(&pair[1].blacklisted_at) > parse_admin_time(&pair[0].blacklisted_at)
    }) {
        return Err("user_history_order");
    }
    let user = decode_user(StrictUser {
        id: value.id,
        username: value.username,
        forum_account_id: value.forum_account_id,
        status: value.status,
        key_status: value.key_status,
        state_version: value.state_version,
        created_at: value.created_at,
        updated_at: value.updated_at,
    })?;
    let mut user: AdminUser = user.into();
    user.blacklist_count = value.blacklist_count;
    user.blacklist_history = history;
    Ok(user)
}

fn decode_user(value: StrictUser) -> Result<AdminUserResponse, &'static str> {
    if !valid_uuid(&value.id)
        || !valid_safe_text(&value.username, 256)
        || value
            .forum_account_id
            .as_deref()
            .is_some_and(|account_id| !valid_forum_account_id(account_id))
        || !matches!(value.status.as_str(), "active" | "blacklisted")
        || !matches!(value.key_status.as_str(), "active" | "revoked")
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.state_version)
        || !valid_admin_time(&value.created_at)
        || !valid_admin_time(&value.updated_at)
    {
        return Err("user");
    }
    Ok(AdminUserResponse {
        id: value.id,
        username: value.username,
        forum_account_id: value.forum_account_id,
        status: value.status,
        key_status: value.key_status,
        state_version: value.state_version,
        created_at: value.created_at,
        updated_at: value.updated_at,
    })
}

fn valid_forum_account_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0] != b'0'
        && bytes.iter().all(u8::is_ascii_digit)
}

fn decode_blacklist_history(
    value: StrictBlacklistHistory,
) -> Result<AdminUserBlacklistHistory, &'static str> {
    if !valid_safe_text(&value.public_reason, 1024) || !valid_admin_time(&value.blacklisted_at) {
        return Err("blacklist_history");
    }
    Ok(AdminUserBlacklistHistory {
        public_reason: value.public_reason,
        blacklisted_at: value.blacklisted_at,
    })
}

fn decode_pagination(
    value: StrictUserPagination,
) -> Result<AdminUserPaginationResponse, &'static str> {
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page_size)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.total)
    {
        return Err("pagination");
    }
    Ok(AdminUserPaginationResponse {
        page: value.page,
        page_size: value.page_size,
        total: value.total,
    })
}

fn valid_generated_key(value: &str) -> bool {
    (32..=256).contains(&value.len()) && !value.chars().any(char::is_control)
}

fn append_query(url: &mut String, name: &str, value: &str) {
    url.push('&');
    url.push_str(name);
    url.push('=');
    url.push_str(&url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>());
}
