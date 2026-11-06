//! 单轮候选轮转：按持久环形批次逐条开始，预算耗尽即停止，不越过未处理后缀推进游标。

use std::time::Instant;

use crate::errors::ApplicationError;
use crate::ports::mail::MoogleMailboxSweepCandidate;
use crate::ports::operations::ExternalWriteActivity;

use super::MailboxMaintenanceReport;
use super::MailboxMaintenanceService;
use super::diagnostics::{check_deadline, record_unpersisted_error, repository_error};

/// 单轮最多核对的投递数；达到上限即停止，不越过未处理后缀推进持久游标。
const RECOVERY_PER_SWEEP: usize = 20;

impl MailboxMaintenanceService {
    /// Worker 以同一总截止点包住完整 future；DB、凭据排队与所有远端操作均计入维护预算。
    /// 普通页面的远端失败分开记录，不清除已获证据；游标与 discard 围栏在取消后仍保留。
    pub async fn sweep(
        &self,
        world_id: &str,
        deadline: Instant,
    ) -> Result<MailboxMaintenanceReport, ApplicationError> {
        self.sweep_with_activity(world_id, deadline, &ExternalWriteActivity::default()).await
    }

    /// 活动从本次 discard 围栏调用前覆盖交接与外写；确认一次性尝试提交后才授权 POST。
    pub async fn sweep_with_activity(
        &self,
        world_id: &str,
        deadline: Instant,
        activity: &ExternalWriteActivity,
    ) -> Result<MailboxMaintenanceReport, ApplicationError> {
        if !matches!(world_id, "main" | "isekai") {
            return Err(repository_error("维护世界无效"));
        }
        check_deadline(deadline)?;
        let mut report = MailboxMaintenanceReport::default();
        let candidates = self.store.unproven_sends(world_id, 200).await?;
        let sent = self.recover_sent(world_id, &candidates, deadline).await?;
        report.sent_observed = sent.0;
        report.sent_recovered = sent.1;
        report.sent_candidates = sent.2;
        report.sent_matched = sent.3;
        report.sent_state_failed = sent.4;
        report.sent_state_error = sent.5;
        report.sweep_requeued = sent.6;
        check_deadline(deadline)?;
        self.clean_compose(world_id, &candidates, deadline, activity, &mut report).await?;
        Ok(report)
    }

    async fn recover_sent(
        &self,
        world_id: &str,
        candidates: &[MoogleMailboxSweepCandidate],
        deadline: Instant,
    ) -> Result<(usize, usize, usize, usize, usize, Option<String>, usize), ApplicationError> {
        // 只有可重建的 Send 候选才读取 Sent；Take 与纯游标轮转不依赖该列表。
        let needs_sent = candidates
            .iter()
            .any(|slot| slot.send.as_ref().is_some_and(|candidate| candidate.operation == "send"));
        let (entries, mut state_error) = if needs_sent {
            match self.maintenance.sent_entries(world_id).await {
                Ok(entries) => (Some(entries), None),
                Err(error) => {
                    record_unpersisted_error("read_sent_entries", world_id, &error);
                    (None, Some(error.to_string()))
                }
            }
        } else {
            (None, None)
        };
        // 按已读取的稳定环形批次逐条开始；候选批次本身不推进持久游标。
        let mut recovery_budget = RECOVERY_PER_SWEEP;
        let mut observed = 0;
        let mut attempted = 0;
        let mut recovered = 0;
        let mut matched_count = 0;
        let mut failed = usize::from(state_error.is_some());
        let mut requeued = 0;
        for slot in candidates {
            if recovery_budget == 0 {
                break;
            }
            check_deadline(deadline)?;
            if entries.is_none()
                && slot.send.as_ref().is_some_and(|candidate| candidate.operation == "send")
            {
                // 未获得完整 Sent 列表就没有开始此候选，保留它及其后缀的持久轮转位置。
                break;
            }
            if !self.store.record_sweep_attempt(world_id, slot.delivery_id).await? {
                continue;
            }
            attempted += 1;
            // 不可重建的旧行也获得一次轮转机会，但不读取远端、不产生恢复证明。
            let Some(candidate) = slot.send.as_ref() else { continue };
            if candidate.world_id != world_id {
                continue;
            }
            let step = match candidate.operation.as_str() {
                "send" => {
                    let Some(entries) = entries.as_deref() else { continue };
                    recovery_budget -= 1;
                    if let Some(expectation) = candidate.send_expectation.as_ref() {
                        let mut step = self
                            .recover_frozen_send(
                                world_id,
                                candidate,
                                expectation,
                                entries,
                                candidates,
                                deadline,
                            )
                            .await?;
                        if !step.recovered {
                            self.sight_settlement_reference(
                                world_id,
                                candidate,
                                expectation,
                                entries,
                                deadline,
                                &mut step,
                            )
                            .await?;
                        }
                        step
                    } else {
                        match candidate.origin_kind.as_str() {
                            "settlement" => {
                                self.recover_settlement(world_id, candidate, entries).await?
                            }
                            "mail_intake" => {
                                self.recover_notice(world_id, candidate, entries).await?
                            }
                            _ => continue,
                        }
                    }
                }
                "take" if candidate.origin_kind == "mail_intake" => {
                    recovery_budget -= 1;
                    self.recover_take(world_id, candidate).await?
                }
                _ => continue,
            };
            if step.matched {
                matched_count += 1;
            }
            if step.observed {
                observed += 1;
            }
            if step.recovered {
                recovered += 1;
            }
            if step.failed {
                failed += 1;
            }
            if step.requeued {
                requeued += 1;
            }
            if step.error.is_some() {
                state_error = step.error;
            }
        }
        Ok((observed, recovered, attempted, matched_count, failed, state_error, requeued))
    }
}
