//! sending fence 前的完整排除集与 fresh 临时边界；缺失时在同一预算内失败关闭。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use std::{collections::HashSet, sync::Arc};

use auction_application::mail::{
    MailScanEvent, MailScanMode, MailScanOutcome, MailScanReason, MailScanStream,
};
use auction_application::sent_baseline::{
    SentBaselineEvent, SentBaselineMode, SentBaselinePhase, SentBaselineReason,
};
use chrono::Utc;
use tokio::time::Instant;

use super::{MoogleMailSentBaseline, SentBaselineKind, error_reason, telemetry::ShadowSample};
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailSender, MoogleMailWorld,
    client::{MoogleMailReadRound, SentBoundary},
    sender::proof::{check_deadline, scan_timeout, within_deadline},
};

/// 锚点消失、分页位移、顺序异常或超限只否定前缀起点；最新分钟组扫描仍可重新取得边界。
/// 读取失败、预算耗尽、作用域变化与影子缓存差异不能靠重试掩盖。
fn boundary_prefix_retryable(reason: SentBaselineReason) -> bool {
    matches!(
        reason,
        SentBaselineReason::AnchorMissing
            | SentBaselineReason::PageDiscontinuity
            | SentBaselineReason::OrderViolation
            | SentBaselineReason::AnchorChanged
            | SentBaselineReason::InvalidMid
            | SentBaselineReason::PageLimit
            | SentBaselineReason::BoundaryUnavailable
    )
}

