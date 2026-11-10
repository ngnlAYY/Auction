//! Admin 进程使用的 Worker control 命令/状态 client（UDS-only）。
//! 只实现 `WorkerControlPort`：状态、邮件进度、外部账号快照与唤醒命令。协议常量与线上
//! DTO 由 `auction-application` 拥有；本模块只保留传输、超时与稳定错误分类。

use std::time::Duration;

use auction_application::auction::ApplicationFuture;
use auction_application::external_accounts::ExternalAccountStatus;
use auction_application::operations::{
    WORKER_CONTROL_EXTERNAL_ACCOUNTS_PATH, WORKER_CONTROL_HEADER_REQUEST_ID,
    WORKER_CONTROL_HEADER_VERSION, WORKER_CONTROL_MAIL_PROGRESS_PATH,
    WORKER_CONTROL_PROTOCOL_VERSION, WORKER_CONTROL_STATUS_PATH, WORKER_CONTROL_TRIGGER_POLL_PATH,
    WorkerControlCondition, WorkerControlEnvelope, WorkerControlExternalAccounts,
    WorkerControlMailProgress, WorkerControlMailProgressResult, WorkerControlPort,
    WorkerControlStatus, WorkerControlStatusResult, WorkerControlTrigger,
    WorkerControlTriggerPollCommand, WorkerControlTriggerPollResult,
};
use reqwest::redirect::Policy;
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use super::valid_absolute_path;

/// 与 server 中间件一致的状态/命令响应上限；远端读取有独立的 1 MiB 上界。
const MAX_RESPONSE_BYTES: usize = 64 << 10;

/// Worker control 的稳定错误分类；不保存远端正文。
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
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

/// 构造期错误；运行期错误统一走 `WorkerControlClientError`。
#[derive(Debug, thiserror::Error)]
pub enum WorkerControlClientBuildError {
    #[error("Worker control socket 路径无效")]
    InvalidSocketPath,
    #[error("Worker control client 建立失败：{0}")]
    Client(#[source] std::io::Error),
}

/// Admin 进程使用的 UDS-only Worker control client。
#[derive(Clone)]
pub struct WorkerControlClient {
    client: reqwest::Client,
}

impl WorkerControlClient {
    /// 创建 client；不会在构造期间连接 socket。
    pub fn new(
        socket_path: impl AsRef<std::path::Path>,
        request_timeout: Duration,
    ) -> Result<Self, WorkerControlClientBuildError> {
        let socket_path = socket_path.as_ref().to_owned();
        if !valid_absolute_path(&socket_path) {
            return Err(WorkerControlClientBuildError::InvalidSocketPath);
        }
        // Worker control 使用 reqwest，即使通过 Unix socket 连接也需要先安装 Rustls provider。
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .unix_socket(socket_path)
            .no_proxy()
            .timeout(request_timeout)
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|error| WorkerControlClientBuildError::Client(std::io::Error::other(error)))?;
        Ok(Self { client })
    }

    /// 读取 Worker control 状态；错误只保留稳定分类。
    pub async fn fetch_status(
        &self,
    ) -> Result<WorkerControlStatusResult, WorkerControlClientError> {
        self.request(WORKER_CONTROL_STATUS_PATH, None::<()>, "ok").await
    }

    async fn fetch_mail_progress(
        &self,
    ) -> Result<WorkerControlMailProgressResult, WorkerControlClientError> {
        self.request(WORKER_CONTROL_MAIL_PROGRESS_PATH, None::<()>, "ok").await
    }

    /// 读取外部账号运行时快照；只接受当前协议形状。
    async fn fetch_external_accounts(
        &self,
    ) -> Result<ExternalAccountStatus, WorkerControlClientError> {
        self.request(WORKER_CONTROL_EXTERNAL_ACCOUNTS_PATH, None::<()>, "ok").await
    }

    /// 触发 Worker 唤醒所有已装配的 MoogleMail poller。
    pub async fn trigger_poll(
        &self,
        command_id: Uuid,
    ) -> Result<WorkerControlTriggerPollResult, WorkerControlClientError> {
        let input = WorkerControlTriggerPollCommand { command_id: command_id.to_string() };
        self.request(WORKER_CONTROL_TRIGGER_POLL_PATH, Some(input), "accepted").await
    }

