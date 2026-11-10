//! Sent 列表摘要的有界扫描与单封实收状态读取；观测本身不证明投递成功。

use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailWorld, parser::MoogleMailSentSummary,
};
use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use super::super::{MoogleMailSender, proof};
use chrono::{DateTime, Utc};
use std::fmt;
use tokio::time::{Instant, timeout_at};

/// 一次 Sent 维护扫描的摘要与覆盖范围；前缀不能据此证明更旧条目缺席。
#[derive(Clone, Debug)]
pub struct ScannedSentSummaries {
    pub summaries: Vec<MoogleMailSentListEntry>,
    /// `None` 表示完整列表；`Some(at)` 表示所有 `sent_at >= at` 的条目已覆盖。
    pub covered_from: Option<DateTime<Utc>>,
    pub complete: bool,
    pub page_observation: Option<auction_application::mail::MailScanPageObservation>,
}

/// Sent 列表摘要；`collected` 只有读取详情后才可知。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSentListEntry {
    pub external_mail_id: String,
    pub recipient: String,
    pub subject: String,
    pub sent_at: DateTime<Utc>,
}

impl fmt::Debug for MoogleMailSentListEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSentListEntry")
            .field("external_mail_id", &self.external_mail_id)
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("sent_at", &self.sent_at)
            .finish()
    }
}

/// 单封 Sent 邮件的当前待领取状态。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSentObservation {
    pub external_mail_id: String,
    /// 详情页完整收件人；列表投影不能作为最终身份。
    pub recipient: String,
    pub subject: String,
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub remaining_equipment_ids: Vec<String>,
    pub collected: bool,
    /// 详情正文的规范摘要；通知类恢复证明据此逐字比对，正文本身不返回。
    pub body_digest: [u8; 32],
}

impl fmt::Debug for MoogleMailSentObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSentObservation")
            .field("external_mail_id", &self.external_mail_id)
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("remaining_credits", &self.remaining_credits)
            .field("cod_pending", &self.cod_pending)
            .field("remaining_equipment_ids", &self.remaining_equipment_ids)
            .field("collected", &self.collected)
            .field("body_digest", &self.body_digest)
            .finish()
    }
}

