//! MoogleMail Worker 的远程读取编排。
//!
//! 每个已配置 world 都拥有独立调度器、唤醒器和持久化邮箱状态。本模块只负责
//! claim → 续租 → 远程读取 → 安全事实映射 → commit/fail；HTTP、SQL 和 MoogleMail HTML
//! 解析分别由应用端口、存储适配器和 integrations owner 提供。

use std::{future::Future, pin::Pin, sync::Arc};

use auction_application::{
    Clock, MAX_MAIL_POLL_BATCH_SIZE, MailMessage, MailPollBatch, MailPollClaim, MailPollIntake,
    MailPollStore, MailProcessingDecision, MailProcessor, RecurringSchedule,
};
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult, WorkerRetryKind};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::json;
use tokio::sync::Notify;
use uuid::Uuid;

use super::{PersistentWorker, WorkerError, poll_delay};

/// 一次远程 MoogleMail 读取请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailPollFetchRequest {
    pub mailbox_key: String,
    pub world_id: String,
    pub cursor_value: Option<String>,
    pub limit: u16,
}

/// 已由 integrations 解析完成、等待业务处理器消费的 MoogleMail 邮件。
///
/// 摘要字段用于入站幂等和安全日志；`business` 只在 Worker 进程内短暂携带正文和装备 key，
/// 绝不进入 `MailPollBatch` 或数据库。
pub struct MailPollFetchedMessage {
    pub external_mail_id: String,
    pub world_id: String,
    pub received_at: DateTime<Utc>,
    pub subject: String,
    pub content_digest: [u8; 32],
    pub attachment_count: u32,
    pub credits: i64,
    pub cod: i64,
    pub returnable: bool,
    pub claimable: bool,
    pub business: Option<MailMessage>,
}

impl std::fmt::Debug for MailPollFetchedMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MailPollFetchedMessage")
            .field("external_mail_id", &self.external_mail_id)
            .field("world_id", &self.world_id)
            .field("received_at", &self.received_at)
            .field("subject", &"[REDACTED]")
            .field("content_digest", &self.content_digest)
            .field("attachment_count", &self.attachment_count)
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("returnable", &self.returnable)
            .field("claimable", &self.claimable)
            .field("business", &self.business.as_ref().map(|_| "[PRESENT]"))
            .finish()
    }
}

/// 一次远程 MoogleMail 读取结果。
#[derive(Debug)]
pub struct MailPollPage {
    pub mailbox_key: String,
    pub world_id: String,
    pub next_cursor: Option<String>,
    pub messages: Vec<MailPollFetchedMessage>,
}

/// 远程 MoogleMail 读取失败的安全分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailPollGatewayError {
    pub failure_code: String,
    pub safe_detail: String,
    pub temporary: bool,
}

/// integrations 适配器实现的窄远程读取端口。
pub trait MailPollGateway: Send + Sync {
    fn fetch(
        &self,
        request: MailPollFetchRequest,
    ) -> Pin<Box<dyn Future<Output = Result<MailPollPage, MailPollGatewayError>> + Send + '_>>;
}

/// 单个 MoogleMail 邮箱的独立调度、超时、续租与重试边界。
#[derive(Debug, Clone, Copy)]
pub struct MailPollRuntimeConfig {
    pub batch_size: u16,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
    pub renew_interval: std::time::Duration,
    pub fetch_attempts: u16,
    pub fetch_timeout: std::time::Duration,
    pub process_attempts: u16,
    pub item_timeout: std::time::Duration,
    pub retry_base: std::time::Duration,
    pub retry_maximum: std::time::Duration,
}

impl Default for MailPollRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(60),
            renew_interval: std::time::Duration::from_secs(10),
            fetch_attempts: 3,
            fetch_timeout: std::time::Duration::from_secs(30),
            process_attempts: 3,
            item_timeout: std::time::Duration::from_secs(30),
            retry_base: std::time::Duration::from_secs(1),
            retry_maximum: std::time::Duration::from_secs(30),
        }
    }
}

