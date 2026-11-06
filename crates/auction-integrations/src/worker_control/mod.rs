//! W1.10：Admin 与 CLI 的远端只读改经 Worker control 代为执行的客户侧适配器。
//! 本模块只拥有 Unix socket 传输与稳定错误分类；凭据、请求门、账号熔断与 W1.9 主动暂停
//! 都由 Worker 进程裁决，调用方不得回退到本地直连。

use auction_application::operations::{
    WORKER_RECOVERY_SCAN_TIMEOUT_SECONDS, WorkerEquipmentReadRequest, WorkerEquipmentReadResponse,
    WorkerRecoveryScanRequest, WorkerRecoveryScanResponse, WorkerRemoteReadError,
    WorkerRemoteReadFuture, WorkerRemoteReadPort, WorkerSentLookupRequest,
    WorkerSentLookupResponse,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Component, Path},
    time::Duration,
};
use url::Url;

/// 与 Worker control server 保持一致的协议版本。
const PROTOCOL_VERSION: &str = "v1";
const EQUIPMENT_READ_PATH: &str = "/v1/equipment/reads";
const SENT_LOOKUP_PATH: &str = "/v1/mail/sent:lookup";
const RECOVERY_SCAN_PATH: &str = "/v1/recovery/external:scan";
const HEADER_PROTOCOL_VERSION: &str = "x-worker-control-version";
const HEADER_REQUEST_ID: &str = "x-request-id";
/// 恢复扫描响应最多包含 2000 条摘要；与其他只读路由共用 1 MiB 上界。
const MAX_RESPONSE_BYTES: usize = 1 << 20;

/// 构造期错误；运行期错误统一走 `WorkerRemoteReadError`。
#[derive(Debug, thiserror::Error)]
pub enum WorkerRemoteReadClientError {
    #[error("Worker control socket 路径无效")]
    InvalidSocketPath,
    #[error("Worker control client 建立失败：{0}")]
    Client(#[source] std::io::Error),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    code: String,
    message: String,
    data: Option<T>,
}

/// 只连接 Worker control Unix socket 的远端读取 client。
#[derive(Clone)]
pub struct WorkerRemoteReadClient {
    client: reqwest::Client,
}

impl WorkerRemoteReadClient {
    /// 建立 client；不会在构造期间连接 socket。超时上界与 server 中间件一致。
    pub fn new(
        socket_path: impl AsRef<Path>,
        request_timeout: Duration,
    ) -> Result<Self, WorkerRemoteReadClientError> {
        let socket_path = socket_path.as_ref().to_owned();
        if !valid_absolute_path(&socket_path) {
            return Err(WorkerRemoteReadClientError::InvalidSocketPath);
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .unix_socket(socket_path)
            .no_proxy()
            .timeout(request_timeout.min(Duration::from_secs(WORKER_RECOVERY_SCAN_TIMEOUT_SECONDS)))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| WorkerRemoteReadClientError::Client(std::io::Error::other(error)))?;
        Ok(Self { client })
    }

