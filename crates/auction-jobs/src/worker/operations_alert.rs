//! Worker operations alert 扫描、通知入队和终态对账。
//!
//! 这里不复制告警阈值或 delivery 状态机；阈值由 `auction-application` 计算，批次和 lease
//! 由 `OperationsAlertStore` 持久化。外部 MoogleMail 发送仍由普通 delivery Worker 负责。

use std::{sync::Arc, time::Duration};

use auction_application::{
    ApplicationError, MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation,
    MailDeliveryOrigin, MailNotificationTemplate, OperationsAlertDispatchErrorCode,
    OperationsAlertError, OperationsAlertObserveCommand, OperationsAlertPolicy,
    OperationsAlertSource, OperationsAlertSourceReader, OperationsAlertStore,
    OperationsAlertTarget, evaluate_snapshot, request_hash,
};
use auction_domain::CommandId;
use chrono::{DateTime, Duration as ChronoDuration, Utc};

/// 三个 operations alert 循环共享的调度与 deadline 边界。
#[derive(Debug, Clone, Copy)]
pub struct OperationsAlertRuntimeConfig {
    pub poll_interval: Duration,
    pub scan_timeout: Duration,
    pub dispatch_timeout: Duration,
    pub reconcile_timeout: Duration,
    pub reconcile_limit: u16,
}

impl OperationsAlertRuntimeConfig {
    fn validate(self) -> bool {
        !self.poll_interval.is_zero()
            && self.poll_interval <= Duration::from_secs(86_400)
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
    pub delivery: Arc<dyn MailDeliveryEnqueuer>,
    pub policy: OperationsAlertPolicy,
    pub target: OperationsAlertTarget,
    pub config: OperationsAlertRuntimeConfig,
}

impl OperationsAlertRuntime {
    /// 建立并校验告警运行时；端口只接受应用层窄接口。
    pub fn new(
        source: Arc<dyn OperationsAlertSourceReader>,
        store: Arc<dyn OperationsAlertStore>,
        delivery: Arc<dyn MailDeliveryEnqueuer>,
        policy: OperationsAlertPolicy,
        target: OperationsAlertTarget,
        config: OperationsAlertRuntimeConfig,
    ) -> Result<Self, ApplicationError> {
        policy.validate().map_err(ApplicationError::from)?;
        target.validate().map_err(ApplicationError::from)?;
        if !config.validate() || config.dispatch_timeout >= policy.claim_lease {
            return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
        }
        Ok(Self { source, store, delivery, policy, target, config })
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
    ) -> Vec<auction_application::OperationsAlertSourceObservation> {
        let source_result =
            tokio::time::timeout(self.policy.source_timeout, self.source.snapshot(now)).await;
        let snapshot = match source_result {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "operations alert source snapshot unavailable");
                return unavailable_observations();
            }
            Err(_) => {
                tracing::warn!("operations alert source snapshot timed out");
                return unavailable_observations();
            }
        };
        match evaluate_snapshot(&snapshot, &self.policy) {
            Ok(observations) => observations,
            Err(error) => {
                tracing::warn!(error = %error, "operations alert source snapshot failed validation");
                unavailable_observations()
            }
        }
    }

    /// 执行一轮持久化通知批次分发。
    pub async fn dispatch(&self, now: DateTime<Utc>) -> Result<(), ApplicationError> {
        let lease_until = checked_add(now, self.policy.claim_lease)?;
        let request = auction_application::OperationsAlertClaimRequest {
            target: self.target.clone(),
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
            &self.target,
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
            world_id: self.target.world_id.clone(),
            external_mail_id: format!("operations-alert:{batch_id}"),
            recipient: self.target.recipient.clone(),
            template: Some(MailNotificationTemplate::OperationsAlert),
            result_code: Some("operations_alert".to_owned()),
            user_key_user_id: None,
            credits: 0,
            cod: 0,
            attachment_ids: Vec::new(),
            notification_body: Some(claim.summary.clone()),
        };
        match self.delivery.enqueue(command).await {
            Ok(result) if result.delivery_id == claim.delivery_id => {
                self.store.mark_enqueued(claim, self.policy.cooldown).await?;
            }
            Ok(_) => {
                self.fail_claim(claim, OperationsAlertDispatchErrorCode::DependencyMismatch)
                    .await?;
            }
            Err(error) => {
                tracing::warn!(error = %error, "operations alert delivery enqueue failed");
                self.fail_claim(claim, OperationsAlertDispatchErrorCode::Persistence).await?;
            }
        }
        Ok(())
    }

    async fn fail_claim(
        &self,
        claim: auction_application::OperationsAlertBatchClaim,
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

fn unavailable_observations() -> Vec<auction_application::OperationsAlertSourceObservation> {
    [
        OperationsAlertSource::Process,
        OperationsAlertSource::Delivery,
        OperationsAlertSource::Fulfillment,
        OperationsAlertSource::Recovery,
    ]
    .into_iter()
    .filter_map(|source| match auction_application::OperationsAlertSourceObservation::unavailable(source) {
        Ok(observation) => Some(observation),
        Err(error) => {
            tracing::error!(error = %error, source = %source, "operations alert unavailable observation construction failed");
            None
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
