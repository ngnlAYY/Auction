//! Admin 改名历史读取端口。
//! 改名记录是追加式身份事实。这里仅暴露安全投影和有界分页；回滚由 Users owner 的
//! 幂等命令和状态版本 fence 负责，避免在读取端重复状态迁移。

use auction_domain::{valid_optional_text, valid_text};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ports::auction::ApplicationFuture;
use crate::ports::core::valid_page_window;

/// Admin 改名历史允许的最大单页大小。
pub const MAX_ADMIN_RENAME_PAGE_SIZE: i64 = 200;

/// Admin 改名历史允许的最大偏移。
pub const MAX_ADMIN_RENAME_OFFSET: i64 = 2_147_483_647;

/// Admin 改名历史查询条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRenameListRequest {
    pub user_id: Option<Uuid>,
    pub name: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl AdminRenameListRequest {
    /// 校验用户筛选、名称筛选和有界数字分页。
    pub fn validate(&self) -> bool {
        self.user_id.is_none_or(|id| !id.is_nil())
            && valid_optional_text(self.name.as_deref(), 256, true)
            && valid_page_window(
                self.limit,
                self.offset,
                MAX_ADMIN_RENAME_PAGE_SIZE,
                MAX_ADMIN_RENAME_OFFSET,
            )
    }
}

/// 改名历史的安全投影，不包含命令摘要或内部数据库字段；当前用户版本用于回滚 fence。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRenameRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    /// 读取快照时用户当前的版本，用于回滚命令的 state-version fence。
    pub user_state_version: i64,
    pub previous_username: String,
    pub next_username: String,
    pub actor: String,
    pub created_at: DateTime<Utc>,
    pub reverted_at: Option<DateTime<Utc>>,
}

impl AdminRenameRecord {
    /// 校验追加式改名事实的身份、文本和时间边界。
    pub fn validate(&self) -> bool {
        !self.id.is_nil()
            && !self.user_id.is_nil()
            && self.user_state_version > 0
            && valid_text(&self.previous_username, 256)
            && valid_text(&self.next_username, 256)
            && self.previous_username != self.next_username
            && valid_text(&self.actor, 256)
            && valid_time(self.created_at)
            && self.reverted_at.is_none_or(|value| valid_time(value) && value >= self.created_at)
    }
}

/// Admin 改名历史分页结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRenamePage {
    pub entries: Vec<AdminRenameRecord>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl AdminRenamePage {
    /// 校验存储 owner 返回的分页形状、筛选命中和稳定排序。
    pub fn validate(&self, request: &AdminRenameListRequest) -> bool {
        request.validate()
            && self.total >= 0
            && self.limit == request.limit
            && self.offset == request.offset
            && i64::try_from(self.entries.len()).is_ok_and(|entry_count| entry_count <= self.limit)
            && {
                // 名称筛选对整页相同；保留 Unicode 小写语义，只归一化一次。
                let needle = request.name.as_deref().map(str::to_lowercase);
                self.entries.iter().enumerate().all(|(index, entry)| {
                    entry.validate()
                        && request.user_id.is_none_or(|user_id| entry.user_id == user_id)
                        && needle.as_deref().is_none_or(|needle| {
                            entry.previous_username.to_lowercase().contains(needle)
                                || entry.next_username.to_lowercase().contains(needle)
                        })
                        && self.entries.get(index + 1).is_none_or(|next| {
                            entry.created_at > next.created_at
                                || entry.created_at == next.created_at && entry.id > next.id
                        })
                })
            }
    }
}

/// Admin 改名历史读取端口。
pub trait AdminRenameReader: Send + Sync {
    fn list(&self, request: AdminRenameListRequest) -> ApplicationFuture<'_, AdminRenamePage>;
}

fn valid_time(value: DateTime<Utc>) -> bool {
    value.timestamp_nanos_opt().is_some()
}
