//! runtime 配置投影的回归用例：GET、校验与更新响应都经 `map_snapshot` 生成，邮件区块必须
//! 补齐全部省略键，且补齐后的文档原样提交时仍序列化回同一指纹；告警区块的长期未领取阈值只在
//! 新页面显式 include 时输出，缺省投影保持已发布闭集。

use std::collections::BTreeSet;
use std::sync::Arc;

use auction_application::admin::{
    AdminRuntimeConfigError, AdminRuntimeConfigPort, AdminRuntimeDocument,
    AdminRuntimeEquipmentFilterConfig, AdminRuntimeSnapshot, AdminRuntimeUpdateCommand,
    AdminRuntimeWorldsConfig,
};
use auction_application::core::{ApplicationError, ApplicationFuture};
use axum::body::{Bytes, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{StatusCode, Uri};
use axum::response::Response;
use serde_json::{Value, json};

use super::{
    RuntimeProjection, get, map_error, map_snapshot, runtime_projection, update, validate,
};
use crate::admin::auctions::command_test_support::{VALID_UUID, headers, state_with_runtime};

/// 已发布页面（基线 b2300b46）`valid_alerts` 严格校验的告警区块闭集：缺省投影必须恰好是它。
const BASELINE_ALERT_KEYS: [&str; 21] = [
    "managed",
    "enabled",
    "scan_interval_seconds",
    "jitter_seconds",
    "source_timeout_seconds",
    "batch_window_seconds",
    "cooldown_seconds",
    "claim_lease_seconds",
    "retry_delay_seconds",
    "max_signals_per_scan",
    "max_batch_items",
    "max_body_bytes",
    "delivery_pending_count_threshold",
    "delivery_pending_age_seconds",
    "delivery_manual_reconcile_count_threshold",
    "fulfillment_backlog_count_threshold",
    "fulfillment_backlog_age_seconds",
    "recovery_overdue_count_threshold",
    "recovery_overdue_age_seconds",
    "world_id",
    "recipient",
];

/// 缺省投影：旧页面不带 include。
const DEFAULT_PROJECTION: RuntimeProjection =
    RuntimeProjection { equipment_taxonomy: false, collection_overdue_threshold: false };

/// 新页面带 `include=fulfillment_collection_overdue_age_seconds`。
const THRESHOLD_PROJECTION: RuntimeProjection =
    RuntimeProjection { equipment_taxonomy: false, collection_overdue_threshold: true };

/// 管理端 `crates/auction-web-admin/src/api/runtime_config/operations.rs` 的 `valid_mail` 严格校验
/// 的邮件区块闭集；`AdminRuntimeMailConfig` 新增字段时投影随之变化，本表与浏览器闭集须一起同步。
const MAIL_KEYS: [&str; 23] = [
    "managed",
    "send_intervals",
    "schedule",
    "maximum_response_bytes",
    "minimum_request_interval_millis",
    "request_timeout_seconds",
    "batch_size",
    "maximum_pages",
    "jitter_seconds",
    "lease_duration_seconds",
    "renew_interval_seconds",
    "fetch_attempts",
    "fetch_timeout_seconds",
    "process_attempts",
    "item_timeout_seconds",
    "batch_timeout_seconds",
    "retry_base_seconds",
    "retry_maximum_seconds",
    "rekey_enabled",
    "account_reply_window_seconds",
    "explanation_mail_window_seconds",
    "explanation_mail_limit",
    "send_classes",
];

fn document(value: Value) -> AdminRuntimeDocument {
    serde_json::from_value(value).expect("测试 runtime 文档必须可解析")
}

/// 按文件快照投影（缺省投影）；返回投影文档与快照指纹。
fn project(document: AdminRuntimeDocument) -> (Value, String) {
    project_with(document, DEFAULT_PROJECTION)
}

fn project_with(document: AdminRuntimeDocument, projection: RuntimeProjection) -> (Value, String) {
    let fingerprint = document.fingerprint();
    let snapshot = AdminRuntimeSnapshot {
        document,
        fingerprint: fingerprint.clone(),
        restart_required: false,
    };
    let response = map_snapshot(snapshot, projection).expect("合法快照必须可投影");
    assert_eq!(response.fingerprint, fingerprint, "投影不改变物理文档指纹");
    (response.document, fingerprint)
}

fn alert_keys(projected: &Value) -> BTreeSet<&str> {
    projected["operations"]["alerts"]
        .as_object()
        .expect("投影必须包含告警区块")
        .keys()
        .map(String::as_str)
        .collect()
}

fn mail_keys(projected: &Value) -> BTreeSet<&str> {
    projected["operations"]["mail"]
        .as_object()
        .expect("投影必须包含邮件区块")
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn legacy_templates_project_business_defaults_with_the_original_snapshot_fingerprint() {
    let source = document(json!({"mail_templates": {"register_result": "旧正文"}}));
    assert!(source.worlds.is_none() && source.equipment_filter.is_none());
    let (projected, snapshot_fingerprint) = project(source);

    assert_eq!(
        projected["worlds"],
        serde_json::to_value(AdminRuntimeWorldsConfig::default()).unwrap()
    );
    assert_eq!(
        projected["equipment_filter"],
        serde_json::to_value(AdminRuntimeEquipmentFilterConfig::default()).unwrap()
    );
    assert_eq!(projected["mail_templates"]["register_result"], "旧正文");
    assert_ne!(document(projected).fingerprint(), snapshot_fingerprint);
}

/// 没有 operations 的旧文件与只写了 `managed` 的旧邮件区块：省略键全部按服务端默认值补齐。
#[test]
fn old_documents_project_every_mail_key_with_server_defaults() {
    for source in [json!({}), json!({"operations": {"mail": {"managed": true}}})] {
        let (projected, _) = project(document(source));
        assert_eq!(mail_keys(&projected), MAIL_KEYS.into_iter().collect());
        let mail = &projected["operations"]["mail"];
        assert_eq!(mail["batch_timeout_seconds"], 600);
        assert_eq!(mail["rekey_enabled"], true);
        assert_eq!(mail["account_reply_window_seconds"], 86_400);
        assert_eq!(mail["explanation_mail_window_seconds"], 86_400);
        assert_eq!(mail["explanation_mail_limit"], 10);
        assert_eq!(
            mail["send_intervals"],
            json!({
                "outbid_seconds": 0,
                "account_seconds": 0,
                "intake_seconds": 0,
                "settlement_seconds": 0,
                "equipment_seconds": 0,
                "operations_seconds": 0,
                "manual_seconds": 0
            })
        );
        assert_eq!(
            mail["send_classes"],
            json!({
                "account_reply": true,
                "intake_reply": true,
                "settlement_notice": true,
                "outbid_reminder": true,
                "player_summary": true,
                "operations_alert": true
            })
        );
    }
}

/// 显式写入的非默认值优先，补齐只填缺失键。
#[test]
fn explicit_mail_values_are_not_overwritten_by_defaults() {
    let (projected, _) = project(document(json!({
        "operations": {"mail": {
            "managed": true,
            "batch_timeout_seconds": 900,
            "rekey_enabled": false,
            "explanation_mail_limit": 0,
            "send_intervals": {"outbid_seconds": 30},
            "send_classes": {"player_summary": false}
        }}
    })));
    assert_eq!(mail_keys(&projected), MAIL_KEYS.into_iter().collect());
    let mail = &projected["operations"]["mail"];
    assert_eq!(mail["batch_timeout_seconds"], 900);
    assert_eq!(mail["rekey_enabled"], false);
    assert_eq!(mail["explanation_mail_limit"], 0);
    assert_eq!(mail["account_reply_window_seconds"], 86_400);
    assert_eq!(mail["send_intervals"]["outbid_seconds"], 30);
    assert_eq!(mail["send_intervals"]["manual_seconds"], 0);
    assert_eq!(mail["send_classes"]["player_summary"], false);
    assert_eq!(mail["send_classes"]["account_reply"], true);
}

/// 管理端把补齐后的完整投影原样作为候选提交：服务端解析后与补齐前的文档同一指纹，
/// 默认值不会写进物理文件，也不会让未改动的保存产生新的命令摘要。
#[test]
fn resubmitted_projection_keeps_the_canonical_fingerprint() {
    for source in [json!({}), json!({"operations": {"mail": {"managed": true}}})] {
        let mut materialized = document(source.clone());
        materialized.materialize_business_defaults();
        let (projected, _) = project(document(source));
        assert_eq!(document(projected).fingerprint(), materialized.fingerprint());
    }
}

/// 缺省投影的告警区块恰好是已发布闭集：长期未领取阈值不论省略还是显式写了非默认值都不输出，
/// 已打开的旧页面按字段数严格校验照常通过。
#[test]
fn default_projection_keeps_the_published_alerts_closed_set() {
    for source in [
        json!({}),
        json!({"operations": {"alerts": {"managed": true}}}),
        json!({"operations": {"alerts": {
            "managed": true,
            "fulfillment_collection_overdue_age_seconds": 86_400
        }}}),
    ] {
        let (projected, _) = project(document(source));
        assert_eq!(alert_keys(&projected), BASELINE_ALERT_KEYS.into_iter().collect());
    }
}

/// 扩展投影给出生效阈值：省略即默认 259200，显式值优先；投影不改变物理文档指纹。新页面把
/// 扩展投影原样提交时，显式默认值按省略落成，落成候选与补齐前的文档同一指纹。
#[test]
fn threshold_projection_shows_the_effective_threshold() {
    for (source, expected) in [
        (json!({}), 259_200),
        (json!({"operations": {"alerts": {"managed": true}}}), 259_200),
        (
            json!({"operations": {"alerts": {
                "managed": true,
                "fulfillment_collection_overdue_age_seconds": 86_400
            }}}),
            86_400,
        ),
    ] {
        let current = document(source.clone());
        let (projected, _) = project_with(document(source), THRESHOLD_PROJECTION);
        let mut keys = BASELINE_ALERT_KEYS.into_iter().collect::<BTreeSet<_>>();
        keys.insert("fulfillment_collection_overdue_age_seconds");
        assert_eq!(alert_keys(&projected), keys);
        assert_eq!(
            projected["operations"]["alerts"]["fulfillment_collection_overdue_age_seconds"],
            expected
        );
        let mut materialized = current.clone();
        materialized.materialize_business_defaults();
        let resubmitted = document(projected).resolve_update(&current);
        assert_eq!(resubmitted.fingerprint(), materialized.fingerprint());
    }
}

async fn parse(query: &str) -> Result<RuntimeProjection, StatusCode> {
    let uri: Uri = format!("/api/v1/admin/config{query}").parse().expect("测试 URI");
    match runtime_projection(&uri) {
        Ok(projection) => Ok(projection),
        Err(response) => {
            let status = response.status();
            let body = to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
            let body: Value = serde_json::from_slice(&body).expect("错误响应是 JSON");
            assert_eq!(body["code"], "invalid_query", "{query}");
            Err(status)
        }
    }
}

/// 两个扩展参数各自独立；未知取值、空值、重复出现或未登记的键都为 400 invalid_query。
#[tokio::test]
async fn runtime_projection_accepts_only_registered_includes() {
    assert_eq!(parse("").await, Ok(DEFAULT_PROJECTION));
    assert_eq!(
        parse("?include=fulfillment_collection_overdue_age_seconds").await,
        Ok(THRESHOLD_PROJECTION)
    );
    assert_eq!(
        parse(
            "?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds"
        )
        .await,
        Ok(RuntimeProjection { equipment_taxonomy: true, collection_overdue_threshold: true })
    );
    for query in [
        "?include=",
        "?include=collection_wait",
        "?include=FULFILLMENT_COLLECTION_OVERDUE_AGE_SECONDS",
        "?include=fulfillment_collection_overdue_age_seconds&include=fulfillment_collection_overdue_age_seconds",
        "?include_equipment_taxonomy=false",
        "?fields=summary",
    ] {
        assert_eq!(parse(query).await, Err(StatusCode::BAD_REQUEST), "{query}");
    }
}

const WORKER_FILE_HINT: &str =
    "修改告警配置或覆盖的邮件世界前，须在 Admin 配置 worker_config_file 并重启";
const UNREADABLE_WORKER_FILE_HINT: &str =
    "无法加载 Worker 部署配置，请检查 Admin 的 worker_config_file 指向的文件是否可读";
const CHANGED_WORKER_FILE_HINT: &str = "Worker 部署配置已变更，请重启 Admin 后再保存";

struct RuntimeWithWorkerError {
    current: AdminRuntimeDocument,
    worker_error: &'static str,
}

impl AdminRuntimeConfigPort for RuntimeWithWorkerError {
    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
        Box::pin(async {
            Ok(AdminRuntimeSnapshot {
                fingerprint: self.current.fingerprint(),
                document: self.current.clone(),
                restart_required: false,
            })
        })
    }

    fn validate(
        &self,
        document: AdminRuntimeDocument,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        // 覆盖不变时无需部署基值；模拟文件适配器接收落成候选的检查边界。
        if document.operations.as_ref().map(|operations| &operations.alerts)
            != self.current.operations.as_ref().map(|operations| &operations.alerts)
        {
            return Err(
                AdminRuntimeConfigError::InvalidDocument(self.worker_error.to_owned()).into()
            );
        }
        Ok(AdminRuntimeSnapshot {
            fingerprint: document.fingerprint(),
            document,
            restart_required: false,
        })
    }

    fn update(
        &self,
        command: AdminRuntimeUpdateCommand,
    ) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
        Box::pin(async move {
            assert_eq!(command.expected_fingerprint, self.current.fingerprint());
            self.validate(command.document.resolve_update(&self.current))
        })
    }
}

