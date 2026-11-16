//! Admin 邮件凭据在线检查的 Worker 侧路由。请求只含凭据 ID 与期望状态版本；Worker 自己按 ID
//! 读取信封、前后复核版本，并经与邮件循环共用请求门、在途锁和账号忙熔断的客户端探测。响应只含
//! 世界与状态码，错误只给稳定分类，不回显 Cookie、正文或错误 cause。

use std::sync::Arc;

use auction_application::admin::{AdminCredentialCheckPort, AdminCredentialError};
use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::operations::{
    WORKER_CONTROL_MAIL_CREDENTIAL_CHECK_PATH, WorkerMailCredentialCheckConflict,
    WorkerMailCredentialCheckRequest, WorkerMailCredentialCheckResponse,
};
use axum::body::Bytes;
use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;

use super::super::protocol::ResponseCode;
use super::super::protocol::response;
use super::{json_request, request_id_or_response};
use crate::worker::diagnostics;

/// 未装配时（未启用邮件、或 Worker 没有凭据数据密钥）明确返回能力不可用，Admin 不得改为直连。
pub(in crate::control::transport) async fn mail_credential_check(
    Extension(checks): Extension<Option<Arc<dyn AdminCredentialCheckPort>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id_or_response(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let request: WorkerMailCredentialCheckRequest = match json_request(&headers, &body, &request_id)
    {
        Ok(request) => request,
        Err(response) => return *response,
    };
    if !request.validate() {
        return response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "凭据检查请求无效",
            Option::<()>::None,
            &request_id,
        );
    }
    let Some(checks) = checks else {
        return response(
            StatusCode::NOT_IMPLEMENTED,
            ResponseCode::CapabilityUnavailable,
            "Worker 未装配邮件凭据检查能力",
            Option::<WorkerMailCredentialCheckResponse>::None,
            &request_id,
        );
    };
    match checks.check(request.credential_id, request.expected_state_version).await {
        Ok(result) => response(
            StatusCode::OK,
            ResponseCode::Ok,
            "邮件凭据检查完成",
            Some(WorkerMailCredentialCheckResponse::new(
                result.credential_id,
                result.state_version,
                &result.checks,
            )),
            &request_id,
        ),
        Err(error) => check_error_response(error, &request_id),
    }
}

