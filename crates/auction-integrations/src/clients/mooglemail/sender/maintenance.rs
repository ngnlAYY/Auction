//! 定期维护读取：compose 草稿快照、受控 discard、Sent 摘要与实收探查。
//!
//! 普通列表和详情不持有 compose 排他会话；只有草稿读取与 discard 需要窄互斥。
//! 观测本身不提升为投递成功；discard 只丢弃调用方已精确匹配到本地已知投递的草稿。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use super::*;
use chrono::{DateTime, Utc};
use tokio::time::{Instant, timeout_at};

/// compose 页当前状态的只读投影。
pub struct MoogleMailComposeSnapshot {
    pub recipient: String,
    pub subject: String,
    pub body: String,
    pub credits: i64,
    pub cod: Option<i64>,
    pub attachment_count: usize,
    /// 字典序排列的精确装备集合，不以附件计数代替资产身份。
    pub equipment_ids: Vec<String>,
    pub clean: bool,
}

impl MoogleMailComposeSnapshot {
    pub const fn is_clean(&self) -> bool {
        self.clean
    }
}

/// 受控 discard 的结果；已干净时不做任何远端写入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoogleMailDiscardOutcome {
    AlreadyClean,
    Discarded,
}

/// 入站邮件当前资产快照；take 恢复据此判断动作是否已生效。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailInboxAssets {
    pub sender: String,
    pub equipment_ids: Vec<String>,
    pub credits: i64,
    pub cod: i64,
}

