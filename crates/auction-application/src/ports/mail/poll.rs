//! MoogleMail 轮询 Worker 的持久化端口。
//! 该端口只表达已通过外部适配器解析的安全入站事实、数据库游标和租约。原始 HTML、Cookie、
//! mmtoken、装备 key 和邮件正文不进入持久化对象；业务分类或外部发送必须在更窄的应用端口中
//! 另行完成。

use auction_domain::valid_text;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::{AdminMailPollState, ApplicationFuture, MailDisplaySnapshot};

#[path = "poll/progress.rs"]
mod progress;
pub use progress::{
    MAX_MAIL_PROGRESS_POLLERS, MailPollProgressFailure, MailPollProgressObserver,
    MailPollProgressOutcome, MailPollProgressSnapshot, MailPollProgressStage, is_safe_mail_poll_id,
};

#[path = "poll/reads.rs"]
mod reads;
pub use reads::{MailPollReadJournal, MailPollReadSummary};

/// 单次 MoogleMail 拉取允许提交的最大邮件数。
pub const MAX_MAIL_POLL_BATCH_SIZE: u16 = 1000;
/// 轮询失败码允许的最大字节数。
pub const MAX_MAIL_POLL_FAILURE_CODE_BYTES: usize = 128;
/// 轮询失败安全详情允许的最大字节数。
pub const MAX_MAIL_POLL_FAILURE_DETAIL_BYTES: usize = 512;
/// 入站邮件决定码允许的最大字节数。
pub const MAX_MAIL_POLL_DECISION_BYTES: usize = 256;

/// Worker 从数据库领取的一封邮箱轮询租约。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailPollClaim {
    pub mailbox_key: String,
    pub worker_id: String,
    pub claim_token: Uuid,
    pub cursor_value: Option<String>,
    pub state_version: i64,
    pub paused: bool,
    pub attempt_count: u32,
    pub lease_until: DateTime<Utc>,
}

impl MailPollClaim {
    /// 校验存储适配器返回的租约边界，防止错误状态进入远程请求路径。
    /// `paused` 只表示领取时的自动调度状态；管理员手动触发可以在不改变该状态的情况下
    /// 执行一次轮询，因此这里不能把暂停本身视为租约无效。
    pub fn validate(&self) -> bool {
        valid_text(&self.mailbox_key, 256)
            && valid_text(&self.worker_id, 256)
            && !self.claim_token.is_nil()
            && valid_optional_text(self.cursor_value.as_deref(), 256)
            && self.state_version > 0
            && self.attempt_count > 0
            && self.lease_until.timestamp_nanos_opt().is_some()
    }
}

/// 一封已经完成远端协议解析的安全入站事实。
/// `decision_metadata` 只能保存不含正文、Cookie、token 和装备 key 的 JSON 对象。Worker 会在
/// 业务处理 owner 返回安全决定后再写入该事实；`accepted`、`rejected` 和 `failed` 只表示应用
/// 决定，不代表后续外部 return/take/send 已经完成。
#[derive(Debug, Clone, PartialEq)]
pub struct MailPollIntake {
    pub id: Uuid,
    pub external_mail_id: String,
    pub sender: Option<String>,
    pub subject: Option<String>,
    pub world_id: String,
    pub mail_kind: String,
    pub status: String,
    pub content_digest: [u8; 32],
    pub received_at: DateTime<Utc>,
    pub decision_code: String,
    pub decision_metadata: Value,
    pub message_display: Option<MailDisplaySnapshot>,
    pub equipment_diagnostic: Option<crate::MailEquipmentDiagnosticSnapshot>,
    pub processed_at: DateTime<Utc>,
}

/// 远端邮件是否已有完整的本地处理证明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailPollCompletion {
    Missing,
    Completed,
}

impl MailPollIntake {
    /// 校验入站事实的闭集、摘要、JSON 对象和时间边界。
    pub fn validate(&self) -> bool {
        !self.id.is_nil()
            && valid_text(&self.external_mail_id, 512)
            && self.sender.is_some() == self.subject.is_some()
            && self.sender.as_deref().is_none_or(|value| valid_text(value, 256))
            && self.subject.as_deref().is_none_or(|value| valid_text(value, 512))
            && valid_text(&self.world_id, 256)
            && matches!(
                self.mail_kind.as_str(),
                "equipment" | "bid" | "delivery" | "registration" | "rebind" | "unknown"
            )
            && matches!(self.status.as_str(), "accepted" | "rejected" | "failed")
            && self.content_digest != [0; 32]
            && valid_text(&self.decision_code, MAX_MAIL_POLL_DECISION_BYTES)
            && self.decision_metadata.is_object()
            && (self.message_display.is_none() || self.sender.is_some() && self.subject.is_some())
            && self.message_display.as_ref().is_none_or(MailDisplaySnapshot::validate)
            && self.equipment_diagnostic.as_ref().is_none_or(|diagnostic| {
                self.mail_kind == "equipment" && self.status == "rejected" && diagnostic.validate()
            })
            && self.received_at.timestamp_nanos_opt().is_some()
            && self.processed_at.timestamp_nanos_opt().is_some()
            && self.processed_at >= self.received_at
    }
}

