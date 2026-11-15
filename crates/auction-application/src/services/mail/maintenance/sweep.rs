//! 单轮候选轮转：按持久环形批次逐条开始，预算耗尽即停止，不越过未处理后缀推进游标。

use std::time::Instant;

use super::MailboxMaintenanceReport;
use super::MailboxMaintenanceService;
use super::diagnostics::{check_deadline, record_unpersisted_error, repository_error};
use crate::errors::ApplicationError;
use crate::ports::mail::MoogleMailboxSweepCandidate;
use crate::ports::operations::ExternalWriteActivity;

/// 单轮最多核对的投递数。
const RECOVERY_PER_SWEEP: usize = 20;

impl MailboxMaintenanceService {
    /// Worker 以同一总截止点包住完整 future；DB、凭据排队与所有远端操作均计入维护预算。
    /// 普通页面的远端失败分开记录，不清除已获证据；游标与 discard 围栏在取消后仍保留。
    /// 活动从本次 discard 围栏调用前覆盖交接与外写；确认一次性尝试提交后才授权 POST。
    pub async fn sweep(
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
        self.recover_sent(world_id, &candidates, deadline, &mut report).await?;
        check_deadline(deadline)?;
        self.clean_compose(world_id, &candidates, deadline, activity, &mut report).await?;
        Ok(report)
    }

