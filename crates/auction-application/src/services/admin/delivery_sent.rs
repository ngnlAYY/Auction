//! Admin 人工 Sent 证明用例。
//!
//! Admin 只能复用维护流程已写入投递的 Sent 证据；命令本身与运维 CLI 共用
//! `SentConfirmationCommand` 与同一幂等摘要，因此同 ID 同参重放仍返回原结果。
//! 证据引用用于锁内选取观察，不进入命令摘要。

use std::sync::Arc;

use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::ports::delivery::{
    DeliveryAdminError, DeliveryAdminPort, DeliverySentConfirmationPort, ManualSentEvidence,
    SentConfirmationCommand, SentConfirmationPreview, SentConfirmationResult,
};

/// 选择已采集证据后再提交人工证明的服务；不重新实现确认事务。
pub struct DeliverySentProofService {
    deliveries: Arc<dyn DeliveryAdminPort>,
    confirmation: Arc<dyn DeliverySentConfirmationPort>,
}

impl DeliverySentProofService {
    pub fn new(
        deliveries: Arc<dyn DeliveryAdminPort>,
        confirmation: Arc<dyn DeliverySentConfirmationPort>,
    ) -> Self {
        Self { deliveries, confirmation }
    }

    /// 预览与 CLI 相同的准入快照；引用不存在的证据直接拒绝，不产生副作用。
    pub async fn preview(
        &self,
        delivery_id: Uuid,
        evidence: ManualSentEvidence,
        evidence_reference: &str,
    ) -> Result<SentConfirmationPreview, ApplicationError> {
        self.require_recorded_evidence(delivery_id, &evidence, evidence_reference).await?;
        self.confirmation
            .preview_sent_confirmation(delivery_id, evidence, Some(evidence_reference))
            .await
    }

    /// 提交与 CLI 同一命令；同 `command_id` 同参由原确认事务重放原结果。
    pub async fn confirm(
        &self,
        command: SentConfirmationCommand,
        evidence_reference: &str,
    ) -> Result<SentConfirmationResult, ApplicationError> {
        command.validate()?;
        validate_reference(evidence_reference, &command.evidence.external_mail_id)?;
        // 先由确认事务查命令账本，再锁定投递查观察；观察被覆盖也不能阻断同参重放（不变量 1、2）。
        self.confirmation.confirm_sent(command, Some(evidence_reference)).await
    }

    /// 证据必须仍是该投递的同一份维护观察，匹配 MID 与详情中的观察时刻。
    /// 该检查只缩小可选项，不把观察升级为发送证明。
    async fn require_recorded_evidence(
        &self,
        delivery_id: Uuid,
        evidence: &ManualSentEvidence,
        evidence_reference: &str,
    ) -> Result<(), ApplicationError> {
        validate_reference(evidence_reference, &evidence.external_mail_id)?;
        if delivery_id.is_nil() {
            return Err(DeliveryAdminError::Invalid.into());
        }
        let detail = self.deliveries.detail(delivery_id).await?;
        // 只有维护流程写入的 Sent 观察/目击可被引用；既不接受草稿丢弃证据，也不把既有
        // 人工确认当成新证明的来源，避免循环引用。
        let matched = detail.evidence.iter().any(|item| {
            matches!(item.kind.as_str(), "mailbox_sweep" | "sent_reference_sighting")
                && item.reference == evidence_reference
                && item.external_mail_id.as_deref() == Some(evidence.external_mail_id.as_str())
                && evidence.matches_observation_time(item.observed_at)
        });
        if !matched {
            return Err(DeliveryAdminError::Stale.into());
        }
        Ok(())
    }
}

