//! 影子比对、扫描结论、事件与观察者。

use std::collections::HashSet;
use std::sync::Arc;

use auction_domain::valid_text;

use super::MAX_MAIL_SCAN_ID_BYTES;
use super::plan::{MailScanMode, MailScanReason, MailScanStream};

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

pub(super) struct NoopMailScanObserver;

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