    async fn post<I, T>(&self, path: &str, input: &I) -> Result<T, WorkerRemoteReadError>
    where
        I: Serialize,
        T: for<'de> Deserialize<'de>,
    {
        let request_id = uuid::Uuid::new_v4().to_string();
        let body = serde_json::to_vec(input).map_err(|error| {
            let _ = error;
            WorkerRemoteReadError::Invalid
        })?;
        let url = Url::parse(&format!("http://worker-control{path}")).map_err(|error| {
            let _ = error;
            WorkerRemoteReadError::Incompatible
        })?;
        let response = self
            .client
            .post(url)
            .header(HEADER_REQUEST_ID, &request_id)
            .header(HEADER_PROTOCOL_VERSION, PROTOCOL_VERSION)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(classify_request_error)?;
        let status = response.status();
        if status.is_success()
            && (response
                .headers()
                .get(HEADER_PROTOCOL_VERSION)
                .and_then(|value| value.to_str().ok())
                != Some(PROTOCOL_VERSION)
                || response.headers().get(HEADER_REQUEST_ID).and_then(|value| value.to_str().ok())
                    != Some(request_id.as_str()))
        {
            return Err(WorkerRemoteReadError::Incompatible);
        }
        if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
            return Err(WorkerRemoteReadError::Incompatible);
        }
        let mut raw = Vec::new();
        let mut response = response;
        while let Some(chunk) = response.chunk().await.map_err(classify_request_error)? {
            if chunk.len() > MAX_RESPONSE_BYTES - raw.len() {
                return Err(WorkerRemoteReadError::Incompatible);
            }
            raw.extend_from_slice(&chunk);
        }
        let envelope: Envelope<T> = serde_json::from_slice(&raw).map_err(|error| {
            let _ = error;
            WorkerRemoteReadError::Incompatible
        })?;
        if envelope.message.is_empty() {
            return Err(WorkerRemoteReadError::Incompatible);
        }
        match (status.is_success(), envelope.code.as_str()) {
            (true, "ok") => envelope.data.ok_or(WorkerRemoteReadError::Incompatible),
            (_, "paused") => Err(WorkerRemoteReadError::Paused),
            (_, "not_paused") => Err(WorkerRemoteReadError::NotPaused),
            (_, "capability_unavailable") => Err(WorkerRemoteReadError::Unavailable),
            (_, "invalid_command_id") => Err(WorkerRemoteReadError::Invalid),
            _ => Err(match status.as_u16() {
                501 | 503 => WorkerRemoteReadError::Unavailable,
                504 => WorkerRemoteReadError::Timeout,
                _ => WorkerRemoteReadError::Incompatible,
            }),
        }
    }
}

