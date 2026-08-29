//! Admin 改名历史 API 客户端。
//!
//! 页面只接收改名事实的安全投影；回滚前按旧版语义重新读取目标用户，并用最新版本、
//! CSRF 和新命令 ID 提交。浏览器不自行推导用户状态；同一条改名事实的重试会复用同一个命令 ID。

pub(crate) use auction_contracts::AdminRenameResponse as AdminRename;
use auction_contracts::{
    AdminRenameListResponse, AdminRenamePaginationResponse, AdminRenameRevertRequest,
};
use gloo_net::http::Request;
use serde::Deserialize;
use uuid::Uuid;

use super::users::decode_user_mutation;
use super::{
    AdminUser, AdminUserStatusFilter, ApiResponse, MAX_JSON_SAFE_INTEGER, RequiredOption,
    TABLE_PAGE_SIZE, fetch_admin_users, invalid_request_error, invalid_response_error,
    response_error, send_get, valid_admin_time, valid_trimmed_text, valid_uuid,
};

/// 旧 rename decoder 允许对象增加字段；`user_state_version` 是新版兼容扩展，不参与回滚 fence。
#[derive(Debug, Deserialize)]
struct StrictRename {
    actor: String,
    created_at: String,
    id: String,
    next_username: String,
    previous_username: String,
    reverted_at: RequiredOption<String>,
    user_id: String,
    #[serde(default)]
    user_state_version: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct StrictRenamePagination {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
struct StrictRenameList {
    pagination: StrictRenamePagination,
    renames: Vec<StrictRename>,
}

/// 改名历史的浏览器分页投影；wire 中嵌套的 pagination 在这里展平。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminRenamePage {
    pub renames: Vec<AdminRename>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

impl From<AdminRenameListResponse> for AdminRenamePage {
    fn from(data: AdminRenameListResponse) -> Self {
        Self {
            renames: data.renames,
            page: data.pagination.page,
            page_size: data.pagination.page_size,
            total: data.pagination.total,
        }
    }
}

pub(crate) async fn fetch_admin_renames(
    user_id: String,
    name: String,
    page: i64,
) -> Result<AdminRenamePage, String> {
    let mut query =
        url::form_urlencoded::Serializer::new(format!("page={page}&page_size={TABLE_PAGE_SIZE}"));
    if user_id != "all" {
        query.append_pair("user_id", &user_id);
    }
    if !name.trim().is_empty() {
        query.append_pair("name", name.trim());
    }
    let url = format!("/api/v1/admin/renames?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictRenameList> =
        response.json().await.map_err(invalid_response_error)?;
    decode_rename_list(payload.data, &user_id, page)
        .map(AdminRenamePage::from)
        .map_err(invalid_response_error)
}

/// 回滚一条改名事实；提交前重新读取目标用户，避免使用历史行中的过期版本 fence。
pub(crate) async fn revert_admin_rename(
    command_id: Uuid,
    rename: &AdminRename,
) -> Result<AdminUser, String> {
    let rename_id = Uuid::parse_str(&rename.id).map_err(|_| "invalid_request".to_owned())?;
    let user_id = Uuid::parse_str(&rename.user_id).map_err(|_| "invalid_request".to_owned())?;
    let users =
        fetch_admin_users(rename.next_username.clone(), AdminUserStatusFilter::All, 1).await?;
    let current_user = users
        .users
        .into_iter()
        .find(|user| user.id == rename.user_id)
        .ok_or_else(|| "invalid_response".to_owned())?;
    let expected_state_version = current_user.state_version;
    let csrf_token = super::fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/renames/{rename_id}/revert");
    let command =
        AdminRenameRevertRequest { command_id: command_id.to_string(), expected_state_version };
    let response = super::send_request(|abort_signal| {
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
    let user = decode_user_mutation(response, false).await?.user;
    if user.id != user_id.to_string()
        || Some(user.state_version) != expected_state_version.checked_add(1)
    {
        return Err("invalid_response".to_owned());
    }
    Ok(user)
}

fn decode_rename_list(
    data: StrictRenameList,
    user_filter: &str,
    requested_page: i64,
) -> Result<AdminRenameListResponse, &'static str> {
    let pagination = decode_rename_pagination(data.pagination)?;
    if pagination.page != requested_page
        || pagination.page_size != TABLE_PAGE_SIZE
        || i64::try_from(data.renames.len())
            .map_or(true, |count| count > pagination.page_size || count > pagination.total)
    {
        return Err("rename_pagination");
    }
    let renames = data.renames.into_iter().map(decode_rename).collect::<Result<Vec<_>, _>>()?;
    if user_filter != "all" && renames.iter().any(|rename| rename.user_id != user_filter) {
        return Err("rename_filter");
    }
    Ok(AdminRenameListResponse { renames, pagination })
}

fn decode_rename(value: StrictRename) -> Result<AdminRename, &'static str> {
    if !valid_uuid(&value.id)
        || !valid_uuid(&value.user_id)
        || !valid_trimmed_text(&value.previous_username)
        || !valid_trimmed_text(&value.next_username)
        || !valid_trimmed_text(&value.actor)
        || !valid_admin_time(&value.created_at)
        || value.reverted_at.0.as_deref().is_some_and(|time| !valid_admin_time(time))
        || value
            .user_state_version
            .is_some_and(|version| !(1..=MAX_JSON_SAFE_INTEGER).contains(&version))
    {
        return Err("rename");
    }
    Ok(AdminRename {
        id: value.id,
        user_id: value.user_id,
        user_state_version: value.user_state_version.unwrap_or_default(),
        previous_username: value.previous_username,
        next_username: value.next_username,
        actor: value.actor,
        created_at: value.created_at,
        reverted_at: value.reverted_at.0,
    })
}

fn decode_rename_pagination(
    value: StrictRenamePagination,
) -> Result<AdminRenamePaginationResponse, &'static str> {
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page_size)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.total)
    {
        return Err("rename_pagination");
    }
    Ok(AdminRenamePaginationResponse {
        page: value.page,
        page_size: value.page_size,
        total: value.total,
    })
}
