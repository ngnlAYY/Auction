//! Worker operations alert 扫描、通知入队和终态对账；阈值由 `auction-application`
//! 计算，批次与 lease 由 `OperationsAlertStore` 持久化，发送仍由普通 delivery Worker
//! 负责。

use std::{sync::Arc, time::Duration};

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError,
};
use auction_application::mail::{
    MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
    MailNotificationTemplate,
};
use auction_application::operations::{
    OperationsAlertDispatchErrorCode, OperationsAlertError, OperationsAlertObserveCommand,
    OperationsAlertPolicy, OperationsAlertSource, OperationsAlertSourceReader,
    OperationsAlertStore, OperationsAlertTarget, evaluate_snapshot, request_hash,
};
use auction_domain::CommandId;
use chrono::{DateTime, Duration as ChronoDuration, Utc};

use crate::worker::diagnostics;

/// 三个 operations alert 循环共享的调度与 deadline 边界。
#[derive(Debug, Clone, Copy)]
pub struct OperationsAlertRuntimeConfig {
    pub poll_interval: Duration,
    pub jitter: Duration,
    pub scan_timeout: Duration,
    pub dispatch_timeout: Duration,
    pub reconcile_timeout: Duration,
    pub reconcile_limit: u16,
}

impl OperationsAlertRuntimeConfig {
    fn validate(self) -> bool {
        !self.poll_interval.is_zero()
            && self.poll_interval <= Duration::from_secs(86_400)
            && self.jitter <= self.poll_interval
            && !self.scan_timeout.is_zero()
            && self.scan_timeout <= Duration::from_secs(86_400)
            && !self.dispatch_timeout.is_zero()
            && self.dispatch_timeout <= Duration::from_secs(86_400)
            && !self.reconcile_timeout.is_zero()
            && self.reconcile_timeout <= Duration::from_secs(86_400)
            && (1..=64).contains(&self.reconcile_limit)
    }
}

/// Worker 运行时使用的 operations alert 端口组合。
pub struct OperationsAlertRuntime {
    pub source: Arc<dyn OperationsAlertSourceReader>,
    pub store: Arc<dyn OperationsAlertStore>,
    /// 只有主动通知启用时注入；扫描/告警台模式为 `None`。
    pub delivery: Option<Arc<dyn MailDeliveryEnqueuer>>,
    pub policy: OperationsAlertPolicy,
    /// 只有主动通知启用时注入；扫描/告警台模式为 `None`。
    pub target: Option<OperationsAlertTarget>,
    pub config: OperationsAlertRuntimeConfig,
}

impl OperationsAlertRuntime {
    /// 建立并校验告警运行时；扫描与通知能力可独立存在，但通知必须同时具备 delivery 与 target。
    pub fn new(
        source: Arc<dyn OperationsAlertSourceReader>,
        store: Arc<dyn OperationsAlertStore>,
        delivery: Option<Arc<dyn MailDeliveryEnqueuer>>,
        policy: OperationsAlertPolicy,
        target: Option<OperationsAlertTarget>,
        config: OperationsAlertRuntimeConfig,
    ) -> Result<Self, ApplicationError> {
        policy.validate().map_err(ApplicationError::from)?;
        if delivery.is_some() != target.is_some() {
            return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
        }
        if let Some(target) = &target {
            target.validate().map_err(ApplicationError::from)?;
        }
        if !config.validate() || (target.is_some() && config.dispatch_timeout >= policy.claim_lease)
        {
            return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
        }
        Ok(Self { source, store, delivery, policy, target, config })
    }

    /// 是否启用主动 MoogleMail 通知；扫描/告警台模式为 false。
    pub const fn notification_enabled(&self) -> bool {
        self.target.is_some()
    }

    /// 执行一轮来源观察；失败不阻塞独立的 dispatch/reconcile 循环。
    pub async fn scan(&self, now: DateTime<Utc>) -> Result<(), ApplicationError> {
        let observations = self.observe(now).await;
        self.store
            .observe(OperationsAlertObserveCommand {
                observations,
                now,
                batch_window: self.policy.batch_window,
            })
            .await
    }