impl WorkerRemoteReadPort for WorkerRemoteReadClient {
    fn read_equipment(
        &self,
        request: WorkerEquipmentReadRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerEquipmentReadResponse> {
        Box::pin(async move {
            if !request.validate() {
                return Err(WorkerRemoteReadError::Invalid);
            }
            self.post(EQUIPMENT_READ_PATH, &request).await
        })
    }

    fn lookup_sent(
        &self,
        request: WorkerSentLookupRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerSentLookupResponse> {
        Box::pin(async move {
            if !request.validate() {
                return Err(WorkerRemoteReadError::Invalid);
            }
            self.post(SENT_LOOKUP_PATH, &request).await
        })
    }

    fn recovery_scan(
        &self,
        request: WorkerRecoveryScanRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerRecoveryScanResponse> {
        Box::pin(async move {
            if !request.validate() {
                return Err(WorkerRemoteReadError::Invalid);
            }
            let response: WorkerRecoveryScanResponse =
                self.post(RECOVERY_SCAN_PATH, &request).await?;
            if response.validate() {
                Ok(response)
            } else {
                Err(WorkerRemoteReadError::Incompatible)
            }
        })
    }
}

fn classify_request_error(error: reqwest::Error) -> WorkerRemoteReadError {
    if error.is_timeout() {
        WorkerRemoteReadError::Timeout
    } else if error.is_connect() {
        WorkerRemoteReadError::Unavailable
    } else if error.is_decode() {
        WorkerRemoteReadError::Incompatible
    } else {
        WorkerRemoteReadError::Unavailable
    }
}

/// 只接受规范化的绝对路径，拒绝 `..` 与空组件。
fn valid_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path.as_os_str().len() <= 4096
        && path.components().all(|component| {
            matches!(component, Component::RootDir | Component::Normal(_) | Component::CurDir)
        })
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;
    use auction_application::operations::WorkerRemoteReadPort;
    use std::path::PathBuf;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    /// 极简 UDS HTTP/1.1 应答器：只验证 client 侧的协议、路径与错误分类。
    async fn serve_once(
        listener: UnixListener,
        status_line: &'static str,
        code: &'static str,
        data: Option<&'static str>,
        version: &'static str,
    ) -> String {
        let (mut socket, _) = listener.accept().await.expect("test client connects");
        let mut buffer = vec![0_u8; 4096];
        let read = socket.read(&mut buffer).await.expect("request is readable");
        let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
        let request_id = request
            .lines()
            .find_map(|line| line.strip_prefix("x-request-id: "))
            .unwrap_or_default()
            .trim()
            .to_owned();
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or_default()
            .to_owned();
        let body = match data {
            Some(data) => format!("{{\"code\":\"{code}\",\"message\":\"m\",\"data\":{data}}}"),
            None => format!("{{\"code\":\"{code}\",\"message\":\"m\",\"data\":null}}"),
        };
        let response = format!(
            "{status_line}\r\ncontent-type: application/json\r\nx-worker-control-version: {version}\r\nx-request-id: {request_id}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.expect("response is writable");
        path
    }

    fn socket_path() -> (PathBuf, UnixListener) {
        let directory = std::env::temp_dir().join(format!(
            "auction-worker-control-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).expect("test directory");
        let path = directory.join("control.sock");
        let listener = UnixListener::bind(&path).expect("bind test control socket");
        (path, listener)
    }

    fn client(path: PathBuf) -> WorkerRemoteReadClient {
        WorkerRemoteReadClient::new(path, Duration::from_secs(5)).expect("client builds")
    }

    fn equipment_request() -> WorkerEquipmentReadRequest {
        WorkerEquipmentReadRequest {
            world_id: "main".to_owned(),
            equipment_url: "https://hentaiverse.org/equip/300753957/0123456789".to_owned(),
        }
    }

    #[tokio::test]
    async fn ok_envelope_is_decoded_from_the_worker_socket() {
        let (path, listener) = socket_path();
        let server = tokio::spawn(serve_once(
            listener,
            "HTTP/1.1 200 OK",
            "ok",
            Some(concat!(
                "{\"adapter\":\"hentaiverse-equipment-v1\",\"source_fingerprint\":[1,1,1,1,1,1,1,1,",
                "1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],",
                "\"external_equipment_id\":\"300753957\",\"name\":\"Legendary Phase Shoes\",",
                "\"category\":\"Shoes\",\"level\":500,\"tradeable\":true,",
                "\"attributes\":{},\"collected_at\":\"2026-11-04T00:00:00Z\",",
                "\"key\":\"0123456789\"}"
            )),
            "v1",
        ));
        let response = client(path)
            .read_equipment(equipment_request())
            .await
            .expect("worker accepted the read");
        assert_eq!(response.external_equipment_id, "300753957");
        assert_eq!(response.key, "0123456789");
        assert_eq!(server.await.expect("server task"), "/v1/equipment/reads");
    }

    #[tokio::test]
    async fn paused_and_unavailable_codes_are_classified() {
        let (path, listener) = socket_path();
        let server =
            tokio::spawn(serve_once(listener, "HTTP/1.1 409 Conflict", "paused", None, "v1"));
        let error = client(path).read_equipment(equipment_request()).await.expect_err("paused");
        assert_eq!(error, WorkerRemoteReadError::Paused);
        server.await.expect("server task");

        let (path, listener) = socket_path();
        let server = tokio::spawn(serve_once(
            listener,
            "HTTP/1.1 501 Not Implemented",
            "capability_unavailable",
            None,
            "v1",
        ));
        let error = client(path).read_equipment(equipment_request()).await.expect_err("down");
        assert_eq!(error, WorkerRemoteReadError::Unavailable);
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn protocol_version_mismatch_and_malformed_envelope_are_incompatible() {
        let (path, listener) = socket_path();
        let server = tokio::spawn(serve_once(listener, "HTTP/1.1 200 OK", "ok", Some("{}"), "v2"));
        let error = client(path).read_equipment(equipment_request()).await;
        assert!(
            matches!(error, Err(WorkerRemoteReadError::Incompatible)),
            "unexpected result: {error:?}"
        );
        server.await.expect("server task");

        // 协议版本正确但 data 形状不符时同样失败关闭，不猜测半成品结果。
        let (path, listener) = socket_path();
        let server = tokio::spawn(serve_once(listener, "HTTP/1.1 200 OK", "ok", Some("{}"), "v1"));
        let error = client(path).read_equipment(equipment_request()).await;
        assert!(matches!(error, Err(WorkerRemoteReadError::Incompatible)));
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn invalid_socket_paths_are_rejected_before_connecting() {
        assert!(matches!(
            WorkerRemoteReadClient::new("relative/control.sock", Duration::from_secs(1)),
            Err(WorkerRemoteReadClientError::InvalidSocketPath)
        ));
        assert!(matches!(
            WorkerRemoteReadClient::new("/tmp/../etc/control.sock", Duration::from_secs(1)),
            Err(WorkerRemoteReadClientError::InvalidSocketPath)
        ));
    }
}