    async fn request<T, I>(
        &self,
        path: &str,
        input: Option<I>,
        expected_code: &str,
    ) -> Result<T, WorkerControlClientError>
    where
        T: DeserializeOwned,
        I: Serialize,
    {
        let request_id = Uuid::new_v4().to_string();
        let url = format!("http://worker-control{path}");
        let mut request = match input {
            Some(input) => self.client.post(url).json(&input),
            None => self.client.get(url),
        };
        request = request
            .header(WORKER_CONTROL_HEADER_REQUEST_ID, &request_id)
            .header(WORKER_CONTROL_HEADER_VERSION, WORKER_CONTROL_PROTOCOL_VERSION);
        let mut response = request.send().await.map_err(classify_request_error)?;
        let status = response.status();
        if status == reqwest::StatusCode::GATEWAY_TIMEOUT {
            return Err(WorkerControlClientError::Timeout);
        }
        if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            return Err(WorkerControlClientError::Unavailable);
        }
        if !status.is_success() {
            return Err(WorkerControlClientError::Rejected);
        }
        let protocol_version = response
            .headers()
            .get(WORKER_CONTROL_HEADER_VERSION)
            .and_then(|value| value.to_str().ok());
        let response_request_id = response
            .headers()
            .get(WORKER_CONTROL_HEADER_REQUEST_ID)
            .and_then(|value| value.to_str().ok());
        if protocol_version != Some(WORKER_CONTROL_PROTOCOL_VERSION)
            || response_request_id != Some(request_id.as_str())
            || !response.headers().get(reqwest::header::CONTENT_TYPE).is_some_and(|value| {
                value.to_str().ok().is_some_and(|value| {
                    value.split(';').next().is_some_and(|value| value.trim() == "application/json")
                })
            })
        {
            return Err(WorkerControlClientError::Incompatible);
        }
        if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
            return Err(WorkerControlClientError::Incompatible);
        }
        // Content-Length 可以缺失；逐块检查，不能先无界收集正文再拒绝超限响应。
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(classify_request_error)? {
            if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
                return Err(WorkerControlClientError::Incompatible);
            }
            body.extend_from_slice(&chunk);
        }
        let envelope: WorkerControlEnvelope<T> =
            serde_json::from_slice(&body).map_err(|error| {
                // 解码错误只记录形状与位置，不记录响应正文。
                tracing::debug!(error = %error, "Worker control 响应解码失败");
                WorkerControlClientError::Incompatible
            })?;
        if envelope.code != expected_code || envelope.message.is_empty() {
            return Err(WorkerControlClientError::Incompatible);
        }
        envelope.data.ok_or(WorkerControlClientError::Incompatible)
    }
}

fn classify_request_error(error: reqwest::Error) -> WorkerControlClientError {
    if error.is_timeout() {
        WorkerControlClientError::Timeout
    } else {
        WorkerControlClientError::Unavailable
    }
}

fn valid_status(status: &WorkerControlStatusResult) -> bool {
    matches!(status.state.as_str(), "starting" | "serving" | "degraded" | "draining")
        && !status.version.is_empty()
        && status.version.len() <= 64
        && status.version == status.version.trim()
        && status.version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | '-')
        })
}

fn condition_for(error: WorkerControlClientError) -> WorkerControlCondition {
    match error {
        WorkerControlClientError::Unavailable => WorkerControlCondition::Unavailable,
        WorkerControlClientError::Timeout => WorkerControlCondition::Timeout,
        WorkerControlClientError::Incompatible => WorkerControlCondition::Incompatible,
        WorkerControlClientError::Rejected => WorkerControlCondition::Rejected,
    }
}

impl WorkerControlPort for WorkerControlClient {
    fn mail_progress(&self) -> ApplicationFuture<'_, WorkerControlMailProgress> {
        Box::pin(async move {
            match self.fetch_mail_progress().await {
                Ok(result) => {
                    let snapshot = WorkerControlMailProgress {
                        condition: WorkerControlCondition::Available,
                        observed_at: Some(result.observed_at),
                        pollers: result.pollers,
                    };
                    if snapshot.validate() {
                        Ok(snapshot)
                    } else {
                        Ok(WorkerControlMailProgress::unavailable(
                            WorkerControlCondition::Incompatible,
                        ))
                    }
                }
                Err(error) => Ok(WorkerControlMailProgress::unavailable(condition_for(error))),
            }
        })
    }

    fn external_accounts(&self) -> ApplicationFuture<'_, WorkerControlExternalAccounts> {
        Box::pin(async move {
            match self.fetch_external_accounts().await {
                Ok(status) if status.validate() => Ok(WorkerControlExternalAccounts {
                    condition: WorkerControlCondition::Available,
                    status: Some(status),
                }),
                Ok(_) => Ok(WorkerControlExternalAccounts::unavailable(
                    WorkerControlCondition::Incompatible,
                )),
                Err(error) => Ok(WorkerControlExternalAccounts::unavailable(condition_for(error))),
            }
        })
    }

    fn status(&self) -> ApplicationFuture<'_, WorkerControlStatus> {
        Box::pin(async move {
            match self.fetch_status().await {
                Ok(status) if valid_status(&status) => {
                    let Ok(mail_pollers) = i64::try_from(status.mail_pollers) else {
                        return Ok(WorkerControlStatus::unavailable(
                            WorkerControlCondition::Incompatible,
                        ));
                    };
                    Ok(WorkerControlStatus {
                        condition: WorkerControlCondition::Available,
                        state: status.state,
                        ready: status.ready,
                        version: status.version,
                        mail_pollers,
                    })
                }
                Ok(_) => Ok(WorkerControlStatus::unavailable(WorkerControlCondition::Incompatible)),
                Err(error) => Ok(WorkerControlStatus::unavailable(condition_for(error))),
            }
        })
    }

    fn trigger_poll(&self, command_id: Uuid) -> ApplicationFuture<'_, WorkerControlTrigger> {
        Box::pin(async move {
            match self.trigger_poll(command_id).await {
                Ok(result)
                    if result.accepted
                        && result.command_id == command_id.to_string()
                        && result.pollers > 0 =>
                {
                    let Ok(pollers) = i64::try_from(result.pollers) else {
                        return Ok(WorkerControlTrigger {
                            condition: WorkerControlCondition::Incompatible,
                            accepted: false,
                            pollers: 0,
                        });
                    };
                    Ok(WorkerControlTrigger {
                        condition: WorkerControlCondition::Available,
                        accepted: true,
                        pollers,
                    })
                }
                Ok(_) => Ok(WorkerControlTrigger {
                    condition: WorkerControlCondition::Incompatible,
                    accepted: false,
                    pollers: 0,
                }),
                Err(error) => Ok(WorkerControlTrigger {
                    condition: condition_for(error),
                    accepted: false,
                    pollers: 0,
                }),
            }
        })
    }
}