impl MoogleMailSender {
    /// 原 session 跨越准备、持久 fence 与写后证明；不在 fence 后重建发送前历史。
    pub(crate) async fn prepare_sent_baseline(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailSentBaseline, MoogleMailError> {
        let result = self.prepare_sent_baseline_inner(cookie, world).await;
        if let Err(error) = &result {
            self.observe_budget_error(
                world,
                auction_application::external_metrics::MailPhase::SentBaselineBefore,
                error,
            );
        }
        result
    }

    async fn prepare_sent_baseline_inner(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailSentBaseline, MoogleMailError> {
        // 重启后先恢复持久检查点；没有或作用域不匹配时保持完整扫描。
        self.ensure_sent_scan_checkpoint(cookie, world).await;
        let deadline = Instant::now()
            .checked_add(self.verification_budget)
            .ok_or_else(|| scan_timeout(world))?;
        let round = self.client.read_round(cookie, world, true)?;
        let scope = round.sent_scope();
        let kind = match self.sent_baseline_mode {
            SentBaselineMode::Full => SentBaselineKind::Full(
                self.prepare_full(cookie, world, &round, deadline, true).await?,
            ),
            SentBaselineMode::Shadow => {
                self.prepare_shadow(cookie, world, &round, deadline).await?
            }
            SentBaselineMode::Watermark => {
                match self.daily_sent_verification(cookie, world, &round).await {
                    Some(seen) => SentBaselineKind::Full(seen),
                    None => {
                        match self.prepare_boundary(cookie, world, &round, deadline, None).await {
                            Ok(boundary) => SentBaselineKind::Watermark(boundary),
                            Err(reason) => {
                                self.baseline_event(
                                    world,
                                    SentBaselineEvent::Fallback {
                                        phase: SentBaselinePhase::BeforeSend,
                                        reason,
                                    },
                                );
                                // 不变量 6、9：异常边界不能借缓存跳过完整历史，且回退不重置截止点。
                                SentBaselineKind::Full(
                                    self.prepare_full(cookie, world, &round, deadline, false)
                                        .await?,
                                )
                            }
                        }
                    }
                }
            }
        };
        self.client.check_sent_scope(cookie, &scope)?;
        Ok(MoogleMailSentBaseline { world, scope, kind })
    }

    /// 每日完整校验只用于排除集与水位刷新；页数超限或读取失败只告警，绝不阻断发送。
    async fn daily_sent_verification(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        round: &MoogleMailReadRound,
    ) -> Option<HashSet<String>> {
        let checkpoint = self.sent_scan_checkpoint(cookie, world)?;
        let plan = auction_application::mail::plan_mail_scan(
            Some(&checkpoint),
            Utc::now(),
            self.client.mail_scan_daily_full_interval(),
        );
        self.observe_scan(
            world,
            MailScanEvent::Plan {
                stream: MailScanStream::Sent,
                mode: plan.mode,
                reason: plan.reason,
            },
        );
        if plan.mode != MailScanMode::Full
            || !matches!(
                plan.reason,
                MailScanReason::DailyVerification | MailScanReason::NoFullVerification
            )
        {
            return None;
        }
        // 只使用三分之一预算，确保校验失败后水位前缀仍有读取与证明预算。
        let verify_deadline = Instant::now().checked_add(self.verification_budget / 3)?;
        match self.prepare_full(cookie, world, round, verify_deadline, false).await {
            Ok(seen) => {
                self.observe_scan(
                    world,
                    MailScanEvent::Verification {
                        stream: MailScanStream::Sent,
                        outcome: MailScanOutcome::Complete,
                        reason: plan.reason,
                    },
                );
                Some(seen)
            }
            Err(error) => {
                let reason = error_reason(&error);
                let outcome = match reason {
                    SentBaselineReason::PageLimit => MailScanOutcome::PageLimit,
                    SentBaselineReason::Canceled => MailScanOutcome::Canceled,
                    _ => MailScanOutcome::Error,
                };
                self.observe_scan(
                    world,
                    MailScanEvent::Verification {
                        stream: MailScanStream::Sent,
                        outcome,
                        reason: plan.reason,
                    },
                );
                tracing::warn!(
                    world = world.as_str(),
                    outcome = outcome.label(),
                    "MoogleMail 每日完整校验未完成；继续水位前缀，不阻断发送"
                );
                // 记录本次尝试，下一次每日校验在 24 小时后；水位保持不变。
                self.remember_sent_scan(cookie, world, None, true).await;
                None
            }
        }
    }

    async fn prepare_shadow(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        round: &MoogleMailReadRound,
        deadline: Instant,
    ) -> Result<SentBaselineKind, MoogleMailError> {
        let sample = ShadowSample::new(Arc::clone(&self.sent_baseline_observer), world);
        let cached = self.cached_sent_baseline(cookie, world);
        let reused = cached.is_some();
        // 与旧完整扫描共享同一轮次的页面；数值或顺序不满足时仍只由旧证明裁决。
        let boundary = self.prepare_boundary(cookie, world, round, deadline, cached.as_ref()).await;
        let full = match cached {
            Some(seen) => Ok(seen),
            None => self.prepare_full(cookie, world, round, deadline, false).await,
        };
        let seen = match full {
            Ok(seen) => seen,
            Err(error) => {
                sample.finish(SentBaselineEvent::Uncomparable(error_reason(&error)));
                return Err(error);
            }
        };
        let boundary = boundary.and_then(|boundary| {
            // 缓存可能来自更早窗口；顶部新增或完整集合大于 W 都不能记成一致样本。
            for mail_id in &seen {
                if boundary.is_new(mail_id)? {
                    return Err(if reused {
                        SentBaselineReason::CacheBoundaryDelta
                    } else {
                        SentBaselineReason::OrderViolation
                    });
                }
            }
            Ok(boundary)
        });
        let boundary = match boundary {
            Ok(boundary) => Some(boundary),
            Err(reason) => {
                sample.finish(SentBaselineEvent::Uncomparable(reason));
                None
            }
        };
        Ok(SentBaselineKind::Shadow { seen, boundary, sample })
    }

    async fn prepare_boundary(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        round: &MoogleMailReadRound,
        deadline: Instant,
        cached: Option<&HashSet<String>>,
    ) -> Result<SentBoundary, SentBaselineReason> {
        // 上一轮完整扫描或边界捕获留下的水位只缩短前缀起点；边界仍按本轮最新行重算。
        let preferred = self
            .sent_scan_checkpoint(cookie, world)
            .and_then(|checkpoint| checkpoint.watermark)
            .and_then(|watermark| {
                let numeric_mid =
                    auction_application::mail::numeric_mail_scan_id(&watermark.mail_id)?;
                Some(SentBoundary::Mail {
                    mail_id: watermark.mail_id,
                    numeric_mid,
                    sent_at: watermark.observed_at,
                })
            });
        if let Some(preferred) = preferred.as_ref() {
            match self.scan_boundary(cookie, world, round, deadline, Some(preferred), cached).await
            {
                Ok(boundary) => {
                    self.remember_boundary(cookie, world, &boundary).await;
                    return Ok(boundary);
                }
                Err(reason) if boundary_prefix_retryable(reason) => {
                    tracing::debug!(
                        world = world.as_str(),
                        reason = reason.label(),
                        "MoogleMail 发送前水位前缀回退最新分钟组"
                    );
                }
                Err(reason) => return Err(reason),
            }
        }
        let boundary = self.scan_boundary(cookie, world, round, deadline, None, cached).await?;
        self.remember_boundary(cookie, world, &boundary).await;
        Ok(boundary)
    }

    /// 单次边界扫描；`boundary` 存在时读到锚点分钟前即停，仍要求锚点行与顺序一致。
    async fn scan_boundary(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        round: &MoogleMailReadRound,
        deadline: Instant,
        boundary: Option<&SentBoundary>,
        cached: Option<&HashSet<String>>,
    ) -> Result<SentBoundary, SentBaselineReason> {
        let scan = within_deadline(
            world,
            deadline,
            self.client.observe_sent_boundary(
                cookie,
                round,
                self.verification_pages,
                boundary,
                deadline,
            ),
        )
        .await
        .map_err(|error| {
            safe_diagnostics::warn(
                "mooglemail_baseline",
                "read_boundary",
                safe_diagnostics::project(&error),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            error_reason(&error)
        })?;
        let boundary = scan.before_boundary()?;
        // Shadow 保留旧缓存裁决，但捕获窗口不同的两组候选不能冒充等价样本。
        if let Some(seen) = cached
            && scan.summaries.iter().any(|summary| !seen.contains(&summary.external_mail_id))
        {
            return Err(SentBaselineReason::CacheBoundaryDelta);
        }
        tracing::debug!(%world, pages_scanned = scan.pages_scanned, empty = matches!(boundary, SentBoundary::Empty), "MoogleMail 已取得发送前临时边界");
        Ok(boundary)
    }

    /// 只把已取得的边界作为下一轮前缀起点；空列表不伪造水位。
    async fn remember_boundary(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        boundary: &SentBoundary,
    ) {
        let SentBoundary::Mail { mail_id, sent_at, .. } = boundary else {
            return;
        };
        self.remember_sent_scan(
            cookie,
            world,
            Some(auction_application::mail::MailScanWatermark {
                mail_id: mail_id.clone(),
                observed_at: *sent_at,
            }),
            false,
        )
        .await;
    }

    async fn prepare_full(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        round: &MoogleMailReadRound,
        deadline: Instant,
        allow_cache: bool,
    ) -> Result<HashSet<String>, MoogleMailError> {
        if allow_cache && let Some(seen) = self.cached_sent_baseline(cookie, world) {
            tracing::debug!(%world, mails = seen.len(), "MoogleMail 复用已证明的 Sent 基线");
            return Ok(seen);
        }
        let list = within_deadline(
            world,
            deadline,
            self.client.observe_sent_list(cookie, round, self.verification_pages),
        )
        .await?;
        if !list.complete {
            return Err(self.scan_page_limit(world, "baseline", list.summaries.len()));
        }
        let seen: HashSet<String> =
            list.summaries.iter().map(|summary| summary.external_mail_id.clone()).collect();
        check_deadline(deadline, world)?;
        // 完整枚举即具备排除集资格；fence 中止不会把它变成任务完成事实。
        self.remember_sent_baseline(cookie, world, seen.clone());
        // 水位只服务后续前缀；完整枚举成功才更新每日校验时间。
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
        Ok(seen)
    }
}