/// 一次远端邮件拉取成功后等待原子提交的批次。
#[derive(Debug, Clone, PartialEq)]
pub struct MailPollBatch {
    pub mailbox_key: String,
    pub claim_token: Uuid,
    pub next_cursor: Option<String>,
    pub polled_at: DateTime<Utc>,
    pub next_poll_at: DateTime<Utc>,
    pub items: Vec<MailPollIntake>,
}

impl MailPollBatch {
    /// 校验批次必须与领取的邮箱和租约对应，且整批才能进入同一数据库事务。
    pub fn validate(&self, claim: &MailPollClaim) -> bool {
        self.mailbox_key == claim.mailbox_key
            && self.claim_token == claim.claim_token
            && valid_optional_text(self.next_cursor.as_deref(), 256)
            && self.items.len() <= usize::from(MAX_MAIL_POLL_BATCH_SIZE)
            && self.polled_at.timestamp_nanos_opt().is_some()
            && self.next_poll_at.timestamp_nanos_opt().is_some()
            && self.next_poll_at >= self.polled_at
            && self.items.iter().all(MailPollIntake::validate)
    }
}

/// `claim_due`、`renew`、`commit` 和 `fail` 必须由同一数据库 owner 实现租约 fence。远程 GET
/// 只发生在 `claim_due` 提交之后，详情 GET 还须先经 MailPollReadJournal 提交恢复身份；入站事实和 cursor 只有在 `commit` 的本地事务中同时写入成功
/// 后才能推进，避免远端成功而本地丢游标或重复执行不受控。
pub trait MailPollStore: Send + Sync {
    /// 在业务处理器或外部副作用前核验一封邮件是否已有完整处理证明。
    fn lookup_completion(
        &self,
        mailbox_key: String,
        world_id: String,
        external_mail_id: String,
        received_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, MailPollCompletion>;

    /// 确保一个启用的邮箱拥有初始轮询状态，并收敛配置缩短后遗留的过晚调度水位。
    /// 已暂停邮箱、活动租约和最近失败的最大退避不得被启动过程越过；没有变化时不能递增版本。
    fn ensure_state(
        &self,
        mailbox_key: String,
        now: DateTime<Utc>,
        scheduled_next_at: DateTime<Utc>,
        retry_maximum: std::time::Duration,
    ) -> ApplicationFuture<'_, AdminMailPollState>;

    /// 领取指定邮箱中 due 且租约已过期的状态，返回值必须来自数据库锁和条件更新。
    fn claim_due(
        &self,
        mailbox_key: String,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
        force: bool,
    ) -> ApplicationFuture<'_, Vec<MailPollClaim>>;

    /// 在 token、owner、版本和当前租约仍有效时延长处理期限，不改变状态版本。
    fn renew(
        &self,
        claim: MailPollClaim,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, MailPollClaim>;

    /// 在租约 token 仍有效时原子写入入站事实、命令账本和 cursor。
    fn commit(
        &self,
        claim: MailPollClaim,
        batch: MailPollBatch,
    ) -> ApplicationFuture<'_, AdminMailPollState>;

    /// 原子提交失败项之前的连续完成前缀，并保存失败退避、释放同一租约。
    /// 不支持此能力的适配器必须拒绝，不能退化为跨事务 commit 后 fail。
    fn fail_after_prefix(
        &self,
        _claim: MailPollClaim,
        _batch: MailPollBatch,
        _retry_at: DateTime<Utc>,
        _failure_code: String,
        _safe_detail: String,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async {
            Err(crate::ApplicationError::CapabilityUnavailable("邮件完成前缀与失败退避需原子提交"))
        })
    }

    /// 在租约 token 仍有效时保存失败原因、退避时间并释放租约。
    fn fail(
        &self,
        claim: MailPollClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, AdminMailPollState>;
}

fn valid_optional_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| {
        value.is_empty()
            || (value.len() <= maximum
                && value == value.trim()
                && !value.chars().any(char::is_control))
    })
}
