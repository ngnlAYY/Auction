//! Admin 改名历史 API 客户端。
//!
//! 页面只接收改名事实的安全投影；回滚命令携带列表快照中的用户版本、CSRF 和新命令 ID，
//! 不在浏览器端推导用户状态；同一条改名事实的重试会复用同一个命令 ID。

use gloo_net::http::Request;
use serde::Deserialize;
use uuid::Uuid;

use super::{AdminUser, ApiResponse};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRename {
    pub id: String,
    pub user_id: String,
    pub user_state_version: i64,
    pub previous_username: String,
    pub next_username: String,
    pub actor: String,
    pub created_at: String,
    pub reverted_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminRenameRevertData {
    user: AdminUser,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminRenamePagination {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminRenameListData {
    renames: Vec<AdminRename>,
    pagination: AdminRenamePagination,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminRenamePage {
    pub renames: Vec<AdminRename>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

impl From<AdminRenameListData> for AdminRenamePage {
    fn from(data: AdminRenameListData) -> Self {
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
    let mut query = url::form_urlencoded::Serializer::new(format!("page={page}&page_size=50"));
    if user_id != "all" {
        query.append_pair("user_id", &user_id);
    }
    if !name.trim().is_empty() {
        query.append_pair("name", name.trim());
    }
    let url = format!("/api/v1/admin/renames?{}", query.finish());
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminRenameListData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

/// 回滚一条改名事实；服务端负责再次校验当前用户名、记录状态和版本 fence。
pub(crate) async fn revert_admin_rename(
    command_id: Uuid,
    rename: &AdminRename,
) -> Result<AdminUser, String> {
    let rename_id = Uuid::parse_str(&rename.id).map_err(|_| "invalid rename id".to_owned())?;
    let user_id = Uuid::parse_str(&rename.user_id).map_err(|_| "invalid user id".to_owned())?;
    let csrf_token = super::fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/renames/{rename_id}/revert");
    let request = Request::post(&url)
        .header("x-csrf-token", &csrf_token)
        .json(&AdminRenameRevertRequest {
            command_id,
            expected_state_version: rename.user_state_version,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminRenameRevertData> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.user.id != user_id.to_string()
        || Some(payload.data.user.state_version) != rename.user_state_version.checked_add(1)
    {
        return Err("rename revert response does not match version fence".to_owned());
    }
    Ok(payload.data.user)
}

#[derive(Debug, Clone, serde::Serialize)]
struct AdminRenameRevertRequest {
    command_id: Uuid,
    expected_state_version: i64,
}
