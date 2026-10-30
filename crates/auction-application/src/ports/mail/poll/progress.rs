//! 邮件轮询的只读运行观测；不参与租约、业务授权、游标或命令结果。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::MailPollClaim;

/// 同一 active 邮件凭据仅装配 Main/Isekai 两个世界。
pub const MAX_MAIL_PROGRESS_POLLERS: usize = 2;

/// 仅允许展示正十进制 MID，不能把任意 opaque cursor 当作安全诊断值。
pub fn is_safe_mail_poll_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && matches!(value.as_bytes().first(), Some(b'1'..=b'9'))
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// 入站轮询观测阶段；成功/失败状态只描述本次轮询，不代表外部投递完成。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MailPollProgressStage {
    Fetching,
    ReadingDetail,
    Processing,
    Committing,
    Idle,
    Failed,
    Canceled,
}

impl MailPollProgressStage {
    /// 返回稳定的诊断阶段代码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fetching => "fetching",
            Self::ReadingDetail => "reading_detail",
            Self::Processing => "processing",
            Self::Committing => "committing",
            Self::Idle => "idle",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    /// 只有在途阶段允许继续接收同一 claim 的更新。
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Fetching | Self::ReadingDetail | Self::Processing | Self::Committing)
    }
}

/// 最近失败定位；重启后清空，不替代持久化失败记录或加密证据。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailPollProgressFailure {
    pub stage: MailPollProgressStage,
    pub external_mail_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

/// 仅含邮箱、世界、数字 MID 与时间的运行快照；不含 claim token 或原始 cursor。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailPollProgressSnapshot {
    pub mailbox_key: String,
    pub world_id: String,
    pub stage: MailPollProgressStage,
    pub external_mail_id: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub last_failure: Option<MailPollProgressFailure>,
}

impl MailPollProgressSnapshot {
    /// 校验跨进程观测的闭集、字段关联和时间边界；不接受任意文本游标。
    pub fn validate(&self, observed_at: DateTime<Utc>) -> bool {
        let has_mail_stage = matches!(
            self.stage,
            MailPollProgressStage::ReadingDetail | MailPollProgressStage::Processing
        );
        auction_domain::valid_text(&self.mailbox_key, 256)
            && matches!(self.world_id.as_str(), "main" | "isekai")
            && self.updated_at.timestamp_nanos_opt().is_some()
            && self.updated_at <= observed_at
            && self.external_mail_id.as_deref().is_none_or(is_safe_mail_poll_id)
            && (has_mail_stage || self.external_mail_id.is_none())
            && self.last_failure.as_ref().is_none_or(|failure| {
                failure.stage.is_active()
                    && failure.occurred_at.timestamp_nanos_opt().is_some()
                    && failure.occurred_at <= observed_at
                    && failure.external_mail_id.as_deref().is_none_or(is_safe_mail_poll_id)
                    && (matches!(
                        failure.stage,
                        MailPollProgressStage::ReadingDetail | MailPollProgressStage::Processing
                    ) || failure.external_mail_id.is_none())
            })
    }
}

/// 轮询任务的观测收口；取消不证明邮件处理失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailPollProgressOutcome {
    Succeeded,
    Failed,
    Canceled,
}

/// 同步且尽力而为的观测回调；实现必须有界，失败只能丢观测，不能决定业务结果。
/// claim 身份仅用于隔离迟到回调，禁止进入跨进程或浏览器投影。
pub trait MailPollProgressObserver: Send + Sync {
    /// 登记当前 claim；新版本可替代旧版本的进程内观测。
    fn begin(&self, claim: &MailPollClaim, world_id: &str);
    /// 在真实读取/业务调用前更新目标，不能以待读队首推断当前邮件。
    fn update(
        &self,
        claim: &MailPollClaim,
        stage: MailPollProgressStage,
        external_mail_id: Option<&str>,
    );
    /// 只收口同一 claim 的观测；旧任务结束不能覆盖新任务。
    fn finish(&self, claim: &MailPollClaim, outcome: MailPollProgressOutcome);
}
