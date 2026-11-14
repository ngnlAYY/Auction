//! 增量扫描计划、进程内检查点与收件窗口选择。
//!
//! 检查点只决定“是否需要完整校验”，提交游标仍由 claim 持有；前缀扫描只缩小
//! 观察范围，窗口内容仍需逐封详情解析与既有证明流程。

use std::collections::VecDeque;

use auction_application::mail::{
    MailScanAnchor, MailScanCheckpoint, MailScanEvent, MailScanOutcome, MailScanReason,
    MailScanStream, MailScanWatermark, advance_mail_scan_watermark, plan_mail_scan,
};
use chrono::{DateTime, Utc};

use super::super::super::client::MoogleMailReadRound;
use super::MoogleMailGateway;
use super::window::{RecoveryRows, incoming_window_rows, new_mail_ids};
use crate::clients::mooglemail::error::MoogleMailError;
use crate::clients::mooglemail::gateway::protocol_error;
use crate::clients::mooglemail::parser::MoogleMailSummary;
use crate::clients::mooglemail::protocol::{MoogleMailCookie, MoogleMailWorld};

impl MoogleMailGateway {
    fn scan_plan(
        &self,
        world: MoogleMailWorld,
        stream: MailScanStream,
    ) -> auction_application::mail::MailScanPlan {
        let checkpoint = self
            .scan_checkpoints
            .lock()
            .ok()
            .and_then(|checkpoints| checkpoints.get(&(world, stream)).cloned());
        plan_mail_scan(checkpoint.as_ref(), Utc::now(), self.client.mail_scan_daily_full_interval())
    }

    /// 只推进进程内检查点；水位不是提交游标，轮询仍以 claim.cursor_value 为锚点。
    fn record_scan_checkpoint(
        &self,
        world: MoogleMailWorld,
        stream: MailScanStream,
        newest: Option<MailScanWatermark>,
        observed_at: DateTime<Utc>,
        full: bool,
    ) {
        let Ok(mut checkpoints) = self.scan_checkpoints.lock() else {
            tracing::warn!("MoogleMail 扫描检查点锁不可用；本轮结果照常返回");
            return;
        };
        let key = (world, stream);
        let previous = checkpoints.get(&key).cloned();
        let watermark = newest
            .map(|newest| {
                advance_mail_scan_watermark(
                    previous.as_ref().and_then(|c| c.watermark.as_ref()),
                    &newest,
                )
            })
            .or_else(|| previous.as_ref().and_then(|c| c.watermark.clone()));
        let last_full_scan_at = if full {
            Some(observed_at)
        } else {
            previous.and_then(|checkpoint| checkpoint.last_full_scan_at)
        };
        checkpoints.insert(key, MailScanCheckpoint { watermark, last_full_scan_at });
    }

    fn observe_scan(&self, world: MoogleMailWorld, event: MailScanEvent) {
        self.scan_observer.observe(world.as_str(), event);
    }

    /// 目标已知的恢复读取：读到最低待读 MID 即停；锚点缺失时回退完整枚举并保持失败关闭。
    pub(super) async fn recovery_rows(
        &self,
        world: MoogleMailWorld,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        read_mail: bool,
        anchor: Option<&MailScanAnchor>,
    ) -> Result<RecoveryRows, MoogleMailError> {
        let stream = if read_mail { MailScanStream::ReadMail } else { MailScanStream::Inbox };
        if let Some(anchor) = anchor {
            let scan = self
                .client
                .observe_incoming_prefix(
                    cookie,
                    round,
                    self.config.maximum_pages,
                    read_mail,
                    anchor,
                )
                .await?;
            if scan.anchor_found {
                self.observe_scan(
                    world,
                    MailScanEvent::Prefix {
                        stream,
                        outcome: MailScanOutcome::Complete,
                        reason: None,
                    },
                );
                tracing::debug!(
                    world = world.as_str(),
                    stream = stream.label(),
                    pages_scanned = scan.pages_scanned,
                    "MoogleMail 恢复读取在最低待读 MID 处停止"
                );
                return Ok(RecoveryRows::Prefix(scan));
            }
            self.observe_scan(
                world,
                MailScanEvent::Prefix {
                    stream,
                    outcome: MailScanOutcome::Error,
                    reason: scan.issue,
                },
            );
        }
        let list = self
            .client
            .observe_incoming_list(cookie, round, self.config.maximum_pages, read_mail)
            .await?;
        if !list.complete {
            return Err(protocol_error(world));
        }
        self.observe_scan(
            world,
            MailScanEvent::Verification {
                stream,
                outcome: MailScanOutcome::Complete,
                reason: MailScanReason::RequestedFull,
            },
        );
        Ok(RecoveryRows::Full(list))
    }

