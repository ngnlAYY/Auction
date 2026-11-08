//! Worker control HTTP 路由处理器。

use self::parent::*;
use crate::control::transport as parent;

pub(super) async fn live(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    response(StatusCode::OK, ResponseCode::Ok, "Worker 存活", Some(handle.live()), &request_id)
}

pub(super) async fn ready(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    let result = handle.ready();
    if !result.ready {
        return response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::NotReady,
            "Worker 尚未就绪",
            Option::<ReadyResult>::None,
            &request_id,
        );
    }
    response(StatusCode::OK, ResponseCode::Ok, "Worker 已就绪", Some(result), &request_id)
}

pub(super) async fn status(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    response(
        StatusCode::OK,
        ResponseCode::Ok,
        "Worker 状态可用",
        Some(handle.status()),
        &request_id,
    )
}

/// 仅返回当前进程的安全邮件观测；不触发远端读取或业务写入。
pub(super) async fn mail_progress(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    let snapshot = handle.mail_progress.snapshot();
    let Some(observed_at) = snapshot.observed_at.filter(|_| snapshot.validate()) else {
        return response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::Unavailable,
            "邮件运行观测不可用",
            Option::<MailProgressResult>::None,
            &request_id,
        );
    };
    response(
        StatusCode::OK,
        ResponseCode::Ok,
        "邮件运行观测已加载",
        Some(MailProgressResult { observed_at, pollers: snapshot.pollers }),
        &request_id,
    )
}

/// 仅返回当前进程的安全外部账号观测；不触发远端读取或业务写入。
pub(super) async fn external_accounts(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    match handle.external_accounts() {
        Some(status) => response(
            StatusCode::OK,
            ResponseCode::Ok,
            "外部账号运行观测可用",
            Some(status),
            &request_id,
        ),
        None => response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::CapabilityUnavailable,
            "外部账号运行观测不可用",
            Option::<ExternalAccountStatus>::None,
            &request_id,
        ),
    }
}

pub(super) async fn trigger_poll(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    if !headers.get(header::CONTENT_TYPE).is_some_and(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(';').next().is_some_and(|value| value.trim() == "application/json")
        })
    }) {
        return response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ResponseCode::Invalid,
            "请求必须使用 application/json",
            Option::<()>::None,
            &request_id,
        );
    }
    let command: TriggerPollCommand = match serde_json::from_slice(&body) {
        Ok(command) => command,
        Err(error) => {
            let _ = error;
            return response(
                StatusCode::BAD_REQUEST,
                ResponseCode::Invalid,
                "请求 JSON 无效",
                Option::<()>::None,
                &request_id,
            );
        }
    };
    match handle.trigger_poll(command) {
        Ok(result) => response(
            StatusCode::ACCEPTED,
            ResponseCode::Accepted,
            "轮询唤醒已接受",
            Some(result),
            &request_id,
        ),
        Err(TriggerError::InvalidCommand) => response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "命令标识无效",
            Option::<()>::None,
            &request_id,
        ),
        Err(TriggerError::NotReady | TriggerError::NoPollers) => response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::NotReady,
            "Worker 尚未就绪",
            Option::<()>::None,
            &request_id,
        ),
        Err(TriggerError::Unavailable) => response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::Unavailable,
            "Worker 控制面不可用",
            Option::<()>::None,
            &request_id,
        ),
    }
}