impl MoogleMailSender {
    /// 有界扫描 Sent 列表，只返回摘要；逐封详情由调用方按需探查。
    ///
    /// 常规轮按持久水位读前缀，每日完整校验；前缀缺失、锚点消失或超限回退完整扫描。
    /// 前缀与水位都不是发送证明，旧候选缺席也不会被当作未发送。
    pub async fn scan_sent_summaries(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<ScannedSentSummaries, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        // 维护扫描允许复用共享读取轮次，不强制新的请求屏障。
        let round = self.client.read_round(cookie, world, false)?;
        self.ensure_sent_scan_checkpoint(cookie, world).await;
        let checkpoint = self.sent_scan_checkpoint(cookie, world);
        let plan = auction_application::mail::plan_mail_scan(
            checkpoint.as_ref(),
            Utc::now(),
            self.client.mail_scan_daily_full_interval(),
        );
        self.observe_scan(
            world,
            auction_application::mail::MailScanEvent::Plan {
                stream: auction_application::mail::MailScanStream::Sent,
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
            match timeout_at(
                deadline,
                self.client.observe_sent_prefix_strict(
                    cookie,
                    &round,
                    self.verification_pages,
                    anchor,
                ),
            )
            .await
            {
                Ok(Ok(scan))
                    if scan.bounded
                        && scan.anchor_sent_at.is_some()
                        && scan.anchor_sent_at == anchor.observed_at =>
                {
                    self.observe_scan(
                        world,
                        auction_application::mail::MailScanEvent::Prefix {
                            stream: auction_application::mail::MailScanStream::Sent,
                            outcome: auction_application::mail::MailScanOutcome::Complete,
                            reason: None,
                        },
                    );
                    self.remember_sent_scan(
                        cookie,
                        world,
                        scan.summaries.first().map(|summary| {
                            auction_application::mail::MailScanWatermark {
                                mail_id: summary.external_mail_id.clone(),
                                observed_at: summary.sent_at,
                            }
                        }),
                        false,
                    )
                    .await;
                    if let Err(error) = proof::check_deadline(deadline, world) {
                        self.observe_budget_error(
                            world,
                            auction_application::external_metrics::MailPhase::SentScan,
                            &error,
                        );
                        return Err(error);
                    }
                    return Ok(ScannedSentSummaries {
                        summaries: sent_entries(&scan.summaries),
                        covered_from: anchor.observed_at,
                        complete: true,
                        page_observation: None,
                    });
                }
                Ok(Ok(_)) => {
                    self.observe_scan(
                        world,
                        auction_application::mail::MailScanEvent::Prefix {
                            stream: auction_application::mail::MailScanStream::Sent,
                            outcome: auction_application::mail::MailScanOutcome::Error,
                            reason: Some(auction_application::mail::MailScanReason::PageLimit),
                        },
                    );
                }
                Ok(Err(error)) => {
                    self.observe_budget_error(
                        world,
                        auction_application::external_metrics::MailPhase::SentScan,
                        &error,
                    );
                    self.observe_scan(
                        world,
                        auction_application::mail::MailScanEvent::Prefix {
                            stream: auction_application::mail::MailScanStream::Sent,
                            outcome: auction_application::mail::MailScanOutcome::Error,
                            reason: Some(
                                auction_application::mail::MailScanReason::ReadUnavailable,
                            ),
                        },
                    );
                }
                Err(tokio::time::error::Elapsed { .. }) => {
                    self.observe_scan(
                        world,
                        auction_application::mail::MailScanEvent::Prefix {
                            stream: auction_application::mail::MailScanStream::Sent,
                            outcome: auction_application::mail::MailScanOutcome::Error,
                            reason: Some(
                                auction_application::mail::MailScanReason::ReadUnavailable,
                            ),
                        },
                    );
                }
            }
            if let Err(error) = proof::check_deadline(deadline, world) {
                self.observe_budget_error(
                    world,
                    auction_application::external_metrics::MailPhase::SentScan,
                    &error,
                );
                return Err(error);
            }
        }
        if plan.mode == auction_application::mail::MailScanMode::Full
            && let Some(anchor) = shadow_anchor.as_ref()
        {
            // 完整校验轮同时观察前缀，供影子比较；失败只丢弃样本，不改变完整结果。
            if let Ok(Ok(scan)) = timeout_at(
                deadline,
                self.client.observe_sent_prefix_strict(
                    cookie,
                    &round,
                    self.verification_pages,
                    anchor,
                ),
            )
            .await
                && scan.bounded
            {
                prefix = Some(scan);
            }
        }
        let list = timeout_at(
            deadline,
            self.client.observe_sent_list(cookie, &round, self.verification_pages),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_maintenance",
                "read_sent_list_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            self.observe_budget(
                world,
                auction_application::external_metrics::MailPhase::SentScan,
                auction_application::external_metrics::MailBudget::Time,
            );
            proof::scan_timeout(world)
        })??;
        if let Err(error) = proof::check_deadline(deadline, world) {
            self.observe_budget_error(
                world,
                auction_application::external_metrics::MailPhase::SentScan,
                &error,
            );
            return Err(error);
        }
        if !list.complete {
            self.observe_budget(
                world,
                auction_application::external_metrics::MailPhase::SentScan,
                auction_application::external_metrics::MailBudget::Pages,
            );
            self.observe_scan(
                world,
                auction_application::mail::MailScanEvent::Verification {
                    stream: auction_application::mail::MailScanStream::Sent,
                    outcome: auction_application::mail::MailScanOutcome::PageLimit,
                    reason: auction_application::mail::MailScanReason::PageLimit,
                },
            );
            // 超限也推进每日校验时间，避免每个维护轮重复整箱扫描；水位只缩短下一轮前缀。
            self.remember_sent_scan(
                cookie,
                world,
                list.summaries.first().map(|summary| {
                    auction_application::mail::MailScanWatermark {
                        mail_id: summary.external_mail_id.clone(),
                        observed_at: summary.sent_at,
                    }
                }),
                true,
            )
            .await;
            return Ok(ScannedSentSummaries {
                summaries: sent_entries(&list.summaries),
                covered_from: None,
                complete: false,
                page_observation: self.sent_page_observation(
                    cookie,
                    world,
                    list.pages_scanned,
                    self.verification_pages,
                ),
            });
        }
        let summaries = sent_entries(&list.summaries);
        if let Err(error) = proof::check_deadline(deadline, world) {
            self.observe_budget_error(
                world,
                auction_application::external_metrics::MailPhase::SentScan,
                &error,
            );
            return Err(error);
        }
        let verification_reason = if plan.mode == auction_application::mail::MailScanMode::Full {
            plan.reason
        } else {
            auction_application::mail::MailScanReason::RequestedFull
        };
        self.observe_scan(
            world,
            auction_application::mail::MailScanEvent::Verification {
                stream: auction_application::mail::MailScanStream::Sent,
                outcome: auction_application::mail::MailScanOutcome::Complete,
                reason: verification_reason,
            },
        );
        self.remember_sent_scan(
            cookie,
            world,
            list.summaries.first().map(|summary| auction_application::mail::MailScanWatermark {
                mail_id: summary.external_mail_id.clone(),
                observed_at: summary.sent_at,
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
                    stream: auction_application::mail::MailScanStream::Sent,
                    outcome: auction_application::mail::MailScanOutcome::Complete,
                    mid_sets_equal: shadow.is_match(),
                },
            );
            if !shadow.is_match() {
                tracing::warn!(
                    world = world.as_str(),
                    legacy_only = shadow.legacy_only,
                    candidate_only = shadow.candidate_only,
                    "MoogleMail Sent 水位前缀与完整枚举的新增 MID 集合不一致"
                );
            }
        }
        Ok(ScannedSentSummaries {
            summaries,
            covered_from: None,
            complete: true,
            page_observation: self.sent_page_observation(
                cookie,
                world,
                list.pages_scanned,
                self.verification_pages,
            ),
        })
    }

    fn sent_page_observation(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        pages_scanned: usize,
        page_limit: usize,
    ) -> Option<auction_application::mail::MailScanPageObservation> {
        let persisted = cookie.persisted_credential()?;
        let pages_scanned = u32::try_from(pages_scanned).ok()?;
        let page_limit = u32::try_from(page_limit).ok()?;
        ((1..=page_limit).contains(&pages_scanned) && page_limit <= 1_000).then(|| {
            auction_application::mail::MailScanPageObservation {
                credential_id: persisted.id,
                credential_version: persisted.version,
                world_id: world.as_str().to_owned(),
                endpoint_scope: self.client.endpoint_scope().to_owned(),
                pages_scanned,
                page_limit,
                observed_at: Utc::now(),
            }
        })
    }

    /// 读取单封 Sent 的实收状态；读取失败由调用方保留上一轮证据。
    pub async fn inspect_sent(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        summary: &MoogleMailSentListEntry,
        _observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailSentObservation, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        let result = timeout_at(deadline, async {
            let round = self.client.read_round(cookie, world, false)?;
            let detail = self.client.sent_detail(world, &summary.external_mail_id)?;
            let document = self.client.get_in_round(&detail, cookie, &round).await?;
            let parser_summary = MoogleMailSentSummary {
                external_mail_id: summary.external_mail_id.clone(),
                world,
                recipient: summary.recipient.clone(),
                subject: summary.subject.clone(),
                sent_at: summary.sent_at,
            };
            let proof = self
                .client
                .diagnose_parse(
                    &detail,
                    &document,
                    self.parser.parse_sent_content_proof(&document, &parser_summary),
                )
                .await?;
            let collected = proof.remaining_credits == 0
                && proof.cod_pending == 0
                && proof.remaining_equipment_ids.is_empty();
            self.client.check_read_round(cookie, &round).await?;
            Ok(MoogleMailSentObservation {
                external_mail_id: proof.external_mail_id,
                recipient: proof.recipient,
                subject: proof.subject,
                remaining_credits: proof.remaining_credits,
                cod_pending: proof.cod_pending,
                remaining_equipment_ids: proof.remaining_equipment_ids,
                collected,
                body_digest: proof.body_digest,
            })
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_maintenance",
                "read_sent_state_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            proof::scan_timeout(world)
        })?;
        proof::check_deadline(deadline, world)?;
        result
    }
}

fn sent_entries(
    summaries: &[crate::clients::mooglemail::parser::MoogleMailSentSummary],
) -> Vec<MoogleMailSentListEntry> {
    summaries
        .iter()
        .map(|summary| MoogleMailSentListEntry {
            external_mail_id: summary.external_mail_id.clone(),
            recipient: summary.recipient.clone(),
            subject: summary.subject.clone(),
            sent_at: summary.sent_at,
        })
        .collect()
}
