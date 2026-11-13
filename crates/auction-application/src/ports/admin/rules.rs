//! Admin 规则文档的请求、发布命令与读写端口。

use std::future::Future;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use auction_domain::{CommandId, valid_text};

use crate::ports::auction::ApplicationFuture;
use crate::ports::core::{
    RequestHash, TransactionFuture, TransactionPolicy, TransactionPolicyError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRulesRequest {
    pub locale: String,
}

impl AdminRulesRequest {
    /// 校验规则语言标签；管理端沿用旧系统的语言标签语法，不把任意输入带入 SQL。
    pub fn validate(&self) -> Result<(), AdminRulesError> {
        valid_locale(&self.locale).then_some(()).ok_or(AdminRulesError::InvalidLocale)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminRulesCandidate {
    pub locale: String,
    pub title: String,
    pub markdown: String,
}

impl AdminRulesCandidate {
    /// 只修剪标题首尾空白；Markdown 正文保持原始内容，避免发布时改写用户文档。
    pub fn normalize(mut self) -> Self {
        self.title = self.title.trim().to_owned();
        self
    }

    /// 校验规则候选的语言、标题和正文边界。
    pub fn validate(&self) -> Result<(), AdminRulesError> {
        validate_document_fields(&self.locale, &self.title, &self.markdown)
    }
}

/// 已发布规则文档的安全快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminRulesDocument {
    pub locale: String,
    pub title: String,
    pub markdown: String,
    pub published_by: String,
    pub revision: i64,
    pub published_at: DateTime<Utc>,
}

impl AdminRulesDocument {
    /// 校验从存储层返回的完整规则快照。
    pub fn validate(&self) -> Result<(), AdminRulesError> {
        validate_document_fields(&self.locale, &self.title, &self.markdown)?;
        if self.revision <= 0
            || self.published_at.timestamp_micros() == 0
            || !valid_text(&self.published_by, MAX_RULE_ACTOR_BYTES)
        {
            return Err(AdminRulesError::StoredResultCorrupt);
        }
        Ok(())
    }
}

/// 规则发布命令；HTTP 之外的入口也必须复用同一命令和摘要语义。
#[derive(Debug, Clone)]
pub struct AdminRulesPublishCommand {
    pub command_id: CommandId,
    pub expected_revision: i64,
    pub locale: String,
    pub title: String,
    pub markdown: String,
    pub actor_id: String,
    /// CLI 装配根采集的身份；不参与业务摘要。
    pub operator_identity: Option<crate::ports::core::CliOperatorIdentity>,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl AdminRulesPublishCommand {
    /// 按规则候选的规范化约定生成命令副本。
    pub fn normalize(mut self) -> Self {
        self.title = self.title.trim().to_owned();
        self
    }

    /// 校验不依赖数据库当前版本的命令字段。
    pub fn validate(&self) -> Result<(), AdminRulesError> {
        if self.command_id.is_nil()
            || self.expected_revision < 0
            || self.expected_revision == i64::MAX
            || !valid_text(&self.actor_id, MAX_RULE_ACTOR_BYTES)
            || self
                .request_id
                .as_deref()
                .is_some_and(|value| !valid_text(value, MAX_RULE_REQUEST_ID_BYTES))
        {
            return Err(AdminRulesError::InvalidCommand);
        }
        validate_document_fields(&self.locale, &self.title, &self.markdown)
    }
}

/// 规则命令账本中可安全重放的最小记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRulesStoredCommand {
    pub command_id: CommandId,
    pub command_kind: String,
    pub locale: String,
    pub request_hash: RequestHash,
    pub result_payload: Vec<u8>,
}

impl AdminRulesStoredCommand {
    /// 校验账本记录形状，防止把损坏结果交给 HTTP 重放。
    pub fn validate(&self) -> Result<(), AdminRulesError> {
        if self.command_id.is_nil()
            || self.command_kind.trim().is_empty()
            || !valid_locale(&self.locale)
            || self.result_payload.is_empty()
            || serde_json::from_slice::<serde_json::Value>(&self.result_payload)
                .ok()
                .is_none_or(|value| !value.is_object())
        {
            return Err(AdminRulesError::StoredResultCorrupt);
        }
        Ok(())
    }

    /// 判断账本记录是否对应同一个规则发布请求。
    pub fn matches_request(
        &self,
        command_kind: &str,
        locale: &str,
        request_hash: RequestHash,
    ) -> bool {
        self.command_kind == command_kind
            && self.locale == locale
            && self.request_hash == request_hash
    }
}

pub trait AdminRulesReader: Send + Sync {
    fn get_admin_rules(
        &self,
        request: AdminRulesRequest,
    ) -> ApplicationFuture<'_, Option<AdminRulesDocument>>;
}

pub trait AdminRulesPort: Send + Sync {
    fn get(&self, request: AdminRulesRequest) -> ApplicationFuture<'_, Option<AdminRulesDocument>>;

    fn publish(
        &self,
        command: AdminRulesPublishCommand,
    ) -> ApplicationFuture<'_, AdminRulesDocument>;
}

/// 规则事务的专用错误集合；它不复用竞价事务，避免误用锁集合和提交语义。
#[derive(Debug, Error)]
pub enum AdminRulesTransactionError {
    #[error("规则发布与当前版本冲突：当前版本 {current_revision}")]
    RevisionConflict { current_revision: i64 },
    #[error("规则命令账本发生唯一冲突")]
    Conflict,
    #[error("规则事务读取或写入失败: {0}")]
    Failed(String),
    #[error("规则事务发生可重试的序列化冲突")]
    SerializationConflict,
}

/// 规则事务 runner 的失败类型边界。
pub trait AdminRulesTransactionFailure:
    From<AdminRulesTransactionError> + From<TransactionPolicyError> + Send
{
    fn is_serialization_conflict(&self) -> bool;
}

/// 规则发布事务的窄适配器能力。
pub trait AdminRulesTransaction: Send {
    fn find_command(
        &mut self,
        command_id: CommandId,
    ) -> impl Future<Output = Result<Option<AdminRulesStoredCommand>, AdminRulesTransactionError>> + Send;

    fn insert_command(
        &mut self,
        command: AdminRulesStoredCommand,
    ) -> impl Future<Output = Result<(), AdminRulesTransactionError>> + Send;

    fn publish_document(
        &mut self,
        command: AdminRulesPublishCommand,
        published_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminRulesDocument, AdminRulesTransactionError>> + Send;

    fn append_publish_record(
        &mut self,
        command: AdminRulesPublishCommand,
        result: AdminRulesDocument,
    ) -> impl Future<Output = Result<(), AdminRulesTransactionError>> + Send;
}

/// 规则发布事务 runner；PostgreSQL 使用 Serializable 与有限重试。
pub trait AdminRulesTransactionRunner: Send + Sync {
    type Transaction: AdminRulesTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: AdminRulesTransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send;
}

/// 规则文档允许的最大标题字节数。
pub const MAX_RULE_TITLE_BYTES: usize = 512;
/// 规则正文允许的最大字节数；HTTP 层还会施加请求体上限。
pub const MAX_RULE_MARKDOWN_BYTES: usize = 1 << 20;
/// 规则操作者标识允许的最大字节数。
pub const MAX_RULE_ACTOR_BYTES: usize = 128;
/// 请求关联 ID 允许的最大字节数。
pub const MAX_RULE_REQUEST_ID_BYTES: usize = 128;

/// 候选、发布命令和存储快照复用借用校验，避免仅为检查上限而复制整篇 Markdown。
fn validate_document_fields(
    locale: &str,
    title: &str,
    markdown: &str,
) -> Result<(), AdminRulesError> {
    if !valid_locale(locale) {
        return Err(AdminRulesError::InvalidLocale);
    }
    if !valid_text(title, MAX_RULE_TITLE_BYTES) || markdown.len() > MAX_RULE_MARKDOWN_BYTES {
        return Err(AdminRulesError::InvalidDocument);
    }
    Ok(())
}

fn valid_locale(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 35
        && value == value.trim()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value.chars().all(|character| character == '-' || character.is_ascii_alphanumeric())
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AdminRulesError {
    #[error("规则语言标签无效")]
    InvalidLocale,
    #[error("规则文档无效")]
    InvalidDocument,
    #[error("规则发布命令无效")]
    InvalidCommand,
    #[error("请求与既有规则发布命令冲突")]
    CommandConflict,
    #[error("规则文档版本冲突")]
    RevisionConflict { current_revision: i64 },
    #[error("规则发布结果损坏")]
    StoredResultCorrupt,
}