impl MailPollRuntimeConfig {
    fn validate(self) -> bool {
        self.batch_size > 0
            && self.batch_size <= MAX_MAIL_POLL_BATCH_SIZE
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && self.lease_duration <= std::time::Duration::from_secs(86_400)
            && !self.renew_interval.is_zero()
            && self.renew_interval < self.lease_duration
            && (1..=10).contains(&self.fetch_attempts)
            && !self.fetch_timeout.is_zero()
            && self.fetch_timeout <= std::time::Duration::from_secs(300)
            && (1..=10).contains(&self.process_attempts)
            && !self.item_timeout.is_zero()
            && self.item_timeout <= std::time::Duration::from_secs(300)
            && !self.retry_base.is_zero()
            && self.retry_base <= self.retry_maximum
            && self.retry_maximum <= std::time::Duration::from_secs(300)
    }
}

/// Worker 中单个 MoogleMail world 的依赖与独立唤醒器。
#[derive(Clone)]
pub struct MailPollRuntime {
    pub store: Arc<dyn MailPollStore>,
    pub gateway: Arc<dyn MailPollGateway>,
    pub processor: Option<Arc<dyn MailProcessor>>,
    pub world_id: String,
    pub mailbox_key: String,
    pub config: MailPollRuntimeConfig,
    schedule: Arc<dyn RecurringSchedule>,
    wakeup: Arc<Notify>,
}

impl MailPollRuntime {
    /// 建立一个已经校验、可独立运行的邮箱轮询器。
    pub fn new(
        store: Arc<dyn MailPollStore>,
        gateway: Arc<dyn MailPollGateway>,
        processor: Option<Arc<dyn MailProcessor>>,
        world_id: String,
        mailbox_key: String,
        schedule: Arc<dyn RecurringSchedule>,
        config: MailPollRuntimeConfig,
    ) -> Result<Self, WorkerError> {
        let runtime = Self {
            store,
            gateway,
            processor,
            world_id,
            mailbox_key,
            config,
            schedule,
            wakeup: Arc::new(Notify::new()),
        };
        if !runtime.validate() {
            return Err(WorkerError::InvalidMailPollConfiguration);
        }
        Ok(runtime)
    }

    /// 合并一次手动触发；轮询正在执行时，通知会保留到下一轮领取。
    pub(crate) fn trigger(&self) {
        self.wakeup.notify_one();
    }

    fn validate(&self) -> bool {
        matches!(self.world_id.as_str(), "main" | "isekai")
            && !self.mailbox_key.is_empty()
            && self.mailbox_key == self.mailbox_key.trim()
            && self.mailbox_key.len() <= 256
            && !self.mailbox_key.chars().any(char::is_control)
            && self.schedule.next_after(Utc::now()).is_some()
            && self.config.validate()
    }

    fn loop_kind(&self) -> WorkerLoop {
        if self.world_id == "isekai" { WorkerLoop::MailIsekai } else { WorkerLoop::MailMain }
    }
}

/// 按旧版语义独立运行一个邮箱：等待下一 cron 时刻与 `[0, jitter]` 后轮询，手动触发跳过等待和 due 时间。
pub(crate) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    runtime: MailPollRuntime,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    loop {
        if *shutdown.borrow() {
            return;
        }
        let now = worker.clock.now();
        let Some(next_wake_at) = runtime.schedule.next_after(now) else {
            tracing::error!(
                world_id = %runtime.world_id,
                mailbox_key = %runtime.mailbox_key,
                "MoogleMail cron 无法计算下一次触发时刻，停止该邮箱循环"
            );
            return;
        };
        let Ok(schedule_delay) = next_wake_at.signed_duration_since(now).to_std() else {
            tracing::error!(
                world_id = %runtime.world_id,
                mailbox_key = %runtime.mailbox_key,
                "MoogleMail cron 返回了非未来触发时刻，停止该邮箱循环"
            );
            return;
        };
        let wait = poll_delay(schedule_delay, runtime.config.jitter);
        let scheduled_wake = tokio::time::Instant::now() + wait;
        let scheduled_sleep = tokio::time::sleep_until(scheduled_wake);
        tokio::pin!(scheduled_sleep);
        let force = tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
            () = &mut scheduled_sleep => {
                worker.metrics.observe_schedule_delay(
                    runtime.loop_kind(),
                    tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
                );
                false
            }
            () = runtime.wakeup.notified() => true,
        };

        if let Err(error) = run_cycle(worker.as_ref(), &runtime, force).await {
            tracing::warn!(
                error = %error,
                world_id = %runtime.world_id,
                mailbox_key = %runtime.mailbox_key,
                "MoogleMail 邮箱轮询失败；该邮箱将在自身下一轮重试"
            );
        }
    }
}

