//! MoogleMail Worker 的远程读取编排。
//!
//! 本模块只负责 claim → 远程读取 → 安全事实映射 → commit/fail；HTTP、SQL 和 MoogleMail HTML
//! 解析分别由应用端口、存储适配器和 integrations owner 提供。

use std::{future::Future, pin::Pin, sync::Arc};

use auction_application::{
    Clock, MAX_MAIL_POLL_BATCH_SIZE, MailPollBatch, MailPollClaim, MailPollIntake, MailPollStore,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::json;
use uuid::Uuid;

use super::{PersistentWorker, WorkerError, retry_delay};

/// 一次远程 MoogleMail 读取请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailPollFetchRequest {
    pub mailbox_key: String,
    pub world_id: String,
    pub cursor_value: Option<String>,
    pub limit: u16,
}

/// 已由 integrations 解析完成、但尚未进入业务处理器的安全邮件摘要。
///
/// 外部适配器必须在返回该类型前清除正文中的装备 key；这里不提供正文、Cookie、token 或
/// 原始 HTML 字段。
#[derive(Clone, PartialEq, Eq)]
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
            .finish()
    }
}

/// 一次远程 MoogleMail 读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Worker 中可选的 MoogleMail 轮询依赖集合。
pub struct MailPollRuntime {
    pub store: Arc<dyn MailPollStore>,
    pub gateway: Arc<dyn MailPollGateway>,
    pub world_id: String,
    pub batch_size: u16,
}

/// 执行一轮所有 due 邮箱的 MoogleMail 读取。
pub async fn run_cycle<C>(
    worker: &PersistentWorker<C>,
    runtime: &MailPollRuntime,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    if runtime.world_id.is_empty() || runtime.world_id != runtime.world_id.trim() {
        return Err(WorkerError::InvalidMailPollConfiguration);
    }
    if runtime.batch_size == 0 || runtime.batch_size > MAX_MAIL_POLL_BATCH_SIZE {
        return Err(WorkerError::InvalidMailPollConfiguration);
    }
    let now = worker.clock.now();
    let lease_until = now
        .checked_add_signed(super::lease_duration(worker.poll_interval))
        .ok_or(WorkerError::InvalidPollInterval)?;
    let claims = runtime
        .store
        .claim_due(worker.worker_id.clone(), now, lease_until, worker.concurrency.min(1000))
        .await?;
    if claims.is_empty() {
        return Ok(());
    }
    let mut tasks = tokio::task::JoinSet::new();
    for claim in claims {
        tasks.spawn(process_claim(
            Arc::clone(&runtime.store),
            Arc::clone(&runtime.gateway),
            Arc::clone(&worker.clock),
            claim,
            runtime.world_id.clone(),
            runtime.batch_size,
            worker.poll_interval,
        ));
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::error!(error = %error, "MoogleMail Worker 任务 join 失败");
        }
    }
    Ok(())
}

async fn process_claim<C>(
    store: Arc<dyn MailPollStore>,
    gateway: Arc<dyn MailPollGateway>,
    clock: Arc<C>,
    claim: MailPollClaim,
    world_id: String,
    batch_size: u16,
    poll_interval: std::time::Duration,
) where
    C: Clock + 'static,
{
    if !claim.validate() || claim.mailbox_key.is_empty() {
        tracing::error!("MoogleMail Worker 收到无效租约");
        return;
    }
    let request = MailPollFetchRequest {
        mailbox_key: claim.mailbox_key.clone(),
        world_id: world_id.clone(),
        cursor_value: claim.cursor_value.clone(),
        limit: batch_size,
    };
    let polled_at = clock.now();
    let result = gateway.fetch(request).await;
    let page = match result {
        Ok(page) => page,
        Err(error) => {
            record_failure(
                store,
                clock,
                claim,
                error.failure_code,
                error.safe_detail,
                error.temporary,
            )
            .await;
            return;
        }
    };
    if !valid_page(&page, &claim, &world_id, batch_size) {
        record_failure(
            store,
            clock,
            claim,
            "invalid_gateway_page".to_owned(),
            "MoogleMail gateway 返回的批次不符合安全边界".to_owned(),
            false,
        )
        .await;
        return;
    }
    let next_poll_at = polled_at
        .checked_add_signed(ChronoDuration::seconds(
            i64::try_from(poll_interval.as_secs()).unwrap_or(i64::MAX),
        ))
        .unwrap_or(polled_at);
    let items = page
        .messages
        .into_iter()
        .map(|message| intake_from_message(&claim.mailbox_key, message, polled_at))
        .collect::<Vec<_>>();
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
    }
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
        })
}

fn intake_from_message(
    mailbox_key: &str,
    message: MailPollFetchedMessage,
    processed_at: DateTime<Utc>,
) -> MailPollIntake {
    let mail_kind = classify_kind(&message.subject).to_owned();
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
        status: "accepted".to_owned(),
        content_digest: message.content_digest,
        received_at: message.received_at,
        decision_code: "parsed_pending_processing".to_owned(),
        decision_metadata: json!({
            "processor_state": "pending",
            "attachment_count": message.attachment_count,
            "credits": message.credits,
            "cod": message.cod,
            "returnable": message.returnable,
            "claimable": message.claimable,
        }),
        processed_at,
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
    temporary: bool,
) where
    C: Clock + 'static,
{
    let now = clock.now();
    let delay = if temporary { retry_delay(claim.attempt_count) } else { ChronoDuration::hours(1) };
    let retry_at = now.checked_add_signed(delay).unwrap_or(now);
    if let Err(error) = store.fail(claim, retry_at, failure_code, safe_detail).await {
        tracing::error!(error = %error, "MoogleMail Worker 保存失败退避状态失败");
    }
}
