//! Admin 改名历史 API 客户端。
//!
//! 页面只接收改名事实的安全投影；首次回滚按稳定用户 ID 读取当前版本，再冻结完整命令。
//! 浏览器不自行推导用户状态；结果不确定时必须重放同一个命令 ID 与版本，不能重新读取后改写负载。

pub(crate) use auction_contracts::AdminRenameResponse as AdminRename;
use auction_contracts::{
    AdminRenameListResponse, AdminRenamePaginationResponse, AdminRenameRevertRequest,
};
use gloo_net::http::Request;
use serde::Deserialize;
use uuid::Uuid;

use super::users::decode_user_mutation;
use super::{
    AdminUser, ApiResponse, MAX_JSON_SAFE_INTEGER, RequiredOption, TABLE_PAGE_SIZE,
    invalid_request_error, invalid_response_error, response_error, send_get, valid_admin_time,
    valid_trimmed_text, valid_uuid,
};

/// 旧 rename decoder 允许对象增加字段；普通展示仍允许省略 `user_state_version`，准备回滚时
/// 则必须从按稳定用户 ID 重新读取的当前投影取得非零版本，不能把兼容零值当作 fence。
///
/// 保留镜像的理由：wire 允许扩展字段且允许省略 `user_state_version`，而 contracts 的
/// `AdminRenameResponse` 将该字段作为必填标量、不能加 `deny_unknown_fields`；镜像继续按
/// 旧语义逐项校验。同步义务：`AdminRenameResponse`/`AdminRenameListResponse`/
/// `AdminRenamePaginationResponse` 字段集变化时必须同步本文件；decode 函数中构造 DTO 的
/// 穷举字段列表会在编译期捕捉漂移。
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
    page_size: i64,
) -> Result<AdminRenamePage, String> {
    super::pagination::validate(page, page_size, 200, i64::from(i32::MAX))?;
    let mut query =
        url::form_urlencoded::Serializer::new(format!("page={page}&page_size={page_size}"));
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
    decode_rename_list(payload.data, &user_id, page, page_size)
        .map(AdminRenamePage::from)
        .map_err(invalid_response_error)
}

/// 当前页面冻结的回滚命令；只有首次准备读取版本，后续重试不再改变路径或请求正文。
#[derive(Debug, Clone)]
pub(crate) struct AdminRenameRevertCommand {
    rename_id: Uuid,
    user_id: Uuid,
    request: AdminRenameRevertRequest,
}

/// 复用改名历史的精确用户筛选读取当前版本，不依赖用户名、匹配数量或目标历史所在页码。
pub(crate) async fn prepare_admin_rename_revert(
    command_id: Uuid,
    rename: &AdminRename,
) -> Result<AdminRenameRevertCommand, String> {
    let rename_id = Uuid::parse_str(&rename.id).map_err(|_| "invalid_request".to_owned())?;
    let user_id = Uuid::parse_str(&rename.user_id).map_err(|_| "invalid_request".to_owned())?;
    if command_id.is_nil() || rename_id.is_nil() || user_id.is_nil() {
        return Err("invalid_request".to_owned());
    }
    let history =
        fetch_admin_renames(user_id.to_string(), String::new(), 1, TABLE_PAGE_SIZE).await?;
    // 每行的版本来自同一次事务中 JOIN 的当前用户；任一改名行都能提供该用户的版本。
    let expected_state_version = history
        .renames
        .first()
        .map(|record| record.user_state_version)
        .filter(|version| *version > 0)
        .ok_or_else(|| "invalid_response".to_owned())?;
    if history.renames.iter().any(|record| record.user_state_version != expected_state_version) {
        return Err("invalid_response".to_owned());
    }
    Ok(AdminRenameRevertCommand {
        rename_id,
        user_id,
        request: AdminRenameRevertRequest {
            command_id: command_id.to_string(),
            expected_state_version,
        },
    })
}

/// 发送已冻结的回滚命令；响应丢失后的重试直接进入服务端命令账本重放。
pub(crate) async fn revert_admin_rename(
    command: &AdminRenameRevertCommand,
) -> Result<AdminUser, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/renames/{}/revert", command.rename_id);
    let response = super::send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command.request)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let user = decode_user_mutation(response, false).await?.user;
    if user.id != command.user_id.to_string()
        || Some(user.state_version) != command.request.expected_state_version.checked_add(1)
    {
        return Err("invalid_response".to_owned());
    }
    Ok(user)
}

fn decode_rename_list(
    data: StrictRenameList,
    user_filter: &str,
    requested_page: i64,
    requested_size: i64,
) -> Result<AdminRenameListResponse, &'static str> {
    let pagination = decode_rename_pagination(data.pagination)?;
    if pagination.page != requested_page
        || pagination.page_size != requested_size
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