async fn run_cycle<C>(
    worker: &PersistentWorker<C>,
    runtime: &MailPollRuntime,
    force: bool,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let loop_kind = runtime.loop_kind();
    let iteration_started = std::time::Instant::now();
    let result = run_cycle_inner(worker, runtime, force).await;
    let metric_result = match &result {
        Ok(result) => *result,
        Err(_) => WorkerResult::Error,
    };
    worker.metrics.observe_iteration(loop_kind, metric_result, iteration_started.elapsed());
    result.map(|_| ())
}

async fn run_cycle_inner<C>(
    worker: &PersistentWorker<C>,
    runtime: &MailPollRuntime,
    force: bool,
) -> Result<WorkerResult, WorkerError>
where
    C: Clock + 'static,
{
    if !runtime.validate() {
        return Err(WorkerError::InvalidMailPollConfiguration);
    }
    let now = worker.clock.now();
    let lease_duration = ChronoDuration::from_std(runtime.config.lease_duration)
        .map_err(|_| WorkerError::InvalidMailPollConfiguration)?;
    let lease_until =
        now.checked_add_signed(lease_duration).ok_or(WorkerError::InvalidMailPollConfiguration)?;
    // 一个 runtime 只拥有一个 mailbox 状态；领取上限固定为一，避免让未来异常数据形成预领取队列。
    let claims = runtime
        .store
        .claim_due(
            runtime.mailbox_key.clone(),
            worker.worker_id.clone(),
            now,
            lease_until,
            1,
            force,
        )
        .await?;
    if claims.is_empty() {
        return Ok(WorkerResult::Success);
    }

    let context = MailPollProcessContext {
        store: Arc::clone(&runtime.store),
        gateway: Arc::clone(&runtime.gateway),
        processor: runtime.processor.clone(),
        clock: Arc::clone(&worker.clock),
        world_id: runtime.world_id.clone(),
        mailbox_key: runtime.mailbox_key.clone(),
        config: runtime.config,
        schedule: Arc::clone(&runtime.schedule),
        metrics: Arc::clone(&worker.metrics),
        loop_kind: runtime.loop_kind(),
    };
    let mut iteration_result = WorkerResult::Success;
    for claim in claims {
        let item_result = process_claim(context.clone(), claim).await;
        super::merge_worker_result(&mut iteration_result, item_result);
    }
    Ok(iteration_result)
}

struct MailPollProcessContext<C> {
    store: Arc<dyn MailPollStore>,
    gateway: Arc<dyn MailPollGateway>,
    processor: Option<Arc<dyn MailProcessor>>,
    clock: Arc<C>,
    world_id: String,
    mailbox_key: String,
    config: MailPollRuntimeConfig,
    schedule: Arc<dyn RecurringSchedule>,
    metrics: Arc<dyn WorkerRecorder>,
    loop_kind: WorkerLoop,
}

impl<C> Clone for MailPollProcessContext<C> {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            gateway: Arc::clone(&self.gateway),
            processor: self.processor.clone(),
            clock: Arc::clone(&self.clock),
            world_id: self.world_id.clone(),
            mailbox_key: self.mailbox_key.clone(),
            config: self.config,
            schedule: Arc::clone(&self.schedule),
            metrics: Arc::clone(&self.metrics),
            loop_kind: self.loop_kind,
        }
    }
}

