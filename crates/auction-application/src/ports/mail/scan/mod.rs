// MoogleMail Sent / Inbox / Read Mail 的 MID 水位与增量前缀扫描策略。
//
// 水位表示“可作为下一轮前缀起点的最高 MID 与列表时间”；完整校验是否完成单独记录，
// 它永远不是发送、收取、领取或已读证明。任何缺失、锚点消失、分页重复或顺序
// 违反都必须回退完整扫描；完整扫描超限只记录告警，不能阻断发送或轮询。
//
// 本模块只含闭集分类与纯决策，不读取配置、不访问远端，也不保存正文或凭据。

use std::{collections::HashSet, sync::Arc};

use auction_domain::valid_text;
use chrono::{DateTime, Duration, Utc};

/// 单个 MID 允许的最大字节数；与数据库与轮询游标边界一致。
pub const MAX_MAIL_SCAN_ID_BYTES: usize = 256;

/// 每日完整校验的默认间隔；部署可在 `[worker.mooglemail]
/// mail_scan_daily_full_interval_seconds` 覆盖，缺失时保持 24 小时。
pub const MAIL_SCAN_DAILY_FULL_INTERVAL: Duration = Duration::hours(24);

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

    fn numeric(&self) -> Option<u128> {
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

/// 单行观测的推进结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailPrefixPage {
    /// 尚未到达锚点，需要继续读取下一页。
    Continue,
    /// 锚点已找到；本页剩余行仍会校验，随后可停止。
    Done,
}

/// 前缀扫描的最终结论；`Complete` 只表示“更新的 MID 已全部枚举”。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailPrefixOutcome {
    /// 到达锚点；`newest` 是本轮观察到的最新 MID，没有新行时为 `None`（保持原水位）。
    Complete { newest: Option<MailScanWatermark> },
    /// 前缀不连续或超限；调用方必须完整扫描。
    Fallback(MailScanReason),
}

/// 有界前缀扫描状态机。
///
/// 列表按时间降序返回；任何时间上升、重复 MID 或未遇锚点就跨过边界都终止为回退。
/// 锚点行本身必须存在且时间一致；否则不能证明“所有更新的邮件都在本前缀中”。
#[derive(Debug)]
pub struct MailPrefixScan {
    anchor: MailScanAnchor,
    anchor_value: u128,
    previous_at: Option<DateTime<Utc>>,
    seen: HashSet<String>,
    newest: Option<MailScanWatermark>,
    found_anchor: bool,
    rows: usize,
}

impl MailPrefixScan {
    /// 锚点无效时仍返回实例，`finish` 给出 `InvalidMid` 回退。
    pub fn new(anchor: MailScanAnchor) -> Self {
        let anchor_value = anchor.numeric().unwrap_or_default();
        Self {
            anchor,
            anchor_value,
            previous_at: None,
            seen: HashSet::new(),
            newest: None,
            found_anchor: false,
            rows: 0,
        }
    }

    /// 已观察行数；只用于预算与指标，不参与裁决。
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 是否已读到锚点行；调用方据此在本页结束后停止翻页。
    pub fn anchor_found(&self) -> bool {
        self.found_anchor
    }

    /// 按列表顺序推进一行；错误只否定前缀连续性，不直接改写业务状态。
    pub fn observe(
        &mut self,
        mail_id: &str,
        observed_at: DateTime<Utc>,
    ) -> Result<MailPrefixPage, MailScanReason> {
        if !self.anchor.validate() || observed_at.timestamp_nanos_opt().is_none() {
            return Err(MailScanReason::InvalidMid);
        }
        let value = numeric_mail_scan_id(mail_id).ok_or(MailScanReason::InvalidMid)?;
        if !self.seen.insert(mail_id.to_owned()) {
            return Err(MailScanReason::PageDiscontinuity);
        }
        if let Some(previous) = self.previous_at
            && observed_at > previous
        {
            // 列表必须按时间降序；同分钟并列允许，上升意味着翻页位移。
            return Err(MailScanReason::OrderViolation);
        }
        self.previous_at = Some(observed_at);
        self.rows += 1;

        if self.found_anchor {
            return Ok(MailPrefixPage::Done);
        }
        if value > self.anchor_value {
            let observed = MailScanWatermark { mail_id: mail_id.to_owned(), observed_at };
            if self.newest.as_ref().is_none_or(|current| {
                numeric_mail_scan_id(&current.mail_id).is_none_or(|current| value > current)
            }) {
                self.newest = Some(observed);
            }
            return Ok(MailPrefixPage::Continue);
        }
        if value == self.anchor_value {
            if self.anchor.mail_id != mail_id {
                return Err(MailScanReason::InvalidMid);
            }
            if self.anchor.observed_at.is_some_and(|expected| expected != observed_at) {
                return Err(MailScanReason::AnchorChanged);
            }
            self.found_anchor = true;
            return Ok(MailPrefixPage::Done);
        }
        // 已经读到比锚点更旧的邮件却从未看到锚点行：前缀缺口不能猜测补齐。
        Err(MailScanReason::AnchorMissing)
    }

