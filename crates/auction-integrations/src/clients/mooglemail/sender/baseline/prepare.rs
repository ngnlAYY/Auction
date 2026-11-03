//! sending fence 前的完整排除集与 fresh 临时边界；缺失时在同一预算内失败关闭。

use std::{collections::HashSet, sync::Arc};

use auction_application::{
    SentBaselineEvent, SentBaselineMode, SentBaselinePhase, SentBaselineReason,
};
use tokio::time::Instant;

use super::{MoogleMailSentBaseline, SentBaselineKind, error_reason, telemetry::ShadowSample};
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailSender, MoogleMailWorld,
    client::{MoogleMailReadRound, SentBoundary},
    sender::proof::{check_deadline, scan_timeout, within_deadline},
};

impl MoogleMailSender {
    /// 原 session 跨越准备、持久 fence 与写后证明；不在 fence 后重建发送前历史。
    pub(crate) async fn prepare_sent_baseline(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailSentBaseline, MoogleMailError> {
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
                            self.prepare_full(cookie, world, &round, deadline, false).await?,
                        )
                    }
                }
            }
        };
        self.client.check_sent_scope(cookie, &scope)?;
        Ok(MoogleMailSentBaseline { world, scope, kind })
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
        let scan = within_deadline(
            world,
            deadline,
            self.client.observe_sent_boundary(
                cookie,
                round,
                self.verification_pages,
                None,
                deadline,
            ),
        )
        .await
        .map_err(|error| error_reason(&error))?;
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
        Ok(seen)
    }
}
