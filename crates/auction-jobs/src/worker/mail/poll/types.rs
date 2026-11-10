//! MoogleMail Worker 轮询协议、运行时配置和单邮箱运行时。

use std::{future::Future, pin::Pin, sync::Arc};

use auction_application::external_metrics::{
    ExternalMetricsEvent, ExternalMetricsObserver, MailBudget, MailPhase, MailWorld,
};
use auction_application::mail::{
    MAX_MAIL_POLL_BATCH_SIZE, MailDiagnosticRecorder, MailMessage, MailPollClaim,
    MailPollProgressObserver, MailPollStore, MailProcessor, MailboxMaintenanceService,
};
use auction_application::runtime_switches::RuntimeSwitches;
use auction_application::scheduling::RecurringSchedule;
use auction_observability::WorkerLoop;
use chrono::{DateTime, Utc};
use tokio::sync::Notify;

use crate::worker::runtime::WorkerError;

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
    pub message_display: Option<auction_application::mail::MailDisplaySnapshot>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailPollRuntimeConfig {
    /// 完整 Read Mail 纯观察的最小间隔；每次维护结束后检查，首次延后同一间隔。
    pub read_mail_observation_interval: std::time::Duration,
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
            read_mail_observation_interval: std::time::Duration::from_secs(3600),
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
    /// 只替换 Worker 热读取字段；租约、续租与观察间隔仍来自启动配置。
    /// 组合后不满足预算约束时拒绝本次替换，调用方继续使用旧值。
    pub(crate) fn apply_hot(
        &mut self,
        hot: &auction_application::admin::WorkerMailHotConfig,
    ) -> bool {
        let mut next = *self;
        next.batch_size = hot.batch_size;
        next.jitter = std::time::Duration::from_secs(hot.jitter_seconds);
        next.fetch_attempts = hot.fetch_attempts;
        next.fetch_timeout = std::time::Duration::from_secs(hot.fetch_timeout_seconds);
        next.process_attempts = hot.process_attempts;
        next.item_timeout = std::time::Duration::from_secs(hot.item_timeout_seconds);
        next.batch_timeout = std::time::Duration::from_secs(hot.batch_timeout_seconds);
        next.retry_base = std::time::Duration::from_secs(hot.retry_base_seconds);
        next.retry_maximum = std::time::Duration::from_secs(hot.retry_maximum_seconds);
        if !next.validate() {
            return false;
        }
        *self = next;
        true
    }

    fn validate(self) -> bool {
        (std::time::Duration::from_secs(60)..=std::time::Duration::from_secs(86_400))
            .contains(&self.read_mail_observation_interval)
            && self.batch_size > 0
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
pub struct MailPollSchedules {
    /// 无相位收件日程。
    pub receive: Arc<dyn RecurringSchedule>,
    /// 启动时冻结世界相位的收件日程。
    pub phased_receive: Arc<dyn RecurringSchedule>,
    /// 维护日程；始终按同一世界相位预构建。
    pub maintenance: Arc<dyn RecurringSchedule>,
}

/// 轮询运行时需要的共享快照与日程；把启动期绑定收拢为一个参数。
#[derive(Clone)]
pub struct MailPollRuntimeBindings {
    /// 三套启动时冻结的日程。
    pub schedules: MailPollSchedules,
    /// 与进程其余 owner 共享的不可变开关快照。
    pub switches: RuntimeSwitches,
}

/// Worker 中单个 MoogleMail world 的依赖与独立唤醒器。
#[derive(Clone)]
pub struct MailPollRuntime {
    pub store: Arc<dyn MailPollStore>,
    pub gateway: Arc<dyn MailPollGateway>,
    pub processor: Option<Arc<dyn MailProcessor>>,
    pub(super) diagnostics: Option<Arc<dyn MailDiagnosticRecorder>>,
    pub(super) progress_observer: Option<Arc<dyn MailPollProgressObserver>>,
    pub(in crate::worker::mail) maintenance: Option<Arc<MailboxMaintenanceService>>,
    pub(crate) external_metrics:
        Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
    pub world_id: String,
    pub mailbox_key: String,
    pub config: MailPollRuntimeConfig,
    /// 每次选择收件/维护路径都重新读取；不缓存为启动事实。
    pub(in crate::worker::mail) switches: RuntimeSwitches,
    /// 无相位收件日程，供旧路径和相位关闭时使用。
    pub(super) schedule: Arc<dyn RecurringSchedule>,
    /// 启动时冻结世界相位的收件日程；与 `schedule` 同时预构建。
    pub(super) phased_schedule: Arc<dyn RecurringSchedule>,
    pub(in crate::worker::mail) maintenance_schedule: Arc<dyn RecurringSchedule>,
    /// 同一世界维护的唯一执行门；两个循环都必须在执行维护前取得。
    pub(in crate::worker::mail) maintenance_lock: Arc<tokio::sync::Mutex<()>>,
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
        bindings: MailPollRuntimeBindings,
        config: MailPollRuntimeConfig,
    ) -> Result<Self, WorkerError> {
        let runtime = Self {
            store,
            gateway,
            processor,
            diagnostics: None,
            progress_observer: None,
            maintenance: None,
            external_metrics: auction_application::external_metrics::noop_external_metrics_observer(
            ),
            world_id,
            mailbox_key,
            config,
            switches: bindings.switches,
            schedule: bindings.schedules.receive,
            phased_schedule: bindings.schedules.phased_receive,
            maintenance_schedule: bindings.schedules.maintenance,
            maintenance_lock: Arc::new(tokio::sync::Mutex::new(())),
            wakeup: Arc::new(Notify::new()),
        };
        if !runtime.validate() {
            return Err(WorkerError::InvalidMailPollConfiguration);
        }
        Ok(runtime)
    }

    /// 注入只读运行观测；不可用只影响诊断，不影响命令、游标或租约。
    pub fn with_progress_observer(mut self, observer: Arc<dyn MailPollProgressObserver>) -> Self {
        self.progress_observer = Some(observer);
        self
    }

    /// 为异常邮件注入独立证据记录器；不参与命令、游标或租约的正确性判断。
    pub fn with_diagnostics(mut self, diagnostics: Arc<dyn MailDiagnosticRecorder>) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    /// 注入当前世界的维护服务；两条路径共用同一服务与互斥门。
    pub fn with_maintenance(mut self, maintenance: Arc<MailboxMaintenanceService>) -> Self {
        self.maintenance = Some(maintenance);
        self
    }

    /// 注入同步外部客户端指标；关闭或非 Worker 角色使用 no-op。
    pub fn with_external_metrics(
        mut self,
        observer: Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
    ) -> Self {
        self.external_metrics = observer;
        self
    }

    /// 记录本 world 的预算耗尽；只读同步计数，不改变任何截止点。
    pub(crate) fn observe_budget(&self, phase: MailPhase, budget: MailBudget) {
        observe_budget(&self.world_id, self.external_metrics.as_ref(), phase, budget);
    }

    /// 维护服务存在时 runner 始终启动任务；旧路径下任务只 idle，不退出。
    pub(in crate::worker) fn has_independent_maintenance(&self) -> bool {
        self.maintenance.is_some()
    }

    /// 合并一次手动触发；轮询正在执行时，通知会保留到下一轮领取。
    pub(crate) fn trigger(&self) {
        self.wakeup.notify_one();
    }

    pub(in crate::worker) fn validate(&self) -> bool {
        let now = Utc::now();
        matches!(self.world_id.as_str(), "main" | "isekai")
            && auction_domain::valid_text(&self.mailbox_key, 256)
            && self.schedule.next_after(now).is_some_and(|next| next > now)
            && self.phased_schedule.next_after(now).is_some_and(|next| next > now)
            // 即使基线关闭也校验维护日程，保证在线切换不会暴露启动时未发现的配置错误。
            && self.maintenance_schedule.next_after(now).is_some_and(|next| next > now)
            && self.config.validate()
    }

    pub(in crate::worker) fn loop_kind(&self) -> WorkerLoop {
        if self.world_id == "isekai" { WorkerLoop::MailIsekai } else { WorkerLoop::MailMain }
    }
}

/// 把 Worker 的 world 字符串映射到闭集后记录预算耗尽；未知 world 不产生标签。
pub(crate) fn observe_budget(
    world_id: &str,
    observer: &dyn ExternalMetricsObserver,
    phase: MailPhase,
    budget: MailBudget,
) {
    let world = match world_id {
        "main" => MailWorld::Main,
        "isekai" => MailWorld::Isekai,
        _ => return,
    };
    observer.observe(ExternalMetricsEvent::MailBudgetExhausted { world, phase, budget });
}

#[cfg(test)]
mod hot_reload_tests {
    use auction_application::admin::WorkerMailHotConfig;
    use auction_application::delivery::DeliverySendIntervals;

    use super::MailPollRuntimeConfig;

    fn startup() -> MailPollRuntimeConfig {
        MailPollRuntimeConfig {
            lease_duration: std::time::Duration::from_secs(90),
            renew_interval: std::time::Duration::from_secs(15),
            ..MailPollRuntimeConfig::default()
        }
    }

    fn hot() -> WorkerMailHotConfig {
        WorkerMailHotConfig {
            batch_size: 12,
            jitter_seconds: 2,
            fetch_attempts: 4,
            fetch_timeout_seconds: 40,
            process_attempts: 5,
            item_timeout_seconds: 50,
            batch_timeout_seconds: 700,
            retry_base_seconds: 2,
            retry_maximum_seconds: 60,
            account_reply_window_seconds: 86_400,
            explanation_mail_window_seconds: 86_400,
            explanation_mail_limit: 10,
            send_classes: auction_application::mail::MailSendClasses::default(),
            send_intervals: DeliverySendIntervals { manual_seconds: 9, ..Default::default() },
        }
    }

    #[test]
    fn hot_overlay_replaces_budgets_and_keeps_lease() {
        let mut config = startup();
        assert!(config.apply_hot(&hot()));
        assert_eq!(config.batch_size, 12);
        assert_eq!(config.fetch_attempts, 4);
        assert_eq!(config.fetch_timeout, std::time::Duration::from_secs(40));
        assert_eq!(config.process_attempts, 5);
        assert_eq!(config.item_timeout, std::time::Duration::from_secs(50));
        assert_eq!(config.batch_timeout, std::time::Duration::from_secs(700));
        assert_eq!(config.retry_base, std::time::Duration::from_secs(2));
        assert_eq!(config.retry_maximum, std::time::Duration::from_secs(60));
        // 租约、续租与观察间隔是重启字段。
        assert_eq!(config.lease_duration, startup().lease_duration);
        assert_eq!(config.renew_interval, startup().renew_interval);
        assert_eq!(config.read_mail_observation_interval, startup().read_mail_observation_interval);
    }

    #[test]
    fn inconsistent_hot_candidate_keeps_previous_config() {
        let mut config = startup();
        let previous = config;
        let mut invalid = hot();
        // 批次预算小于 fetch + item，必须整体拒绝而不是部分应用。
        invalid.batch_timeout_seconds = 10;
        assert!(!config.apply_hot(&invalid));
        assert_eq!(config, previous);
    }
}
