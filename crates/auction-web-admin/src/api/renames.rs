//! Admin 改名历史 API 客户端。
//!
//! 页面只接收改名事实的安全投影；回滚命令尚未接入，因此本模块不生成写入请求。

use gloo_net::http::Request;
use serde::Deserialize;

use super::ApiResponse;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRename {
    pub id: String,
    pub user_id: String,
    pub previous_username: String,
    pub next_username: String,
    pub actor: String,
    pub created_at: String,
    pub reverted_at: Option<String>,
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