    async fn observe(
        &self,
        now: DateTime<Utc>,
    ) -> Vec<auction_application::operations::OperationsAlertSourceObservation> {
        let source_result =
            tokio::time::timeout(self.policy.source_timeout, self.source.snapshot(now)).await;
        let snapshot = match source_result {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                diagnostics::warn(
                    "operations_alert",
                    "read_sources",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                return unavailable_observations();
            }
            Err(error) => {
                diagnostics::warn(
                    "operations_alert",
                    "read_sources_timeout",
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                return unavailable_observations();
            }
        };
        match evaluate_snapshot(&snapshot, &self.policy) {
            Ok(observations) => observations,
            Err(error) => {
                diagnostics::warn(
                    "operations_alert",
                    "evaluate_sources",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                unavailable_observations()
            }
        }
    }

    pub async fn dispatch(&self, now: DateTime<Utc>) -> Result<(), ApplicationError> {
        let (Some(delivery), Some(target)) = (&self.delivery, &self.target) else {
            return Ok(());
        };
        let lease_until = checked_add(now, self.policy.claim_lease)?;
        let request = auction_application::operations::OperationsAlertClaimRequest {
            target: target.clone(),
            now,
            lease_until,
            max_items: self.policy.max_batch_items,
            max_body_bytes: self.policy.max_body_bytes,
        };
        let Some(claim) = self.store.claim_due_batch(request).await? else {
            return Ok(());
        };
        claim.validate(self.policy.max_body_bytes).map_err(ApplicationError::from)?;
        let expected_request_hash = request_hash(
            claim.batch_id,
            claim.delivery_id,
            claim.command_id,
            target,
            &claim.summary,
        )?;
        if expected_request_hash != claim.request_hash {
            self.fail_claim(claim, OperationsAlertDispatchErrorCode::DependencyMismatch).await?;
            return Ok(());
        }
        let batch_id = claim.batch_id;
        let command = MailDeliveryCommand {
            command_id: CommandId::from_uuid(claim.command_id),
            request_hash: claim.request_hash,
            origin: MailDeliveryOrigin::OperationsAlert,
            origin_id: format!("operations_alert:{batch_id}"),
            operation: MailDeliveryOperation::Send,
            world_id: target.world_id.clone(),
            external_mail_id: format!("operations-alert:{batch_id}"),
            recipient: target.recipient.clone(),
            template: Some(MailNotificationTemplate::OperationsAlert),
            result_code: Some("operations_alert".to_owned()),
            user_key_user_id: None,
            credits: 0,
            cod: 0,
            attachment_ids: Vec::new(),
            notification_body: Some(claim.summary.clone()),
        };
        match delivery.enqueue(command).await {
            Ok(result) if result.delivery_id == claim.delivery_id => {
                self.store.mark_enqueued(claim, self.policy.cooldown).await?;
            }
            Ok(_) => {
                self.fail_claim(claim, OperationsAlertDispatchErrorCode::DependencyMismatch)
                    .await?;
            }
            Err(error) => {
                diagnostics::warn(
                    "operations_alert",
                    "enqueue_delivery",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                self.fail_claim(claim, OperationsAlertDispatchErrorCode::Persistence).await?;
            }
        }
        Ok(())
    }

    async fn fail_claim(
        &self,
        claim: auction_application::operations::OperationsAlertBatchClaim,
        error_code: OperationsAlertDispatchErrorCode,
    ) -> Result<(), ApplicationError> {
        self.store.mark_claim_failed(claim, error_code, self.policy.retry_delay).await
    }

    /// 对账一批已经进入 delivery 的通知，并返回本轮处理数量。
    pub async fn reconcile(
        &self,
        now: DateTime<Utc>,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        if !(1..=64).contains(&limit) {
            return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
        }
        let batches = self.store.list_enqueued(limit).await?;
        let processed = batches.len();
        for batch in batches {
            let status = batch.delivery_status;
            self.store.mark_terminal(batch, status, now).await?;
        }
        Ok(processed)
    }
}

fn unavailable_observations()
-> Vec<auction_application::operations::OperationsAlertSourceObservation> {
    // 来源闭集由应用层唯一维护；新增来源只需扩展闭集，这里不再重复名单。
    OperationsAlertSource::ALL
        .into_iter()
        .filter_map(|source| {
            match auction_application::operations::OperationsAlertSourceObservation::unavailable(
                source,
            ) {
                Ok(observation) => Some(observation),
                Err(error) => {
                    diagnostics::error(
                        "operations_alert",
                        "build_unavailable_observation",
                        error.diagnostic_kind(),
                        DiagnosticContext {
                            disposition: Some(DiagnosticDisposition::Unpersisted),
                            ..Default::default()
                        },
                    );
                    None
                }
            }
        })
        .collect()
}

fn checked_add(now: DateTime<Utc>, duration: Duration) -> Result<DateTime<Utc>, ApplicationError> {
    let duration = ChronoDuration::from_std(duration)
        .map_err(|_| ApplicationError::OperationsAlert(OperationsAlertError::Invalid))?;
    now.checked_add_signed(duration)
        .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::Invalid))
}
