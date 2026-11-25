//! 批次内寄售候选的映射、超时与安全失败分类。
//!
//! 候选处理不占用旧入站批次事务；它由应用层在自己的 deposit 事务中写决定、推进游标并
//! 释放租约。本模块只把 Worker 的两态消息转成应用端口输入，并把失败收敛为安全码。

use std::time::Instant;

use auction_application::mail::{
    ConsignmentIngressOutcome, ConsignmentIngressRequest, ConsignmentIngressService,
    ConsignmentMailCandidate, MailMessage, MailPollClaim, MailProcessingDecision, MailProcessor,
};
use auction_time::Timestamp;

use super::business::{MailProcessFailure, RoutedMail, process_with_retry, route_with_retry};
use super::diagnostics::{MailMessageDiagnostic, finalize_legacy_result};
use crate::worker::mail::poll::MailPollConsignmentCandidate;
use crate::worker::mail::poll::MailPollRuntimeConfig;
use crate::worker::mail::poll::failure::MailPollMetricContext;

/// 候选处理失败的安全分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConsignmentFailure {
    pub(super) failure_code: String,
    pub(super) safe_detail: String,
}

/// 候选处理对批次循环的下一步。
pub(super) enum CandidateStep {
    /// 寄售事务已推进游标并释放租约。
    Committed,
    /// 旧路由先归属并产出决定；调用方按旧信入站。
    Legacy(MailProcessingDecision),
    /// 未完成；游标停在候选之前。
    Failed(ConsignmentFailure),
}

