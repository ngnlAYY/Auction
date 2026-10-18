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
    /// 详情页完整标题；列表标题可能被截断。
    pub subject: String,
    pub collected: bool,
    pub cod_pending: i64,
    pub remaining_credits: i64,
    pub remaining_equipment_ids: Vec<String>,
    /// 详情正文的规范摘要；用于通知类恢复证明的逐字比对。
    pub body_digest: [u8; 32],
}

/// 入站邮件当前仍挂着的资产；用于 take 动作是否已生效。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxInboxAssets {
    pub equipment_ids: Vec<String>,
    pub credits: i64,
    pub cod: i64,
}

/// 通知类投递可重建的发送身份；来自部署模板与登录 key，不含正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxNoticeExpectation {
    pub subject: String,
    pub body_digest: [u8; 32],
}

/// 仍待外部证据、允许维护流程比对的发送投递。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxUnprovenSend {
    pub delivery_id: Uuid,
    /// `settlement` 或 `mail_intake`；决定恢复证明的匹配规则。
    pub origin_kind: String,
    pub world_id: String,
    pub recipient: String,
    pub cod: i64,
    pub credits: i64,
    /// 入站邮件 ID；通知类正文渲染需要它作为默认 reference。
    pub inbound_mail_id: String,
    /// 通知类结果码；缺失时无法重建正文。
    pub result_code: Option<String>,
    /// 通知类登录 key 所属用户；只以 UUID 引用，明文不落库。
    pub user_key_user_id: Option<Uuid>,
    /// 发送围栏时间；用于限定通知类 Sent 候选的时间窗。
    pub sending_at: DateTime<Utc>,
    /// `send` 或 `take`；take 只核对收件箱，不重建正文。
    pub operation: String,
    /// take 动作要领取的入站附件 ID。
    pub attachment_ids: Vec<String>,
    /// 维护已重排队次数；达到上限后保留人工核对，避免无限重试。
    pub sweep_retries: i64,
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

    /// 用部署模板与登录 key 重建通知类投递的精确主题与正文摘要。
    /// 无法重建的模板返回 `None`，调用方保留原投递状态。
    fn notice_expectation<'a>(
        &'a self,
        candidate: &'a MoogleMailboxUnprovenSend,
    ) -> MailboxFuture<'a, Option<MoogleMailboxNoticeExpectation>>;

    /// 只读确认入站邮件是否仍存在并返回当前资产；`None` 只表示远端明确答复邮件不存在。
    fn inbox_assets<'a>(
        &'a self,
        world_id: &'a str,
        external_mail_id: &'a str,
    ) -> MailboxFuture<'a, Option<MoogleMailboxInboxAssets>>;
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

    /// 把唯一精确匹配的 Sent MID 绑定回通知类投递；通知没有履约事实可重新排队。
    fn bind_recovered_notice<'a>(
        &'a self,
        delivery_id: Uuid,
        external_mail_id: String,
        metadata: Value,
    ) -> MailboxFuture<'a, ()>;

    /// 把已不在收件箱的 take 动作收口为成功；附件已随邮件一起消失。
    fn bind_recovered_action<'a>(
        &'a self,
        delivery_id: Uuid,
        external_mail_id: String,
        metadata: Value,
    ) -> MailboxFuture<'a, ()>;

    /// 把仍未证明的投递退回收件/发送队列重试，并累计重试次数。
    fn requeue_unproven<'a>(
        &'a self,
        delivery_id: Uuid,
        retries: i64,
        reason: &'a str,
    ) -> MailboxFuture<'a, ()>;
}
