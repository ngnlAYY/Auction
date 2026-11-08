//! Worker control 的 Unix socket 协议、服务端和 Admin client；socket 使用安全 runtime
//! 目录、单实例锁、inode 清理、固定 group/权限和显式 UID/GID peer allowlist，
//! 响应不含配置、Cookie、游标或错误 cause。
//! 唤醒命令只触发下一轮调度，邮件事实仍必须经过数据库命令账本和入站事务。

use std::{
    collections::VecDeque,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::worker::diagnostics;

#[cfg(unix)]
use std::{
    collections::HashSet,
    ffi::CString,
    fs::{File, OpenOptions},
};

use super::MailPollProgressTracker;
use auction_application::auction::ApplicationFuture;
use auction_application::external_accounts::{ExternalAccountStatus, ExternalAccountStatusReader};
use auction_application::mail::MailPollProgressSnapshot;
use auction_application::operations::{
    WorkerControlCondition, WorkerControlExternalAccounts, WorkerControlMailProgress,
    WorkerControlPort, WorkerControlStatus, WorkerControlTrigger, WorkerEquipmentReadRequest,
    WorkerEquipmentReadResponse, WorkerRemoteReadError, WorkerRemoteReadPort,
    WorkerSentLookupRequest, WorkerSentLookupResponse,
};
use auction_observability::{METRICS_ROUTE, MetricsError, ProcessMetrics};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::sync::Notify;
use uuid::Uuid;
mod client;
mod handlers;
mod protocol;
mod server;

use client::*;
use handlers::*;
use protocol::*;

pub use client::WorkerControlClient;
pub use protocol::{LiveResult, ReadyResult, StatusResult, TriggerPollCommand, TriggerPollResult};
pub use server::WorkerControlServer;

/// 当前 Worker control HTTP/JSON 协议版本。
pub const WORKER_CONTROL_PROTOCOL_VERSION: &str = "v1";
pub const WORKER_CONTROL_LIVE_PATH: &str = "/v1/health/live";
pub const WORKER_CONTROL_READY_PATH: &str = "/v1/health/ready";
pub const WORKER_CONTROL_STATUS_PATH: &str = "/v1/status";
/// 只读的进程内邮件观测，不包含原始游标或 claim 凭据。
pub const WORKER_CONTROL_MAIL_PROGRESS_PATH: &str = "/v1/mail/progress";
/// W5.3：只读的外部账号运行时快照；不包含凭据、Cookie、URL 或页面正文。
pub const WORKER_CONTROL_EXTERNAL_ACCOUNTS_PATH: &str = "/v1/external-accounts";
/// Worker control MoogleMail 唤醒路径。
pub const WORKER_CONTROL_TRIGGER_POLL_PATH: &str = "/v1/mail/polls:trigger";
/// Worker control peer-authenticated Prometheus scrape path。
pub const WORKER_CONTROL_METRICS_PATH: &str = METRICS_ROUTE;
/// W1.10：Admin 手工装备 URL 读取经 Worker 执行。
pub const WORKER_CONTROL_EQUIPMENT_READ_PATH: &str = "/v1/equipment/reads";
/// W1.10：CLI `mail inspect-sent` 的远端 Sent 只读核查经 Worker 执行。
pub const WORKER_CONTROL_SENT_LOOKUP_PATH: &str = "/v1/mail/sent:lookup";
/// W10.9：恢复后外部核对在全局暂停下扫描 Sent、Read Mail 与论坛新楼层。
pub const WORKER_CONTROL_RECOVERY_SCAN_PATH: &str = "/v1/recovery/external:scan";

const HEADER_PROTOCOL_VERSION: &str = "x-worker-control-version";
const HEADER_REQUEST_ID: &str = "x-request-id";
const DEFAULT_BODY_LIMIT: usize = 1024;
/// 远端读取路由使用更长的请求超时；单次读取仍由 Worker 自己的 fetch 预算终止。
const REMOTE_READ_REQUEST_TIMEOUT: Duration =
    Duration::from_secs(auction_application::operations::WORKER_REMOTE_READ_TIMEOUT_SECONDS);
/// 恢复核对会跨多个列表页与论坛主题；使用独立的更长上界，单页仍受 fetch 预算约束。
const RECOVERY_SCAN_REQUEST_TIMEOUT: Duration =
    Duration::from_secs(auction_application::operations::WORKER_RECOVERY_SCAN_TIMEOUT_SECONDS);
const MAX_RESPONSE_BYTES: usize = 64 << 10;
const MAX_REMEMBERED_TRIGGER_COMMANDS: usize = 256;

/// Worker control server 错误。
#[derive(Debug, Error)]
pub enum WorkerControlServerError {
    #[error("Worker control socket 路径无效")]
    InvalidSocketPath,
    #[error("Worker control runtime 目录无效")]
    InvalidRuntimeDirectory,
    #[error("Worker control 安全配置无效：{0}")]
    InvalidSecurityConfig(&'static str),
    #[error("Worker control Unix group 无法解析")]
    UnknownGroup,
    #[error("Worker control runtime 目录不满足 owner/group/权限约束")]
    UnsafeRuntimeDirectory,
    #[error("Worker control socket 或实例锁已被活动实例占用")]
    ActiveSocket,
    #[error("Worker control socket 状态无法安全确认")]
    UnverifiableSocket,
    #[error("Worker control socket 已被非 socket 文件占用")]
    SocketPathOccupied,
    #[error("Worker control socket 不受当前平台支持")]
    UnsupportedPlatform,
    #[error("Worker control socket IO 失败：{0}")]
    Io(#[source] std::io::Error),
    #[error("Worker control server 运行失败：{0}")]
    Serve(#[source] std::io::Error),
}

/// Worker control client 的安全错误分类；不保存远端正文。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum WorkerControlClientError {
    #[error("Worker control unavailable")]
    Unavailable,
    #[error("Worker control timeout")]
    Timeout,
    #[error("Worker control incompatible")]
    Incompatible,
    #[error("Worker control rejected")]
    Rejected,
}

/// Worker control server 配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerControlServerConfig {
    /// Unix socket 的规范化绝对路径。
    pub socket_path: PathBuf,
    /// 必须预先存在且由当前进程用户拥有的运行目录。
    pub runtime_directory: PathBuf,
    pub group: String,
    pub allowed_peer_uids: Vec<u32>,
    pub allowed_peer_gids: Vec<u32>,
    pub request_timeout: Duration,
    /// JSON 请求体上限；不能超过协议的保守边界。
    pub body_limit: usize,
}

impl WorkerControlServerConfig {
    /// 建立并校验 Worker control server 配置。
    pub fn new(
        socket_path: impl Into<PathBuf>,
        runtime_directory: impl Into<PathBuf>,
        group: impl Into<String>,
        allowed_peer_uids: Vec<u32>,
        allowed_peer_gids: Vec<u32>,
    ) -> Result<Self, WorkerControlServerError> {
        let socket_path = socket_path.into();
        let runtime_directory = runtime_directory.into();
        let group = group.into();
        validate_socket_path(&socket_path)?;
        validate_runtime_directory_path(&runtime_directory)?;
        if socket_path.parent() != Some(runtime_directory.as_path()) {
            return Err(WorkerControlServerError::InvalidSecurityConfig(
                "socket 必须直接位于 runtime 目录",
            ));
        }
        if !valid_safe_identifier(&group) {
            return Err(WorkerControlServerError::InvalidSecurityConfig("group 必须是安全标识"));
        }
        if allowed_peer_uids.len() + allowed_peer_gids.len() > 64 {
            return Err(WorkerControlServerError::InvalidSecurityConfig(
                "peer allowlist 不能超过 64 项",
            ));
        }
        if duplicate_u32(&allowed_peer_uids) || duplicate_u32(&allowed_peer_gids) {
            return Err(WorkerControlServerError::InvalidSecurityConfig(
                "peer allowlist 不能包含重复 ID",
            ));
        }
        if allowed_peer_uids.is_empty() && allowed_peer_gids.is_empty() {
            return Err(WorkerControlServerError::InvalidSecurityConfig(
                "必须显式配置 peer UID 或 GID allowlist",
            ));
        }
        Ok(Self {
            socket_path,
            runtime_directory,
            group,
            allowed_peer_uids,
            allowed_peer_gids,
            request_timeout: Duration::from_secs(5),
            body_limit: DEFAULT_BODY_LIMIT,
        })
    }
}

/// Worker 进程控制状态的内存 owner。
/// 该状态只描述当前进程，不是数据库权威状态；`mail_pollers` 只用于判断唤醒是否被接受。
#[derive(Clone)]
pub struct WorkerControlHandle {
    inner: Arc<WorkerControlInner>,
    mail_progress: MailPollProgressTracker,
    /// W5.3：进程内外部账号快照；未装配时为 `None`，不能解释为空闲。
    external_accounts: Option<Arc<dyn ExternalAccountStatusReader>>,
    /// 数据库/schema 证明与审计模式门禁；与 Public/Admin `/health/ready` 和 Worker 心跳同一
    /// 判定。无数据库的骨架进程不装配，此时只看进程状态。
    readiness: Option<Arc<dyn auction_application::operations::ProcessReadinessPort>>,
}

struct WorkerControlInner {
    // 就绪、降级原因和 drain 共用一个原子快照，避免并发刷新用旧投影覆盖新故障。
    state: std::sync::atomic::AtomicU8,
    mail_poll_requested: std::sync::atomic::AtomicBool,
    version: String,
    mail_pollers: usize,
    poll_wakeup: Arc<Notify>,
    fatal_wakeup: Arc<Notify>,
    trigger_history: Mutex<TriggerHistory>,
}

const CONTROL_STARTED: u8 = 1;
const CONTROL_DEGRADED: u8 = 2;
const CONTROL_FORUM_DEGRADED: u8 = 4;
const CONTROL_DRAINING: u8 = 8;

#[derive(Clone, Copy)]
enum WorkerControlState {
    Starting,
    Serving,
    Degraded,
    Draining,
}

impl WorkerControlState {
    fn from_flags(value: u8) -> Self {
        if value & CONTROL_DRAINING != 0 {
            Self::Draining
        } else if value & (CONTROL_DEGRADED | CONTROL_FORUM_DEGRADED) != 0 {
            Self::Degraded
        } else if value & CONTROL_STARTED != 0 {
            Self::Serving
        } else {
            Self::Starting
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Serving => "serving",
            Self::Degraded => "degraded",
            Self::Draining => "draining",
        }
    }
}

struct TriggerHistory {
    order: VecDeque<String>,
    results: std::collections::HashMap<String, TriggerPollResult>,
}

impl WorkerControlHandle {
    pub fn new(version: impl Into<String>, mail_pollers: usize) -> Self {
        Self {
            inner: Arc::new(WorkerControlInner {
                state: std::sync::atomic::AtomicU8::new(0),
                mail_poll_requested: std::sync::atomic::AtomicBool::new(false),
                version: version.into(),
                mail_pollers,
                poll_wakeup: Arc::new(Notify::new()),
                fatal_wakeup: Arc::new(Notify::new()),
                trigger_history: Mutex::new(TriggerHistory {
                    order: VecDeque::with_capacity(MAX_REMEMBERED_TRIGGER_COMMANDS),
                    results: std::collections::HashMap::with_capacity(
                        MAX_REMEMBERED_TRIGGER_COMMANDS,
                    ),
                }),
            }),
            mail_progress: MailPollProgressTracker::default(),
            external_accounts: None,
            readiness: None,
        }
    }

    /// 注入数据库就绪探针；`/v1/health/ready` 据此与 Public/Admin、Worker 心跳保持同一口径。
    pub fn with_readiness(
        mut self,
        readiness: Arc<dyn auction_application::operations::ProcessReadinessPort>,
    ) -> Self {
        self.readiness = Some(readiness);
        self
    }

    /// 在克隆控制句柄前注入同一份邮件观测 owner；不改变任何就绪或调度事实。
    pub fn with_mail_progress(mut self, progress: MailPollProgressTracker) -> Self {
        self.mail_progress = progress;
        self
    }

    /// 注入 W5.3 的外部账号只读快照；未提供时 control 端点明确不可用。
    pub fn with_external_accounts(
        mut self,
        reader: Option<Arc<dyn ExternalAccountStatusReader>>,
    ) -> Self {
        self.external_accounts = reader;
        self
    }

    /// 标记 Worker 已经可以接受控制请求。
    pub fn mark_serving(&self) {
        // Rust 1.99 的新名 try_update 在 MSRV 1.94.1 尚未稳定；保留单次原子状态迁移。
        #[allow(deprecated)]
        let _ = self.inner.state.fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |state| Some((state & CONTROL_DRAINING) | CONTROL_STARTED),
        );
    }

    /// 记录论坛循环最近一轮健康状态；成功轮次可以恢复该依赖造成的降级。
    pub fn mark_forum_cycle(&self, healthy: bool) {
        // 与其他原子更新点一致，MSRV 1.94.1 仍须使用 fetch_update。
        #[allow(deprecated)]
        let _ = self.inner.state.fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |state| {
                Some(if healthy {
                    (state | CONTROL_STARTED) & !CONTROL_FORUM_DEGRADED
                } else {
                    state | CONTROL_STARTED | CONTROL_FORUM_DEGRADED
                })
            },
        );
    }

    /// 返回 control 与 heartbeat 共用的进程就绪事实。
    pub fn is_ready(&self) -> bool {
        matches!(self.operational_state(), WorkerControlState::Serving)
    }

    /// 返回不可逆的排空门禁；循环在每次领取新工作前检查，在途 claim 仍由原 owner 收口。
    pub fn is_draining(&self) -> bool {
        self.inner.state.load(std::sync::atomic::Ordering::Acquire) & CONTROL_DRAINING != 0
    }

    /// 标记 Worker 正在收口；之后的手动唤醒会被拒绝。
    pub fn mark_draining(&self) {
        self.inner.state.fetch_or(CONTROL_DRAINING, std::sync::atomic::Ordering::AcqRel);
    }

    /// 致命循环故障通知只供装配根的一个接收者使用；通知在首次等待前仍保留。
    pub fn fatal_wakeup(&self) -> Arc<Notify> {
        Arc::clone(&self.inner.fatal_wakeup)
    }

    /// 原子撤销就绪并停止新领取，再通知装配根启动有界排空。
    pub(crate) fn report_fatal(&self) {
        self.inner.state.fetch_or(
            CONTROL_STARTED | CONTROL_DEGRADED | CONTROL_DRAINING,
            std::sync::atomic::Ordering::AcqRel,
        );
        self.inner.fatal_wakeup.notify_one();
    }

    pub fn poll_wakeup(&self) -> Arc<Notify> {
        Arc::clone(&self.inner.poll_wakeup)
    }

    /// 取出一次手动轮询请求；Worker 主循环据此让所有邮箱忽略下一次调度时间。
    pub fn take_mail_poll_request(&self) -> bool {
        self.inner.mail_poll_requested.swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    fn live(&self) -> LiveResult {
        LiveResult { state: self.operational_state().as_str().to_owned() }
    }

    /// 进程处于 serving，且已装配的数据库就绪探针通过；任一不满足即未就绪（失败关闭）。
    async fn ready(&self) -> ReadyResult {
        if !self.is_ready() {
            return ReadyResult { ready: false };
        }
        let database_ready = match &self.readiness {
            Some(readiness) => readiness.check().await.is_ok(),
            None => true,
        };
        ReadyResult { ready: database_ready }
    }

    fn status(&self) -> StatusResult {
        let state = self.operational_state();
        StatusResult {
            mail_pollers: self.inner.mail_pollers,
            ready: matches!(state, WorkerControlState::Serving),
            state: state.as_str().to_owned(),
            version: self.inner.version.clone(),
        }
    }

    /// 读取 W5.3 外部账号快照；未装配或观测 owner 不可用时返回 `None`。
    fn external_accounts(&self) -> Option<ExternalAccountStatus> {
        self.external_accounts.as_ref()?.snapshot()
    }

    fn operational_state(&self) -> WorkerControlState {
        WorkerControlState::from_flags(self.inner.state.load(std::sync::atomic::Ordering::Acquire))
    }

    fn trigger_poll(&self, command: TriggerPollCommand) -> Result<TriggerPollResult, TriggerError> {
        if !auction_domain::valid_text(&command.command_id, 64)
            || Uuid::parse_str(&command.command_id).is_err()
        {
            return Err(TriggerError::InvalidCommand);
        }
        let mut history = self.inner.trigger_history.lock().map_err(|error| {
            diagnostics::warn(
                "control_trigger",
                "lock_history",
                error.diagnostic_kind(),
                DiagnosticContext {
                    command_id: Uuid::parse_str(&command.command_id).ok(),
                    route: Some(WORKER_CONTROL_TRIGGER_POLL_PATH),
                    ..Default::default()
                },
            );
            TriggerError::Unavailable
        })?;
        if let Some(result) = history.results.get(&command.command_id).cloned() {
            return Ok(result);
        }
        if !self.is_ready() {
            return Err(TriggerError::NotReady);
        }
        if self.inner.mail_pollers == 0 {
            return Err(TriggerError::NoPollers);
        }
        let result = TriggerPollResult {
            accepted: true,
            command_id: command.command_id.clone(),
            pollers: self.inner.mail_pollers,
        };
        if history.order.len() == MAX_REMEMBERED_TRIGGER_COMMANDS
            && let Some(oldest) = history.order.pop_front()
        {
            history.results.remove(&oldest);
        }
        history.order.push_back(command.command_id);
        history.results.insert(result.command_id.clone(), result.clone());
        drop(history);
        self.inner.mail_poll_requested.store(true, std::sync::atomic::Ordering::Release);
        self.inner.poll_wakeup.notify_one();
        Ok(result)
    }
}
