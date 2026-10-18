//! MoogleMail 定期维护端口：compose 草稿、受控 discard、Sent 观察和本地证据写入。
//!
//! 维护流程只读取远端页面并记录观察；它不改变投递状态，也不自行宣告发送成功。

use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

pub type MailboxFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;

use crate::ApplicationError;

/// compose 页当前草稿的只读投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxComposeState {
    pub clean: bool,
    pub recipient: String,
    pub subject: String,
    pub body: String,
    pub credits: i64,
    /// 页面未提供可证明的 CoD 时为空。
    pub cod: Option<i64>,
    pub attachment_count: usize,
}

/// 受控 discard 结果；已干净时没有发生远端写入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoogleMailboxDiscard {
    AlreadyClean,
    Discarded,
}

/// Sent 列表摘要；领取状态必须单独探查。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxSentEntry {
    pub external_mail_id: String,
    pub recipient: String,
    pub subject: String,
    pub sent_at: DateTime<Utc>,
}

/// 单封 Sent 的当前待领取状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxSentState {
    pub external_mail_id: String,
    pub collected: bool,
    pub cod_pending: i64,
    pub remaining_credits: i64,
    pub remaining_equipment_ids: Vec<String>,
}

/// 仍待外部证据、允许维护流程比对的发送投递。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxUnprovenSend {
    pub delivery_id: Uuid,
    pub world_id: String,
    pub recipient: String,
    pub cod: i64,
    /// 业务义务 ID；出现在标题的方括号或正文 `reference` 之后。
    pub reference: String,
    /// 邮件模板；carrier 付款邮件没有装备附件。
    pub template: String,
}

/// 远端三页读取端口。
pub trait MoogleMailboxMaintenance: Send + Sync {
    fn compose<'a>(&'a self, world_id: &'a str) -> MailboxFuture<'a, MoogleMailboxComposeState>;

    fn discard_compose<'a>(&'a self, world_id: &'a str) -> MailboxFuture<'a, MoogleMailboxDiscard>;

    fn sent_entries<'a>(
        &'a self,
        world_id: &'a str,
    ) -> MailboxFuture<'a, Vec<MoogleMailboxSentEntry>>;

    fn sent_state<'a>(
        &'a self,
        world_id: &'a str,
        entry: &'a MoogleMailboxSentEntry,
        observed_at: DateTime<Utc>,
    ) -> MailboxFuture<'a, MoogleMailboxSentState>;
}

/// 维护流程的本地读取与追加证据端口；不修改投递状态机。
pub trait MoogleMailboxEvidenceStore: Send + Sync {
    fn unproven_sends<'a>(
        &'a self,
        world_id: &'a str,
        limit: i64,
    ) -> MailboxFuture<'a, Vec<MoogleMailboxUnprovenSend>>;

    fn record_sent_observation<'a>(
        &'a self,
        delivery_id: Uuid,
        observation: Value,
    ) -> MailboxFuture<'a, ()>;

    fn record_compose_discard<'a>(
        &'a self,
        delivery_id: Uuid,
        observation: Value,
    ) -> MailboxFuture<'a, ()>;

    /// 把唯一精确匹配的 Sent MID 绑定回原投递，并把履约重新排队等待收取证据。
    /// 只用于升级前 `write_unknown` 的历史投递；成功证明标记为恢复证明，不冒充实时发送证明。
    fn bind_recovered_send<'a>(
        &'a self,
        delivery_id: Uuid,
        external_mail_id: String,
        metadata: Value,
    ) -> MailboxFuture<'a, ()>;
}
