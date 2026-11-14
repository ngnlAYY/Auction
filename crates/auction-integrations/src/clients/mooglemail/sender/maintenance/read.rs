//! Read Mail 的维护观察：水位前缀加每日一次完整校验，只统计观察到数量。
//!
//! 水位与游标都不是已读或收取证明；完整枚举超限只告警，绝不阻断轮询。

use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailWorld, parser::MoogleMailSummary,
};
use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::safe_diagnostics;

use super::super::{MoogleMailSender, proof};
use tokio::time::{Instant, timeout_at};

impl MoogleMailSender {
    /// 已读历史列表与 Take/持久 pending 恢复共用数据；不得据此创建新入站命令。
    ///
    /// 维护只统计观察到数量：当日走水位前缀，每日一次完整校验；完整枚举超限只告警，
    /// 绝不因页数上限阻断轮询。水位与游标都不是已读或收取证明。
    pub async fn scan_read_summaries(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<Vec<MoogleMailSummary>, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        let round = self.client.read_round(cookie, world, false)?;
        // 重启后的首个维护轮恢复持久检查点；作用域不匹配或损坏时回退完整校验。
        self.ensure_read_scan_checkpoint(cookie, world).await;
        let checkpoint = self.read_scan_checkpoint(cookie, world);
        let plan = auction_application::mail::plan_mail_scan(
            checkpoint.as_ref(),
            chrono::Utc::now(),
            self.client.mail_scan_daily_full_interval(),
        );
        self.observe_scan(
            world,
            auction_application::mail::MailScanEvent::Plan {
                stream: auction_application::mail::MailScanStream::ReadMail,
                mode: plan.mode,
                reason: plan.reason,
            },
        );
        let shadow_anchor = checkpoint.as_ref().and_then(|checkpoint| {
            checkpoint.watermark.as_ref().map(auction_application::mail::MailScanWatermark::anchor)
        });
        let mut prefix = None;
        if plan.mode == auction_application::mail::MailScanMode::Incremental
            && let Some(anchor) = plan.after.as_ref()
        {
            let scan = timeout_at(
                deadline,
                self.client.observe_incoming_prefix(
                    cookie,
                    &round,
                    self.verification_pages,
                    true,
                    anchor,
                ),
            )
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_maintenance",
                    "read_mail_prefix_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                proof::scan_timeout(world)
            })??;
            if scan.anchor_found {
                self.observe_scan(
                    world,
                    auction_application::mail::MailScanEvent::Prefix {
                        stream: auction_application::mail::MailScanStream::ReadMail,
                        outcome: auction_application::mail::MailScanOutcome::Complete,
                        reason: None,
                    },
                );
                self.remember_read_scan(cookie, world, scan.newest.clone(), false).await;
                if let Err(error) = proof::check_deadline(deadline, world) {
                    self.observe_budget_error(
                        world,
                        auction_application::external_metrics::MailPhase::ReadScan,
                        &error,
                    );
                    return Err(error);
                }
                return Ok(scan.summaries);
            }
            self.observe_scan(
                world,
                auction_application::mail::MailScanEvent::Prefix {
                    stream: auction_application::mail::MailScanStream::ReadMail,
                    outcome: auction_application::mail::MailScanOutcome::Error,
                    reason: scan.issue,
                },
            );
            if let Err(error) = proof::check_deadline(deadline, world) {
                self.observe_budget_error(
                    world,
                    auction_application::external_metrics::MailPhase::ReadScan,
                    &error,
                );
                return Err(error);
            }
        }
        if plan.mode == auction_application::mail::MailScanMode::Full
            && let Some(anchor) = shadow_anchor.as_ref()
        {
            // 完整校验轮同时观察前缀，供影子比较；失败只丢弃样本，不改变完整结果。
            if let Ok(scan) = timeout_at(
                deadline,
                self.client.observe_incoming_prefix(
                    cookie,
                    &round,
                    self.verification_pages,
                    true,
                    anchor,
                ),
            )
            .await
                && let Ok(scan) = scan
                && scan.anchor_found
            {
                prefix = Some(scan);
            }
        }
        let list = timeout_at(
            deadline,
            self.client.observe_incoming_list(cookie, &round, self.verification_pages, true),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_maintenance",
                "read_inbox_list_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            self.observe_budget(
                world,
                auction_application::external_metrics::MailPhase::ReadScan,
                auction_application::external_metrics::MailBudget::Time,
            );
            proof::scan_timeout(world)
        })??;
        if let Err(error) = proof::check_deadline(deadline, world) {
            self.observe_budget_error(
                world,
                auction_application::external_metrics::MailPhase::ReadScan,
                &error,
            );
            return Err(error);
        }
        let outcome = if list.complete {
            auction_application::mail::MailScanOutcome::Complete
        } else {
            auction_application::mail::MailScanOutcome::PageLimit
        };
        // 计划外的完整枚举（前缀回退）以 requested_full 记录，不冒充每日校验。
        let verification_reason = if plan.mode == auction_application::mail::MailScanMode::Full {
            plan.reason
        } else {
            auction_application::mail::MailScanReason::RequestedFull
        };
        self.observe_scan(
            world,
            auction_application::mail::MailScanEvent::Verification {
                stream: auction_application::mail::MailScanStream::ReadMail,
                outcome,
                reason: if list.complete {
                    verification_reason
                } else {
                    auction_application::mail::MailScanReason::PageLimit
                },
            },
        );
        if !list.complete {
            self.observe_budget(
                world,
                auction_application::external_metrics::MailPhase::ReadScan,
                auction_application::external_metrics::MailBudget::Pages,
            );
            tracing::warn!(
                world = world.as_str(),
                pages_scanned = list.summaries.len(),
                "MoogleMail Read Mail 完整校验超过页数上限；仅告警并保留观察前缀"
            );
            // 超限仍保留已观察的最高 MID 作为下一轮前缀起点；它只缩短观察窗口，
            // 且每日校验时间已推进，不会每个维护轮都重扫整箱。旧于锚点的行不再重数。
            self.remember_read_scan(
                cookie,
                world,
                list.summaries.first().map(|summary| {
                    auction_application::mail::MailScanWatermark {
                        mail_id: summary.external_mail_id.clone(),
                        observed_at: summary.received_at,
                    }
                }),
                true,
            )
            .await;
            return Ok(list.summaries.clone());
        }
        self.remember_read_scan(
            cookie,
            world,
            list.summaries.first().map(|summary| auction_application::mail::MailScanWatermark {
                mail_id: summary.external_mail_id.clone(),
                observed_at: summary.received_at,
            }),
            true,
        )
        .await;
        if let Some(prefix) = prefix {
            let anchor = shadow_anchor.as_ref().and_then(|anchor| {
                auction_application::mail::numeric_mail_scan_id(&anchor.mail_id)
            });
            let legacy = list
                .summaries
                .iter()
                .filter(|summary| {
                    anchor.is_none_or(|anchor| {
                        auction_application::mail::numeric_mail_scan_id(&summary.external_mail_id)
                            .is_some_and(|value| value > anchor)
                    })
                })
                .map(|summary| summary.external_mail_id.clone())
                .collect();
            let candidate = prefix
                .summaries
                .iter()
                .filter(|summary| {
                    anchor.is_none_or(|anchor| {
                        auction_application::mail::numeric_mail_scan_id(&summary.external_mail_id)
                            .is_some_and(|value| value > anchor)
                    })
                })
                .map(|summary| summary.external_mail_id.clone())
                .collect();
            let shadow = auction_application::mail::compare_mail_scan_mids(&legacy, &candidate);
            self.observe_scan(
                world,
                auction_application::mail::MailScanEvent::Shadow {
                    stream: auction_application::mail::MailScanStream::ReadMail,
                    outcome: auction_application::mail::MailScanOutcome::Complete,
                    mid_sets_equal: shadow.is_match(),
                },
            );
            if !shadow.is_match() {
                tracing::warn!(
                    world = world.as_str(),
                    legacy_only = shadow.legacy_only,
                    candidate_only = shadow.candidate_only,
                    "MoogleMail Read Mail 水位前缀与完整枚举的新增 MID 集合不一致"
                );
            }
        }
        let summaries = list.summaries.clone();
        if let Err(error) = proof::check_deadline(deadline, world) {
            self.observe_budget_error(
                world,
                auction_application::external_metrics::MailPhase::ReadScan,
                &error,
            );
            return Err(error);
        }
        Ok(summaries)
    }
}
