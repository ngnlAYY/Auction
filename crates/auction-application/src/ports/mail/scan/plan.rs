//! 扫描流、水位、锚点、检查点与扫描计划。

use chrono::{DateTime, Duration, Utc};

use super::observe::numeric_mail_scan_id;

/// 增量扫描所属的远端列表；同一水位不能跨列表或跨 world 复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailScanStream {
    Sent,
    Inbox,
    ReadMail,
}

impl MailScanStream {
    /// 指标预置的闭集。
    pub const ALL: [Self; 3] = [Self::Sent, Self::Inbox, Self::ReadMail];

    /// 固定标签；不含远端身份。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Inbox => "inbox",
            Self::ReadMail => "read_mail",
        }
    }
}

/// 可作为下一轮前缀起点的最高 MID 与其列表时间；时间只用于顺序与每日校验，不作证明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailScanWatermark {
    pub mail_id: String,
    pub observed_at: DateTime<Utc>,
}

impl MailScanWatermark {
    /// 空 ID、前导零、非十进制或非法时间都不能进入水位比较。
    pub fn validate(&self) -> bool {
        numeric_mail_scan_id(&self.mail_id).is_some()
            && self.observed_at.timestamp_nanos_opt().is_some()
    }

    /// 已知时间的锚点；时间缺失的游标锚点由 [`MailScanAnchor::from_cursor`] 表达。
    pub fn anchor(&self) -> MailScanAnchor {
        MailScanAnchor { mail_id: self.mail_id.clone(), observed_at: Some(self.observed_at) }
    }
}

/// 前缀扫描的锚点；轮询游标只有 MID，列表时间在读到锚点行后才知道。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailScanAnchor {
    pub mail_id: String,
    pub observed_at: Option<DateTime<Utc>>,
}

impl MailScanAnchor {
    /// 持久游标已经保证该邮件及其更旧窗口已处理；这里只借用 MID。
    pub fn from_cursor(mail_id: &str) -> Option<Self> {
        numeric_mail_scan_id(mail_id)?;
        Some(Self { mail_id: mail_id.to_owned(), observed_at: None })
    }

    pub(super) fn numeric(&self) -> Option<u128> {
        numeric_mail_scan_id(&self.mail_id)
    }

    /// 锚点必须是无前导零的十进制 MID；时间可缺失（持久游标只知道 MID）。
    pub fn validate(&self) -> bool {
        self.numeric().is_some()
            && self.observed_at.is_none_or(|time| time.timestamp_nanos_opt().is_some())
    }
}

/// 一个列表的扫描检查点；水位可缺失（空列表或首次读取），每日校验时间独立保存。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailScanCheckpoint {
    pub watermark: Option<MailScanWatermark>,
    /// 最近一次完整扫描尝试时间；从未完整校验时为 `None`。
    /// 成功或超限都更新，避免每日校验被无限重试。
    pub last_full_scan_at: Option<DateTime<Utc>>,
}

impl MailScanCheckpoint {
    /// 无效水位不能驱动增量读取；时间必须可表示。
    pub fn validate(&self) -> bool {
        // 增量轮会把水位推进到最近一次完整校验之后，二者没有全局先后关系。
        self.watermark.as_ref().is_none_or(MailScanWatermark::validate)
            && self.last_full_scan_at.is_none_or(|time| time.timestamp_nanos_opt().is_some())
    }
}

/// 本轮读取方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailScanMode {
    Full,
    Incremental,
}

impl MailScanMode {
    /// 指标预置的闭集。
    pub const ALL: [Self; 2] = [Self::Full, Self::Incremental];

    /// 固定标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }
}

/// 选择完整扫描或回退的闭集原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailScanReason {
    /// 首次运行或没有检查点。
    NoCheckpoint,
    /// 检查点存在但还没有可用水位（例如空列表）。
    NoWatermark,
    /// 尚无完整校验时间；本轮补做一次完整校验，超限仍只告警。
    NoFullVerification,
    /// 检查点水位或时间无效。
    InvalidCheckpoint,
    /// 到期的每日完整校验。
    DailyVerification,
    /// 调用方显式要求完整扫描。
    RequestedFull,
    /// 水位有效且未到每日校验；本轮只读前缀。
    Incremental,
    /// 水位在前缀扫描中缺失，必须回退完整扫描。
    AnchorMissing,
    /// 锚点邮件的时间与水位不一致。
    AnchorChanged,
    /// 分页出现重复 MID，位移不能被当作连续前缀。
    PageDiscontinuity,
    /// 列表时间非降序，无法证明前缀连续。
    OrderViolation,
    /// MID 不是无前导零的十进制。
    InvalidMid,
    /// 未能在页数上限内到达锚点。
    PageLimit,
    /// 读取或预算失败；本轮不能给出前缀结论。
    ReadUnavailable,
    /// 调用取消或租约结束。
    Canceled,
}

