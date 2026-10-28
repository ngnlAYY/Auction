//! Admin 代理历史读取端口。
//! A15 只读取已经提交的代理命令结果，不把代理上限、Cookie 或数据库原始负载直接暴露给
//! HTTP。PostgreSQL 适配器从同一 Serializable 快照读取命令账本、代理结果和用户投影。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use auction_domain::{LotId, valid_text};

use super::ApplicationFuture;

/// Admin 代理历史允许的最大单页大小。
pub const MAX_ADMIN_PROXY_EVENT_PAGE_SIZE: i64 = 100;

/// Admin 代理历史允许的最大 offset。
pub const MAX_ADMIN_PROXY_EVENT_OFFSET: i64 = 100_000;

/// 游标分页不返回精确总数时使用的稳定哨兵值。
pub const UNKNOWN_ADMIN_PROXY_EVENT_TOTAL: i64 = -1;

/// Admin 代理历史的起始或时间加命令 ID 游标边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminProxyEventCursor {
    /// 从最新一页开始读取。
    Start,
    /// 从指定的历史位置之后继续读取。
    Boundary { created_at: DateTime<Utc>, command_id: Uuid },
}

impl AdminProxyEventCursor {
    /// 校验游标边界必须包含非零时间和非空命令标识。
    pub fn validate(self) -> bool {
        match self {
            Self::Start => true,
            Self::Boundary { created_at, command_id } => {
                created_at.timestamp_micros() != 0 && !command_id.is_nil()
            }
        }
    }
}

/// Admin 代理历史查询条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminProxyEventListRequest {
    pub lot_id: LotId,
    pub limit: i64,
    pub offset: i64,
    pub cursor: Option<AdminProxyEventCursor>,
}

impl AdminProxyEventListRequest {
    /// 校验标识、游标和有界分页。
    pub fn validate(self) -> bool {
        !self.lot_id.is_nil()
            && self.limit > 0
            && self.limit <= MAX_ADMIN_PROXY_EVENT_PAGE_SIZE
            && self.offset >= 0
            && self.offset <= MAX_ADMIN_PROXY_EVENT_OFFSET
            && self.cursor.is_none_or(|cursor| cursor.validate())
    }
}

/// Admin 代理历史安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminProxyEvent {
    pub command_id: Uuid,
    pub authorization_id: Uuid,
    pub authorization_sequence: i64,
    pub usage_number: i64,
    pub lot_id: LotId,
    pub user_id: Uuid,
    pub username: String,
    pub mode: String,
    pub maximum_amount: i64,
    pub status: String,
    pub state_version: i64,
    pub created_at: DateTime<Utc>,
}

impl AdminProxyEvent {
    /// 校验浏览器可见代理历史的闭集字段和身份关系。
    pub fn validate(&self, lot_id: LotId) -> bool {
        !self.command_id.is_nil()
            && !self.authorization_id.is_nil()
            && self.authorization_sequence > 0
            && self.usage_number > 0
            && self.usage_number <= self.authorization_sequence
            && self.lot_id == lot_id
            && !self.user_id.is_nil()
            && valid_text(&self.username, 256)
            && matches!(self.mode.as_str(), "fast" | "slow")
            && self.maximum_amount > 0
            && matches!(
                self.status.as_str(),
                "active" | "final_fast" | "exhausted" | "frozen" | "closed"
            )
            && self.state_version > 0
            && self.created_at.timestamp_micros() != 0
    }
}

/// Admin 代理历史分页结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminProxyEventPage {
    pub lot_id: LotId,
    pub entries: Vec<AdminProxyEvent>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub cursor: Option<AdminProxyEventCursor>,
    pub next_cursor: Option<AdminProxyEventCursor>,
    pub has_more: bool,
}

impl AdminProxyEventPage {
    /// 校验页形状、游标模式和所有安全投影。
    pub fn validate(&self, request: AdminProxyEventListRequest) -> bool {
        let cursor_mode = request.cursor.is_some();
        request.validate()
            && self.lot_id == request.lot_id
            && self.limit == request.limit
            && self.offset == request.offset
            && self.cursor == request.cursor
            && i64::try_from(self.entries.len()).is_ok_and(|entry_count| entry_count <= self.limit)
            && self.entries.iter().all(|entry| entry.validate(request.lot_id))
            && if cursor_mode {
                self.total == UNKNOWN_ADMIN_PROXY_EVENT_TOTAL
                    && self.has_more == self.next_cursor.is_some()
            } else {
                self.total >= 0 && !self.has_more && self.next_cursor.is_none()
            }
    }
}

/// Admin 代理历史窄读取端口。
pub trait AdminProxyEventReader: Send + Sync {
    fn list_admin_proxy_events(
        &self,
        request: AdminProxyEventListRequest,
    ) -> ApplicationFuture<'_, Option<AdminProxyEventPage>>;
}
