//! MoogleMail Worker 轮询协议、运行时配置和单邮箱运行时。

use std::{future::Future, pin::Pin, sync::Arc};

use auction_application::{
    MAX_MAIL_POLL_BATCH_SIZE, MailDiagnosticRecorder, MailMessage, MailPollClaim, MailPollStore,
    MailProcessor, MailboxMaintenanceService, RecurringSchedule,
};
use auction_observability::WorkerLoop;
use chrono::{DateTime, Utc};
use tokio::sync::Notify;

use super::super::WorkerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailPollFetchRequest {
    pub mailbox_key: String,
    pub world_id: String,
    /// 已完成邮件的 external_mail_id；gateway 必须只返回该 ID 之前的未完成窗口。
    pub cursor_value: Option<String>,
    pub limit: u16,
    pub deadline: Option<std::time::Instant>,
}

/// 已由 integrations 解析完成、等待业务处理器消费的 MoogleMail 邮件。
/// 摘要字段用于入站幂等和安全日志；`business` 只在 Worker 进程内短暂携带正文和装备 key，
/// 绝不进入 `MailPollBatch` 或数据库。
pub struct MailPollFetchedMessage {
    pub external_mail_id: String,
    pub world_id: String,
    pub received_at: DateTime<Utc>,
    pub sender: Option<String>,
    pub subject: String,
    pub message_display: Option<auction_application::MailDisplaySnapshot>,
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
            .field("sender", &self.sender.as_deref().map(|_| "[PRESENT]"))
            .field("subject", &"[REDACTED]")
            .field("message_display", &self.message_display.as_ref().map(|_| "[PRESENT]"))
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

#[derive(Debug)]
pub struct MailPollPage {
    pub mailbox_key: String,
    pub world_id: String,
    /// 本页最后一封消息的 external_mail_id；只有该页全部处理成功时才可提交。
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

    /// 持久轮询入口；可能改变已读状态的适配器须覆盖此方法并在详情 GET 前登记恢复队列。
    fn fetch_claimed(
        &self,
        request: MailPollFetchRequest,
        _claim: MailPollClaim,
    ) -> Pin<Box<dyn Future<Output = Result<MailPollPage, MailPollGatewayError>> + Send + '_>> {
        self.fetch(request)
    }
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
    /// 单次 mailbox claim 的总处理预算；耗尽时只提交已完成前缀并推进到其邮件 ID。
    pub batch_timeout: std::time::Duration,
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
            batch_timeout: std::time::Duration::from_secs(600),
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
            && !self.batch_timeout.is_zero()
            && self.batch_timeout <= std::time::Duration::from_secs(86_400)
            && self.batch_timeout >= self.fetch_timeout.saturating_add(self.item_timeout)
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
    pub(super) diagnostics: Option<Arc<dyn MailDiagnosticRecorder>>,
    pub(super) maintenance: Option<Arc<MailboxMaintenanceService>>,
    pub world_id: String,
    pub mailbox_key: String,
    pub config: MailPollRuntimeConfig,
    pub(super) schedule: Arc<dyn RecurringSchedule>,
    pub(super) wakeup: Arc<Notify>,
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
            diagnostics: None,
            maintenance: None,
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

    /// 为异常邮件注入独立证据记录器；不参与命令、游标或租约的正确性判断。
    pub fn with_diagnostics(mut self, diagnostics: Arc<dyn MailDiagnosticRecorder>) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    /// 注入三页定期维护；只在主世界运行，失败只记录日志，不影响收件轮询。
    pub fn with_maintenance(mut self, maintenance: Arc<MailboxMaintenanceService>) -> Self {
        self.maintenance = Some(maintenance);
        self
    }

    /// 合并一次手动触发；轮询正在执行时，通知会保留到下一轮领取。
    pub(crate) fn trigger(&self) {
        self.wakeup.notify_one();
    }

    pub(super) fn validate(&self) -> bool {
        matches!(self.world_id.as_str(), "main" | "isekai")
            && !self.mailbox_key.is_empty()
            && self.mailbox_key == self.mailbox_key.trim()
            && self.mailbox_key.len() <= 256
            && !self.mailbox_key.chars().any(char::is_control)
            && self.schedule.next_after(Utc::now()).is_some()
            && self.config.validate()
    }

    pub(super) fn loop_kind(&self) -> WorkerLoop {
        if self.world_id == "isekai" { WorkerLoop::MailIsekai } else { WorkerLoop::MailMain }
    }
}
