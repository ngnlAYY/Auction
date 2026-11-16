//! runtime 配置投影的回归用例：GET、校验与更新响应都经 `map_snapshot` 生成，邮件区块必须
//! 补齐全部省略键，且补齐后的文档原样提交时仍序列化回同一指纹；告警区块的长期未领取阈值只在
//! 新页面显式 include 时输出，缺省投影保持已发布闭集。

use std::collections::BTreeSet;

use auction_application::admin::{AdminRuntimeDocument, AdminRuntimeSnapshot};
use axum::body::to_bytes;
use axum::http::{StatusCode, Uri};
use serde_json::{Value, json};

use super::{RuntimeProjection, map_snapshot, runtime_projection};

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