/// 在整封邮件处理期间维护数据库租约；续租失败时立即取消本地 future，并由 fence 阻止陈旧提交。
async fn process_claim<C>(context: MailPollProcessContext<C>, claim: MailPollClaim) -> WorkerResult
where
    C: Clock + 'static,
{
    if !claim.validate() || claim.mailbox_key != context.mailbox_key {
        tracing::error!("MoogleMail Worker 收到无效租约");
        return WorkerResult::Error;
    }
    let Ok(lease_duration) = ChronoDuration::from_std(context.config.lease_duration) else {
        return WorkerResult::Error;
    };
    let store = Arc::clone(&context.store);
    let clock = Arc::clone(&context.clock);
    let renew_timeout = context.config.renew_interval;
    let mut renewal_claim = claim.clone();
    let work = process_claim_inner(context, claim);
    tokio::pin!(work);

    loop {
        let renewal_sleep = tokio::time::sleep(renew_timeout);
        tokio::pin!(renewal_sleep);
        tokio::select! {
            result = &mut work => return result,
            () = &mut renewal_sleep => {
                let now = clock.now();
                let Some(lease_until) = now.checked_add_signed(lease_duration) else {
                    tracing::error!(mailbox_key = %renewal_claim.mailbox_key, "MoogleMail 续租截止时间溢出");
                    return WorkerResult::Error;
                };
                match tokio::time::timeout(
                    renew_timeout,
                    store.renew(renewal_claim.clone(), now, lease_until),
                )
                .await
                {
                    Ok(Ok(updated)) => renewal_claim = updated,
                    Ok(Err(error)) => {
                        tracing::error!(
                            error = %error,
                            mailbox_key = %renewal_claim.mailbox_key,
                            "MoogleMail 租约续期失败，取消当前陈旧处理"
                        );
                        return WorkerResult::Error;
                    }
                    Err(_) => {
                        tracing::error!(
                            mailbox_key = %renewal_claim.mailbox_key,
                            "MoogleMail 租约续期超时，取消当前陈旧处理"
                        );
                        return WorkerResult::Error;
                    }
                }
            }
        }
    }
}