impl MailScanReason {
    /// 指标预置的闭集。
    pub const ALL: [Self; 15] = [
        Self::NoCheckpoint,
        Self::NoWatermark,
        Self::NoFullVerification,
        Self::InvalidCheckpoint,
        Self::DailyVerification,
        Self::RequestedFull,
        Self::Incremental,
        Self::AnchorMissing,
        Self::AnchorChanged,
        Self::PageDiscontinuity,
        Self::OrderViolation,
        Self::InvalidMid,
        Self::PageLimit,
        Self::ReadUnavailable,
        Self::Canceled,
    ];

    /// 固定标签；不含错误原文或远端身份。
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoCheckpoint => "no_checkpoint",
            Self::NoWatermark => "no_watermark",
            Self::NoFullVerification => "no_full_verification",
            Self::InvalidCheckpoint => "invalid_checkpoint",
            Self::DailyVerification => "daily_verification",
            Self::RequestedFull => "requested_full",
            Self::Incremental => "incremental",
            Self::AnchorMissing => "anchor_missing",
            Self::AnchorChanged => "anchor_changed",
            Self::PageDiscontinuity => "page_discontinuity",
            Self::OrderViolation => "order_violation",
            Self::InvalidMid => "invalid_mid",
            Self::PageLimit => "page_limit",
            Self::ReadUnavailable => "read_unavailable",
            Self::Canceled => "canceled",
        }
    }
}

/// 本轮扫描计划；`after` 只在增量读取时出现。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailScanPlan {
    pub mode: MailScanMode,
    pub reason: MailScanReason,
    pub after: Option<MailScanAnchor>,
}

impl MailScanPlan {
    /// 显式完整扫描。
    pub fn full(reason: MailScanReason) -> Self {
        Self { mode: MailScanMode::Full, reason, after: None }
    }

    /// 从水位继续的前缀扫描。
    pub fn incremental(after: MailScanAnchor) -> Self {
        Self {
            mode: MailScanMode::Incremental,
            reason: MailScanReason::Incremental,
            after: Some(after),
        }
    }
}

/// 决定本轮是否执行每日完整校验；水位在前缀扫描中仍要独立核对。
pub fn plan_mail_scan(
    checkpoint: Option<&MailScanCheckpoint>,
    now: DateTime<Utc>,
    daily_full_interval: Duration,
) -> MailScanPlan {
    if daily_full_interval <= Duration::zero() || now.timestamp_nanos_opt().is_none() {
        return MailScanPlan::full(MailScanReason::RequestedFull);
    }
    let Some(checkpoint) = checkpoint else {
        return MailScanPlan::full(MailScanReason::NoCheckpoint);
    };
    if !checkpoint.validate() {
        return MailScanPlan::full(MailScanReason::InvalidCheckpoint);
    }
    let Some(last_full_scan_at) = checkpoint.last_full_scan_at else {
        // 只有前缀水位而从未完整校验：本轮补做一次完整校验；超限仍只告警。
        return MailScanPlan::full(MailScanReason::NoFullVerification);
    };
    // 时钟回拨或未来检查点不能推迟校验；按到期处理，先做一次完整读取。
    if now < last_full_scan_at {
        return MailScanPlan::full(MailScanReason::DailyVerification);
    }
    if now - last_full_scan_at >= daily_full_interval {
        return MailScanPlan::full(MailScanReason::DailyVerification);
    }
    let Some(watermark) = checkpoint.watermark.as_ref() else {
        return MailScanPlan::full(MailScanReason::NoWatermark);
    };
    if !watermark.validate() {
        return MailScanPlan::full(MailScanReason::InvalidCheckpoint);
    }
    MailScanPlan::incremental(watermark.anchor())
}