pub(super) async fn metrics(
    Extension(metrics): Extension<Option<Arc<ProcessMetrics>>>,
    uri: Uri,
) -> Response {
    if uri.path() != WORKER_CONTROL_METRICS_PATH || uri.query().is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(metrics) = metrics else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match metrics.encode() {
        Ok(body) => {
            ([(header::CONTENT_TYPE, ProcessMetrics::content_type())], body).into_response()
        }
        Err(MetricsError::Disabled) => StatusCode::NOT_FOUND.into_response(),
        Err(MetricsError::Registry(_) | MetricsError::Encode(_)) => {
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub(super) async fn not_found(headers: HeaderMap) -> Response {
    let request_id = request_id(&headers).unwrap_or_else(|_| Uuid::new_v4().to_string());
    response(
        StatusCode::NOT_FOUND,
        ResponseCode::Invalid,
        "请求的资源不存在",
        Option::<()>::None,
        &request_id,
    )
}

/// W1.9/W1.10：Admin 装备 URL 读取经 Worker 执行；凭据、请求门与暂停判定都在 Worker 进程。
/// 请求体只含世界与公开装备链接，响应不含 Cookie、正文或错误 cause。
pub(super) async fn equipment_read(
    Extension(reads): Extension<Option<Arc<dyn WorkerRemoteReadPort>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    if !json_content_type(&headers) {
        return response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ResponseCode::Invalid,
            "请求必须使用 application/json",
            Option::<()>::None,
            &request_id,
        );
    }
    let request: WorkerEquipmentReadRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            let _ = error;
            return response(
                StatusCode::BAD_REQUEST,
                ResponseCode::Invalid,
                "请求 JSON 无效",
                Option::<()>::None,
                &request_id,
            );
        }
    };
    if !request.validate() {
        return response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "装备读取请求无效",
            Option::<()>::None,
            &request_id,
        );
    }
    let Some(reads) = reads else {
        return response(
            StatusCode::NOT_IMPLEMENTED,
            ResponseCode::CapabilityUnavailable,
            "Worker 未装配远端读取能力",
            Option::<WorkerEquipmentReadResponse>::None,
            &request_id,
        );
    };
    match reads.read_equipment(request).await {
        Ok(result) => {
            response(StatusCode::OK, ResponseCode::Ok, "装备读取完成", Some(result), &request_id)
        }
        Err(error) => remote_read_error_response(error, &request_id),
    }
}

/// CLI Sent 核查的远端读取；目标来自调用方本地数据库，Worker 只执行有界只读扫描。
pub(super) async fn sent_lookup(
    Extension(reads): Extension<Option<Arc<dyn WorkerRemoteReadPort>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    if !json_content_type(&headers) {
        return response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ResponseCode::Invalid,
            "请求必须使用 application/json",
            Option::<()>::None,
            &request_id,
        );
    }
    let request: WorkerSentLookupRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            let _ = error;
            return response(
                StatusCode::BAD_REQUEST,
                ResponseCode::Invalid,
                "请求 JSON 无效",
                Option::<()>::None,
                &request_id,
            );
        }
    };
    if !request.validate() {
        return response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "Sent 核查请求无效",
            Option::<()>::None,
            &request_id,
        );
    }
    let Some(reads) = reads else {
        return response(
            StatusCode::NOT_IMPLEMENTED,
            ResponseCode::CapabilityUnavailable,
            "Worker 未装配远端读取能力",
            Option::<WorkerSentLookupResponse>::None,
            &request_id,
        );
    };
    match reads.lookup_sent(request).await {
        Ok(result) => {
            response(StatusCode::OK, ResponseCode::Ok, "Sent 核查完成", Some(result), &request_id)
        }
        Err(error) => remote_read_error_response(error, &request_id),
    }
}

/// W10.9：只在 W1.9 暂停生效时执行一次有界只读恢复扫描；不写入本地或远端。
pub(super) async fn recovery_scan(
    Extension(reads): Extension<Option<Arc<dyn WorkerRemoteReadPort>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    if !json_content_type(&headers) {
        return response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ResponseCode::Invalid,
            "请求必须使用 application/json",
            Option::<()>::None,
            &request_id,
        );
    }
    let request: auction_application::operations::WorkerRecoveryScanRequest =
        match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(error) => {
                let _ = error;
                return response(
                    StatusCode::BAD_REQUEST,
                    ResponseCode::Invalid,
                    "请求 JSON 无效",
                    Option::<()>::None,
                    &request_id,
                );
            }
        };
    if !request.validate() {
        return response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "恢复扫描请求无效",
            Option::<()>::None,
            &request_id,
        );
    }
    let Some(reads) = reads else {
        return response(
            StatusCode::NOT_IMPLEMENTED,
            ResponseCode::CapabilityUnavailable,
            "Worker 未装配远端读取能力",
            Option::<auction_application::operations::WorkerRecoveryScanResponse>::None,
            &request_id,
        );
    };
    match reads.recovery_scan(request).await {
        Ok(result) => {
            response(StatusCode::OK, ResponseCode::Ok, "恢复扫描完成", Some(result), &request_id)
        }
        Err(error) => remote_read_error_response(error, &request_id),
    }
}

