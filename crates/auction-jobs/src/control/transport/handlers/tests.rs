//! Worker control 传输处理器的回归用例：请求标识、媒体类型与 JSON 错误映射。

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
        ) -> auction_application::operations::WorkerRemoteReadFuture<'_, WorkerEquipmentReadResponse>
        {
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
    let response = equipment_read(Extension(Some(reads)), json_headers(), equipment_body()).await;
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
        ) -> auction_application::operations::WorkerRemoteReadFuture<'_, WorkerEquipmentReadResponse>
        {
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
        Bytes::from_static(br#"{"world_id":"shared","equipment_url":"https://x/1/0123456789"}"#),
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
        ) -> auction_application::operations::WorkerRemoteReadFuture<'_, WorkerEquipmentReadResponse>
        {
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
        ) -> auction_application::operations::WorkerRemoteReadFuture<'_, WorkerEquipmentReadResponse>
        {
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
    fn snapshot(&self) -> Option<auction_application::external_accounts::ExternalAccountStatus> {
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

/// 固定结论的数据库就绪探针替身。
struct FixedReadiness(bool);

impl auction_application::operations::ProcessReadinessPort for FixedReadiness {
    fn check(&self) -> auction_application::auction::ApplicationFuture<'_, ()> {
        let ready = self.0;
        Box::pin(async move {
            if ready {
                Ok(())
            } else {
                Err(auction_application::core::ApplicationError::Repository(
                    auction_application::core::RepositoryError::Failed(
                        "database not ready".to_owned(),
                    ),
                ))
            }
        })
    }
}

#[tokio::test]
async fn ready_requires_database_readiness_when_probe_is_wired() {
    let not_ready =
        WorkerControlHandle::new("test", 0).with_readiness(Arc::new(FixedReadiness(false)));
    not_ready.mark_serving();
    let (status, body) = envelope(ready(State(not_ready), HeaderMap::new()).await).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "not_ready");

    let serving =
        WorkerControlHandle::new("test", 0).with_readiness(Arc::new(FixedReadiness(true)));
    serving.mark_serving();
    let (status, body) = envelope(ready(State(serving), HeaderMap::new()).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["ready"], true);
}

#[tokio::test]
async fn ready_without_probe_keeps_process_state_semantics_for_skeleton() {
    let starting = WorkerControlHandle::new("test", 0);
    let (status, _) = envelope(ready(State(starting.clone()), HeaderMap::new()).await).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    starting.mark_serving();
    let (status, _) = envelope(ready(State(starting), HeaderMap::new()).await).await;
    assert_eq!(status, StatusCode::OK);
}