    /// Sent/Take 统计按字段直接累加进本轮报告，不经位置元组回填；返回错误时整轮报告随之丢弃。
    async fn recover_sent(
        &self,
        world_id: &str,
        candidates: &[MoogleMailboxSweepCandidate],
        deadline: Instant,
        report: &mut MailboxMaintenanceReport,
    ) -> Result<(), ApplicationError> {
        // Sent 每日完整校验与页数告警必须独立于是否存在未知发送候选，因此每轮都读取；
        // 前缀轮仍只读增量。
        let entries = match self.maintenance.sent_entries(world_id).await {
            Ok(entries) => {
                if let Some(observation) = entries.page_observation.as_ref()
                    && let Err(error) = self.store.record_sent_pages(observation).await
                {
                    record_unpersisted_error("record_sent_pages", world_id, &error);
                }
                Some(entries)
            }
            Err(error) => {
                record_unpersisted_error("read_sent_entries", world_id, &error);
                report.sent_state_failed += 1;
                None
            }
        };
        // 按已读取的稳定环形批次逐条开始；候选批次本身不推进持久游标。
        let mut recovery_budget = RECOVERY_PER_SWEEP;
        for slot in candidates {
            if recovery_budget == 0 {
                break;
            }
            check_deadline(deadline)?;
            if entries.is_none()
                && slot.send.as_ref().is_some_and(|candidate| candidate.operation == "send")
            {
                // 未获得 Sent 列表就没有开始此候选，保留它及其后缀的持久轮转位置。
                break;
            }
            if !self.store.record_sweep_attempt(world_id, slot.delivery_id).await? {
                continue;
            }
            report.sent_candidates += 1;
            // 不可重建的旧行也获得一次轮转机会，但不读取远端、不产生恢复证明。
            let Some(candidate) = slot.send.as_ref() else { continue };
            if candidate.world_id != world_id {
                continue;
            }
            let step = match candidate.operation.as_str() {
                "send" => {
                    let Some(coverage) = entries.as_ref() else { continue };
                    // 前缀只覆盖水位之后的时间窗；更旧候选留给每日完整校验或其他证明路径。
                    if !coverage.covers(candidate.sending_at) {
                        continue;
                    }
                    let entries = coverage.entries.as_slice();
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
                        if !step.recovered && coverage.covered_from.is_none() {
                            // 目击扫描不限时间；只有完整列表才能证明同主题邮件没有遗漏。
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
                            // 结算 legacy 路径按 reference 匹配且没有时间窗，前缀不能证明全局唯一。
                            "settlement" if coverage.covered_from.is_none() => {
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
                report.sent_matched += 1;
            }
            if step.observed {
                report.sent_observed += 1;
            }
            if step.recovered {
                report.sent_recovered += 1;
            }
            if step.failed {
                report.sent_state_failed += 1;
            }
            if step.requeued {
                report.sweep_requeued += 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Instant,
    };

    use chrono::{TimeZone, Utc};
    use serde_json::Value;
    use uuid::Uuid;

    use super::super::ComposeCleanupFailure;
    use super::{MailboxMaintenanceReport, MailboxMaintenanceService};
    use crate::errors::ApplicationError;
    use crate::ports::core::ApplicationFuture;
    use crate::ports::core::Clock;
    use crate::ports::delivery::{DeliverySendExpectation, DeliveryTakeExpectation};
    use crate::ports::mail::{
        MailScanPageObservation, MoogleMailboxComposeState, MoogleMailboxDiscard,
        MoogleMailboxEvidenceStore, MoogleMailboxInboxObservation, MoogleMailboxMaintenance,
        MoogleMailboxNoticeExpectation, MoogleMailboxSentEntries, MoogleMailboxSentEntry,
        MoogleMailboxSentState, MoogleMailboxSweepCandidate, MoogleMailboxUnprovenSend,
    };
    use crate::ports::operations::ExternalWriteActivity;

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    struct NoopClock;
    impl Clock for NoopClock {
        fn now(&self) -> chrono::DateTime<Utc> {
            Utc.with_ymd_and_hms(2026, 11, 7, 7, 0, 0).single().expect("fixture time")
        }
    }

    struct FixtureMaintenance {
        coverage: MoogleMailboxSentEntries,
        /// 置位时 Sent 列表与 compose 读取都失败，用于核对部分失败的报告口径。
        reads_fail: bool,
        /// 未清空的草稿；`None` 表示 compose 已干净。
        dirty_compose: Option<MoogleMailboxComposeState>,
        discard_fails: bool,
    }
    impl FixtureMaintenance {
        fn new(coverage: MoogleMailboxSentEntries, reads_fail: bool) -> Self {
            Self { coverage, reads_fail, dirty_compose: None, discard_fails: false }
        }
    }
    impl MoogleMailboxMaintenance for FixtureMaintenance {
        fn compose<'a>(
            &'a self,
            _world_id: &'a str,
        ) -> ApplicationFuture<'a, MoogleMailboxComposeState> {
            let reads_fail = self.reads_fail;
            let dirty = self.dirty_compose.clone();
            Box::pin(async move {
                if reads_fail {
                    return Err(ApplicationError::CapabilityUnavailable("fixture compose"));
                }
                if let Some(dirty) = dirty {
                    return Ok(dirty);
                }
                Ok(MoogleMailboxComposeState {
                    clean: true,
                    recipient: String::new(),
                    subject: String::new(),
                    body: String::new(),
                    credits: 0,
                    cod: None,
                    attachment_count: 0,
                    equipment_ids: Vec::new(),
                    body_digest: [0; 32],
                    observation_credential: None,
                })
            })
        }
        fn discard_compose<'a>(
            &'a self,
            _world_id: &'a str,
            _expected: &'a MoogleMailboxComposeState,
        ) -> ApplicationFuture<'a, MoogleMailboxDiscard> {
            let discard_fails = self.discard_fails;
            Box::pin(async move {
                if discard_fails {
                    return Err(ApplicationError::CapabilityUnavailable("fixture discard"));
                }
                Ok(MoogleMailboxDiscard::AlreadyClean)
            })
        }
        fn sent_entries<'a>(
            &'a self,
            _world_id: &'a str,
        ) -> ApplicationFuture<'a, MoogleMailboxSentEntries> {
            let coverage = self.coverage.clone();
            let reads_fail = self.reads_fail;
            Box::pin(async move {
                if reads_fail {
                    return Err(ApplicationError::CapabilityUnavailable("fixture sent"));
                }
                Ok(coverage)
            })
        }
        fn read_mail_observed<'a>(&'a self, _world_id: &'a str) -> ApplicationFuture<'a, usize> {
            Box::pin(async { Ok(0) })
        }
        fn sent_state<'a>(
            &'a self,
            _world_id: &'a str,
            entry: &'a MoogleMailboxSentEntry,
            _observed_at: chrono::DateTime<Utc>,
        ) -> ApplicationFuture<'a, MoogleMailboxSentState> {
            let state = MoogleMailboxSentState {
                external_mail_id: entry.external_mail_id.clone(),
                recipient: entry.recipient.clone(),
                subject: entry.subject.clone(),
                collected: false,
                cod_pending: 0,
                remaining_credits: 0,
                remaining_equipment_ids: Vec::new(),
                body_digest: [0; 32],
                observation_credential: None,
            };
            Box::pin(async move { Ok(state) })
        }
        fn notice_expectation<'a>(
            &'a self,
            _candidate: &'a MoogleMailboxUnprovenSend,
        ) -> ApplicationFuture<'a, Option<MoogleMailboxNoticeExpectation>> {
            Box::pin(async { Ok(None) })
        }
        fn inbox_assets<'a>(
            &'a self,
            _world_id: &'a str,
            _external_mail_id: &'a str,
        ) -> ApplicationFuture<'a, MoogleMailboxInboxObservation> {
            Box::pin(async {
                Ok(MoogleMailboxInboxObservation { assets: None, observation_credential: None })
            })
        }
    }

    struct FixtureStore {
        bind_called: Arc<AtomicBool>,
        /// 一次性 discard 围栏是否授权。
        fence_granted: bool,
    }
    impl MoogleMailboxEvidenceStore for FixtureStore {
        fn unproven_sends<'a>(
            &'a self,
            _world_id: &'a str,
            _limit: i64,
        ) -> ApplicationFuture<'a, Vec<MoogleMailboxSweepCandidate>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn record_sweep_attempt<'a>(
            &'a self,
            _world_id: &'a str,
            _delivery_id: Uuid,
        ) -> ApplicationFuture<'a, bool> {
            Box::pin(async { Ok(true) })
        }
        fn record_sent_pages<'a>(
            &'a self,
            _observation: &'a MailScanPageObservation,
        ) -> ApplicationFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
        fn begin_compose_discard<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _expectation: Option<&'a DeliverySendExpectation>,
        ) -> ApplicationFuture<'a, bool> {
            let granted = self.fence_granted;
            Box::pin(async move { Ok(granted) })
        }
        fn record_sent_observation<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _observation: Value,
        ) -> ApplicationFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
        fn record_sent_sighting<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _external_mail_id: &'a str,
            _observation: Value,
        ) -> ApplicationFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
        fn record_compose_discard<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _observation: Value,
        ) -> ApplicationFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
        fn bind_recovered_send<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _external_mail_id: String,
            _metadata: Value,
            _expectation: Option<&'a DeliverySendExpectation>,
        ) -> ApplicationFuture<'a, bool> {
            self.bind_called.store(true, Ordering::SeqCst);
            Box::pin(async { Ok(true) })
        }
        fn bind_recovered_action<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _external_mail_id: String,
            _metadata: Value,
            _expectation: &'a DeliveryTakeExpectation,
            _observation: &'a crate::ports::delivery::DeliverySendCredentialBinding,
        ) -> ApplicationFuture<'a, bool> {
            Box::pin(async { Ok(true) })
        }
        fn requeue_unproven<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _retries: i64,
            _reason: &'a str,
            _expectation: &'a DeliveryTakeExpectation,
            _observation: &'a crate::ports::delivery::DeliverySendCredentialBinding,
        ) -> ApplicationFuture<'a, bool> {
            Box::pin(async { Ok(true) })
        }
    }

    fn candidate(sending_at: chrono::DateTime<Utc>, reference: &str) -> MoogleMailboxUnprovenSend {
        MoogleMailboxUnprovenSend {
            delivery_id: Uuid::new_v4(),
            fencing_version: 1,
            sending_fencing_version: 1,
            send_expectation: None,
            take_expectation: None,
            origin_kind: "settlement".to_owned(),
            world_id: "main".to_owned(),
            recipient: "recipient".to_owned(),
            cod: 0,
            credits: 0,
            inbound_mail_id: String::new(),
            result_code: None,
            user_key_user_id: None,
            sending_at,
            outcome_recorded_at: sending_at + chrono::Duration::hours(1),
            operation: "send".to_owned(),
            attachment_ids: Vec::new(),
            sweep_retries: 0,
            reference: reference.to_owned(),
            template: "settlement".to_owned(),
        }
    }

    fn assert_no_bind(candidate: MoogleMailboxUnprovenSend, coverage: MoogleMailboxSentEntries) {
        let bind_called = Arc::new(AtomicBool::new(false));
        let service = MailboxMaintenanceService::new(
            Arc::new(FixtureMaintenance::new(coverage, false)),
            Arc::new(FixtureStore { bind_called: Arc::clone(&bind_called), fence_granted: false }),
            Arc::new(NoopClock),
        );
        let delivery_id = candidate.delivery_id;
        let mut report = MailboxMaintenanceReport::default();
        block_on(service.recover_sent(
            "main",
            &[MoogleMailboxSweepCandidate { delivery_id, send: Some(candidate) }],
            Instant::now() + std::time::Duration::from_secs(60),
            &mut report,
        ))
        .expect("维护不得失败");
        assert!(!bind_called.load(Ordering::SeqCst), "前缀不得触发 bind_recovered_send");
        // 已开始轮转的候选只计入 sent_candidates；未读取远端、未恢复也未失败。
        assert_eq!(
            report,
            MailboxMaintenanceReport { sent_candidates: 1, ..MailboxMaintenanceReport::default() }
        );
    }

    #[test]
    fn prefix_does_not_recover_older_candidate_or_legacy_settlement() {
        let boundary = Utc.with_ymd_and_hms(2026, 11, 7, 6, 0, 0).single().expect("time");
        let prefix = MoogleMailboxSentEntries {
            entries: vec![MoogleMailboxSentEntry {
                external_mail_id: "105".to_owned(),
                recipient: "recipient".to_owned(),
                subject: "[reference]".to_owned(),
                sent_at: boundary,
            }],
            covered_from: Some(boundary),
            complete: true,
            page_observation: None,
        };
        // 旧候选的发送时间在水位前；前缀中的新同主题邮件不能证明旧候选的唯一命中。
        assert_no_bind(
            candidate(boundary - chrono::Duration::seconds(1), "reference"),
            prefix.clone(),
        );
        // 即使候选在水位后，legacy settlement 没有任何时间窗，仍必须等待完整列表。
        assert_no_bind(candidate(boundary + chrono::Duration::minutes(1), "reference"), prefix);
    }

    fn sweep_report(reads_fail: bool) -> MailboxMaintenanceReport {
        let coverage = MoogleMailboxSentEntries {
            entries: Vec::new(),
            covered_from: None,
            complete: true,
            page_observation: None,
        };
        let service = MailboxMaintenanceService::new(
            Arc::new(FixtureMaintenance::new(coverage, reads_fail)),
            Arc::new(FixtureStore {
                bind_called: Arc::new(AtomicBool::new(false)),
                fence_granted: false,
            }),
            Arc::new(NoopClock),
        );
        block_on(service.sweep(
            "main",
            Instant::now() + std::time::Duration::from_secs(60),
            &ExternalWriteActivity::default(),
        ))
        .expect("远端读取失败只记入报告，不中断本轮")
    }

    /// 报告只带闭集阶段与计数，不带错误原文；Worker 据此把部分失败记为 Error。
    #[test]
    fn remote_read_failures_are_reported_as_closed_stages() {
        assert_eq!(
            sweep_report(false),
            MailboxMaintenanceReport { compose_clean: true, ..MailboxMaintenanceReport::default() }
        );
        assert_eq!(
            sweep_report(true),
            MailboxMaintenanceReport {
                compose_failure: Some(ComposeCleanupFailure::ReadCompose),
                sent_state_failed: 1,
                ..MailboxMaintenanceReport::default()
            }
        );
    }

    /// 唯一精确匹配后，围栏拒绝与 discard 失败各自落到对应阶段，且都不标记草稿已清空。
    #[test]
    fn compose_cleanup_failures_name_their_stage() {
        let cleanup = |fence_granted: bool, discard_fails: bool| {
            let send = candidate(NoopClock.now(), "ref-compose");
            let dirty = MoogleMailboxComposeState {
                clean: false,
                recipient: send.recipient.clone(),
                subject: format!("Auction {}", send.reference),
                body: String::new(),
                credits: send.credits,
                cod: Some(send.cod),
                attachment_count: 0,
                equipment_ids: send.attachment_ids.clone(),
                body_digest: [0; 32],
                observation_credential: None,
            };
            let mut maintenance = FixtureMaintenance::new(
                MoogleMailboxSentEntries {
                    entries: Vec::new(),
                    covered_from: None,
                    complete: true,
                    page_observation: None,
                },
                false,
            );
            maintenance.dirty_compose = Some(dirty);
            maintenance.discard_fails = discard_fails;
            let service = MailboxMaintenanceService::new(
                Arc::new(maintenance),
                Arc::new(FixtureStore {
                    bind_called: Arc::new(AtomicBool::new(false)),
                    fence_granted,
                }),
                Arc::new(NoopClock),
            );
            let delivery_id = send.delivery_id;
            let mut report = MailboxMaintenanceReport::default();
            block_on(service.clean_compose(
                "main",
                &[MoogleMailboxSweepCandidate { delivery_id, send: Some(send) }],
                Instant::now() + std::time::Duration::from_secs(60),
                &ExternalWriteActivity::default(),
                &mut report,
            ))
            .expect("草稿清理的远端失败只记入报告");
            assert_eq!(report.compose_matches, 1);
            assert_eq!(report.matched_delivery, Some(delivery_id));
            assert!(!report.compose_clean && !report.discarded);
            report.compose_failure
        };
        assert_eq!(cleanup(false, false), Some(ComposeCleanupFailure::FenceDenied));
        assert_eq!(cleanup(true, true), Some(ComposeCleanupFailure::DiscardCompose));
    }
}
