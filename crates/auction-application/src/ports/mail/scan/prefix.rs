//! 前缀分页扫描与水位推进。

use std::collections::HashSet;

use chrono::{DateTime, Utc};

use super::observe::numeric_mail_scan_id;
use super::plan::{MailScanAnchor, MailScanReason, MailScanWatermark};

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
    pub(super) anchor: MailScanAnchor,
    pub(super) anchor_value: u128,
    pub(super) previous_at: Option<DateTime<Utc>>,
    pub(super) seen: HashSet<String>,
    pub(super) newest: Option<MailScanWatermark>,
    pub(super) found_anchor: bool,
    pub(super) rows: usize,
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