impl fmt::Debug for MoogleMailInboxAssets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailInboxAssets")
            .field("sender", &"[REDACTED]")
            .field("equipment_ids", &self.equipment_ids)
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .finish()
    }
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
    /// 只读读取 compose 页，返回草稿字段；解析失败按协议漂移处理。
    pub async fn inspect_compose(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailComposeSnapshot, MoogleMailError> {
        let compose = self.client.compose(world)?;
        let document = self.client.get(&compose, cookie).await?;
        let state = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await?;
        let clean = state.is_clean();
        let mut fields = state.message_fields();
        let mut equipment_ids = state.equipment_ids().to_vec();
        equipment_ids.sort_unstable();
        Ok(MoogleMailComposeSnapshot {
            recipient: std::mem::take(&mut fields.recipient),
            subject: std::mem::take(&mut fields.subject),
            body: std::mem::take(&mut fields.body),
            credits: state.credits(),
            cod: state.cod(),
            attachment_count: state.attachment_count(),
            equipment_ids,
            clean,
        })
    }

    /// 丢弃 compose 草稿并在 POST 后重新读取证明页面已干净。
    /// 只允许对已登记一次性尝试且精确匹配已知投递的草稿调用；失败只读核对，不自动重发 discard。
    pub async fn discard_compose(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        expected: &auction_application::mail::MoogleMailboxComposeState,
    ) -> Result<MoogleMailDiscardOutcome, MoogleMailError> {
        let compose = self.client.compose(world)?;
        let document = self.client.get(&compose, cookie).await?;
        let state = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await?;
        if state.is_clean() {
            return Ok(MoogleMailDiscardOutcome::AlreadyClean);
        }
        let fields = state.message_fields();
        let mut equipment_ids = state.equipment_ids().iter().collect::<Vec<_>>();
        equipment_ids.sort_unstable();
        // 原快照与本次 POST 之间重新取得了 session；持锁重读后逐字段一致才有权丢弃。
        if expected.clean
            || fields.recipient != expected.recipient
            || fields.subject != expected.subject
            || fields.body != expected.body
            || state.credits() != expected.credits
            || state.cod() != expected.cod
            || state.attachment_count() != expected.attachment_count
            || !equipment_ids.into_iter().eq(expected.equipment_ids.iter())
        {
            return Err(protocol_drift(world));
        }
        let form = MoogleMailForm::discard(state.token(), &fields).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_maintenance",
                "build_discard_form",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            protocol_drift(world)
        })?;
        self.client.post(&compose, cookie, form).await?;
        let document = self.client.get(&compose, cookie).await.map_err(discard_unknown)?;
        let after = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await
            .map_err(discard_unknown)?;
        if after.is_clean() {
            Ok(MoogleMailDiscardOutcome::Discarded)
        } else {
            Err(write_unknown(world, MoogleMailOperation::Discard))
        }
    }

    /// 有界扫描 Sent 列表，只返回摘要；逐封详情由调用方按需探查。
    pub async fn scan_sent_summaries(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<Vec<MoogleMailSentListEntry>, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        // 维护扫描允许复用共享读取轮次，不强制新的请求屏障。
        let round = self.client.read_round(cookie, world, false)?;
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
            return Err(self.scan_page_limit(world, "maintenance", list.summaries.len()));
        }
        let summaries = list
            .summaries
            .iter()
            .map(|summary| MoogleMailSentListEntry {
                external_mail_id: summary.external_mail_id.clone(),
                recipient: summary.recipient.clone(),
                subject: summary.subject.clone(),
                sent_at: summary.sent_at,
            })
            .collect();
        if let Err(error) = proof::check_deadline(deadline, world) {
            self.observe_budget_error(
                world,
                auction_application::external_metrics::MailPhase::SentScan,
                &error,
            );
            return Err(error);
        }
        // 维护仍需要完整 Sent 才能判断候选邮件缺席；只把成功枚举登记为水位与每日校验。
        self.observe_scan(
            world,
            auction_application::mail::MailScanEvent::Verification {
                stream: auction_application::mail::MailScanStream::Sent,
                outcome: auction_application::mail::MailScanOutcome::Complete,
                reason: auction_application::mail::MailScanReason::RequestedFull,
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
        Ok(summaries)
    }

    /// 已读历史列表与 Take/持久 pending 恢复共用数据；不得据此创建新入站命令。
    ///
    /// 维护只统计观察到数量：当日走水位前缀，每日一次完整校验；完整枚举超限只告警，
    /// 绝不因页数上限阻断轮询。水位与游标都不是已读或收取证明。
    pub async fn scan_read_summaries(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<Vec<super::super::parser::MoogleMailSummary>, MoogleMailError> {
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

    /// 只读确认入站邮件是否仍存在并返回当前附件 ID。
    ///
    /// 先在完整 Inbox/Read Mail 观测中定位既有 MID，再读取详情；历史其他邮件不进入业务。
    /// 两表都缺失仍不能证明 Take，必须由远端明确的 “Mail does not exist” 页面证明；
    /// 登录页或解析漂移一律返回错误，不能收口 take 动作。
    pub async fn inspect_inbox_attachments(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        external_mail_id: &str,
    ) -> Result<Option<MoogleMailInboxAssets>, MoogleMailError> {
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| proof::scan_timeout(world))?;
        let round = self.client.read_round(cookie, world, false)?;
        let result = timeout_at(deadline, async {
            let inbox = self
                .client
                .observe_incoming_list(cookie, &round, self.verification_pages, false)
                .await?;
            proof::check_deadline(deadline, world)?;
            if !inbox.complete {
                self.observe_budget(
                    world,
                    auction_application::external_metrics::MailPhase::SharedRead,
                    auction_application::external_metrics::MailBudget::Pages,
                );
                return Err(MoogleMailError::protocol_drift(MoogleMailOperation::Inbox, world));
            }
            let summary =
                inbox.summaries.iter().find(|summary| summary.external_mail_id == external_mail_id);
            if let Some(summary) = summary {
                return self
                    .inspect_inbox_attachments_in_round(
                        cookie,
                        world,
                        external_mail_id,
                        &round,
                        Some(summary),
                    )
                    .await;
            }
            let read = self
                .client
                .observe_incoming_list(cookie, &round, self.verification_pages, true)
                .await?;
            proof::check_deadline(deadline, world)?;
            if !read.complete {
                self.observe_budget(
                    world,
                    auction_application::external_metrics::MailPhase::SharedRead,
                    auction_application::external_metrics::MailBudget::Pages,
                );
                return Err(MoogleMailError::protocol_drift(MoogleMailOperation::ReadMail, world));
            }
            let summary =
                read.summaries.iter().find(|summary| summary.external_mail_id == external_mail_id);
            self.inspect_inbox_attachments_in_round(
                cookie,
                world,
                external_mail_id,
                &round,
                summary,
            )
            .await
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_maintenance",
                "read_inbox_assets_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            self.observe_budget(
                world,
                auction_application::external_metrics::MailPhase::SharedRead,
                auction_application::external_metrics::MailBudget::Time,
            );
            proof::scan_timeout(world)
        })?;
        if let Err(error) = proof::check_deadline(deadline, world) {
            self.observe_budget_error(
                world,
                auction_application::external_metrics::MailPhase::SharedRead,
                &error,
            );
            return Err(error);
        }
        result
    }

    async fn inspect_inbox_attachments_in_round(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        external_mail_id: &str,
        round: &super::super::client::MoogleMailReadRound,
        summary: Option<&super::super::parser::MoogleMailSummary>,
    ) -> Result<Option<MoogleMailInboxAssets>, MoogleMailError> {
        let target = self.client.detail(world, external_mail_id)?;
        let document = self.client.get_in_round(&target, cookie, round).await?;
        let state = self
            .client
            .diagnose_parse(
                &target,
                &document,
                self.parser.parse_action_state_or_missing(&document, world),
            )
            .await?;
        let Some(state) = state else {
            self.client.check_read_round(cookie, round).await?;
            return Ok(None);
        };
        if let Some(summary) = summary {
            // 历史摘要仅定位原 MID；仍执行完整详情与装备 key 校验，不从截断显示名重建身份。
            let mut message = self
                .client
                .diagnose_parse(&target, &document, self.parser.parse_detail(&document, summary))
                .await?;
            message.clear_secrets();
        }
        let sender = state.sender().to_owned();
        let mut equipment_ids = state.equipment_ids;
        equipment_ids.sort_unstable();
        self.client.check_read_round(cookie, round).await?;
        Ok(Some(MoogleMailInboxAssets {
            sender,
            equipment_ids,
            credits: state.credits,
            cod: state.cod,
        }))
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
            let parser_summary = super::super::parser::MoogleMailSentSummary {
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

fn protocol_drift(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::protocol_drift(MoogleMailOperation::Compose, world)
}

/// POST 后的读取/解析失败仍属未知外写；保留已有加密诊断引用，不降级成可重试读错误。
fn discard_unknown(error: MoogleMailError) -> MoogleMailError {
    let category = if error.category == MoogleMailErrorCategory::SessionExpired {
        MoogleMailErrorCategory::SessionExpired
    } else {
        MoogleMailErrorCategory::WriteUnknown
    };
    let mut unknown =
        MoogleMailError::after_write(MoogleMailOperation::Discard, error.world, category);
    unknown.evidence_id = error.evidence_id;
    unknown.evidence_status = error.evidence_status;
    unknown.evidence_failure = error.evidence_failure;
    unknown
}
