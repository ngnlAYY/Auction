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
                // 入站 Take 与结算退件 Take 都只经原 Take 会话的冻结身份恢复。
                "take"
                    if matches!(
                        candidate.origin_kind.as_str(),
                        "mail_intake" | "settlement_return_action"
                    ) =>
                {
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
    use crate::ports::delivery::{
        DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryTakeExpectation,
    };
    use crate::ports::mail::{
        MailScanPageObservation, MoogleMailboxComposeState, MoogleMailboxDiscard,
        MoogleMailboxEvidenceStore, MoogleMailboxInboxAssets, MoogleMailboxInboxObservation,
        MoogleMailboxMaintenance, MoogleMailboxNoticeExpectation, MoogleMailboxReturnTake,
        MoogleMailboxReturnTakeIdentity, MoogleMailboxReturnTakeObservation,
        MoogleMailboxSentEntries, MoogleMailboxSentEntry, MoogleMailboxSentState,
        MoogleMailboxSweepCandidate, MoogleMailboxUnprovenSend,
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
        /// 收件箱资产观察；缺省为“未返回观察会话”。
        inbox: MoogleMailboxInboxObservation,
        /// 结算退件 Take 的回执观察；缺省为“未返回观察会话”。
        return_take: MoogleMailboxReturnTakeObservation,
    }
    impl FixtureMaintenance {
        fn new(coverage: MoogleMailboxSentEntries, reads_fail: bool) -> Self {
            Self {
                coverage,
                reads_fail,
                dirty_compose: None,
                discard_fails: false,
                inbox: MoogleMailboxInboxObservation { assets: None, observation_credential: None },
                return_take: MoogleMailboxReturnTakeObservation {
                    outcome: MoogleMailboxReturnTake::Unproven,
                    observation_credential: None,
                },
            }
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
            let inbox = self.inbox.clone();
            Box::pin(async move { Ok(inbox) })
        }
        fn return_take_receipt<'a>(
            &'a self,
            _world_id: &'a str,
            _external_mail_id: &'a str,
            _identity: &'a MoogleMailboxReturnTakeIdentity,
        ) -> ApplicationFuture<'a, MoogleMailboxReturnTakeObservation> {
            let observation = self.return_take.clone();
            Box::pin(async move { Ok(observation) })
        }
    }

    /// Take 恢复调用：`("bind", 证明元数据)` 或 `("requeue", 原因)`。
    type TakeCalls = Arc<std::sync::Mutex<Vec<(&'static str, Value)>>>;

    struct FixtureStore {
        bind_called: Arc<AtomicBool>,
        /// 一次性 discard 围栏是否授权。
        fence_granted: bool,
        take_calls: TakeCalls,
    }
    impl FixtureStore {
        fn new(bind_called: Arc<AtomicBool>, fence_granted: bool) -> Self {
            Self { bind_called, fence_granted, take_calls: Arc::default() }
        }
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
            metadata: Value,
            _expectation: &'a DeliveryTakeExpectation,
            _observation: &'a DeliverySendCredentialBinding,
        ) -> ApplicationFuture<'a, bool> {
            if let Ok(mut calls) = self.take_calls.lock() {
                calls.push(("bind", metadata));
            }
            Box::pin(async { Ok(true) })
        }
        fn requeue_unproven<'a>(
            &'a self,
            _delivery_id: Uuid,
            _fencing_version: i64,
            _retries: i64,
            reason: &'a str,
            _expectation: &'a DeliveryTakeExpectation,
            _observation: &'a DeliverySendCredentialBinding,
        ) -> ApplicationFuture<'a, bool> {
            if let Ok(mut calls) = self.take_calls.lock() {
                calls.push(("requeue", Value::from(reason)));
            }
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
            return_take: None,
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
            Arc::new(FixtureStore::new(Arc::clone(&bind_called), false)),
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
            Arc::new(FixtureStore::new(Arc::new(AtomicBool::new(false)), false)),
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
                Arc::new(FixtureStore::new(Arc::new(AtomicBool::new(false)), fence_granted)),
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

    fn original_session() -> DeliverySendCredentialBinding {
        DeliverySendCredentialBinding {
            credential_id: Uuid::from_u128(0x5e7),
            credential_version: 3,
            runtime_generation: 9,
        }
    }

    /// 结算退件 Take：原 Take 会话冻结了凭据，资产为 90 Credits 加一件装备；证据带系统后缀事实。
    fn return_take() -> MoogleMailboxUnprovenSend {
        let mut send = candidate(NoopClock.now(), "");
        send.origin_kind = "settlement_return_action".to_owned();
        send.operation = "take".to_owned();
        send.recipient = "MoogleMail".to_owned();
        send.credits = 90;
        send.attachment_ids = vec!["4401".to_owned()];
        send.inbound_mail_id = "993101".to_owned();
        send.take_expectation = Some(DeliveryTakeExpectation {
            world_id: "main".to_owned(),
            external_mail_id: "993101".to_owned(),
            credential: Some(original_session()),
        });
        send.return_take = Some(MoogleMailboxReturnTakeIdentity {
            sender: "MoogleMail".to_owned(),
            subject: "[RTS] Auction seller payout [ref]".to_owned(),
            original_body_digest: [7; 32],
            returned_from: "Seller".to_owned(),
            cod: 0,
            attachment_count: 2,
        });
        send
    }

    /// 以给定结算退件观察跑一次轮转，返回 Take 恢复调用。旧的收件箱资产观察同时给出“附件已空”，
    /// 结算退件不得据此收口。
    fn recover_return_take(
        send: MoogleMailboxUnprovenSend,
        outcome: MoogleMailboxReturnTake,
        observation_credential: Option<DeliverySendCredentialBinding>,
    ) -> Vec<(&'static str, Value)> {
        let mut maintenance = FixtureMaintenance::new(
            MoogleMailboxSentEntries {
                entries: Vec::new(),
                covered_from: None,
                complete: true,
                page_observation: None,
            },
            false,
        );
        maintenance.inbox = MoogleMailboxInboxObservation {
            assets: inbox("MoogleMail", 0, &[]),
            observation_credential: observation_credential.clone(),
        };
        maintenance.return_take =
            MoogleMailboxReturnTakeObservation { outcome, observation_credential };
        let store = FixtureStore::new(Arc::new(AtomicBool::new(false)), false);
        let calls = Arc::clone(&store.take_calls);
        let service = MailboxMaintenanceService::new(
            Arc::new(maintenance),
            Arc::new(store),
            Arc::new(NoopClock),
        );
        let delivery_id = send.delivery_id;
        let mut report = MailboxMaintenanceReport::default();
        block_on(service.recover_sent(
            "main",
            &[MoogleMailboxSweepCandidate { delivery_id, send: Some(send) }],
            Instant::now() + std::time::Duration::from_secs(60),
            &mut report,
        ))
        .expect("维护不得失败");
        calls.lock().map(|calls| calls.clone()).unwrap_or_default()
    }

    fn inbox(sender: &str, credits: i64, equipment: &[&str]) -> Option<MoogleMailboxInboxAssets> {
        Some(MoogleMailboxInboxAssets {
            sender: sender.to_owned(),
            equipment_ids: equipment.iter().map(|id| (*id).to_owned()).collect(),
            credits,
            cod: 0,
        })
    }

    fn present(sender: &str, credits: i64, equipment: &[&str]) -> MoogleMailboxReturnTake {
        MoogleMailboxReturnTake::Present(inbox(sender, credits, equipment).expect("资产"))
    }

    /// 完成证明：integrations 已在原会话里核对原身份、原正文前缀与领取回执；证明携带预期资产，
    /// 交给存储锁内与动作冻结事实逐项比对。
    #[test]
    fn return_take_with_receipt_proof_binds_exact_proof() {
        let calls = recover_return_take(
            return_take(),
            MoogleMailboxReturnTake::Taken,
            Some(original_session()),
        );
        assert_eq!(calls.len(), 1);
        let (kind, metadata) = &calls[0];
        assert_eq!(*kind, "bind");
        assert_eq!(metadata["proof"], "action_recovered_inbox_v1");
        assert_eq!(metadata["reason"], "take_receipt_proven");
        assert_eq!(metadata["recovered_mid"], "993101");
        assert_eq!(metadata["expected_credits"], 90);
        assert_eq!(metadata["expected_cod"], 0);
        assert_eq!(metadata["expected_attachment_ids"], serde_json::json!(["4401"]));
        assert_eq!(metadata["receipt_attachment_count"], 2);
    }

    /// 附件已为空但领取回执不成立（integrations 判为 Unproven）：即使旧的资产观察显示附件已空，
    /// 也不绑定任何证明，保持结果未知。
    #[test]
    fn return_take_with_empty_attachments_but_no_receipt_stays_unknown() {
        assert!(
            recover_return_take(
                return_take(),
                MoogleMailboxReturnTake::Unproven,
                Some(original_session())
            )
            .is_empty()
        );
    }

    /// 仍在未领：原完整资产（含现金）仍挂在原 MID 上，只重新排队，不绑定任何证明。
    #[test]
    fn return_take_with_assets_still_present_requeues() {
        let calls = recover_return_take(
            return_take(),
            present("MoogleMail", 90, &["4401"]),
            Some(original_session()),
        );
        assert_eq!(calls, vec![("requeue", Value::from("take_assets_still_present"))]);
        // 现金退件没有装备附件也能确认“仍在未领”。
        let mut cash = return_take();
        cash.attachment_ids.clear();
        let calls =
            recover_return_take(cash, present("MoogleMail", 90, &[]), Some(original_session()));
        assert_eq!(calls, vec![("requeue", Value::from("take_assets_still_present"))]);
    }

    /// 无法判定：邮件整体缺失、资产不全、发件人不符、换凭据版本、没有观察会话、原 Take 没有冻结
    /// 凭据或证据缺少系统后缀事实都不改写任何事实；入站 Take 的缺失证明口径不变。
    #[test]
    fn return_take_undecidable_observations_leave_unknown() {
        let session = Some(original_session());
        let taken = || MoogleMailboxReturnTake::Taken;
        assert!(
            recover_return_take(return_take(), MoogleMailboxReturnTake::Missing, session.clone())
                .is_empty(),
            "缺失"
        );
        let partial = present("MoogleMail", 90, &[]);
        assert!(
            recover_return_take(return_take(), partial, session.clone()).is_empty(),
            "部分资产"
        );
        let player = present("BuyerPlayer", 90, &["4401"]);
        assert!(recover_return_take(return_take(), player, session.clone()).is_empty(), "发件人");
        let mut other_version = original_session();
        other_version.credential_version = 4;
        assert!(recover_return_take(return_take(), taken(), Some(other_version)).is_empty());
        assert!(recover_return_take(return_take(), taken(), None).is_empty(), "没有观察会话");
        let mut unbound = return_take();
        if let Some(expectation) = unbound.take_expectation.as_mut() {
            expectation.credential = None;
        }
        assert!(recover_return_take(unbound, taken(), session.clone()).is_empty(), "配置 Cookie");
        let mut no_identity = return_take();
        no_identity.return_take = None;
        assert!(
            recover_return_take(no_identity, taken(), session.clone()).is_empty(),
            "证据缺少系统后缀事实"
        );
        let mut intake = return_take();
        intake.origin_kind = "mail_intake".to_owned();
        intake.recipient = "SellerPlayer".to_owned();
        let mut maintenance = FixtureMaintenance::new(
            MoogleMailboxSentEntries {
                entries: Vec::new(),
                covered_from: None,
                complete: true,
                page_observation: None,
            },
            false,
        );
        maintenance.inbox =
            MoogleMailboxInboxObservation { assets: None, observation_credential: session };
        let store = FixtureStore::new(Arc::new(AtomicBool::new(false)), false);
        let calls = Arc::clone(&store.take_calls);
        let service = MailboxMaintenanceService::new(
            Arc::new(maintenance),
            Arc::new(store),
            Arc::new(NoopClock),
        );
        let delivery_id = intake.delivery_id;
        let mut report = MailboxMaintenanceReport::default();
        block_on(service.recover_sent(
            "main",
            &[MoogleMailboxSweepCandidate { delivery_id, send: Some(intake) }],
            Instant::now() + std::time::Duration::from_secs(60),
            &mut report,
        ))
        .expect("维护不得失败");
        let calls = calls.lock().map(|calls| calls.clone()).unwrap_or_default();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            (calls[0].0, &calls[0].1["reason"]),
            ("bind", &Value::from("inbox_mail_absent"))
        );
    }
}
