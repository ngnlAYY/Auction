//! MoogleMail 定期维护端口：compose 草稿、受控 discard、Sent 观察和本地证据写入。
//!
//! 维护读取远端页面并记录观察；只有可证明的恢复才同事务迁移投递、资金、履约与审计。

use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

pub type MailboxFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;

use crate::ApplicationError;

/// compose 页当前草稿的只读投影。
/// 未发送草稿可能包含登录 key；主题和正文不得进入日志或调试输出。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailboxComposeState {
    pub clean: bool,
    pub recipient: String,
    pub subject: String,
    pub body: String,
    pub credits: i64,
    /// 页面未提供可证明的 CoD 时为空。
    pub cod: Option<i64>,
    pub attachment_count: usize,
    /// 可见草稿的精确装备身份与规范正文摘要，只用于比对原冻结发送。
    pub equipment_ids: Vec<String>,
    pub body_digest: [u8; 32],
    pub observation_credential: Option<crate::DeliverySendCredentialBinding>,
}

impl std::fmt::Debug for MoogleMailboxComposeState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MoogleMailboxComposeState")
            .field("clean", &self.clean)
            .field("recipient", &self.recipient)
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachment_count)
            .finish()
    }
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
    /// 本次详情读取的持久凭据身份；普通观察不产生内部资金或库存授权。
    pub observation_credential: Option<crate::DeliverySendCredentialBinding>,
}

/// 入站邮件当前仍挂着的资产；用于 take 动作是否已生效。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxInboxAssets {
    /// 原 MID 详情证明的发件人，不能仅凭资产为空把另一身份的邮件视为已领取。
    pub sender: String,
    pub equipment_ids: Vec<String>,
    pub credits: i64,
    pub cod: i64,
}

/// 无登录 key 的历史通知保守重建摘要；带 key 的旧行不能用当前 key 冒充原发送内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxNoticeExpectation {
    pub subject: String,
    pub body_digest: [u8; 32],
}

/// 仍待外部证据、允许维护流程比对的发送投递。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxUnprovenSend {
    pub delivery_id: Uuid,
    /// 当前投递行的 fencing version；只用于观察、discard 与恢复事务的 CAS。
    pub fencing_version: i64,
    /// 冻结发送的原始 fencing version；Sending 过期提升当前版本后仍保留此尝试身份。
    /// 无冻结期望的旧行不据此推断历史发送身份，只沿用保守历史证明。
    pub sending_fencing_version: i64,
    /// 真实发送前冻结的精确身份；旧行缺失时只允许原有保守历史证明路径。
    pub send_expectation: Option<crate::DeliverySendExpectation>,
    /// 投递业务 owner；冻结 Send 统一精确比对，旧行仍按历史规则隔离。
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
    /// 发送围栏时间；恢复仅接受此时间之后的有界 Sent 证据，不回退到分钟起点。
    pub sending_at: DateTime<Utc>,
    /// 原未知结果持久化时间，限定本次发送证据上界，不用任意时钟偏移扩展窗口。
    pub outcome_recorded_at: DateTime<Utc>,
    /// `send` 或 `take`；take 只核对收件箱，不重建正文。
    pub operation: String,
    /// take 要领取的附件，或 settlement 冻结 secret 引用解析出的精确装备 ID；不含 secret。
    pub attachment_ids: Vec<String>,
    /// 维护已重排队次数；达到上限后保留人工核对，避免无限重试。
    pub sweep_retries: i64,
    /// 业务义务 ID；出现在标题的方括号或正文 `reference` 之后。
    pub reference: String,
    /// 邮件模板；carrier 付款邮件没有装备附件。
    pub template: String,
}

/// 稳定轮转中的单条候选；不可重建的旧行仍保留 ID，避免永远占据批次前缀。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailboxSweepCandidate {
    pub delivery_id: Uuid,
    /// `None` 只允许推进核对游标，不得据此读取或迁移投递业务状态。
    pub send: Option<MoogleMailboxUnprovenSend>,
}

