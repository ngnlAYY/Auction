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
                // 未获得 Sent 列表就没有开始此候选，保留它及其后缀的持久轮转位置。
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

#[cfg(test)]
mod tests {
    use super::MailboxMaintenanceService;
    use crate::ports::core::Clock;
    use crate::ports::delivery::{DeliverySendExpectation, DeliveryTakeExpectation};
    use crate::ports::mail::{
        MailboxFuture, MoogleMailboxComposeState, MoogleMailboxDiscard, MoogleMailboxEvidenceStore,
        MoogleMailboxInboxObservation, MoogleMailboxMaintenance, MoogleMailboxNoticeExpectation,
        MoogleMailboxSentEntries, MoogleMailboxSentEntry, MoogleMailboxSentState,
        MoogleMailboxSweepCandidate, MoogleMailboxUnprovenSend,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::Value;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Instant,
    };
    use uuid::Uuid;

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
    }
    impl MoogleMailboxMaintenance for FixtureMaintenance {
        fn compose<'a>(
            &'a self,
            _world_id: &'a str,
        ) -> MailboxFuture<'a, MoogleMailboxComposeState> {
            Box::pin(async {
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
        ) -> MailboxFuture<'a, MoogleMailboxDiscard> {
            Box::pin(async { Ok(MoogleMailboxDiscard::AlreadyClean) })
        }
        fn sent_entries<'a>(
            &'a self,
            _world_id: &'a str,
        ) -> MailboxFuture<'a, MoogleMailboxSentEntries> {
            let coverage = self.coverage.clone();
            Box::pin(async move { Ok(coverage) })
        }
        fn read_mail_observed<'a>(&'a self, _world_id: &'a str) -> MailboxFuture<'a, usize> {
            Box::pin(async { Ok(0) })
        }
        fn sent_state<'a>(
            &'a self,
            _world_id: &'a str,
            entry: &'a MoogleMailboxSentEntry,
            _observed_at: chrono::DateTime<Utc>,
        ) -> MailboxFuture<'a, MoogleMailboxSentState> {
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
        ) -> MailboxFuture<'a, Option<MoogleMailboxNoticeExpectation>> {
            Box::pin(async { Ok(None) })
        }
        fn inbox_assets<'a>(
            &'a self,
            _world_id: &'a str,
            _external_mail_id: &'a str,
        ) -> MailboxFuture<'a, MoogleMailboxInboxObservation> {
            Box::pin(async {
                Ok(MoogleMailboxInboxObservation { assets: None, observation_credential: None })
            })
        }
    }

    struct FixtureStore {
        bind_called: Arc<AtomicBool>,
    }
    impl MoogleMailboxEvidenceStore for FixtureStore {
        fn unproven_sends<'a>(
            &'a self,
            _world_id: &'a str,
            _limit: i64,
        ) -> MailboxFuture<'a, Vec<MoogleMailboxSweepCandidate>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn record_sweep_attempt<'a>(
            &'a self,
            _world_id: &'a str,
            _delivery_id: Uuid,
        ) -> MailboxFuture<'a, bool> {
            Box::pin(async { Ok(true) })
        }
        fn begin_compose_discard<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _expectation: Option<&'a DeliverySendExpectation>,
        ) -> MailboxFuture<'a, bool> {
            Box::pin(async { Ok(false) })
        }
        fn record_sent_observation<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _observation: Value,
        ) -> MailboxFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
        fn record_sent_sighting<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _external_mail_id: &'a str,
            _observation: Value,
        ) -> MailboxFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
        fn record_compose_discard<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _observation: Value,
        ) -> MailboxFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
        fn bind_recovered_send<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _external_mail_id: String,
            _metadata: Value,
            _expectation: Option<&'a DeliverySendExpectation>,
        ) -> MailboxFuture<'a, bool> {
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
        ) -> MailboxFuture<'a, bool> {
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
        ) -> MailboxFuture<'a, bool> {
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
            Arc::new(FixtureMaintenance { coverage }),
            Arc::new(FixtureStore { bind_called: Arc::clone(&bind_called) }),
            Arc::new(NoopClock),
        );
        let delivery_id = candidate.delivery_id;
        block_on(service.recover_sent(
            "main",
            &[MoogleMailboxSweepCandidate { delivery_id, send: Some(candidate) }],
            Instant::now() + std::time::Duration::from_secs(60),
        ))
        .expect("维护不得失败");
        assert!(!bind_called.load(Ordering::SeqCst), "前缀不得触发 bind_recovered_send");
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
        };
        // 旧候选的发送时间在水位前；前缀中的新同主题邮件不能证明旧候选的唯一命中。
        assert_no_bind(
            candidate(boundary - chrono::Duration::seconds(1), "reference"),
            prefix.clone(),
        );
        // 即使候选在水位后，legacy settlement 没有任何时间窗，仍必须等待完整列表。
        assert_no_bind(candidate(boundary + chrono::Duration::minutes(1), "reference"), prefix);
    }
}
