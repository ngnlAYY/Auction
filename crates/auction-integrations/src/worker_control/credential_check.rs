//! Admin 邮件凭据在线检查的 control client：只提交凭据 ID 与期望版本，把 Worker 的稳定业务码
//! 映射回凭据错误。Worker 未启用、不可达、超时或协议不兼容一律为“校验暂不可用”，不回退到 Admin
//! 本地直连。

use auction_application::admin::{AdminCredentialCheckObservation, AdminCredentialError};
use auction_application::core::{ApplicationError, ApplicationFuture};
use auction_application::operations::{
    WORKER_CONTROL_MAIL_CREDENTIAL_CHECK_PATH, WorkerMailCredentialCheckConflict,
    WorkerMailCredentialCheckPort, WorkerMailCredentialCheckRequest,
    WorkerMailCredentialCheckResponse,
};

use super::WorkerRemoteReadClient;

impl WorkerMailCredentialCheckPort for WorkerRemoteReadClient {
    fn check_mail_credential(
        &self,
        request: WorkerMailCredentialCheckRequest,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialCheckObservation>> {
        Box::pin(async move {
            if !request.validate() {
                return Err(AdminCredentialError::InvalidRequest.into());
            }
            let (status, envelope) = match self
                .exchange::<_, serde_json::Value>(
                    WORKER_CONTROL_MAIL_CREDENTIAL_CHECK_PATH,
                    &request,
                )
                .await
            {
                Ok(exchange) => exchange,
                Err(error) => {
                    tracing::warn!(
                        reason = error.as_str(),
                        "邮件凭据在线检查未能经 Worker control 完成"
                    );
                    return Err(unavailable());
                }
            };
            match (status.is_success(), envelope.code.as_str()) {
                (true, "ok") => envelope
                    .data
                    .and_then(|data| {
                        serde_json::from_value::<WorkerMailCredentialCheckResponse>(data).ok()
                    })
                    .and_then(|response| response.into_observations(&request))
                    .ok_or_else(unavailable),
                (_, "version_conflict") => {
                    let conflict = envelope
                        .data
                        .and_then(|data| {
                            serde_json::from_value::<WorkerMailCredentialCheckConflict>(data).ok()
                        })
                        .ok_or_else(unavailable)?;
                    Err(AdminCredentialError::VersionConflict {
                        current_version: conflict.current_version,
                    }
                    .into())
                }
                (_, "not_found") => Err(AdminCredentialError::NotFound.into()),
                (_, "invalid_command_id") => Err(AdminCredentialError::InvalidRequest.into()),
                (_, "busy") => Err(AdminCredentialError::CheckBusy.into()),
                // capability_unavailable、paused、超时与未知码：Worker 侧未完成检查。
                _ => Err(unavailable()),
            }
        })
    }
}

fn unavailable() -> ApplicationError {
    ApplicationError::AdminCredential(AdminCredentialError::CheckUnavailable)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use auction_application::admin::{
        AdminCredentialCheckObservation, AdminCredentialCheckStatus, AdminCredentialError,
        AdminCredentialWorld,
    };
    use auction_application::core::ApplicationError;
    use auction_application::operations::{
        WorkerMailCredentialCheckPort, WorkerMailCredentialCheckRequest, valid_control_socket_path,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;
    use uuid::Uuid;

    use crate::WorkerRemoteReadClient;

    const CREDENTIAL: Uuid = Uuid::from_u128(0xabc);

    /// 断言错误类别的判定函数。
    type ErrorMatcher = fn(&ApplicationError) -> bool;

    fn request() -> WorkerMailCredentialCheckRequest {
        WorkerMailCredentialCheckRequest { credential_id: CREDENTIAL, expected_state_version: 4 }
    }

    /// 与读取 client 测试同一路径约束的临时 socket。
    fn socket_path() -> (PathBuf, UnixListener) {
        let directory =
            std::env::temp_dir().join(format!("hv-wc-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&directory).expect("test directory");
        let path = directory.join("control.sock");
        assert!(valid_control_socket_path(&path), "TMPDIR 过长：{}", path.display());
        let listener = UnixListener::bind(&path).expect("bind test control socket");
        (path, listener)
    }

    /// 极简 UDS HTTP/1.1 应答器：回显请求标识与协议版本，返回固定外壳，并交回请求路径与正文。
    async fn serve_once(listener: UnixListener, status_line: &'static str, body: String) -> String {
        let (mut socket, _) = listener.accept().await.expect("test client connects");
        // 读到请求头与 content-length 声明的完整正文为止；正文可能与请求头分开到达。
        let mut raw = Vec::new();
        let mut chunk = [0_u8; 4096];
        let request = loop {
            let read = socket.read(&mut chunk).await.expect("request is readable");
            raw.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&raw).into_owned();
            let complete = text.find("\r\n\r\n").is_some_and(|end| {
                let length = text
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                raw.len() >= end + 4 + length
            });
            if complete || read == 0 {
                break text;
            }
        };
        let request_id = request
            .lines()
            .find_map(|line| line.strip_prefix("x-request-id: "))
            .unwrap_or_default()
            .trim()
            .to_owned();
        let response = format!(
            "{status_line}\r\ncontent-type: application/json\r\nx-worker-control-version: v1\r\nx-request-id: {request_id}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.expect("response is writable");
        request
    }

    async fn call(
        status_line: &'static str,
        body: String,
    ) -> (Result<Vec<AdminCredentialCheckObservation>, ApplicationError>, String) {
        let (path, listener) = socket_path();
        let server = tokio::spawn(serve_once(listener, status_line, body));
        let client =
            WorkerRemoteReadClient::new(path, Duration::from_secs(5)).expect("client builds");
        let result = client.check_mail_credential(request()).await;
        (result, server.await.expect("server task"))
    }

    #[tokio::test]
    async fn ok_result_is_bound_to_the_request_and_carries_no_secret() {
        let (result, sent) = call(
            "HTTP/1.1 200 OK",
            format!(
                r#"{{"code":"ok","message":"m","data":{{"credential_id":"{CREDENTIAL}","state_version":4,"checks":[{{"world":"main","status":"valid"}}]}}}}"#
            ),
        )
        .await;
        assert_eq!(
            result.expect("Worker 完成检查"),
            vec![AdminCredentialCheckObservation {
                world: AdminCredentialWorld::Main,
                status: AdminCredentialCheckStatus::Valid,
            }]
        );
        assert!(sent.starts_with("POST /v1/credentials/mail:check "), "{sent}");
        assert!(
            sent.ends_with(&format!(
                r#"{{"credential_id":"{CREDENTIAL}","expected_state_version":4}}"#
            )),
            "请求正文只有凭据 ID 与期望版本：{sent}"
        );
    }

    #[tokio::test]
    async fn worker_codes_map_to_credential_errors_and_everything_else_is_unavailable() {
        let cases: [(&str, String, ErrorMatcher); 7] = [
            (
                "HTTP/1.1 409 Conflict",
                r#"{"code":"version_conflict","message":"m","data":{"current_version":9}}"#
                    .to_owned(),
                |error| {
                    matches!(
                        error,
                        ApplicationError::AdminCredential(AdminCredentialError::VersionConflict {
                            current_version: 9
                        })
                    )
                },
            ),
            (
                "HTTP/1.1 404 Not Found",
                r#"{"code":"not_found","message":"m","data":null}"#.to_owned(),
                |error| {
                    matches!(
                        error,
                        ApplicationError::AdminCredential(AdminCredentialError::NotFound)
                    )
                },
            ),
            (
                "HTTP/1.1 429 Too Many Requests",
                r#"{"code":"busy","message":"m","data":null}"#.to_owned(),
                |error| {
                    matches!(
                        error,
                        ApplicationError::AdminCredential(AdminCredentialError::CheckBusy)
                    )
                },
            ),
            (
                "HTTP/1.1 501 Not Implemented",
                r#"{"code":"capability_unavailable","message":"m","data":null}"#.to_owned(),
                is_unavailable,
            ),
            (
                "HTTP/1.1 504 Gateway Timeout",
                r#"{"code":"unavailable","message":"m","data":null}"#.to_owned(),
                is_unavailable,
            ),
            // 结果绑定的不是本次请求的版本：按协议不兼容拒收，不展示旧结果。
            (
                "HTTP/1.1 200 OK",
                format!(
                    r#"{{"code":"ok","message":"m","data":{{"credential_id":"{CREDENTIAL}","state_version":5,"checks":[{{"world":"main","status":"valid"}}]}}}}"#
                ),
                is_unavailable,
            ),
            (
                "HTTP/1.1 200 OK",
                r#"{"code":"ok","message":"m","data":{"cookie":"secret"}}"#.to_owned(),
                is_unavailable,
            ),
        ];
        for (status_line, body, expected) in cases {
            let (result, _) = call(status_line, body.clone()).await;
            let error = result.expect_err("Worker 拒绝或协议不兼容");
            assert!(expected(&error), "{status_line} {body}: {error:?}");
        }
    }

    /// Worker 未启用或 socket 不存在：检查失败关闭为暂不可用，不回退到本地直连。
    #[tokio::test]
    async fn missing_worker_socket_is_unavailable() {
        let directory =
            std::env::temp_dir().join(format!("hv-wc-{}", uuid::Uuid::new_v4().simple()));
        let client =
            WorkerRemoteReadClient::new(directory.join("control.sock"), Duration::from_secs(1))
                .expect("client builds");
        let error = client.check_mail_credential(request()).await.expect_err("Worker 不可达");
        assert!(is_unavailable(&error), "{error:?}");
    }

    fn is_unavailable(error: &ApplicationError) -> bool {
        matches!(error, ApplicationError::AdminCredential(AdminCredentialError::CheckUnavailable))
    }
}
