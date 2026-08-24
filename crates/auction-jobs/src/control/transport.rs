//! Worker control 的 Unix socket 协议、服务端和 Admin client。
//!
//! 该模块只暴露存活、就绪、状态和 MoogleMail 唤醒四个窄操作。socket 使用安全 runtime 目录、
//! 单实例锁、inode 清理、固定 group/权限和显式 UID/GID peer allowlist；响应不包含配置、Cookie、
//! 游标或错误 cause。唤醒命令只触发下一轮调度，邮件事实仍必须经过数据库命令账本和入站事务。

use std::{
    collections::VecDeque,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

#[cfg(unix)]
use std::{
    collections::HashSet,
    ffi::CString,
    fs::{File, OpenOptions},
};

use auction_application::{
    ApplicationFuture, WorkerControlCondition, WorkerControlPort, WorkerControlStatus,
    WorkerControlTrigger,
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
#[path = "transport/client.rs"]
mod client;
#[path = "transport/handlers.rs"]
mod handlers;
#[path = "transport/protocol.rs"]
mod protocol;
#[path = "transport/server/mod.rs"]
mod server;

use client::*;
use handlers::*;
use protocol::*;

pub use client::WorkerControlClient;
pub use protocol::{LiveResult, ReadyResult, StatusResult, TriggerPollCommand, TriggerPollResult};
pub use server::WorkerControlServer;

/// 当前 Worker control HTTP/JSON 协议版本。
pub const WORKER_CONTROL_PROTOCOL_VERSION: &str = "v1";
/// Worker control liveness 路径。
pub const WORKER_CONTROL_LIVE_PATH: &str = "/v1/health/live";
/// Worker control readiness 路径。
pub const WORKER_CONTROL_READY_PATH: &str = "/v1/health/ready";
/// Worker control 状态路径。
pub const WORKER_CONTROL_STATUS_PATH: &str = "/v1/status";
/// Worker control MoogleMail 唤醒路径。
pub const WORKER_CONTROL_TRIGGER_POLL_PATH: &str = "/v1/mail/polls:trigger";
/// Worker control peer-authenticated Prometheus scrape path。
pub const WORKER_CONTROL_METRICS_PATH: &str = METRICS_ROUTE;

const HEADER_PROTOCOL_VERSION: &str = "x-worker-control-version";
const HEADER_REQUEST_ID: &str = "x-request-id";
const DEFAULT_BODY_LIMIT: usize = 1024;
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
    /// socket 文件应归属的 Unix group 名称。
    pub group: String,
    /// 允许访问控制面的内核认证 UID。
    pub allowed_peer_uids: Vec<u32>,
    /// 允许访问控制面的内核认证 GID。
    pub allowed_peer_gids: Vec<u32>,
    /// 单个请求的最长处理时间。
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
///
/// 该状态只描述当前进程，不是数据库权威状态；`mail_pollers` 只用于判断唤醒是否被接受。
#[derive(Clone)]
pub struct WorkerControlHandle {
    inner: Arc<WorkerControlInner>,
}

struct WorkerControlInner {
    state: std::sync::atomic::AtomicU8,
    ready: std::sync::atomic::AtomicBool,
    mail_poll_requested: std::sync::atomic::AtomicBool,
    version: String,
    mail_pollers: usize,
    poll_wakeup: Arc<Notify>,
    trigger_history: Mutex<TriggerHistory>,
}

#[derive(Clone, Copy)]
enum WorkerControlState {
    Starting = 0,
    Serving = 1,
    Degraded = 2,
    Draining = 3,
}

impl WorkerControlState {
    fn from_byte(value: u8) -> Self {
        match value {
            1 => Self::Serving,
            2 => Self::Degraded,
            3 => Self::Draining,
            _ => Self::Starting,
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
    /// 建立启动中的 Worker control 状态。
    pub fn new(version: impl Into<String>, mail_pollers: usize) -> Self {
        Self {
            inner: Arc::new(WorkerControlInner {
                state: std::sync::atomic::AtomicU8::new(WorkerControlState::Starting as u8),
                ready: std::sync::atomic::AtomicBool::new(false),
                mail_poll_requested: std::sync::atomic::AtomicBool::new(false),
                version: version.into(),
                mail_pollers,
                poll_wakeup: Arc::new(Notify::new()),
                trigger_history: Mutex::new(TriggerHistory {
                    order: VecDeque::with_capacity(MAX_REMEMBERED_TRIGGER_COMMANDS),
                    results: std::collections::HashMap::with_capacity(
                        MAX_REMEMBERED_TRIGGER_COMMANDS,
                    ),
                }),
            }),
        }
    }

    /// 标记 Worker 已经可以接受控制请求。
    pub fn mark_serving(&self) {
        self.inner.ready.store(true, std::sync::atomic::Ordering::Release);
        self.inner
            .state
            .store(WorkerControlState::Serving as u8, std::sync::atomic::Ordering::Release);
    }

    /// 标记 Worker 存活但当前无法接受新工作。
    pub fn mark_degraded(&self) {
        self.inner.ready.store(false, std::sync::atomic::Ordering::Release);
        self.inner
            .state
            .store(WorkerControlState::Degraded as u8, std::sync::atomic::Ordering::Release);
    }

    /// 标记 Worker 正在收口；之后的手动唤醒会被拒绝。
    pub fn mark_draining(&self) {
        self.inner.ready.store(false, std::sync::atomic::Ordering::Release);
        self.inner
            .state
            .store(WorkerControlState::Draining as u8, std::sync::atomic::Ordering::Release);
    }

    /// 返回下一轮 MoogleMail 轮询使用的唤醒器。
    pub fn poll_wakeup(&self) -> Arc<Notify> {
        Arc::clone(&self.inner.poll_wakeup)
    }

    /// 取出一次手动轮询请求；Worker 主循环据此让所有邮箱忽略下一次调度时间。
    pub fn take_mail_poll_request(&self) -> bool {
        self.inner.mail_poll_requested.swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    fn live(&self) -> LiveResult {
        LiveResult { state: self.state_name().to_owned() }
    }

    fn ready(&self) -> ReadyResult {
        ReadyResult { ready: self.inner.ready.load(std::sync::atomic::Ordering::Acquire) }
    }

    fn status(&self) -> StatusResult {
        StatusResult {
            mail_pollers: self.inner.mail_pollers,
            ready: self.inner.ready.load(std::sync::atomic::Ordering::Acquire),
            state: self.state_name().to_owned(),
            version: self.inner.version.clone(),
        }
    }

    fn state_name(&self) -> &'static str {
        WorkerControlState::from_byte(self.inner.state.load(std::sync::atomic::Ordering::Acquire))
            .as_str()
    }

    fn trigger_poll(&self, command: TriggerPollCommand) -> Result<TriggerPollResult, TriggerError> {
        if command.command_id.is_empty()
            || command.command_id.len() > 64
            || command.command_id != command.command_id.trim()
            || command.command_id.chars().any(char::is_control)
            || Uuid::parse_str(&command.command_id).is_err()
        {
            return Err(TriggerError::InvalidCommand);
        }
        let mut history =
            self.inner.trigger_history.lock().map_err(|_| TriggerError::Unavailable)?;
        if let Some(result) = history.results.get(&command.command_id).cloned() {
            return Ok(result);
        }
        if !self.inner.ready.load(std::sync::atomic::Ordering::Acquire)
            || !matches!(
                WorkerControlState::from_byte(
                    self.inner.state.load(std::sync::atomic::Ordering::Acquire),
                ),
                WorkerControlState::Serving
            )
        {
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
