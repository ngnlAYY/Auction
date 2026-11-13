//! Admin 用户 DTO 映射。

use auction_contracts::{
    AdminUserBlacklistHistoryResponse, AdminUserListItemResponse, AdminUserResponse,
};

/// 将安全用户投影映射为 HTTP 响应。
pub(crate) fn map_user(user: auction_application::admin::AdminUser) -> AdminUserResponse {
    AdminUserResponse {
        id: user.id.to_string(),
        username: user.username,
        forum_account_id: user.forum_account_id,
        status: user.status.as_str().to_owned(),
        key_status: user.key_status.as_str().to_owned(),
        state_version: user.state_version,
        created_at: super::timestamp(user.created_at),
        updated_at: super::timestamp(user.updated_at),
    }
}

/// 将用户列表项及黑名单摘要映射为 HTTP 响应。
pub(crate) fn map_list_user(
    user: auction_application::admin::AdminUserListItem,
) -> AdminUserListItemResponse {
    AdminUserListItemResponse {
        user: map_user(user.user),
        blacklist_count: user.blacklist_count,
        blacklist_history: user
            .blacklist_history
            .into_iter()
            .map(|entry| AdminUserBlacklistHistoryResponse {
                public_reason: entry.public_reason,
                blacklisted_at: super::timestamp(entry.blacklisted_at),
            })
            .collect(),
    }
}