/// 先跑旧路由；只有旧路由全部无法归属时才把候选交给寄售事务。
#[allow(clippy::too_many_arguments, reason = "候选处理需同时携带轮询、预算与指标上下文")]
pub(super) async fn handle_candidate(
    service: &ConsignmentIngressService,
    processor: &dyn MailProcessor,
    business: MailMessage,
    candidate: MailPollConsignmentCandidate,
    claim: &MailPollClaim,
    polled_at: Timestamp,
    next_poll_at: Timestamp,
    deadline: Instant,
    diagnostic: MailMessageDiagnostic,
    config: MailPollRuntimeConfig,
    metric_context: MailPollMetricContext,
) -> CandidateStep {
    let mut pending_diagnostics = Vec::new();
    let mut decision_diagnostic = None;
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        let failure = MailProcessFailure {
            failure_code: "consignment_processing_timeout",
            safe_detail: "寄售候选的旧路由在批次预算内未完成",
        };
        let _ = finalize_legacy_result(
            diagnostic,
            Err(failure),
            pending_diagnostics,
            decision_diagnostic,
        )
        .await;
        return CandidateStep::Failed(ConsignmentFailure {
            failure_code: failure.failure_code.to_owned(),
            safe_detail: failure.safe_detail.to_owned(),
        });
    };
    let routed = tokio::time::timeout(
        remaining,
        route_with_retry(
            processor,
            business,
            config,
            metric_context.clone(),
            &mut pending_diagnostics,
            &mut decision_diagnostic,
        ),
    )
    .await;
    match routed {
        Ok(Ok(RoutedMail::Legacy(decision))) => match finalize_legacy_result(
            diagnostic,
            Ok(decision),
            pending_diagnostics,
            decision_diagnostic,
        )
        .await
        {
            Ok(decision) => CandidateStep::Legacy(decision),
            Err(failure) => CandidateStep::Failed(ConsignmentFailure {
                failure_code: failure.failure_code.to_owned(),
                safe_detail: failure.safe_detail.to_owned(),
            }),
        },
        Ok(Ok(RoutedMail::ConsignmentCandidate(business))) => {
            // 候选由寄售事务自行落定；只有分类仍判定不适用时才继续旧路径留证。
            match process_candidate(
                service,
                Some(&business),
                candidate,
                claim,
                polled_at,
                next_poll_at,
                deadline,
            )
            .await
            {
                Ok(ConsignmentIngressOutcome::Committed(_))
                | Ok(ConsignmentIngressOutcome::Recovered(_)) => CandidateStep::Committed,
                Ok(ConsignmentIngressOutcome::Pending(reason)) => {
                    CandidateStep::Failed(ConsignmentFailure {
                        failure_code: "consignment_pending".to_owned(),
                        safe_detail: format!("寄售来信保留待读：{}", reason.as_str()),
                    })
                }
                Ok(ConsignmentIngressOutcome::NotApplicable) => {
                    let mut pending_diagnostics = Vec::new();
                    let mut decision_diagnostic = None;
                    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                        let failure = MailProcessFailure {
                            failure_code: "consignment_processing_timeout",
                            safe_detail: "寄售候选的旧路由在批次预算内未完成",
                        };
                        let _ = finalize_legacy_result(
                            diagnostic,
                            Err(failure),
                            pending_diagnostics,
                            decision_diagnostic,
                        )
                        .await;
                        return CandidateStep::Failed(ConsignmentFailure {
                            failure_code: failure.failure_code.to_owned(),
                            safe_detail: failure.safe_detail.to_owned(),
                        });
                    };
                    let processed = tokio::time::timeout(
                        remaining,
                        process_with_retry(
                            processor,
                            business,
                            config,
                            metric_context,
                            &mut pending_diagnostics,
                            &mut decision_diagnostic,
                        ),
                    )
                    .await;
                    let processed = match processed {
                        Ok(result) => result,
                        Err(elapsed) => {
                            tracing::warn!(
                                error = %elapsed,
                                "寄售候选的旧路由在批次预算内未完成"
                            );
                            Err(MailProcessFailure {
                                failure_code: "consignment_processing_timeout",
                                safe_detail: "寄售候选的旧路由在批次预算内未完成",
                            })
                        }
                    };
                    match finalize_legacy_result(
                        diagnostic,
                        processed,
                        pending_diagnostics,
                        decision_diagnostic,
                    )
                    .await
                    {
                        Ok(decision) => CandidateStep::Legacy(decision),
                        Err(failure) => CandidateStep::Failed(ConsignmentFailure {
                            failure_code: failure.failure_code.to_owned(),
                            safe_detail: failure.safe_detail.to_owned(),
                        }),
                    }
                }
                Err(failure) => CandidateStep::Failed(failure),
            }
        }
        Ok(Err(failure)) => {
            let _ = finalize_legacy_result(
                diagnostic,
                Err(failure),
                pending_diagnostics,
                decision_diagnostic,
            )
            .await;
            CandidateStep::Failed(ConsignmentFailure {
                failure_code: failure.failure_code.to_owned(),
                safe_detail: failure.safe_detail.to_owned(),
            })
        }
        Err(elapsed) => {
            tracing::warn!(error = %elapsed, "寄售候选的旧路由在批次预算内未完成");
            let failure = MailProcessFailure {
                failure_code: "consignment_processing_timeout",
                safe_detail: "寄售候选的旧路由在批次预算内未完成",
            };
            let _ = finalize_legacy_result(
                diagnostic,
                Err(failure),
                pending_diagnostics,
                decision_diagnostic,
            )
            .await;
            CandidateStep::Failed(ConsignmentFailure {
                failure_code: "consignment_processing_timeout".to_owned(),
                safe_detail: "寄售候选的旧路由在批次预算内未完成".to_owned(),
            })
        }
    }
}

