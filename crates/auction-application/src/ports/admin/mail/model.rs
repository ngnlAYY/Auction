//! Admin Mail owner 的轮询状态和入站邮件安全投影。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::common::{
    MAX_ADMIN_MAIL_DECISION_BYTES, MAX_ADMIN_MAIL_OFFSET, MAX_ADMIN_MAIL_PAGE_SIZE,
    MAX_ADMIN_MAIL_POLLER_COUNT, MAX_ADMIN_MAILBOX_KEY_BYTES, valid_kind, valid_optional_text,
    valid_status, valid_string_list, valid_text,
};
use super::errors::AdminMailError;

/// Admin 邮箱轮询状态的安全投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminMailPollState {
    pub mailbox_key: String,
    pub cursor_value: Option<String>,
    pub last_poll_at: DateTime<Utc>,
    pub next_poll_at: DateTime<Utc>,
    pub consecutive_failures: i32,
    pub paused: bool,
    pub state_version: i64,
    pub updated_at: DateTime<Utc>,
}

impl AdminMailPollState {
    /// 校验轮询状态的时间、版本和文本边界，避免损坏状态进入 Admin surface。
    pub fn validate(&self) -> bool {
        valid_text(&self.mailbox_key, MAX_ADMIN_MAILBOX_KEY_BYTES)
            && valid_optional_text(self.cursor_value.as_deref(), 1024)
            && !self.last_poll_at.timestamp_nanos_opt().is_none()
            && !self.next_poll_at.timestamp_nanos_opt().is_none()
            && self.next_poll_at >= self.last_poll_at
            && self.consecutive_failures >= 0
            && self.state_version > 0
            && !self.updated_at.timestamp_nanos_opt().is_none()
    }
}

/// Admin 邮件 dashboard 展示的单个轮询器安全摘要。
///
/// 这里刻意不包含 cursor_value；游标属于同步实现细节，不能进入浏览器或管理概览响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminMailPollerSnapshot {
    pub mailbox_key: String,
    pub last_poll_at: DateTime<Utc>,
    pub next_poll_at: DateTime<Utc>,
    pub consecutive_failures: i32,
    pub paused: bool,
    pub state_version: i64,
    pub updated_at: DateTime<Utc>,
}

impl AdminMailPollerSnapshot {
    /// 校验轮询器摘要的时间、版本和文本边界。
    pub fn validate(&self) -> bool {
        valid_text(&self.mailbox_key, MAX_ADMIN_MAILBOX_KEY_BYTES)
            && !self.last_poll_at.timestamp_nanos_opt().is_none()
            && !self.next_poll_at.timestamp_nanos_opt().is_none()
            && self.next_poll_at >= self.last_poll_at
            && self.consecutive_failures >= 0
            && self.state_version > 0
            && !self.updated_at.timestamp_nanos_opt().is_none()
    }
}

/// Admin 邮件 dashboard 的全量轮询器安全摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminMailDashboardSnapshot {
    pub observed_at: DateTime<Utc>,
    pub pollers: Vec<AdminMailPollerSnapshot>,
}

impl AdminMailDashboardSnapshot {
    /// 校验观察时间、数量上限和 mailbox_key 唯一性。
    pub fn validate(&self) -> bool {
        if self.observed_at.timestamp_nanos_opt().is_none()
            || self.pollers.len() > MAX_ADMIN_MAIL_POLLER_COUNT
        {
            return false;
        }
        for (index, poller) in self.pollers.iter().enumerate() {
            if !poller.validate()
                || self.pollers[..index]
                    .iter()
                    .any(|previous| previous.mailbox_key == poller.mailbox_key)
            {
                return false;
            }
        }
        true
    }
}

/// 读取单个邮箱轮询状态的请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminMailPollStateRequest {
    pub mailbox_key: String,
}

impl AdminMailPollStateRequest {
    /// 校验邮箱标识，防止空值或控制字符穿过应用边界。
    pub fn validate(&self) -> Result<(), AdminMailError> {
        valid_text(&self.mailbox_key, MAX_ADMIN_MAILBOX_KEY_BYTES)
            .then_some(())
            .ok_or(AdminMailError::InvalidRequest)
    }
}

/// Admin 入站邮件列表筛选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminMailIntakeFilter {
    pub mailbox_key: Option<String>,
    pub world_id: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub decision: Option<String>,
    pub received_from: Option<DateTime<Utc>>,
    pub received_to: Option<DateTime<Utc>>,
    pub page: i64,
    pub page_size: i64,
}