fn validate_reference(reference: &str, external_mail_id: &str) -> Result<(), ApplicationError> {
    let Some((kind, mid)) = reference.split_once(':') else {
        return Err(DeliveryAdminError::Invalid.into());
    };
    if !matches!(kind, "mailbox_sweep" | "sent_reference_sighting")
        || mid != external_mail_id
        || !auction_domain::valid_positive_decimal(mid, 64)
    {
        return Err(DeliveryAdminError::Invalid.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use auction_time::Timestamp;

    use super::*;
    use crate::ports::core::ApplicationFuture;
    use crate::ports::delivery::{
        DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminListRequest, DeliveryAdminPage,
        DeliveryAdminResult, DeliveryAdminSentEvidence, DeliveryAdminSnapshot,
        SentConfirmationPreview,
    };

    struct FakeDeliveries {
        evidence: Vec<DeliveryAdminSentEvidence>,
    }

    impl DeliveryAdminPort for FakeDeliveries {
        fn list(
            &self,
            _request: DeliveryAdminListRequest,
        ) -> ApplicationFuture<'_, DeliveryAdminPage> {
            Box::pin(async { Err(DeliveryAdminError::Unavailable.into()) })
        }

        fn detail(
            &self,
            delivery_id: Uuid,
        ) -> ApplicationFuture<'_, crate::ports::delivery::DeliveryAdminDetail> {
            let evidence = self.evidence.clone();
            Box::pin(async move {
                Ok(crate::ports::delivery::DeliveryAdminDetail {
                    delivery: snapshot(delivery_id),
                    attempts: Vec::new(),
                    evidence,
                    custody: crate::ports::delivery::DeliveryAdminCustody {
                        collected: None,
                        cod_pending: None,
                        remaining_credits: None,
                        receipt: None,
                        observed_at: None,
                        operator_confirmed_collected: false,
                    },
                    dispositions: Vec::new(),
                    obligation: None,
                    recovery: None,
                })
            })
        }

        fn execute(
            &self,
            _command: DeliveryAdminCommand,
        ) -> ApplicationFuture<'_, DeliveryAdminResult> {
            Box::pin(async { Err(DeliveryAdminError::Unavailable.into()) })
        }

        fn player_support(
            &self,
            _request: crate::ports::delivery::PlayerSupportRequest,
        ) -> ApplicationFuture<'_, crate::ports::delivery::PlayerSupportBundle> {
            Box::pin(async { Err(DeliveryAdminError::Unavailable.into()) })
        }
    }

    struct FakeConfirmation {
        submitted: Mutex<Option<(SentConfirmationCommand, String)>>,
    }

    impl DeliverySentConfirmationPort for FakeConfirmation {
        fn preview_sent_confirmation<'a>(
            &'a self,
            delivery_id: Uuid,
            _evidence: ManualSentEvidence,
            _evidence_reference: Option<&'a str>,
        ) -> ApplicationFuture<'a, SentConfirmationPreview> {
            Box::pin(async move {
                Ok(SentConfirmationPreview {
                    eligible: true,
                    blockers: Vec::new(),
                    delivery_id,
                    expected_fencing_version: 3,
                    external_reference: "mooglemail:main:send:7000001".to_owned(),
                    requires_collected_asset_acceptance: false,
                    requires_attempt_history_acceptance: false,
                    prior_unbound_attempts: false,
                    sent_before_current_attempt: false,
                    corrects_admin_failure: false,
                })
            })
        }

        fn confirm_sent<'a>(
            &'a self,
            command: SentConfirmationCommand,
            evidence_reference: Option<&'a str>,
        ) -> ApplicationFuture<'a, SentConfirmationResult> {
            let result = SentConfirmationResult {
                command_id: command.command_id,
                delivery_id: command.delivery_id,
                external_reference: "mooglemail:main:send:7000001".to_owned(),
                replayed: false,
            };
            *self.submitted.lock().expect("test mutex") =
                Some((command, evidence_reference.expect("Admin 证据引用").to_owned()));
            Box::pin(async move { Ok(result) })
        }
    }

    fn snapshot(delivery_id: Uuid) -> DeliveryAdminSnapshot {
        let time = Timestamp::from_ymd_hms(2026, 11, 2, 1, 14, 0).expect("timestamp");
        DeliveryAdminSnapshot {
            id: delivery_id,
            origin_kind: "settlement".to_owned(),
            origin_id: Uuid::nil().to_string(),
            recipient: "Sample".to_owned(),
            cod: Some(0),
            status: "manual_reconcile".to_owned(),
            template: Some("settlement_seller_payout".to_owned()),
            category: Some("settlement".to_owned()),
            world: Some("main".to_owned()),
            retirement_reason: None,
            suppression_reason: None,
            lease_expires_at: None,
            fencing_version: 3,
            attempt_count: 1,
            claimed_at: None,
            sending_at: Some(time),
            completed_at: Some(time),
            external_reference: None,
            failure_code: Some("write_unknown".to_owned()),
            reason_code: None,
            take_required: false,
            returned_mail_id: None,
            created_at: time,
            updated_at: time,
        }
    }

    fn evidence() -> ManualSentEvidence {
        ManualSentEvidence {
            schema_version: 1,
            world_id: "main".to_owned(),
            external_mail_id: "7000001".to_owned(),
            sender_uid: "1".to_owned(),
            sender_name: "Auction".to_owned(),
            recipient: "Sample".to_owned(),
            subject: "Payment".to_owned(),
            sent_minute: Timestamp::from_ymd_hms(2026, 11, 2, 1, 14, 0).expect("minute"),
            observed_at: Timestamp::from_ymd_hms(2026, 11, 2, 1, 20, 0).expect("observed"),
            original_body_digest: [1; 32],
            observed_body_digest: [1; 32],
            remaining_credits: 0,
            cod_pending: 0,
            equipment_ids: Vec::new(),
            collected: false,
            receipt: String::new(),
        }
    }

    fn recorded(mid: &str) -> DeliveryAdminSentEvidence {
        DeliveryAdminSentEvidence {
            reference: format!("mailbox_sweep:{mid}"),
            kind: "mailbox_sweep".to_owned(),
            external_mail_id: Some(mid.to_owned()),
            observed_at: Timestamp::from_ymd_hms(2026, 11, 2, 1, 20, 0),
            sent_at: None,
            recipient: Some("Sample".to_owned()),
            subject: Some("Payment".to_owned()),
            body_digest: Some([1; 32]),
            collected: Some(false),
            cod_pending: Some(0),
            remaining_credits: Some(0),
            remaining_equipment_count: Some(0),
            remaining_equipment_ids: Some(Vec::new()),
            pre_collection_equipment_ids: None,
            receipt: None,
        }
    }

    /// 只轮询立即就绪的假端口，不引入额外异步运行时依赖。
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::hint::spin_loop(),
            }
        }
    }

    fn service() -> (DeliverySentProofService, Arc<FakeConfirmation>) {
        let confirmation = Arc::new(FakeConfirmation { submitted: Mutex::new(None) });
        let deliveries = Arc::new(FakeDeliveries { evidence: vec![recorded("7000001")] });
        (DeliverySentProofService::new(deliveries, confirmation.clone()), confirmation)
    }

    fn command(delivery_id: Uuid) -> SentConfirmationCommand {
        SentConfirmationCommand {
            command_id: Uuid::from_u128(7),
            delivery_id,
            expected_fencing_version: 3,
            actor: "admin-operator".to_owned(),
            operator_identity: None,
            reason: "原 Sent 详情与冻结身份一致".to_owned(),
            evidence: evidence(),
            accept_collected_assets: false,
            accept_attempt_history: false,
        }
    }

    #[test]
    fn confirm_requires_well_formed_evidence_reference() {
        let (service, confirmation) = service();
        let delivery_id = Uuid::from_u128(9);
        for invalid in
            ["", "compose_discard:7000001", "mailbox_sweep:999", "mailbox_sweep:7000001 "]
        {
            let error = block_on(service.confirm(command(delivery_id), invalid));
            assert!(matches!(
                error,
                Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::Invalid))
            ));
        }
        assert!(confirmation.submitted.lock().expect("test mutex").is_none());
    }

    #[test]
    fn confirm_reuses_the_exact_sent_confirmation_command() {
        let (service, confirmation) = service();
        let delivery_id = Uuid::from_u128(9);
        let expected = command(delivery_id);
        let expected_digest = expected.request_digest().expect("digest");
        let result =
            block_on(service.confirm(expected.clone(), "mailbox_sweep:7000001")).expect("confirm");
        let (submitted, reference) =
            confirmation.submitted.lock().expect("test mutex").clone().expect("command");
        assert_eq!(submitted.command_id, expected.command_id);
        assert_eq!(submitted.request_digest().expect("digest"), expected_digest);
        assert_eq!(reference, "mailbox_sweep:7000001");
        assert_eq!(result.command_id, expected.command_id);
    }

    #[test]
    fn preview_with_overwritten_evidence_reports_stale() {
        let (service, _confirmation) = service();
        let delivery_id = Uuid::from_u128(9);
        let mut wrong = evidence();
        wrong.external_mail_id = "999".to_owned();
        let error = block_on(service.preview(delivery_id, wrong, "mailbox_sweep:999"));
        assert!(matches!(error, Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale))));
    }

    #[test]
    fn preview_rejects_the_old_time_when_the_same_mid_is_observed_after_collection() {
        let delivery_id = Uuid::from_u128(9);
        let before = evidence();
        let mut collected = recorded(&before.external_mail_id);
        let observed_at = Timestamp::parse_from_rfc3339("2026-11-02T01:21:00.123456789Z")
            .expect("Worker 观测时刻");
        collected.observed_at = Some(observed_at);
        collected.collected = Some(true);
        let confirmation = Arc::new(FakeConfirmation { submitted: Mutex::new(None) });
        let service = DeliverySentProofService::new(
            Arc::new(FakeDeliveries { evidence: vec![collected] }),
            confirmation,
        );
        let reference = "mailbox_sweep:7000001";
        let stale = block_on(service.preview(delivery_id, before.clone(), reference));
        assert!(matches!(stale, Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale))));

        let mut current = before;
        current.collected = true;
        current.observed_at = Timestamp::from_timestamp_micros(observed_at.timestamp_micros())
            .expect("详情采用微秒精度");
        assert!(block_on(service.preview(delivery_id, current, reference)).is_ok());
    }

    #[test]
    fn delivery_action_kind_is_stable_for_command_reuse() {
        assert_eq!(DeliveryAdminAction::ReconcileFailed.command_kind(), "reconcile");
    }
}