    /// 优先从前缀构建窗口；每日校验、缺锚点或超限才完整枚举。
    ///
    /// 窗口内容仍需业务幂等；水位缺失或分页位移时立即回退完整扫描。
    pub(super) async fn select_incoming_window(
        &self,
        world: MoogleMailWorld,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<VecDeque<MoogleMailSummary>, MoogleMailError> {
        let plan = self.scan_plan(world, MailScanStream::Inbox);
        self.observe_scan(
            world,
            MailScanEvent::Plan {
                stream: MailScanStream::Inbox,
                mode: plan.mode,
                reason: plan.reason,
            },
        );
        let anchor = cursor.and_then(MailScanAnchor::from_cursor);
        let mut prefix = None;
        if let Some(anchor) = anchor.as_ref() {
            let scan = self
                .client
                .observe_incoming_prefix(cookie, round, self.config.maximum_pages, false, anchor)
                .await?;
            if scan.anchor_found {
                self.observe_scan(
                    world,
                    MailScanEvent::Prefix {
                        stream: MailScanStream::Inbox,
                        outcome: MailScanOutcome::Complete,
                        reason: None,
                    },
                );
                tracing::debug!(
                    world = world.as_str(),
                    pages_scanned = scan.pages_scanned,
                    "MoogleMail 收件前缀扫描在锚点处停止"
                );
                prefix = Some(scan);
            } else {
                let reason = scan.issue.unwrap_or(MailScanReason::ReadUnavailable);
                self.observe_scan(
                    world,
                    MailScanEvent::Prefix {
                        stream: MailScanStream::Inbox,
                        outcome: MailScanOutcome::Error,
                        reason: Some(reason),
                    },
                );
            }
        }
        // `NoWatermark`（例如空列表或上次超限）仍可用持久游标做前缀；
        // 只有首次、检查点损坏、从未校验或每日到期才需要完整枚举。
        let verification_due = matches!(
            plan.reason,
            MailScanReason::NoCheckpoint
                | MailScanReason::InvalidCheckpoint
                | MailScanReason::NoFullVerification
                | MailScanReason::DailyVerification
                | MailScanReason::RequestedFull
        );
        if !verification_due && let Some(prefix) = prefix {
            self.record_scan_checkpoint(
                world,
                MailScanStream::Inbox,
                prefix.newest.clone(),
                round.observed_at(),
                false,
            );
            return Ok(incoming_window_rows(&prefix.summaries, cursor, limit));
        }
        let full = match self
            .client
            .observe_incoming_list(cookie, round, self.config.maximum_pages, false)
            .await
        {
            Ok(full) => full,
            Err(error) => {
                if let Some(prefix) = prefix {
                    // 每日校验读取失败只告警；前缀已经证明比游标更新的窗口完整。
                    self.record_scan_checkpoint(
                        world,
                        MailScanStream::Inbox,
                        None,
                        round.observed_at(),
                        true,
                    );
                    self.observe_scan(
                        world,
                        MailScanEvent::Verification {
                            stream: MailScanStream::Inbox,
                            outcome: MailScanOutcome::Error,
                            reason: plan.reason,
                        },
                    );
                    tracing::warn!(
                        world = world.as_str(),
                        "MoogleMail 完整校验读取失败；沿用已确认的前缀窗口"
                    );
                    return Ok(incoming_window_rows(&prefix.summaries, cursor, limit));
                }
                return Err(error);
            }
        };
        let outcome =
            if full.complete { MailScanOutcome::Complete } else { MailScanOutcome::PageLimit };
        // 计划外的完整枚举（前缀缺失回退）以 requested_full 记录，不冒充每日校验。
        let verification_reason =
            if verification_due { plan.reason } else { MailScanReason::RequestedFull };
        self.observe_scan(
            world,
            MailScanEvent::Verification {
                stream: MailScanStream::Inbox,
                outcome,
                reason: if full.complete { verification_reason } else { MailScanReason::PageLimit },
            },
        );
        let watermark = if full.complete {
            full.summaries.first().map(|summary| MailScanWatermark {
                mail_id: summary.external_mail_id.clone(),
                observed_at: summary.received_at,
            })
        } else {
            // 未到末页的枚举不能推进“已完整扫描”的水位；只更新校验时间。
            None
        };
        // 超限也算一次校验尝试；避免每个轮询轮都重扫同一超长列表。
        self.record_scan_checkpoint(
            world,
            MailScanStream::Inbox,
            watermark,
            round.observed_at(),
            true,
        );
        if !full.complete {
            // 超过页数上限只告警；若前缀已确认窗口完整则继续轮询，否则维持原失败关闭。
            if let Some(prefix) = &prefix {
                return Ok(incoming_window_rows(&prefix.summaries, cursor, limit));
            }
            return Err(protocol_error(world));
        }
        if let Some(prefix) = &prefix {
            self.compare_incoming_shadow(world, &full.summaries, &prefix.summaries, cursor);
        }
        Ok(incoming_window_rows(&full.summaries, cursor, limit))
    }

    fn compare_incoming_shadow(
        &self,
        world: MoogleMailWorld,
        legacy: &[MoogleMailSummary],
        candidate: &[MoogleMailSummary],
        cursor: Option<&str>,
    ) {
        let legacy = new_mail_ids(legacy, cursor);
        let candidate = new_mail_ids(candidate, cursor);
        let shadow = auction_application::mail::compare_mail_scan_mids(&legacy, &candidate);
        self.observe_scan(
            world,
            MailScanEvent::Shadow {
                stream: MailScanStream::Inbox,
                outcome: MailScanOutcome::Complete,
                mid_sets_equal: shadow.is_match(),
            },
        );
        if !shadow.is_match() {
            tracing::warn!(
                world = world.as_str(),
                legacy_only = shadow.legacy_only,
                candidate_only = shadow.candidate_only,
                "MoogleMail 增量窗口与完整枚举的新增 MID 集合不一致"
            );
        }
    }
}
