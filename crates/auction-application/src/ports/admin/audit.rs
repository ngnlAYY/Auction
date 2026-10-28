//! Admin 审计读取端口。
//! 这里表达旧系统的两种只读查询形状：带时间/ID 稳定边界的 keyset 游标，以及带总数的数字
//! 分页。metadata 只允许安全的 JSON 对象，数据库行和原始 SQL 不跨过这个端口。

use auction_domain::valid_text as valid_record_text;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use super::ApplicationFuture;

/// Admin 审计单页最大记录数。
pub const MAX_ADMIN_AUDIT_PAGE_SIZE: i64 = 200;

/// Admin 审计数字分页允许的最大偏移，保持旧实现的有界 int32 乘法语义。
pub const MAX_ADMIN_AUDIT_OFFSET: i64 = 2_147_483_647;

/// Admin 审计记录的时间/ID keyset 游标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminAuditCursor {
    pub occurred_at: DateTime<Utc>,
    pub id: Uuid,
}

impl AdminAuditCursor {
    /// 校验游标不能携带空标识或零时间。
    pub fn validate(self) -> bool {
        !self.id.is_nil() && !self.occurred_at.timestamp_nanos_opt().is_none()
    }
}

/// Admin 审计查询条件；`page` 与 `cursor` 互斥，缺省时读取第一段 keyset 页面。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminAuditListRequest {
    pub actor: Option<String>,
    pub actor_type: Option<String>,
    pub action: Option<String>,
    pub entity: Option<String>,
    pub source: Option<String>,
    pub result: Option<String>,
    pub request_id: Option<String>,
    pub occurred_from: Option<DateTime<Utc>>,
    pub occurred_to: Option<DateTime<Utc>>,
    pub page: Option<i64>,
    pub page_size: i64,
    pub cursor: Option<AdminAuditCursor>,
}

impl AdminAuditListRequest {
    /// 校验筛选闭集、时间区间和两种分页形状的边界。
    pub fn validate(&self) -> bool {
        if self.page.is_some() && self.cursor.is_some()
            || self.page_size <= 0
            || self.page_size > MAX_ADMIN_AUDIT_PAGE_SIZE
        {
            return false;
        }
        if let Some(page) = self.page {
            let Some(offset) =
                page.checked_sub(1).and_then(|value| value.checked_mul(self.page_size))
            else {
                return false;
            };
            if page < 1 || offset > MAX_ADMIN_AUDIT_OFFSET {
                return false;
            }
        }
        if self.cursor.is_some_and(|cursor| !cursor.validate()) {
            return false;
        }
        valid_filter_text(self.actor.as_deref(), 256)
            && valid_filter_text(self.actor_type.as_deref(), 128)
            && valid_filter_text(self.action.as_deref(), 128)
            && valid_filter_text(self.entity.as_deref(), 385)
            && valid_filter_text(self.source.as_deref(), 128)
            && valid_filter_text(self.result.as_deref(), 128)
            && valid_filter_text(self.request_id.as_deref(), 128)
            && valid_actor_type(self.actor_type.as_deref())
            && valid_source(self.source.as_deref())
            && valid_result(self.result.as_deref())
            && self.occurred_from.zip(self.occurred_to).is_none_or(|(from, to)| from <= to)
    }

    /// 返回已校验数字页对应的零基偏移。
    pub fn offset(&self) -> Option<i64> {
        self.page
            .and_then(|page| page.checked_sub(1))
            .and_then(|page| page.checked_mul(self.page_size))
    }
}

/// Admin 审计记录的安全投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminAuditRecord {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub actor_type: String,
    pub actor_id: String,
    pub action: String,
    pub entity_type: String,
    pub entity_id: String,
    pub result: String,
    pub request_id: Option<String>,
    pub source: String,
    pub metadata: Value,
}

impl AdminAuditRecord {
    /// 校验存储层返回的枚举、文本和 metadata 对象边界。
    pub fn validate(&self) -> bool {
        !self.id.is_nil()
            && !self.occurred_at.timestamp_nanos_opt().is_none()
            && valid_actor_type(Some(&self.actor_type))
            && valid_source(Some(&self.source))
            && valid_result(Some(&self.result))
            && valid_record_text(&self.actor_id, 512)
            && valid_record_text(&self.action, 512)
            && valid_record_text(&self.entity_type, 512)
            && valid_record_text(&self.entity_id, 512)
            && valid_record_text_option(self.request_id.as_deref(), 512)
            && self.metadata.is_object()
            && serde_json::to_vec(&self.metadata).is_ok_and(|bytes| bytes.len() <= (1 << 20))
    }
}

/// Admin 审计查询结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminAuditPage {
    pub entries: Vec<AdminAuditRecord>,
    pub page: Option<i64>,
    pub page_size: i64,
    pub total: Option<i64>,
    pub next_cursor: Option<AdminAuditCursor>,
    pub has_more: bool,
}

impl AdminAuditPage {
    /// 校验分页结果没有越过应用端口声明的边界。
    pub fn validate(&self, request: &AdminAuditListRequest) -> bool {
        request.validate()
            && self.page == request.page
            && self.page_size == request.page_size
            && i64::try_from(self.entries.len())
                .is_ok_and(|entry_count| entry_count <= request.page_size)
            && self.total.is_none_or(|total| total >= 0)
            && self.next_cursor.is_none_or(AdminAuditCursor::validate)
            && (request.page.is_some() || self.total.is_none())
            && self.entries.iter().all(AdminAuditRecord::validate)
    }
}

/// Admin 审计端口参数错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AdminAuditError {
    #[error("Admin 审计查询参数无效")]
    InvalidRequest,
}

/// Admin 审计只读端口。
pub trait AdminAuditPort: Send + Sync {
    fn list(&self, request: AdminAuditListRequest) -> ApplicationFuture<'_, AdminAuditPage>;
}

fn valid_filter_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| {
        value.is_empty()
            || value.len() <= maximum
                && value == value.trim()
                && !value.chars().any(char::is_control)
    })
}

fn valid_record_text_option(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| valid_record_text(value, maximum))
}

fn valid_actor_type(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        value.is_empty() || matches!(value, "public_user" | "admin" | "worker" | "system")
    })
}

fn valid_source(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        value.is_empty() || matches!(value, "public" | "admin" | "worker" | "system")
    })
}

fn valid_result(value: Option<&str>) -> bool {
    value
        .is_none_or(|value| value.is_empty() || matches!(value, "success" | "rejected" | "failure"))
}