async fn json_response(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let body = to_bytes(response.into_body(), 96 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn legacy_threshold_request_has_the_same_preview_and_save_result() {
    let current = document(json!({"operations": {"alerts": {
        "managed": true,
        "fulfillment_collection_overdue_age_seconds": 86_400
    }}}));
    let fingerprint = current.fingerprint();
    let port: Arc<dyn AdminRuntimeConfigPort> =
        Arc::new(RuntimeWithWorkerError { current, worker_error: WORKER_FILE_HINT });
    let address = ConnectInfo("127.0.0.1:1234".parse().unwrap());
    let candidate =
        json!({"operations": {"alerts": {"managed": true}}, "allow_manual_early_open": true});
    let save = update(
        state_with_runtime(Some(port.clone())), address, headers(),
        "/api/v1/admin/config".parse().unwrap(),
        Bytes::from(json!({"expected_fingerprint": fingerprint, "command_id": VALID_UUID, "document": candidate}).to_string()),
    ).await;
    let (status, save) = json_response(save).await;
    assert_eq!(status, StatusCode::OK);

    for query in ["", "?include=fulfillment_collection_overdue_age_seconds"] {
        let preview = validate(
            state_with_runtime(Some(port.clone())),
            address,
            headers(),
            format!("/api/v1/admin/config:validate{query}").parse().unwrap(),
            Bytes::from(json!({"document": candidate}).to_string()),
        )
        .await;
        let (status, preview) = json_response(preview).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert_eq!(preview["data"]["fingerprint"], save["data"]["fingerprint"], "{query}");
        let alerts = &preview["data"]["document"]["operations"]["alerts"];
        if query.is_empty() {
            assert!(
                !alerts
                    .as_object()
                    .unwrap()
                    .contains_key("fulfillment_collection_overdue_age_seconds")
            );
        } else {
            assert_eq!(alerts["fulfillment_collection_overdue_age_seconds"], 86_400);
        }
    }
}

#[tokio::test]
async fn invalid_runtime_response_exposes_only_fixed_worker_file_hints() {
    let address = ConnectInfo("127.0.0.1:1234".parse().unwrap());
    let candidate = json!({"operations":{"alerts":{"managed":true}}});
    for hint in [WORKER_FILE_HINT, UNREADABLE_WORKER_FILE_HINT, CHANGED_WORKER_FILE_HINT] {
        let current = AdminRuntimeDocument::default();
        let fingerprint = current.fingerprint();
        let state = state_with_runtime(Some(Arc::new(RuntimeWithWorkerError {
            current,
            worker_error: hint,
        })));
        let preview = validate(
            state.clone(),
            address,
            headers(),
            "/api/v1/admin/config:validate".parse().unwrap(),
            Bytes::from(json!({"document": candidate}).to_string()),
        )
        .await;
        let save = update(
            state,
            address,
            headers(),
            "/api/v1/admin/config".parse().unwrap(),
            Bytes::from(
                json!({
                    "expected_fingerprint": fingerprint,
                    "command_id": VALID_UUID,
                    "document": candidate,
                })
                .to_string(),
            ),
        )
        .await;
        for response in [preview, save] {
            let (status, body) = json_response(response).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["code"], "invalid_runtime_config");
            assert_eq!(body["message"], hint);
        }

        let reason = format!("{hint} /fixture/private.toml");
        let response = map_error(AdminRuntimeConfigError::InvalidDocument(reason).into());
        let (status, body) = json_response(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_runtime_config");
        assert_eq!(body["message"], "runtime 配置文档无效");
    }
}

/// 完整 GET 响应与浏览器解码器共用夹具。改动投影时，三份夹具和两端测试须一起更新。
#[tokio::test]
async fn runtime_config_get_matches_browser_fixtures() {
    let uri: Uri = "/api/v1/admin/config?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds"
        .parse()
        .unwrap();
    let address = ConnectInfo("127.0.0.1:20123".parse().unwrap());
    let long_prefix = format!("/{}/", "装".repeat(200));
    let long_endpoint = format!("https://example.invalid/{}/v1/traces", "a".repeat(2100));
    for (name, input, fixture) in [
        (
            "default",
            json!({}),
            include_str!(
                "../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
            ),
        ),
        (
            "long-values",
            json!({
                "equipment_source": {"allowed_path_prefixes": [long_prefix]},
                "tracing": {"enabled": true, "sample_ratio_ppm": 1, "endpoint": long_endpoint},
            }),
            include_str!(
                "../../../../auction-contracts/tests/fixtures/admin/runtime-config-long-values.json"
            ),
        ),
        (
            "disabled-origins",
            json!({
                "security": {
                    "allowed_origins_enabled": false,
                    "allowed_origins": ["", " https://example.invalid ", ""],
                },
            }),
            include_str!(
                "../../../../auction-contracts/tests/fixtures/admin/runtime-config-disabled-origins.json"
            ),
        ),
    ] {
        // 固定文档由端口计算真实文件指纹，比较保留指纹和全部响应字段。
        let state = state_with_runtime(Some(Arc::new(RuntimeWithWorkerError {
            current: document(input),
            worker_error: WORKER_FILE_HINT,
        })));
        let response = get(state, address, headers(), uri.clone()).await;
        let (status, actual) = json_response(response).await;
        assert_eq!(status, StatusCode::OK, "{name}");
        let expected: Value = serde_json::from_str(fixture).unwrap();
        assert_eq!(actual, expected, "{name}");
    }
}