/// 远端三页读取端口。
pub trait MoogleMailboxMaintenance: Send + Sync {
    fn compose<'a>(&'a self, world_id: &'a str) -> MailboxFuture<'a, MoogleMailboxComposeState>;

    /// 调用前必须持久登记一次性尝试；持有同一会话重读并逐字段比对预期草稿后才可 POST。
    fn discard_compose<'a>(
        &'a self,
        world_id: &'a str,
        expected: &'a MoogleMailboxComposeState,
    ) -> MailboxFuture<'a, MoogleMailboxDiscard>;

    fn sent_entries<'a>(
        &'a self,
        world_id: &'a str,
    ) -> MailboxFuture<'a, Vec<MoogleMailboxSentEntry>>;

    /// 只观察 Read Mail 列表并返回数量，不把历史已读邮件重新投进业务 intake。
    fn read_mail_observed<'a>(&'a self, world_id: &'a str) -> MailboxFuture<'a, usize>;

    fn sent_state<'a>(
        &'a self,
        world_id: &'a str,
        entry: &'a MoogleMailboxSentEntry,
        observed_at: DateTime<Utc>,
    ) -> MailboxFuture<'a, MoogleMailboxSentState>;

    /// 仅无登录 key 的旧行可按原模板保守核对，且正文必须保留原入站关联。
    /// 不读取当前登录 key；缺少原内容身份或通用重复模板返回 `None`，禁止自动重发。
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

/// 本地观察与受证明恢复端口；状态变更必须与资金、履约及追加审计原子提交。
pub trait MoogleMailboxEvidenceStore: Send + Sync {
    /// 按持久游标稳定回绕读取候选；读取本身不推进，未处理后缀必须留给下一轮。
    fn unproven_sends<'a>(
        &'a self,
        world_id: &'a str,
        limit: i64,
    ) -> MailboxFuture<'a, Vec<MoogleMailboxSweepCandidate>>;

    /// 单条开始核对前推进游标；提交不确定时停止，不把尝试位置当作业务完成证明。
    fn record_sweep_attempt<'a>(
        &'a self,
        world_id: &'a str,
        delivery_id: Uuid,
    ) -> MailboxFuture<'a, bool>;

    /// 原子登记原 fencing 尝试的一次性 discard；状态、冻结身份变化或提交未知均不授权外写。
    fn begin_compose_discard<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        expectation: Option<&'a crate::DeliverySendExpectation>,
    ) -> MailboxFuture<'a, bool>;

    fn record_sent_observation<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        observation: Value,
    ) -> MailboxFuture<'a, ()>;

    fn record_compose_discard<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        observation: Value,
    ) -> MailboxFuture<'a, ()>;

    /// 把唯一精确匹配的 Sent MID 绑定回原投递，并把履约重新排队等待收取证据。
    /// 资金占用同步收口为 Sent；返回 true 仅表示本事务实际迁移并已确认提交，不把重放计为恢复。
    fn bind_recovered_send<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        external_mail_id: String,
        metadata: Value,
        expectation: Option<&'a crate::DeliverySendExpectation>,
    ) -> MailboxFuture<'a, bool>;

    /// 把已不在收件箱的 take 动作收口为成功；仅实际迁移并确认提交返回 true。
    fn bind_recovered_action<'a>(
        &'a self,
        delivery_id: Uuid,
        external_mail_id: String,
        metadata: Value,
    ) -> MailboxFuture<'a, bool>;

    /// 仅将已正向确认附件仍在的 take 重新排队，返回实际迁移结果；未知 Send 禁止走此入口。
    fn requeue_unproven<'a>(
        &'a self,
        delivery_id: Uuid,
        retries: i64,
        reason: &'a str,
    ) -> MailboxFuture<'a, bool>;
}