/// 严格通用解析失败的候选（没有旧信 envelope）：不经旧路由，直接交寄售事务落成待隔离决定。
///
/// 不变量 6：没有旧 owner 可归属时，分类器若仍判“不适用”则失败关闭，不写任何事实。
pub(super) async fn handle_unparsed_candidate(
    service: &ConsignmentIngressService,
    candidate: MailPollConsignmentCandidate,
    claim: &MailPollClaim,
    polled_at: Timestamp,
    next_poll_at: Timestamp,
    deadline: Instant,
) -> Result<(), ConsignmentFailure> {
    match process_candidate(service, None, candidate, claim, polled_at, next_poll_at, deadline)
        .await?
    {
        ConsignmentIngressOutcome::Committed(_) | ConsignmentIngressOutcome::Recovered(_) => Ok(()),
        ConsignmentIngressOutcome::Pending(reason) => Err(ConsignmentFailure {
            failure_code: "consignment_pending".to_owned(),
            safe_detail: format!("寄售来信保留待读：{}", reason.as_str()),
        }),
        ConsignmentIngressOutcome::NotApplicable => Err(ConsignmentFailure {
            failure_code: "consignment_not_applicable".to_owned(),
            safe_detail: "无旧信解析结果的寄售候选不适用寄售分类，游标保持在候选之前".to_owned(),
        }),
    }
}

