//! Admin 改名历史 API 客户端。
//!
//! 页面只接收改名事实的安全投影；回滚命令携带列表快照中的用户版本、CSRF 和新命令 ID，
//! 不在浏览器端推导用户状态；同一条改名事实的重试会复用同一个命令 ID。

pub(crate) use auction_contracts::AdminRenameResponse as AdminRename;
use auction_contracts::{
    AdminRenameListResponse, AdminRenameRevertRequest, AdminUserMutationResponse,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::{AdminUser, ApiResponse};

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
    let payload: ApiResponse<AdminRenameListResponse> =
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
    let payload: ApiResponse<AdminUserMutationResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.user.id != user_id.to_string()
        || Some(payload.data.user.state_version) != rename.user_state_version.checked_add(1)
    {
        return Err("rename revert response does not match version fence".to_owned());
    }
    Ok(payload.data.user.into())
}
