//! Admin 邮件轮询状态与入站审查端口。
//!
//! 这里保留旧系统 mail owner 的安全投影和暂停命令边界。邮件正文、内容摘要、Cookie 与
//! 外部 MoogleMail 响应都不属于 Admin DTO；暂停命令的版本 fence、幂等账本和审计由同一
//! PostgreSQL 事务完成。

use std::future::Future;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::CommandId;

use super::{
    ApplicationFuture, RequestHash, TransactionFuture, TransactionPolicy, TransactionPolicyError,
};

/// Admin 邮箱标识允许的最大字节数。
pub const MAX_ADMIN_MAILBOX_KEY_BYTES: usize = 256;
/// Admin 邮件查询的最大页大小。
pub const MAX_ADMIN_MAIL_PAGE_SIZE: i64 = 100;
/// Admin 邮件数字分页允许的最大偏移。
pub const MAX_ADMIN_MAIL_OFFSET: i64 = 2_147_483_647;
/// Admin 邮件决定码允许的最大字节数。
pub const MAX_ADMIN_MAIL_DECISION_BYTES: usize = 256;

/// 生成邮箱范围内稳定的命令聚合标识。
pub fn admin_mail_aggregate_id(mailbox_key: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("auction/admin-mail/{mailbox_key}").as_bytes())
}

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
            && self.items.len() as i64 <= request.page_size
            && (self.items.is_empty() && request.offset() >= self.total
                || !self.items.is_empty() && request.offset() < self.total)
            && self.items.iter().all(AdminMailIntakeSummary::validate)
    }
}

/// Admin 邮件暂停/恢复轮询命令。
#[derive(Debug, Clone)]
pub struct AdminMailPollingCommand {
    pub command_id: CommandId,
    pub mailbox_key: String,
    pub paused: bool,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl AdminMailPollingCommand {
    /// 校验不依赖数据库当前状态的命令字段。
    pub fn validate(&self) -> Result<(), AdminMailError> {
        if self.command_id.is_nil()
            || self.expected_state_version <= 0
            || self.expected_state_version == i64::MAX
            || self.request_hash.as_bytes() == [0; 32]
            || !valid_text(&self.mailbox_key, MAX_ADMIN_MAILBOX_KEY_BYTES)
            || !valid_text(&self.actor_id, 512)
            || !valid_optional_text(self.request_id.as_deref(), 512)
        {
            return Err(AdminMailError::InvalidCommand);
        }
        Ok(())
    }
}

/// 邮件 Admin 命令账本中可安全重放的最小记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminMailStoredCommand {
    pub command_id: CommandId,
    pub command_kind: String,
    pub mailbox_key: String,
    pub request_hash: RequestHash,
    pub result_payload: Vec<u8>,
}

impl AdminMailStoredCommand {
    /// 校验账本记录，防止把损坏结果交给 HTTP 重放。
    pub fn validate(&self) -> Result<(), AdminMailError> {
        if self.command_id.is_nil()
            || self.command_kind != "set_polling_paused"
            || !valid_text(&self.mailbox_key, MAX_ADMIN_MAILBOX_KEY_BYTES)
            || self.request_hash.as_bytes() == [0; 32]
            || self.result_payload.is_empty()
            || serde_json::from_slice::<serde_json::Value>(&self.result_payload)
                .ok()
                .is_none_or(|value| !value.is_object())
        {
            return Err(AdminMailError::StoredResultCorrupt);
        }
        Ok(())
    }

    /// 判断账本记录是否对应同一暂停命令。
    pub fn matches_request(&self, command: &AdminMailPollingCommand) -> bool {
        self.command_kind == "set_polling_paused"
            && self.mailbox_key == command.mailbox_key
            && self.request_hash == command.request_hash
    }
}

/// Admin 邮件只读端口。
pub trait AdminMailReader: Send + Sync {
    fn poll_state(
        &self,
        request: AdminMailPollStateRequest,
    ) -> ApplicationFuture<'_, AdminMailPollState>;

