//! 寄售 poll 的锁外预读、同事务决定与前缀提交端口。
//!
//! 本端口只承接已经由 4D1 认证并绑定会话的观察；正文、装备 key、预约码与 Cookie 不进入
//! 任何字段。首次决定、观察、命令回执、待读前缀与轮询游标必须由同一顶层事务提交。

use auction_domain::CommandId;
use auction_domain::SettlementWorld;
use auction_domain::UserId;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeObservation, LegacyReturnEvidence, ReservationBinding,
    TrustedConsignmentRts,
};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::consignment::{DecideEnvelopeCommand, EnvelopeReceipt};
use crate::ports::core::ApplicationFuture;
use crate::ports::mail::MailPollClaim;

/// 首次决定前的锁外预读。
///
/// 只用于准备命令、锁集合与卷宗恢复；锁内仍按同一 `(world,EID)` 实物、卖家与 K 重新读取
/// 并复算，预读结果不是授权。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopePreflight {
    /// 完整发件人按既有唯一映射得到的卖家；歧义或缺失时为空。
    pub seller_user_id: Option<UserId>,
    /// 与观察逐件规范 EID 对应、仍可为决定所引用的预约。
    pub reservations: Vec<ReservationBinding>,
    /// 已提交的首次决定；存在时只推进前缀，不重写观察或决定。
    pub existing_decision: Option<EnvelopeReceipt>,
    /// 当前世界就绪投影引用的证据会话。
    pub session_id: Uuid,
    /// 当前世界已签收的收件能力。
    pub capabilities: EnvelopeCapabilities,
    /// 数据库时钟读取的预读时刻。
    pub now: Timestamp,
    /// 全部预约共享的季窗口；不一致或缺失时为空。
    pub season_window_id: Option<Uuid>,
    /// 系统 RTS 按定位线索唯一指认的寄售原轮；只用于定位与取锁，接管写端在锁内按持久派发事实复判。
    pub consignment_rts: Option<TrustedConsignmentRts>,
}

/// 系统 RTS 的寄售来源定位线索：主题证明的原义务与正文末行的派发引用原文。
///
/// 锁外线索，不是授权：存储只用它在派发轮里唯一定位原轮，读不出或不唯一就没有来源（整封隔离）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsignmentRtsLocator {
    /// `[RTS] ` + 寄售规范主题里的义务 UUID。
    pub obligation_id: Uuid,
    /// RTS 正文末行的派发引用原文（寄售轮 ID）；缺失时只在该义务恰有一轮时定位。
    pub dispatch_reference: Option<String>,
}

/// 首次观察在本事务内写入所需的原始字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollRawObservation {
    /// 稳定原信键。
    pub mail_key: OriginalMailKey,
    /// 认证证据会话。
    pub session_id: Uuid,
    /// 邮箱键；只用于待读定位。
    pub mailbox_key: String,
    /// 远端观察身份。
    pub remote_observation_id: String,
    /// 观察时刻。
    pub observed_at: Timestamp,
    /// 解析器版本。
    pub parser_version: String,
    /// 受控证据引用。
    pub evidence_ref: Option<String>,
    /// 主题摘要。
    pub subject_digest: [u8; 32],
    /// 正文摘要。
    pub body_digest: [u8; 32],
    /// 完整发件人显示名。
    pub sender: Option<String>,
    /// 完整收件人显示名。
    pub recipient: Option<String>,
    /// 原始附件数；读不出时为空。
    pub raw_count: Option<u32>,
    /// 领域观察。
    pub observation: EnvelopeObservation,
    /// 系统 RTS 上按旧路径口径拆出的退件证据。
    pub legacy_evidence: LegacyReturnEvidence,
    /// 原始收件分钟文本。
    pub received_minute_raw: String,
    /// 原始收件时区解释。
    pub received_timezone: String,
    /// 首次收件时刻。
    pub first_seen_at: Timestamp,
    /// 冻结季。
    pub season_window_id: Option<Uuid>,
}

/// 一封寄售来信的原子提交。
///
/// `command` 为空表示已有决定，只推进前缀并释放租约；`raw` 与 `command` 必须同时存在或同时
/// 为空。`cursor_mid` 是本次要越过的邮件；其决定、观察与命令回执在同一事务内提交。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollEnvelopeWrite {
    /// 稳定原信键。
    pub mail_key: OriginalMailKey,
    /// 命令 UUID；已有决定时取其原命令 UUID。
    pub command_id: CommandId,
    /// 首次决定命令；已有决定时为空。
    pub command: Option<DecideEnvelopeCommand>,
    /// 首次观察的原始字段；已有决定时为空。
    pub raw: Option<PollRawObservation>,
    /// 当前轮询租约。
    pub claim: MailPollClaim,
    /// 本次轮询时刻。
    pub polled_at: Timestamp,
    /// 下一次调度时刻。
    pub next_poll_at: Timestamp,
    /// 本次要推进到的邮件 MID。
    pub cursor_mid: String,
}

/// 寄售 poll 的存储端口。
pub trait PollEnvelopeStore: Send + Sync {
    /// 锁外预读卖家、预约、已有决定、会话与能力；任何不一致都失败关闭。
    ///
    /// `rts` 是系统 RTS 的寄售来源线索；存储据此定位原轮写进 [`EnvelopePreflight::consignment_rts`]。
    fn load_envelope_preflight(
        &self,
        key: OriginalMailKey,
        observation: EnvelopeObservation,
        sender: Option<String>,
        rts: Option<ConsignmentRtsLocator>,
    ) -> ConsignmentFuture<'_, EnvelopePreflight>;

    /// 同一顶层事务写观察、首次决定、回执与工作，随后删除待读前缀、推进游标并释放租约。
    fn decide_envelope_in_poll(
        &self,
        write: PollEnvelopeWrite,
    ) -> ConsignmentFuture<'_, EnvelopeReceipt>;
}

/// 候选门控的只读端口：世界就绪有效且引用会话仍存在时才允许 4D1 候选分流。
pub trait MailReadinessGate: Send + Sync {
    /// 读取数据库实时有效期；查询失败由调用方按关闭处理。
    fn consignment_candidates_enabled(&self, world: SettlementWorld)
    -> ApplicationFuture<'_, bool>;
}