async fn process_claim_inner<C>(
    context: MailPollProcessContext<C>,
    claim: MailPollClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    let MailPollProcessContext {
        store,
        gateway,
        processor,
        clock,
        world_id,
        mailbox_key: _,
        config,
        schedule,
        metrics,
        loop_kind,
    } = context;
    let metric_context = MailPollMetricContext { metrics: Arc::clone(&metrics), loop_kind };
    let failure_context = MailPollFailureContext {
        schedule: Arc::clone(&schedule),
        config,
        metric_context: metric_context.clone(),
    };
    let request = MailPollFetchRequest {
        mailbox_key: claim.mailbox_key.clone(),
        world_id: world_id.clone(),
        cursor_value: claim.cursor_value.clone(),
        limit: config.batch_size,
    };
    let polled_at = clock.now();
    let page =
        match fetch_with_retry(gateway.as_ref(), request, config, metric_context.clone()).await {
            Ok(page) => page,
            Err(error) => {
                record_failure(
                    store,
                    clock,
                    claim,
                    safe_failure_code(&error.failure_code),
                    safe_failure_detail(&error.safe_detail),
                    failure_context,
                )
                .await;
                return WorkerResult::Error;
            }
        };
    if !valid_page(&page, &claim, &world_id, config.batch_size) {
        record_failure(
            store,
            clock,
            claim,
            "invalid_gateway_page".to_owned(),
            "MoogleMail gateway 返回的批次不符合安全边界".to_owned(),
            failure_context,
        )
        .await;
        return WorkerResult::Error;
    }
    metrics.observe_batch(loop_kind, page.messages.len(), usize::from(config.batch_size));
    let Some(next_poll_at) = schedule.next_after(polled_at) else {
        record_failure(
            Arc::clone(&store),
            Arc::clone(&clock),
            claim,
            "schedule_failed".to_owned(),
            "MoogleMail cron 无法计算下一次触发时刻".to_owned(),
            failure_context,
        )
        .await;
        return WorkerResult::Error;
    };
    let mut items = Vec::with_capacity(page.messages.len());
    for mut message in page.messages {
        let decision = match (processor.as_ref(), message.business.take()) {
            (Some(processor), Some(business)) => {
                let decision = match process_with_retry(
                    processor.as_ref(),
                    business,
                    config,
                    metric_context.clone(),
                )
                .await
                {
                    Ok(decision) => decision,
                    Err(failure) => {
                        record_failure(
                            Arc::clone(&store),
                            Arc::clone(&clock),
                            claim,
                            failure.failure_code.to_owned(),
                            failure.safe_detail.to_owned(),
                            failure_context.clone(),
                        )
                        .await;
                        return WorkerResult::Error;
                    }
                };
                if decision.validate().is_err() {
                    record_failure(
                        Arc::clone(&store),
                        Arc::clone(&clock),
                        claim,
                        "invalid_business_decision".to_owned(),
                        "MoogleMail 业务处理器返回了无效决定".to_owned(),
                        failure_context.clone(),
                    )
                    .await;
                    return WorkerResult::Error;
                }
                decision
            }
            (Some(_), None) => {
                record_failure(
                    Arc::clone(&store),
                    Arc::clone(&clock),
                    claim,
                    "business_payload_missing".to_owned(),
                    "MoogleMail 网关未提供业务 envelope，入站游标保持不变".to_owned(),
                    failure_context.clone(),
                )
                .await;
                return WorkerResult::Error;
            }
            (None, Some(_)) | (None, None) => pending_decision(&message),
        };
        items.push(intake_from_message(&claim.mailbox_key, message, decision, polled_at));
    }
    let batch = MailPollBatch {
        mailbox_key: claim.mailbox_key.clone(),
        claim_token: claim.claim_token,
        next_cursor: page.next_cursor,
        polled_at,
        next_poll_at,
        items,
    };
    if let Err(error) = store.commit(claim.clone(), batch).await {
        tracing::error!(
            error = %error,
            mailbox_key = %claim.mailbox_key,
            "MoogleMail Worker 提交入站批次失败"
        );
        record_failure(
            store,
            clock,
            claim,
            "commit_failed".to_owned(),
            "MoogleMail 入站批次未能确认提交".to_owned(),
            failure_context,
        )
        .await;
        return WorkerResult::Error;
    }
    WorkerResult::Success
}

async fn fetch_with_retry(
    gateway: &dyn MailPollGateway,
    request: MailPollFetchRequest,
    config: MailPollRuntimeConfig,
    metric_context: MailPollMetricContext,
) -> Result<MailPollPage, MailPollGatewayError> {
    for attempt in 1..=config.fetch_attempts {
        let result =
            tokio::time::timeout(config.fetch_timeout, gateway.fetch(request.clone())).await;
        match result {
            Ok(Ok(page)) => return Ok(page),
            Ok(Err(error)) if error.temporary && attempt < config.fetch_attempts => {
                metric_context
                    .metrics
                    .observe_retry(metric_context.loop_kind, WorkerRetryKind::Inline);
                tokio::time::sleep(retry_delay(
                    config.retry_base,
                    config.retry_maximum,
                    u32::from(attempt),
                ))
                .await;
            }
            Ok(Err(error)) => return Err(error),
            Err(_) if attempt < config.fetch_attempts => {
                metric_context
                    .metrics
                    .observe_retry(metric_context.loop_kind, WorkerRetryKind::Inline);
                tokio::time::sleep(retry_delay(
                    config.retry_base,
                    config.retry_maximum,
                    u32::from(attempt),
                ))
                .await;
            }
            Err(_) => {
                return Err(MailPollGatewayError {
                    failure_code: "fetch_timeout".to_owned(),
                    safe_detail: "MoogleMail 远程读取在总时限内未完成".to_owned(),
                    temporary: true,
                });
            }
        }
    }
    Err(MailPollGatewayError {
        failure_code: "fetch_failed".to_owned(),
        safe_detail: "MoogleMail 远程读取未完成".to_owned(),
        temporary: true,
    })
}

struct MailProcessFailure {
    failure_code: &'static str,
    safe_detail: &'static str,
}