    fn list_intakes(
        &self,
        request: AdminMailIntakeFilter,
    ) -> ApplicationFuture<'_, AdminMailIntakePage>;

    fn intake_detail(
        &self,
        intake_id: Uuid,
    ) -> ApplicationFuture<'_, Option<AdminMailIntakeDetail>>;
}

/// Admin 邮件状态命令端口。
pub trait AdminMailCommandPort: Send + Sync {
    fn set_polling_paused(
        &self,
        command: AdminMailPollingCommand,
    ) -> ApplicationFuture<'_, AdminMailPollState>;
}

/// Admin 邮件完整端口；装配根可以用一个 trait object 注入同一 owner。
pub trait AdminMailPort: AdminMailReader + AdminMailCommandPort {}

impl<T> AdminMailPort for T where T: AdminMailReader + AdminMailCommandPort {}

/// Admin 邮件应用错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AdminMailError {
    #[error("Admin 邮件查询参数无效")]
    InvalidRequest,
    #[error("Admin 邮件命令参数无效")]
    InvalidCommand,
    #[error("Admin 邮件命令与已有请求冲突")]
    CommandConflict,
    #[error("Admin 邮件安全投影无效")]
    InvalidProjection,
    #[error("Admin 邮件命令账本结果损坏")]
    StoredResultCorrupt,
}

/// Admin 邮件事务错误。
#[derive(Debug, Error)]
pub enum AdminMailTransactionError {
    #[error("邮件轮询状态不存在")]
    NotFound,
    #[error("邮件轮询状态版本已变化：当前版本 {current_state_version}")]
    StateConflict { current_state_version: i64 },
    #[error("邮件 Admin 命令账本发生唯一冲突")]
    Conflict,
    #[error("邮件 Admin 事务读取或写入失败: {0}")]
    Failed(String),
    #[error("邮件 Admin 事务发生可重试的序列化冲突")]
    SerializationConflict,
}

/// 邮件 Admin 事务 runner 的失败类型边界。
pub trait AdminMailTransactionFailure:
    From<AdminMailTransactionError> + From<TransactionPolicyError> + Send
{
    fn is_serialization_conflict(&self) -> bool;
}

/// 邮件暂停命令事务的窄适配器能力。
pub trait AdminMailTransaction: Send {
    fn find_command(
        &mut self,
        command_id: CommandId,
    ) -> impl Future<Output = Result<Option<AdminMailStoredCommand>, AdminMailTransactionError>> + Send;

    fn insert_command(
        &mut self,
        command: AdminMailStoredCommand,
    ) -> impl Future<Output = Result<(), AdminMailTransactionError>> + Send;

    fn set_polling_paused(
        &mut self,
        command: AdminMailPollingCommand,
        updated_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminMailPollState, AdminMailTransactionError>> + Send;

    fn append_pause_record(
        &mut self,
        command: AdminMailPollingCommand,
        result: AdminMailPollState,
    ) -> impl Future<Output = Result<(), AdminMailTransactionError>> + Send;
}

/// 邮件暂停命令事务 runner；PostgreSQL 使用 Serializable 与有限重试。
pub trait AdminMailTransactionRunner: Send + Sync {
    type Transaction: AdminMailTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: AdminMailTransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send;
}

fn valid_kind(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        value.is_empty()
            || matches!(
                value,
                "equipment" | "bid" | "delivery" | "registration" | "rebind" | "unknown"
            )
    })
}

fn valid_status(value: Option<&str>) -> bool {
    value
        .is_none_or(|value| value.is_empty() || matches!(value, "accepted" | "rejected" | "failed"))
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_optional_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| {
        value.is_empty()
            || (value.len() <= maximum
                && value == value.trim()
                && !value.chars().any(char::is_control))
    })
}

fn valid_string_list(values: &[String]) -> bool {
    values.len() <= 1000 && values.iter().all(|value| valid_text(value, 512))
}
