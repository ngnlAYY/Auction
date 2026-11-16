//! runtime 配置投影的回归用例：GET、校验与更新响应都经 `map_snapshot` 生成，邮件区块必须
//! 补齐全部省略键，且补齐后的文档原样提交时仍序列化回同一指纹。

use std::collections::BTreeSet;

use auction_application::admin::{AdminRuntimeDocument, AdminRuntimeSnapshot};
use serde_json::{Value, json};

use super::map_snapshot;

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

/// 按文件快照投影；返回投影文档与快照指纹。
fn project(document: AdminRuntimeDocument) -> (Value, String) {
    let fingerprint = document.fingerprint();
    let snapshot = AdminRuntimeSnapshot {
        document,
        fingerprint: fingerprint.clone(),
        restart_required: false,
    };
    let response = map_snapshot(snapshot, false).expect("合法快照必须可投影");
    assert_eq!(response.fingerprint, fingerprint, "投影不改变物理文档指纹");
    (response.document, fingerprint)
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

/// 告警区块新增的长期未领取阈值同样按默认值省略、在投影中补齐；显式值优先，
/// 补齐不改变物理文档指纹（见上一用例的重提交校验）。
#[test]
fn alerts_projection_fills_collection_overdue_threshold() {
    for source in [json!({}), json!({"operations": {"alerts": {"managed": true}}})] {
        let (projected, _) = project(document(source));
        assert_eq!(
            projected["operations"]["alerts"]["fulfillment_collection_overdue_age_seconds"],
            259_200
        );
    }
    let (projected, _) = project(document(json!({
        "operations": {"alerts": {
            "managed": true,
            "fulfillment_collection_overdue_age_seconds": 86_400
        }}
    })));
    assert_eq!(
        projected["operations"]["alerts"]["fulfillment_collection_overdue_age_seconds"],
        86_400
    );
    let source = json!({"operations": {"alerts": {"managed": true}}});
    let mut materialized = document(source.clone());
    materialized.materialize_business_defaults();
    let (projected, _) = project(document(source));
    assert_eq!(document(projected).fingerprint(), materialized.fingerprint());
}