fn json_content_type(headers: &HeaderMap) -> bool {
    headers.get(header::CONTENT_TYPE).is_some_and(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(';').next().is_some_and(|value| value.trim() == "application/json")
        })
    })
}

/// 远端读取错误只保留稳定分类；暂停使用 409 以区别于“Worker 不可用”的 503。
fn remote_read_error_response(error: WorkerRemoteReadError, request_id: &str) -> Response {
    match error {
        WorkerRemoteReadError::Paused => response(
            StatusCode::CONFLICT,
            ResponseCode::Paused,
            "外部访问已暂停",
            Option::<()>::None,
            request_id,
        ),
        WorkerRemoteReadError::NotPaused => response(
            StatusCode::CONFLICT,
            ResponseCode::NotPaused,
            "恢复核对要求先暂停外部访问",
            Option::<()>::None,
            request_id,
        ),
        WorkerRemoteReadError::Invalid => response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "远端读取请求无效",
            Option::<()>::None,
            request_id,
        ),
        WorkerRemoteReadError::Unavailable => response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::CapabilityUnavailable,
            "远端读取暂时不可用",
            Option::<()>::None,
            request_id,
        ),
        WorkerRemoteReadError::Timeout => response(
            StatusCode::GATEWAY_TIMEOUT,
            ResponseCode::Unavailable,
            "远端读取超时",
            Option::<()>::None,
            request_id,
        ),
        WorkerRemoteReadError::Incompatible => response(
            StatusCode::BAD_GATEWAY,
            ResponseCode::Unavailable,
            "远端读取结果不受支持",
            Option::<()>::None,
            request_id,
        ),
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::control::transport::handlers as parent;
    use auction_application::operations::{WorkerEquipmentReadResponse, WorkerSentLookupResponse};

    fn json_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"));
        headers
    }

    async fn envelope(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 << 10)
            .await
            .expect("control response body is bounded");
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).expect("control response is JSON");
        (status, value)
    }

    fn equipment_body() -> Bytes {
        Bytes::from_static(
            br#"{"world_id":"main","equipment_url":"https://hentaiverse.org/equip/300753957/0123456789"}"#,
        )
    }

    #[tokio::test]
    async fn equipment_read_without_executor_reports_capability_unavailable() {
        let response = equipment_read(
            Extension(None::<Arc<dyn WorkerRemoteReadPort>>),
            json_headers(),
            equipment_body(),
        )
        .await;
        let (status, body) = envelope(response).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["code"], "capability_unavailable");
        assert!(body["data"].is_null());
    }

    #[tokio::test]
    async fn sent_lookup_without_executor_reports_capability_unavailable() {
        let response = sent_lookup(
            Extension(None::<Arc<dyn WorkerRemoteReadPort>>),
            json_headers(),
            Bytes::from_static(
                br#"{"world":"main","recipient":"player","subject":"subject","external_equipment_id":"1"}"#,
            ),
        )
        .await;
        let (status, body) = envelope(response).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["code"], "capability_unavailable");
    }

    #[tokio::test]
    async fn paused_executor_maps_to_conflict_without_leaking_details() {
        struct Paused;
        impl WorkerRemoteReadPort for Paused {
            fn read_equipment(
                &self,
                _request: WorkerEquipmentReadRequest,
            ) -> auction_application::operations::WorkerRemoteReadFuture<
                '_,
                WorkerEquipmentReadResponse,
            > {
                Box::pin(async { Err(WorkerRemoteReadError::Paused) })
            }

            fn lookup_sent(
                &self,
                _request: WorkerSentLookupRequest,
            ) -> auction_application::operations::WorkerRemoteReadFuture<'_, WorkerSentLookupResponse>
            {
                Box::pin(async { Err(WorkerRemoteReadError::Paused) })
            }
        }
        let reads: Arc<dyn WorkerRemoteReadPort> = Arc::new(Paused);
        let response =
            equipment_read(Extension(Some(reads)), json_headers(), equipment_body()).await;
        let (status, body) = envelope(response).await;
        // 409 而不是 503：客户端必须能区分“暂停”与“Worker 不可用”。
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "paused");
        assert_eq!(body["message"], "外部访问已暂停");
    }

    #[tokio::test]
    async fn invalid_requests_are_rejected_before_reaching_the_executor() {
        struct Exploding;
        impl WorkerRemoteReadPort for Exploding {
            fn read_equipment(
                &self,
                _request: WorkerEquipmentReadRequest,
            ) -> auction_application::operations::WorkerRemoteReadFuture<
                '_,
                WorkerEquipmentReadResponse,
            > {
                panic!("invalid request must not reach the executor")
            }

            fn lookup_sent(
                &self,
                _request: WorkerSentLookupRequest,
            ) -> auction_application::operations::WorkerRemoteReadFuture<'_, WorkerSentLookupResponse>
            {
                panic!("invalid request must not reach the executor")
            }
        }
        let reads: Arc<dyn WorkerRemoteReadPort> = Arc::new(Exploding);
        let response = equipment_read(
            Extension(Some(reads)),
            json_headers(),
            Bytes::from_static(
                br#"{"world_id":"shared","equipment_url":"https://x/1/0123456789"}"#,
            ),
        )
        .await;
        let (status, body) = envelope(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_command_id");

        let response = equipment_read(
            Extension(None::<Arc<dyn WorkerRemoteReadPort>>),
            HeaderMap::new(),
            equipment_body(),
        )
        .await;
        let (status, _) = envelope(response).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    fn recovery_body() -> Bytes {
        Bytes::from_static(
            br#"{"cutoff_at":"2026-11-05T00:00:00Z","maximum_pages":20,"sent_worlds":["main"],"read_mail_worlds":["main"],"include_sent":true,"include_read_mail":true,"include_forum":false,"sent_details":[],"forum_threads":[]}"#,
        )
    }

    #[tokio::test]
    async fn recovery_scan_requires_pause_and_reports_missing_capability() {
        struct NotPaused;
        impl WorkerRemoteReadPort for NotPaused {
            fn read_equipment(
                &self,
                _request: WorkerEquipmentReadRequest,
            ) -> auction_application::operations::WorkerRemoteReadFuture<
                '_,
                WorkerEquipmentReadResponse,
            > {
                Box::pin(async { Err(WorkerRemoteReadError::Unavailable) })
            }

            fn lookup_sent(
                &self,
                _request: WorkerSentLookupRequest,
            ) -> auction_application::operations::WorkerRemoteReadFuture<'_, WorkerSentLookupResponse>
            {
                Box::pin(async { Err(WorkerRemoteReadError::Unavailable) })
            }

            fn recovery_scan(
                &self,
                _request: auction_application::operations::WorkerRecoveryScanRequest,
            ) -> auction_application::operations::WorkerRemoteReadFuture<
                '_,
                auction_application::operations::WorkerRecoveryScanResponse,
            > {
                Box::pin(async { Err(WorkerRemoteReadError::NotPaused) })
            }
        }
        let reads: Arc<dyn WorkerRemoteReadPort> = Arc::new(NotPaused);
        let response = recovery_scan(Extension(Some(reads)), json_headers(), recovery_body()).await;
        let (status, body) = envelope(response).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "not_paused");

        let response = recovery_scan(
            Extension(None::<Arc<dyn WorkerRemoteReadPort>>),
            json_headers(),
            recovery_body(),
        )
        .await;
        let (status, body) = envelope(response).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["code"], "capability_unavailable");
    }

    #[tokio::test]
    async fn recovery_scan_rejects_invalid_request_before_executor() {
        struct Exploding;
        impl WorkerRemoteReadPort for Exploding {
            fn read_equipment(
                &self,
                _request: WorkerEquipmentReadRequest,
            ) -> auction_application::operations::WorkerRemoteReadFuture<
                '_,
                WorkerEquipmentReadResponse,
            > {
                panic!("invalid recovery request must not reach the executor")
            }

            fn lookup_sent(
                &self,
                _request: WorkerSentLookupRequest,
            ) -> auction_application::operations::WorkerRemoteReadFuture<'_, WorkerSentLookupResponse>
            {
                panic!("invalid recovery request must not reach the executor")
            }
        }
        let reads: Arc<dyn WorkerRemoteReadPort> = Arc::new(Exploding);
        let response = recovery_scan(
            Extension(Some(reads)),
            json_headers(),
            Bytes::from_static(
                br#"{"cutoff_at":"2026-11-05T00:00:00Z","maximum_pages":0,"sent_worlds":["main"],"read_mail_worlds":[],"include_sent":true,"include_read_mail":false,"include_forum":false,"sent_details":[],"forum_threads":[]}"#,
            ),
        )
        .await;
        let (status, body) = envelope(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_command_id");
    }

    struct StaticAccounts(auction_application::external_accounts::ExternalAccountStatus);

    impl auction_application::external_accounts::ExternalAccountStatusReader for StaticAccounts {
        fn snapshot(
            &self,
        ) -> Option<auction_application::external_accounts::ExternalAccountStatus> {
            Some(self.0.clone())
        }
    }

    fn idle_accounts() -> auction_application::external_accounts::ExternalAccountStatus {
        use auction_application::external_accounts::{
            ExternalAccountStatus, ExternalForumAccountStatus, ExternalMailAccountStatus,
            ExternalMaintenanceState, ExternalMaintenanceStatus,
        };
        ExternalAccountStatus {
            observed_at: chrono::Utc::now(),
            mail: ExternalMailAccountStatus {
                busy: false,
                busy_age_seconds: None,
                gate_wait_count: 0,
                gate_wait_seconds_total: 0,
                request_count: 0,
                last_request_at: None,
                last_success_at: None,
                last_failure_at: None,
            },
            forum: ExternalForumAccountStatus {
                last_request_at: None,
                last_success_at: None,
                last_failure_at: None,
                write_unknown_count: 0,
            },
            maintenance: ExternalMaintenanceStatus {
                state: ExternalMaintenanceState::Idle,
                last_started_at: None,
                last_success_at: None,
                last_failure_at: None,
                last_canceled_at: None,
                running_age_seconds: None,
            },
        }
    }

    #[tokio::test]
    async fn external_accounts_without_reader_reports_capability_unavailable() {
        let handle = WorkerControlHandle::new("0.1.0", 1);
        let response = external_accounts(State(handle), HeaderMap::new()).await;
        let (status, body) = envelope(response).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "capability_unavailable");
        assert!(body["data"].is_null());
    }

    #[tokio::test]
    async fn external_accounts_returns_snapshot_without_secrets() {
        let reader: Arc<dyn auction_application::external_accounts::ExternalAccountStatusReader> =
            Arc::new(StaticAccounts(idle_accounts()));
        let handle = WorkerControlHandle::new("0.1.0", 1).with_external_accounts(Some(reader));
        let response = external_accounts(State(handle), HeaderMap::new()).await;
        let (status, body) = envelope(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["code"], "ok");
        assert_eq!(body["data"]["mail"]["busy"], false);
        assert_eq!(body["data"]["maintenance"]["state"], "idle");
        assert!(body["data"].get("cookie").is_none());
    }
}