    /// 页数耗尽或列表结束时收尾；未找到锚点一律回退。
    pub fn finish(self, page_limit_reached: bool, list_complete: bool) -> MailPrefixOutcome {
        if !self.anchor.validate() {
            return MailPrefixOutcome::Fallback(MailScanReason::InvalidMid);
        }
        if !self.found_anchor {
            return MailPrefixOutcome::Fallback(if page_limit_reached {
                MailScanReason::PageLimit
            } else if list_complete {
                MailScanReason::AnchorMissing
            } else {
                MailScanReason::ReadUnavailable
            });
        }
        MailPrefixOutcome::Complete { newest: self.newest }
    }
}

/// 水位只前进；不同轮次的较高 MID 才能替换旧值。
pub fn advance_mail_scan_watermark(
    previous: Option<&MailScanWatermark>,
    newest: &MailScanWatermark,
) -> MailScanWatermark {
    let Some(previous) = previous.filter(|previous| previous.validate()) else {
        return newest.clone();
    };
    if !newest.validate() {
        return previous.clone();
    }
    let (Some(previous_value), Some(newest_value)) =
        (numeric_mail_scan_id(&previous.mail_id), numeric_mail_scan_id(&newest.mail_id))
    else {
        return previous.clone();
    };
    if newest_value > previous_value { newest.clone() } else { previous.clone() }
}

/// 新旧两条扫描路径的 MID 集合比较；影子期间新路径不得改变裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailScanShadow {
    pub mid_sets_equal: bool,
    pub legacy_only: usize,
    pub candidate_only: usize,
}

impl MailScanShadow {
    /// 指标预置的闭集结论。
    pub const fn is_match(self) -> bool {
        self.mid_sets_equal
    }

    /// 固定结论标签。
    pub const fn label(self) -> &'static str {
        if self.mid_sets_equal { "match" } else { "mismatch" }
    }
}

/// 比较完整枚举与增量前缀的新增 MID 集合；两侧都必须来自同一读取轮次。
pub fn compare_mail_scan_mids(
    legacy: &HashSet<String>,
    candidate: &HashSet<String>,
) -> MailScanShadow {
    MailScanShadow {
        mid_sets_equal: legacy == candidate,
        legacy_only: legacy.difference(candidate).count(),
        candidate_only: candidate.difference(legacy).count(),
    }
}

/// 完整扫描的终结结果；超限只告警，不阻断发送或轮询。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailScanOutcome {
    Complete,
    PageLimit,
    Error,
    Canceled,
}

impl MailScanOutcome {
    /// 指标预置的闭集。
    pub const ALL: [Self; 4] = [Self::Complete, Self::PageLimit, Self::Error, Self::Canceled];

    /// 固定标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::PageLimit => "page_limit",
            Self::Error => "error",
            Self::Canceled => "canceled",
        }
    }
}

/// 仅含闭集分类的扫描观测；不携带 MID、正文、错误原文或凭据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailScanEvent {
    /// 本轮计划；`reason` 为完整扫描原因，增量固定为 `no_checkpoint`。
    Plan { stream: MailScanStream, mode: MailScanMode, reason: MailScanReason },
    /// 前缀扫描结论；`Complete` 只表示窗口完整，不是业务证明。
    Prefix { stream: MailScanStream, outcome: MailScanOutcome, reason: Option<MailScanReason> },
    /// 完整扫描或每日校验结论。
    Verification { stream: MailScanStream, outcome: MailScanOutcome, reason: MailScanReason },
    /// 影子比较；只记录集合是否一致。
    Shadow { stream: MailScanStream, outcome: MailScanOutcome, mid_sets_equal: bool },
}

/// 扫描观测端口；实现只累加计数，不得改变扫描或业务结果。
pub trait MailScanObserver: Send + Sync {
    /// world 只接受 main/isekai；适配器不能把任意字符串写入标签。
    fn observe(&self, world: &str, event: MailScanEvent);
}

struct NoopMailScanObserver;

impl MailScanObserver for NoopMailScanObserver {
    fn observe(&self, _world: &str, _event: MailScanEvent) {}
}

/// 未装配 exporter 时保持同一扫描与回退路径。
pub fn noop_mail_scan_observer() -> Arc<dyn MailScanObserver> {
    Arc::new(NoopMailScanObserver)
}

/// 只接受无前导零的十进制 MID；不截断、不归一化、不按字典序比较。
pub fn numeric_mail_scan_id(value: &str) -> Option<u128> {
    if !valid_text(value, MAX_MAIL_SCAN_ID_BYTES)
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse::<u128>().ok()
}

#[cfg(test)]
mod tests;