async fn process_with_retry(
    processor: &dyn MailProcessor,
    business: MailMessage,
    config: MailPollRuntimeConfig,
    metric_context: MailPollMetricContext,
) -> Result<MailProcessingDecision, MailProcessFailure> {
    let mut original = Some(business);
    for attempt in 1..=config.process_attempts {
        // 非最终尝试只建立一个显式短生命周期副本；成功或失败返回时，未使用的原 envelope 会清零。
        let message = if attempt == config.process_attempts {
            let Some(message) = original.take() else {
                return Err(MailProcessFailure {
                    failure_code: "business_retry_state_invalid",
                    safe_detail: "MoogleMail 业务重试状态无效",
                });
            };
            message
        } else {
            let Some(message) = original.as_ref() else {
                return Err(MailProcessFailure {
                    failure_code: "business_retry_state_invalid",
                    safe_detail: "MoogleMail 业务重试状态无效",
                });
            };
            message.duplicate_for_retry()
        };
        let result = tokio::time::timeout(config.item_timeout, processor.process(message)).await;
        match result {
            Ok(Ok(decision)) => return Ok(decision),
            Ok(Err(_)) if attempt < config.process_attempts => {
                metric_context
                    .metrics
                    .observe_retry(metric_context.loop_kind, WorkerRetryKind::Inline);
                tokio::time::sleep(retry_delay(
                    config.retry_base,
                    config.retry_maximum,
                    u32::from(attempt),
                ))
                .await;
            }
            Ok(Err(_)) => {
                return Err(MailProcessFailure {
                    failure_code: "business_processing_failed",
                    safe_detail: "MoogleMail 业务处理未完成，入站游标保持不变",
                });
            }
            Err(_) if attempt < config.process_attempts => {
                metric_context
                    .metrics
                    .observe_retry(metric_context.loop_kind, WorkerRetryKind::Inline);
                tokio::time::sleep(retry_delay(
                    config.retry_base,
                    config.retry_maximum,
                    u32::from(attempt),
                ))
                .await;
            }
            Err(_) => {
                return Err(MailProcessFailure {
                    failure_code: "business_processing_timeout",
                    safe_detail: "MoogleMail 单封邮件处理在总时限内未完成",
                });
            }
        }
    }
    Err(MailProcessFailure {
        failure_code: "business_processing_failed",
        safe_detail: "MoogleMail 业务处理未完成，入站游标保持不变",
    })
}

fn valid_page(page: &MailPollPage, claim: &MailPollClaim, world_id: &str, batch_size: u16) -> bool {
    page.mailbox_key == claim.mailbox_key
        && page.world_id == world_id
        && page.messages.len() <= usize::from(batch_size)
        && page.next_cursor.as_deref().is_none_or(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value == value.trim()
                && !value.chars().any(char::is_control)
        })
        && page.messages.iter().enumerate().all(|(index, message)| {
            !message.external_mail_id.is_empty()
                && message.external_mail_id.len() <= 512
                && message.external_mail_id == message.external_mail_id.trim()
                && !message.external_mail_id.chars().any(char::is_control)
                && message.world_id == world_id
                && message.subject.len() <= 512
                && !message.subject.chars().any(char::is_control)
                && message.received_at.timestamp_nanos_opt().is_some()
                && message.content_digest != [0; 32]
                && message.credits >= 0
                && message.cod >= 0
                && message.attachment_count <= 1000
                && (index == 0
                    || page.messages[..index]
                        .iter()
                        .all(|previous| previous.external_mail_id != message.external_mail_id))
                && message.business.as_ref().is_none_or(|business| {
                    business.mailbox_key == claim.mailbox_key
                        && business.external_mail_id == message.external_mail_id
                        && business.world_id == message.world_id
                        && business.received_at == message.received_at
                        && business.content_digest == message.content_digest
                        && business.attachments.len()
                            == usize::try_from(message.attachment_count).unwrap_or(usize::MAX)
                        && business.credits == message.credits
                        && business.cod == message.cod
                        && business.returnable == message.returnable
                        && business.claimable == message.claimable
                        && business.validate().is_ok()
                })
        })
}