/// 业务拒绝按原类别回给 Admin；其余失败统一为能力不可用，并只在 Worker 记录安全分类。
fn check_error_response(error: ApplicationError, request_id: &str) -> Response {
    match error {
        ApplicationError::AdminCredential(AdminCredentialError::VersionConflict {
            current_version,
        }) => response(
            StatusCode::CONFLICT,
            ResponseCode::VersionConflict,
            "凭据状态版本已变化",
            Some(WorkerMailCredentialCheckConflict { current_version }),
            request_id,
        ),
        ApplicationError::AdminCredential(AdminCredentialError::NotFound) => response(
            StatusCode::NOT_FOUND,
            ResponseCode::NotFound,
            "凭据不存在",
            Option::<()>::None,
            request_id,
        ),
        ApplicationError::AdminCredential(AdminCredentialError::InvalidRequest) => response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "凭据检查请求无效",
            Option::<()>::None,
            request_id,
        ),
        ApplicationError::AdminCredential(AdminCredentialError::CheckBusy) => response(
            StatusCode::TOO_MANY_REQUESTS,
            ResponseCode::Busy,
            "凭据检查繁忙，请稍后重试",
            Option::<()>::None,
            request_id,
        ),
        error => {
            diagnostics::warn(
                "control_credential_check",
                "check",
                error.diagnostic_kind(),
                DiagnosticContext {
                    route: Some(WORKER_CONTROL_MAIL_CREDENTIAL_CHECK_PATH),
                    ..Default::default()
                },
            );
            response(
                StatusCode::SERVICE_UNAVAILABLE,
                ResponseCode::CapabilityUnavailable,
                "邮件凭据检查暂不可用",
                Option::<()>::None,
                request_id,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use auction_application::admin::{
        AdminCredentialCheckObservation, AdminCredentialCheckPort, AdminCredentialCheckResult,
        AdminCredentialCheckStatus, AdminCredentialError, AdminCredentialWorld,
    };
    use auction_application::core::{ApplicationError, ApplicationFuture};
    use axum::body::Bytes;
    use axum::extract::Extension;
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use axum::response::Response;
    use chrono::Utc;
    use uuid::Uuid;

    use super::mail_credential_check;

    const CREDENTIAL: Uuid = Uuid::from_u128(0x0c0f_fee0);

    /// 按脚本返回结果并记录收到的请求；未设置结果时被调用即失败关闭。
    struct ScriptedCheck {
        result: Mutex<Option<Result<Vec<AdminCredentialCheckObservation>, AdminCredentialError>>>,
        requests: Mutex<Vec<(Uuid, i64)>>,
    }

    impl ScriptedCheck {
        fn new(
            result: Result<Vec<AdminCredentialCheckObservation>, AdminCredentialError>,
        ) -> Arc<Self> {
            Arc::new(Self { result: Mutex::new(Some(result)), requests: Mutex::new(Vec::new()) })
        }
    }

    impl AdminCredentialCheckPort for ScriptedCheck {
        fn check(
            &self,
            credential_id: Uuid,
            expected_state_version: i64,
        ) -> ApplicationFuture<'_, AdminCredentialCheckResult> {
            self.requests.lock().unwrap().push((credential_id, expected_state_version));
            let result = self.result.lock().unwrap().take();
            Box::pin(async move {
                let checks = result.expect("脚本结果只消费一次").map_err(ApplicationError::from)?;
                Ok(AdminCredentialCheckResult {
                    credential_id,
                    state_version: expected_state_version,
                    checked_at: Utc::now(),
                    checks,
                })
            })
        }
    }

    fn json_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers
    }

    fn body(version: i64) -> Bytes {
        Bytes::from(format!(
            r#"{{"credential_id":"{CREDENTIAL}","expected_state_version":{version}}}"#
        ))
    }

    async fn envelope(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 << 10)
            .await
            .expect("control response body is bounded");
        (status, serde_json::from_slice(&bytes).expect("control response is JSON"))
    }

    async fn call(
        checks: Option<Arc<dyn AdminCredentialCheckPort>>,
        body: Bytes,
    ) -> (StatusCode, serde_json::Value) {
        envelope(mail_credential_check(Extension(checks), json_headers(), body).await).await
    }

    #[tokio::test]
    async fn checks_the_requested_credential_and_returns_only_world_status() {
        let check = ScriptedCheck::new(Ok(vec![AdminCredentialCheckObservation {
            world: AdminCredentialWorld::Isekai,
            status: AdminCredentialCheckStatus::Invalid,
        }]));
        let port: Arc<dyn AdminCredentialCheckPort> = check.clone();
        let (status, value) = call(Some(port), body(5)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["code"], "ok");
        assert_eq!(
            value["data"],
            serde_json::json!({
                "credential_id": CREDENTIAL,
                "state_version": 5,
                "checks": [{"world": "isekai", "status": "invalid"}],
            })
        );
        assert_eq!(*check.requests.lock().unwrap(), vec![(CREDENTIAL, 5)]);
    }

    #[tokio::test]
    async fn without_capability_or_with_invalid_request_the_check_never_runs() {
        let (status, value) = call(None, body(5)).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(value["code"], "capability_unavailable");

        let check = ScriptedCheck::new(Ok(Vec::new()));
        let port: Arc<dyn AdminCredentialCheckPort> = check.clone();
        let (status, value) = call(Some(port.clone()), body(0)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(value["code"], "invalid_command_id");
        let (status, _) = call(
            Some(port),
            Bytes::from(format!(
                r#"{{"credential_id":"{CREDENTIAL}","expected_state_version":5,"cookie":"x"}}"#
            )),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "未知字段（如 Cookie）一律拒绝");
        assert!(check.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn business_rejections_keep_their_class_and_other_failures_are_unavailable() {
        for (error, expected_status, expected_code) in [
            (
                AdminCredentialError::VersionConflict { current_version: 9 },
                StatusCode::CONFLICT,
                "version_conflict",
            ),
            (AdminCredentialError::NotFound, StatusCode::NOT_FOUND, "not_found"),
            (AdminCredentialError::InvalidRequest, StatusCode::BAD_REQUEST, "invalid_command_id"),
            (AdminCredentialError::CheckBusy, StatusCode::TOO_MANY_REQUESTS, "busy"),
            (
                AdminCredentialError::CryptoUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "capability_unavailable",
            ),
            (
                AdminCredentialError::CheckUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "capability_unavailable",
            ),
        ] {
            let port: Arc<dyn AdminCredentialCheckPort> = ScriptedCheck::new(Err(error.clone()));
            let (status, value) = call(Some(port), body(5)).await;
            assert_eq!(status, expected_status, "{error:?}");
            assert_eq!(value["code"], expected_code, "{error:?}");
            if expected_code == "version_conflict" {
                assert_eq!(value["data"], serde_json::json!({"current_version": 9}));
            } else {
                assert!(value["data"].is_null(), "{error:?}");
            }
        }
    }
}