/// 在同一批次预算内把候选交给寄售事务。
pub(super) async fn process_candidate(
    service: &ConsignmentIngressService,
    message: Option<&MailMessage>,
    candidate: MailPollConsignmentCandidate,
    claim: &MailPollClaim,
    polled_at: Timestamp,
    next_poll_at: Timestamp,
    deadline: Instant,
) -> Result<ConsignmentIngressOutcome, ConsignmentFailure> {
    let candidate = ConsignmentMailCandidate {
        world_id: candidate.world_id,
        external_mail_id: candidate.external_mail_id,
        sender: candidate.sender,
        subject: candidate.subject,
        received_minute_raw: candidate.received_minute_raw,
        received_at: candidate.received_at,
        observation: candidate.observation,
        binding: candidate.binding,
        remote_observation_id: candidate.remote_observation_id,
        observed_at: candidate.observed_at,
        subject_digest: candidate.subject_digest,
        body_digest: candidate.body_digest,
        raw_count: candidate.raw_count,
        legacy_evidence: candidate.legacy_evidence,
    };
    let request = ConsignmentIngressRequest {
        message,
        candidate: &candidate,
        claim,
        polled_at,
        next_poll_at,
    };
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return Err(ConsignmentFailure {
            failure_code: "consignment_processing_timeout".to_owned(),
            safe_detail: "寄售来信处理在批次预算内未完成，入站游标保持在候选之前".to_owned(),
        });
    };
    match tokio::time::timeout(remaining, service.process(request)).await {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "寄售来信处理失败，入站游标保持在候选之前");
            Err(ConsignmentFailure {
                failure_code: "consignment_processing_failed".to_owned(),
                safe_detail: "寄售来信处理未完成，入站游标保持在候选之前".to_owned(),
            })
        }
        Err(elapsed) => {
            tracing::warn!(error = %elapsed, "寄售来信处理在批次预算内未完成");
            Err(ConsignmentFailure {
                failure_code: "consignment_processing_timeout".to_owned(),
                safe_detail: "寄售来信处理在批次预算内未完成，入站游标保持在候选之前".to_owned(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use auction_application::consignment::{
        ConsignmentError, ConsignmentFuture, ConsignmentRtsLocator, EnvelopeDisposition,
        EnvelopePreflight, EnvelopeReceipt, PollEnvelopeStore, PollEnvelopeWrite,
    };
    use auction_application::mail::{
        ConsignmentIngressService, MailDiagnosticContext, MailDiagnosticError,
        MailDiagnosticFuture, MailDiagnosticOutcome, MailDiagnosticReceipt, MailDiagnosticRecorder,
        MailMessage, MailPollClaim, MailProcessingAttempt, MailProcessingDecision,
        MailProcessingFuture, MailProcessingRoute, MailProcessor, MailRoutingFuture,
        PreparedMailEvidence,
    };
    use auction_application::settlement::{MailAccountEvidenceBinding, MailRuntimeBinding};
    use auction_domain::SettlementWorld;
    use auction_domain::consignment::OriginalMailKey;
    use auction_domain::consignment::deposit::{
        EnvelopeCapabilities, EnvelopeObservation, LegacyReturnEvidence, SenderIdentity,
    };
    use auction_observability::{WorkerLoop, noop_worker_recorder};
    use auction_time::Timestamp;
    use serde_json::json;
    use uuid::Uuid;

    use super::{CandidateStep, handle_candidate, handle_unparsed_candidate};
    use crate::worker::mail::poll::MailPollConsignmentCandidate;
    use crate::worker::mail::poll::MailPollRuntimeConfig;
    use crate::worker::mail::poll::failure::MailPollMetricContext;
    use crate::worker::mail::poll::processing::diagnostics::MailMessageDiagnostic;

    struct InvalidDecisionProcessor;

    fn invalid_decision() -> MailProcessingDecision {
        MailProcessingDecision {
            mail_kind: "not-a-mail-kind".to_owned(),
            status: "accepted".to_owned(),
            decision_code: "invalid".to_owned(),
            metadata: json!({}),
            equipment_diagnostic: None,
        }
    }

    impl MailProcessor for InvalidDecisionProcessor {
        fn process(&self, _message: MailMessage) -> MailProcessingFuture<'_> {
            Box::pin(async {
                MailProcessingAttempt {
                    result: Ok(invalid_decision()),
                    pending_equipment_diagnostic: None,
                }
            })
        }

        fn route(
            &self,
            _message: MailMessage,
            _consignment_candidate: bool,
        ) -> MailRoutingFuture<'_> {
            Box::pin(async {
                MailProcessingRoute::Legacy(Box::new(MailProcessingAttempt {
                    result: Ok(invalid_decision()),
                    pending_equipment_diagnostic: None,
                }))
            })
        }
    }

    /// 旧路由无法归属、交给寄售候选；寄售判定不适用后回到原 `process`，得到无效决定。
    struct CandidateThenInvalidProcessor;

    impl MailProcessor for CandidateThenInvalidProcessor {
        fn process(&self, _message: MailMessage) -> MailProcessingFuture<'_> {
            Box::pin(async {
                MailProcessingAttempt {
                    result: Ok(invalid_decision()),
                    pending_equipment_diagnostic: None,
                }
            })
        }

        fn route(
            &self,
            _message: MailMessage,
            _consignment_candidate: bool,
        ) -> MailRoutingFuture<'_> {
            Box::pin(async { MailProcessingRoute::ConsignmentCandidate })
        }
    }

    #[derive(Default)]
    struct RecordingRecorder {
        outcomes: Mutex<Vec<MailDiagnosticOutcome>>,
    }

    impl MailDiagnosticRecorder for RecordingRecorder {
        fn prepare_response(
            &self,
            _context: MailDiagnosticContext,
            _status: Option<u16>,
            _body: Option<&[u8]>,
        ) -> Result<PreparedMailEvidence, MailDiagnosticError> {
            Err(MailDiagnosticError::InvalidContext)
        }

        fn prepare_message(
            &self,
            context: MailDiagnosticContext,
            _message: &MailMessage,
        ) -> Result<PreparedMailEvidence, MailDiagnosticError> {
            Ok(PreparedMailEvidence::new(Uuid::new_v4(), context, vec![1], 1, "00".repeat(32)))
        }

        fn retain(
            &self,
            _prepared: PreparedMailEvidence,
            outcome: MailDiagnosticOutcome,
        ) -> MailDiagnosticFuture<'_, MailDiagnosticReceipt> {
            self.outcomes.lock().expect("诊断锁").push(outcome);
            Box::pin(async {
                Ok(MailDiagnosticReceipt {
                    evidence_id: Uuid::new_v4(),
                    plaintext_bytes: 1,
                    sha256: "00".repeat(32),
                    body_available: true,
                })
            })
        }

        fn retain_response(
            &self,
            _context: MailDiagnosticContext,
            _status: Option<u16>,
            _body: Option<&[u8]>,
            _outcome: MailDiagnosticOutcome,
        ) -> MailDiagnosticFuture<'_, MailDiagnosticReceipt> {
            Box::pin(async { Err(MailDiagnosticError::InvalidContext) })
        }
    }

    struct UnusedPollStore;

    impl PollEnvelopeStore for UnusedPollStore {
        fn load_envelope_preflight(
            &self,
            _key: OriginalMailKey,
            _observation: EnvelopeObservation,
            _sender: Option<String>,
            _rts: Option<ConsignmentRtsLocator>,
        ) -> ConsignmentFuture<'_, EnvelopePreflight> {
            Box::pin(async { Err(ConsignmentError::Unavailable) })
        }

        fn decide_envelope_in_poll(
            &self,
            _write: PollEnvelopeWrite,
        ) -> ConsignmentFuture<'_, EnvelopeReceipt> {
            Box::pin(async { Err(ConsignmentError::Unavailable) })
        }
    }

    /// 预读成功、不应被要求写入的替身：只服务“寄售判定不适用”的分支。
    struct EmptyPreflightStore;

    impl PollEnvelopeStore for EmptyPreflightStore {
        fn load_envelope_preflight(
            &self,
            _key: OriginalMailKey,
            _observation: EnvelopeObservation,
            _sender: Option<String>,
            _rts: Option<ConsignmentRtsLocator>,
        ) -> ConsignmentFuture<'_, EnvelopePreflight> {
            Box::pin(async {
                Ok(EnvelopePreflight {
                    seller_user_id: None,
                    reservations: Vec::new(),
                    existing_decision: None,
                    session_id: Uuid::from_u128(4),
                    capabilities: EnvelopeCapabilities {
                        safe_original_return: false,
                        take_ready: false,
                    },
                    now: Timestamp::UNIX_EPOCH,
                    season_window_id: None,
                    consignment_rts: None,
                })
            })
        }

        fn decide_envelope_in_poll(
            &self,
            _write: PollEnvelopeWrite,
        ) -> ConsignmentFuture<'_, EnvelopeReceipt> {
            Box::pin(async { Err(ConsignmentError::Unavailable) })
        }
    }

    fn message() -> MailMessage {
        MailMessage {
            mailbox_key: "mooglemail:main".to_owned(),
            external_mail_id: "100".to_owned(),
            world_id: "main".to_owned(),
            received_at: Timestamp::UNIX_EPOCH,
            sender: "FakeSeller".to_owned(),
            subject: "unknown".to_owned(),
            body: String::new(),
            credits: 0,
            cod: 0,
            attachments: Vec::new(),
            returnable: true,
            claimable: true,
            content_digest: [7; 32],
        }
    }

    fn claim() -> MailPollClaim {
        MailPollClaim {
            mailbox_key: "mooglemail:main".to_owned(),
            worker_id: "worker-test".to_owned(),
            claim_token: Uuid::new_v4(),
            cursor_value: Some("10".to_owned()),
            state_version: 1,
            paused: false,
            attempt_count: 1,
            lease_until: Timestamp::UNIX_EPOCH + auction_time::Duration::hours(1),
        }
    }

    fn candidate() -> MailPollConsignmentCandidate {
        MailPollConsignmentCandidate {
            world_id: "main".to_owned(),
            external_mail_id: "100".to_owned(),
            world_marker: None,
            sender: "FakeSeller".to_owned(),
            subject: "consign".to_owned(),
            received_minute_raw: "2026-01-01 03:00".to_owned(),
            received_at: Timestamp::UNIX_EPOCH,
            observation: EnvelopeObservation {
                observation_id: Uuid::from_u128(1),
                session_binding_digest: [1; 32],
                manifest_digest: [1; 32],
                parser_version: "test-parser".to_owned(),
                subject_is_consign: true,
                sender_identity: SenderIdentity::Verified,
                list_detail_consistent: true,
                asset_count: Some(0),
                duplicate_raw_eid: false,
                canonical_alias_ambiguous: false,
                unknown_attachment: false,
                credits: Some(0),
                cod: Some(0),
                returnable: true,
                takeable: Some(true),
                system_rts: false,
                legacy_auction_owner: None,
                items: Vec::new(),
            },
            binding: MailAccountEvidenceBinding {
                credential_id: Uuid::from_u128(2),
                credential_version: 1,
                runtime: MailRuntimeBinding { generation: 1 },
                authenticated_uid: "12345".to_owned(),
                player_name: "FakeBot".to_owned(),
                world: SettlementWorld::Main,
                observed_at: Timestamp::UNIX_EPOCH,
                protocol_version: "",
                digest: [1; 32],
            },
            remote_observation_id: Uuid::from_u128(3),
            observed_at: Timestamp::UNIX_EPOCH,
            subject_digest: [1; 32],
            body_digest: [1; 32],
            raw_count: Some(0),
            legacy_evidence: LegacyReturnEvidence::default(),
        }
    }

    fn metric_context() -> MailPollMetricContext {
        MailPollMetricContext {
            metrics: noop_worker_recorder(),
            external_metrics: auction_application::external_metrics::noop_external_metrics_observer(
            ),
            loop_kind: WorkerLoop::MailMain,
        }
    }

    #[tokio::test]
    async fn candidate_legacy_decision_is_validated_and_retained() {
        let recorder = Arc::new(RecordingRecorder::default());
        let business = message();
        let diagnostic = MailMessageDiagnostic::prepare(Some(recorder.clone()), &business);
        let service = ConsignmentIngressService::new(
            Arc::new(auction_application::mail::ConsignmentMailService::planner()),
            Arc::new(UnusedPollStore),
        );
        let polled_at = Timestamp::UNIX_EPOCH;
        let step = handle_candidate(
            &service,
            &InvalidDecisionProcessor,
            business,
            candidate(),
            &claim(),
            polled_at,
            polled_at + auction_time::Duration::hours(1),
            Instant::now() + Duration::from_secs(5),
            diagnostic,
            MailPollRuntimeConfig::default(),
            metric_context(),
        )
        .await;
        match step {
            CandidateStep::Failed(failure) => {
                assert_eq!(failure.failure_code, "invalid_business_decision");
            }
            CandidateStep::Legacy(decision) => {
                panic!("候选旧路由的无效决定必须失败关闭：{}", decision.decision_code)
            }
            CandidateStep::Committed => panic!("无效决定不得提交"),
        }
        let outcomes = recorder.outcomes.lock().expect("诊断锁");
        assert!(
            outcomes.iter().any(|outcome| outcome.failure_code == "invalid_business_decision"),
            "候选旧路由的无效决定必须留证"
        );
    }

    #[tokio::test]
    async fn candidate_not_applicable_legacy_decision_is_validated_and_retained() {
        let recorder = Arc::new(RecordingRecorder::default());
        let business = message();
        let diagnostic = MailMessageDiagnostic::prepare(Some(recorder.clone()), &business);
        // 主题与观察都不是 consign、也非系统 RTS：寄售判定“不适用”，回到原 process。
        let service = ConsignmentIngressService::new(
            Arc::new(auction_application::mail::ConsignmentMailService::planner()),
            Arc::new(EmptyPreflightStore),
        );
        let mut not_consignment = candidate();
        not_consignment.observation.subject_is_consign = false;
        // 进入寄售判定前要过候选身份校验：绑定须是有效的内部证据摘要，观察须引用同一摘要。
        not_consignment.binding.protocol_version =
            auction_application::settlement::INTERNAL_EVIDENCE_PROTOCOL;
        not_consignment.binding.digest = not_consignment.binding.compute_digest();
        not_consignment.observation.session_binding_digest = not_consignment.binding.digest;
        let polled_at = Timestamp::UNIX_EPOCH;
        let step = handle_candidate(
            &service,
            &CandidateThenInvalidProcessor,
            business,
            not_consignment,
            &claim(),
            polled_at,
            polled_at + auction_time::Duration::hours(1),
            Instant::now() + Duration::from_secs(5),
            diagnostic,
            MailPollRuntimeConfig::default(),
            metric_context(),
        )
        .await;
        match step {
            CandidateStep::Failed(failure) => {
                assert_eq!(failure.failure_code, "invalid_business_decision");
            }
            CandidateStep::Legacy(decision) => {
                panic!("不适用后回到旧路由的无效决定必须失败关闭：{}", decision.decision_code)
            }
            CandidateStep::Committed => panic!("无效决定不得提交"),
        }
        let outcomes = recorder.outcomes.lock().expect("诊断锁");
        assert!(
            outcomes.iter().any(|outcome| outcome.failure_code == "invalid_business_decision"),
            "不适用后回到旧路由的无效决定必须留证"
        );
    }

    /// 记录首次决定写入的替身：只验证交给存储的命令与观察，不连数据库。
    struct RecordingPollStore {
        writes: Arc<Mutex<Vec<PollEnvelopeWrite>>>,
    }

    impl PollEnvelopeStore for RecordingPollStore {
        fn load_envelope_preflight(
            &self,
            key: OriginalMailKey,
            observation: EnvelopeObservation,
            sender: Option<String>,
            rts: Option<ConsignmentRtsLocator>,
        ) -> ConsignmentFuture<'_, EnvelopePreflight> {
            EmptyPreflightStore.load_envelope_preflight(key, observation, sender, rts)
        }

        fn decide_envelope_in_poll(
            &self,
            write: PollEnvelopeWrite,
        ) -> ConsignmentFuture<'_, EnvelopeReceipt> {
            let receipt = EnvelopeReceipt::new(
                write.command_id,
                EnvelopeDisposition::Quarantined,
                Uuid::from_u128(9),
            );
            self.writes.lock().expect("写入记录").push(write);
            Box::pin(async { Ok(receipt) })
        }
    }

    /// 重复原文 EID 且详情发件人读不出：没有旧信 envelope，仍须落成待隔离决定，
    /// 发件人不进入卖家预读与原始观察，全程不触及旧业务处理器。
    #[tokio::test]
    async fn unparsed_candidate_with_unproven_sender_is_quarantined() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let service = ConsignmentIngressService::new(
            Arc::new(auction_application::mail::ConsignmentMailService::planner()),
            Arc::new(RecordingPollStore { writes: Arc::clone(&writes) }),
        );
        let mut unparsed = candidate();
        unparsed.sender = String::new();
        unparsed.observation.duplicate_raw_eid = true;
        unparsed.observation.list_detail_consistent = false;
        unparsed.observation.sender_identity = SenderIdentity::Unverified;
        unparsed.binding.protocol_version =
            auction_application::settlement::INTERNAL_EVIDENCE_PROTOCOL;
        unparsed.binding.digest = unparsed.binding.compute_digest();
        unparsed.observation.session_binding_digest = unparsed.binding.digest;
        let polled_at = Timestamp::UNIX_EPOCH;
        handle_unparsed_candidate(
            &service,
            unparsed,
            &claim(),
            polled_at,
            polled_at + auction_time::Duration::hours(1),
            Instant::now() + Duration::from_secs(5),
        )
        .await
        .expect("无旧信 envelope 的候选应提交隔离决定");
        let writes = writes.lock().expect("写入记录");
        assert_eq!(writes.len(), 1);
        let command = writes[0].command.as_ref().expect("首次决定命令");
        assert_eq!(command.decision.decision_kind(), "quarantine");
        assert_eq!(writes[0].raw.as_ref().expect("原始观察").sender, None);
        assert_eq!(writes[0].cursor_mid, "100");
    }
}