impl AdminMailIntakeFilter {
    /// 校验筛选闭集、时间范围和分页边界。
    pub fn validate(&self) -> Result<(), AdminMailError> {
        if self.page <= 0
            || self.page_size <= 0
            || self.page_size > MAX_ADMIN_MAIL_PAGE_SIZE
            || self
                .page
                .checked_sub(1)
                .and_then(|value| value.checked_mul(self.page_size))
                .is_none_or(|offset| offset > MAX_ADMIN_MAIL_OFFSET)
            || !valid_optional_text(self.mailbox_key.as_deref(), MAX_ADMIN_MAILBOX_KEY_BYTES)
            || !valid_optional_text(self.world_id.as_deref(), 256)
            || !valid_optional_text(self.decision.as_deref(), MAX_ADMIN_MAIL_DECISION_BYTES)
            || !valid_kind(self.kind.as_deref())
            || !valid_status(self.status.as_deref())
            || self.received_from.zip(self.received_to).is_some_and(|(from, to)| from > to)
        {
            return Err(AdminMailError::InvalidRequest);
        }
        Ok(())
    }

    /// 返回已校验数字页的零基偏移。
    pub fn offset(&self) -> i64 {
        (self.page - 1) * self.page_size
    }
}

/// 入站邮件列表安全摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminMailIntakeSummary {
    pub id: Uuid,
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub world_id: String,
    pub kind: String,
    pub status: String,
    pub decision_code: String,
    pub received_at: DateTime<Utc>,
    pub processed_at: DateTime<Utc>,
}

impl AdminMailIntakeSummary {
    /// 校验摘要只包含允许展示的稳定字段。
    pub fn validate(&self) -> bool {
        !self.id.is_nil()
            && valid_text(&self.mailbox_key, MAX_ADMIN_MAILBOX_KEY_BYTES)
            && valid_text(&self.external_mail_id, 512)
            && valid_text(&self.world_id, 256)
            && valid_kind(Some(&self.kind))
            && valid_status(Some(&self.status))
            && valid_text(&self.decision_code, MAX_ADMIN_MAIL_DECISION_BYTES)
            && !self.received_at.timestamp_nanos_opt().is_none()
            && !self.processed_at.timestamp_nanos_opt().is_none()
            && self.processed_at >= self.received_at
    }
}

/// 入站邮件详情允许展示的固定元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminMailIntakeMetadata {
    pub auction_id: String,
    pub submission_id: String,
    pub user_id: String,
    pub added_count: i32,
    pub existing_count: i32,
    pub preview_revision: i64,
    pub attachment_count: i32,
    pub owner_result_code: String,
    pub reason_code: String,
    pub warning_codes: Vec<String>,
    pub delivery_ids: Vec<String>,
}

impl AdminMailIntakeMetadata {
    /// 校验 JSON 决定元数据的固定字段闭集，不把未知键传播到 DTO。
    pub fn validate(&self) -> bool {
        self.added_count >= 0
            && self.existing_count >= 0
            && self.preview_revision >= 0
            && self.attachment_count >= 0
            && valid_optional_text(Some(&self.auction_id), 512)
            && valid_optional_text(Some(&self.submission_id), 512)
            && valid_optional_text(Some(&self.user_id), 512)
            && valid_optional_text(Some(&self.owner_result_code), 512)
            && valid_optional_text(Some(&self.reason_code), 512)
            && valid_string_list(&self.warning_codes)
            && valid_string_list(&self.delivery_ids)
    }
}

/// 入站邮件详情安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminMailIntakeDetail {
    pub summary: AdminMailIntakeSummary,
    pub metadata: AdminMailIntakeMetadata,
}

/// 入站邮件数字分页结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminMailIntakePage {
    pub items: Vec<AdminMailIntakeSummary>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

impl AdminMailIntakePage {
    /// 校验存储适配器没有越过请求的分页与投影边界。
    pub fn validate(&self, request: &AdminMailIntakeFilter) -> bool {
        request.validate().is_ok()
            && self.page == request.page
            && self.page_size == request.page_size
            && self.total >= 0
            && i64::try_from(self.items.len())
                .is_ok_and(|item_count| item_count <= request.page_size)
            && (self.items.is_empty() && request.offset() >= self.total
                || !self.items.is_empty() && request.offset() < self.total)
            && self.items.iter().all(AdminMailIntakeSummary::validate)
    }
}
