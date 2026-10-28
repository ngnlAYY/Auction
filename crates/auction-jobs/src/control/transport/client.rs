//! Worker control Admin client 与应用端口映射。

use super::*;

/// Admin 进程使用的 UDS-only Worker control client。
#[derive(Clone)]
pub struct WorkerControlClient {
    client: reqwest::Client,
}

impl WorkerControlClient {
    /// 创建 client；不会在构造期间连接 socket。
    pub fn new(
        socket_path: impl AsRef<Path>,
        request_timeout: Duration,
    ) -> Result<Self, WorkerControlServerError> {
        let socket_path = socket_path.as_ref().to_owned();
        validate_socket_path(&socket_path)?;
        // Worker control 使用 reqwest，即使通过 Unix socket 连接也需要先安装 Rustls provider。
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .unix_socket(socket_path)
            .no_proxy()
            .timeout(request_timeout)
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|error| WorkerControlServerError::Io(std::io::Error::other(error)))?;
        Ok(Self { client })
    }

    /// 读取 Worker control 状态；错误只保留稳定分类。
    pub async fn fetch_status(&self) -> Result<StatusResult, WorkerControlClientError> {
        self.request(WORKER_CONTROL_STATUS_PATH, None::<()>, "ok").await
    }

    async fn fetch_mail_progress(&self) -> Result<MailProgressResult, WorkerControlClientError> {
        self.request(WORKER_CONTROL_MAIL_PROGRESS_PATH, None::<()>, "ok").await
    }

    /// 触发 Worker 唤醒所有已装配的 MoogleMail poller。
    pub async fn trigger_poll(
        &self,
        command_id: Uuid,
    ) -> Result<TriggerPollResult, WorkerControlClientError> {
        let input = TriggerPollCommand { command_id: command_id.to_string() };
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
            .header(HEADER_REQUEST_ID, &request_id)
            .header(HEADER_PROTOCOL_VERSION, WORKER_CONTROL_PROTOCOL_VERSION);
        let mut response = request.send().await.map_err(classify_request_error)?;
        let status = response.status();
        if status == StatusCode::GATEWAY_TIMEOUT {
            return Err(WorkerControlClientError::Timeout);
        }
        if status == StatusCode::SERVICE_UNAVAILABLE {
            return Err(WorkerControlClientError::Unavailable);
        }
        if !status.is_success() {
            return Err(WorkerControlClientError::Rejected);
        }
        let protocol_version =
            response.headers().get(HEADER_PROTOCOL_VERSION).and_then(|value| value.to_str().ok());
        let response_request_id =
            response.headers().get(HEADER_REQUEST_ID).and_then(|value| value.to_str().ok());
        if protocol_version != Some(WORKER_CONTROL_PROTOCOL_VERSION)
            || response_request_id != Some(request_id.as_str())
            || !response.headers().get(header::CONTENT_TYPE).is_some_and(|value| {
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
        let envelope: ControlEnvelope<T> =
            serde_json::from_slice(&body).map_err(|_| WorkerControlClientError::Incompatible)?;
        if envelope.code != expected_code || envelope.message.is_empty() {
            return Err(WorkerControlClientError::Incompatible);
        }
        envelope.data.ok_or(WorkerControlClientError::Incompatible)
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
                Err(error) => Ok(WorkerControlMailProgress::unavailable(match error {
                    WorkerControlClientError::Unavailable => WorkerControlCondition::Unavailable,
                    WorkerControlClientError::Timeout => WorkerControlCondition::Timeout,
                    WorkerControlClientError::Incompatible => WorkerControlCondition::Incompatible,
                    WorkerControlClientError::Rejected => WorkerControlCondition::Rejected,
                })),
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
                Err(error) => Ok(WorkerControlStatus::unavailable(match error {
                    WorkerControlClientError::Unavailable => WorkerControlCondition::Unavailable,
                    WorkerControlClientError::Timeout => WorkerControlCondition::Timeout,
                    WorkerControlClientError::Incompatible => WorkerControlCondition::Incompatible,
                    WorkerControlClientError::Rejected => WorkerControlCondition::Rejected,
                })),
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
                    condition: match error {
                        WorkerControlClientError::Unavailable => {
                            WorkerControlCondition::Unavailable
                        }
                        WorkerControlClientError::Timeout => WorkerControlCondition::Timeout,
                        WorkerControlClientError::Incompatible => {
                            WorkerControlCondition::Incompatible
                        }
                        WorkerControlClientError::Rejected => WorkerControlCondition::Rejected,
                    },
                    accepted: false,
                    pollers: 0,
                }),
            }
        })
    }
}

pub(super) fn valid_status(status: &StatusResult) -> bool {
    matches!(status.state.as_str(), "starting" | "serving" | "degraded" | "draining")
        && !status.version.is_empty()
        && status.version.len() <= 64
        && status.version == status.version.trim()
        && status.version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | '-')
        })
}

pub(super) fn classify_request_error(error: reqwest::Error) -> WorkerControlClientError {
    if error.is_timeout() {
        WorkerControlClientError::Timeout
    } else {
        WorkerControlClientError::Unavailable
    }
}

pub(super) fn validate_socket_path(path: &Path) -> Result<(), WorkerControlServerError> {
    if !valid_absolute_path(path) || path.as_os_str().len() > 100 {
        return Err(WorkerControlServerError::InvalidSocketPath);
    }
    Ok(())
}

pub(super) fn validate_runtime_directory_path(path: &Path) -> Result<(), WorkerControlServerError> {
    if !valid_absolute_path(path) {
        return Err(WorkerControlServerError::InvalidRuntimeDirectory);
    }
    Ok(())
}

pub(super) fn valid_absolute_path(path: &Path) -> bool {
    let normalized: PathBuf = path.components().collect();
    path.is_absolute()
        && path != Path::new("/")
        && normalized.as_os_str() == path.as_os_str()
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        && !path.to_string_lossy().chars().any(|character| character == '\0')
}

pub(super) fn valid_safe_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_alphanumeric())
        && value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(super) fn duplicate_u32(values: &[u32]) -> bool {
    let mut values = values.to_vec();
    values.sort_unstable();
    values.windows(2).any(|pair| pair[0] == pair[1])
}
