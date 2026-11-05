//! 临时发送边界与分钟组校验；不收紧通用 opaque MID，也不提供发送成功证明。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

mod scan;

use std::collections::HashMap;

use auction_application::SentBaselineReason;
use chrono::{DateTime, Utc};
use tokio::sync::OwnedSemaphorePermit;

use crate::clients::mooglemail::parser::MoogleMailSentSummary;

/// 每次发送前 fresh 取得；Empty 必须来自空列表和末页证明。
pub(crate) enum SentBoundary {
    Empty,
    Mail { mail_id: String, numeric_mid: u128, sent_at: DateTime<Utc> },
}

impl SentBoundary {
    pub(crate) fn sent_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Empty => None,
            Self::Mail { sent_at, .. } => Some(*sent_at),
        }
    }

    pub(crate) fn is_new(&self, mail_id: &str) -> Result<bool, SentBaselineReason> {
        let value = numeric_mid(mail_id).ok_or(SentBaselineReason::InvalidMid)?;
        Ok(match self {
            Self::Empty => true,
            Self::Mail { numeric_mid, .. } => value > *numeric_mid,
        })
    }

    fn check_row(&self, summary: &MoogleMailSentSummary) -> Result<(), SentBaselineReason> {
        if let Self::Mail { mail_id, numeric_mid: boundary, sent_at } = self {
            let value =
                numeric_mid(&summary.external_mail_id).ok_or(SentBaselineReason::InvalidMid)?;
            if summary.external_mail_id == *mail_id && summary.sent_at != *sent_at {
                return Err(SentBaselineReason::AnchorChanged);
            }
            if (value > *boundary && summary.sent_at < *sent_at)
                || (value <= *boundary && summary.sent_at > *sent_at)
            {
                return Err(SentBaselineReason::OrderViolation);
            }
        }
        Ok(())
    }

    /// 锚点消失时只补齐枚举范围，仍以发送前边界筛选；绝不以发送后最大 MID 重建基线。
    pub(crate) fn check_complete(
        &self,
        summaries: &[MoogleMailSentSummary],
    ) -> Result<(), SentBaselineReason> {
        let mut order = SentOrder::default();
        for summary in summaries {
            order.observe(summary)?;
            self.check_row(summary)?;
        }
        Ok(())
    }
}

/// 前缀携带原字节许可；影子与证明共享这一观测，不复制整箱摘要。
pub(crate) struct SentBoundaryScan {
    pub(crate) summaries: Vec<MoogleMailSentSummary>,
    pub(crate) pages_scanned: usize,
    pub(crate) complete: bool,
    pub(crate) bounded: bool,
    pub(crate) issue: Option<SentBaselineReason>,
    bytes: OwnedSemaphorePermit,
}

impl SentBoundaryScan {
    pub(crate) fn before_boundary(&self) -> Result<SentBoundary, SentBaselineReason> {
        if let Some(reason) = self.issue {
            return Err(reason);
        }
        if !self.bounded {
            return Err(SentBaselineReason::PageLimit);
        }
        let Some(first) = self.summaries.first() else {
            return if self.complete {
                Ok(SentBoundary::Empty)
            } else {
                Err(SentBaselineReason::BoundaryUnavailable)
            };
        };
        let mut maximum = first;
        let mut maximum_value =
            numeric_mid(&first.external_mail_id).ok_or(SentBaselineReason::InvalidMid)?;
        for summary in self.summaries.iter().take_while(|summary| summary.sent_at == first.sent_at)
        {
            let value =
                numeric_mid(&summary.external_mail_id).ok_or(SentBaselineReason::InvalidMid)?;
            if value > maximum_value {
                maximum = summary;
                maximum_value = value;
            }
        }
        Ok(SentBoundary::Mail {
            mail_id: maximum.external_mail_id.clone(),
            numeric_mid: maximum_value,
            sent_at: maximum.sent_at,
        })
    }

    pub(crate) fn check_after(&self, boundary: &SentBoundary) -> Result<(), SentBaselineReason> {
        if let Some(reason) = self.issue {
            return Err(reason);
        }
        if !self.bounded {
            return Err(SentBaselineReason::PageLimit);
        }
        if let SentBoundary::Mail { mail_id, .. } = boundary
            && !self.summaries.iter().any(|summary| summary.external_mail_id == *mail_id)
        {
            return Err(SentBaselineReason::AnchorMissing);
        }
        Ok(())
    }
}

/// 与增量扫描共用的十进制 MID 解析；不截断、不归一化、不按字典序比较。
pub(crate) fn numeric_mid(value: &str) -> Option<u128> {
    let parsed = auction_application::numeric_mail_scan_id(value);
    if parsed.is_none() {
        safe_diagnostics::debug(
            "mooglemail_baseline",
            "parse_mid",
            safe_diagnostics::category(
                auction_application::diagnostics::SafeErrorKind::DOMAIN_REJECTED,
            ),
            DiagnosticContext::default(),
        );
    }
    parsed
}

#[derive(Default)]
struct SentOrder {
    seen: HashMap<String, DateTime<Utc>>,
    minute: Option<DateTime<Utc>>,
    minute_minimum: u128,
    newer_minimum: Option<u128>,
}

impl SentOrder {
    fn observe(&mut self, summary: &MoogleMailSentSummary) -> Result<(), SentBaselineReason> {
        let value = numeric_mid(&summary.external_mail_id).ok_or(SentBaselineReason::InvalidMid)?;
        if let Some(previous) = self.seen.insert(summary.external_mail_id.clone(), summary.sent_at)
        {
            return Err(if previous == summary.sent_at {
                SentBaselineReason::PageDiscontinuity
            } else {
                SentBaselineReason::OrderViolation
            });
        }
        if self.minute.is_some_and(|minute| summary.sent_at > minute) {
            return Err(SentBaselineReason::OrderViolation);
        }
        if self.minute.is_some_and(|minute| summary.sent_at < minute) {
            self.newer_minimum = Some(self.minute_minimum);
        }
        if self.newer_minimum.is_some_and(|minimum| value >= minimum) {
            return Err(SentBaselineReason::OrderViolation);
        }
        self.minute_minimum = if self.minute == Some(summary.sent_at) {
            self.minute_minimum.min(value)
        } else {
            value
        };
        self.minute = Some(summary.sent_at);
        Ok(())
    }
}