fn intake_from_message(
    mailbox_key: &str,
    message: MailPollFetchedMessage,
    decision: MailProcessingDecision,
    processed_at: DateTime<Utc>,
) -> MailPollIntake {
    let mail_kind = decision.mail_kind;
    let id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!(
            "auction/inbound-mail-v1/{mailbox_key}/{}/{}",
            message.world_id, message.external_mail_id
        )
        .as_bytes(),
    );
    let processed_at = processed_at.max(message.received_at);
    MailPollIntake {
        id,
        external_mail_id: message.external_mail_id,
        world_id: message.world_id,
        mail_kind,
        status: decision.status,
        content_digest: message.content_digest,
        received_at: message.received_at,
        decision_code: decision.decision_code,
        decision_metadata: decision.metadata,
        processed_at,
    }
}

fn pending_decision(message: &MailPollFetchedMessage) -> MailProcessingDecision {
    MailProcessingDecision {
        mail_kind: classify_kind(&message.subject).to_owned(),
        status: "accepted".to_owned(),
        decision_code: "parsed_pending_processing".to_owned(),
        metadata: json!({
            "processor_state": "pending",
            "attachment_count": message.attachment_count,
            "credits": message.credits,
            "cod": message.cod,
            "returnable": message.returnable,
            "claimable": message.claimable,
        }),
    }
}

fn classify_kind(subject: &str) -> &'static str {
    match subject.trim().to_ascii_lowercase().as_str() {
        "auction" => "equipment",
        "register" => "registration",
        "rebind" => "rebind",
        value
            if value.starts_with("auction equipment delivery")
                || value.starts_with("auction payment request") =>
        {
            "delivery"
        }
        _ => "unknown",
    }
}

async fn record_failure<C>(
    store: Arc<dyn MailPollStore>,
    clock: Arc<C>,
    claim: MailPollClaim,
    failure_code: String,
    safe_detail: String,
    context: MailPollFailureContext,
) where
    C: Clock + 'static,
{
    let MailPollFailureContext { schedule, config, metric_context } = context;
    metric_context.metrics.observe_retry(metric_context.loop_kind, WorkerRetryKind::Persisted);
    let now = clock.now();
    let delay = retry_delay(config.retry_base, config.retry_maximum, claim.attempt_count);
    let backoff_at =
        now.checked_add_signed(ChronoDuration::from_std(delay).unwrap_or_default()).unwrap_or(now);
    let retry_at =
        schedule.next_after(now).map_or(backoff_at, |scheduled_at| scheduled_at.max(backoff_at));
    if let Err(error) = store.fail(claim, retry_at, failure_code, safe_detail).await {
        tracing::error!(error = %error, "MoogleMail Worker 保存失败退避状态失败");
    }
}

fn retry_delay(
    base: std::time::Duration,
    maximum: std::time::Duration,
    failed_attempt: u32,
) -> std::time::Duration {
    let exponent = failed_attempt.saturating_sub(1).min(31);
    let factor = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
    base.saturating_mul(factor).min(maximum)
}

fn safe_failure_code(value: &str) -> String {
    if !value.is_empty()
        && value == value.trim()
        && value.len() <= 128
        && !value.chars().any(char::is_control)
    {
        value.to_owned()
    } else {
        "gateway_failed".to_owned()
    }
}

fn safe_failure_detail(value: &str) -> String {
    if !value.is_empty()
        && value == value.trim()
        && value.len() <= 512
        && !value.chars().any(char::is_control)
    {
        value.to_owned()
    } else {
        "MoogleMail gateway 返回了无效的安全错误分类".to_owned()
    }
}

#[derive(Clone)]
struct MailPollMetricContext {
    metrics: Arc<dyn WorkerRecorder>,
    loop_kind: WorkerLoop,
}

#[derive(Clone)]
struct MailPollFailureContext {
    schedule: Arc<dyn RecurringSchedule>,
    config: MailPollRuntimeConfig,
    metric_context: MailPollMetricContext,
}
