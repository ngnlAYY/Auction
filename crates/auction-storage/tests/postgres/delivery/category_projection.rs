//! 发送类别的 Rust 只读投影与 SQL `public.delivery_send_category` 的交叉校验。
//!
//! 节奏与出队优先级只读 SQL 函数，Admin 列表用 Rust 镜像展示类别。082 重定义 SQL 函数时漏回
//! rekey 模板（098 才补回），而 Rust 镜像只有手写期望测试，两边分叉无人发现。这里对全部通知
//! 模板 × 来源 × 操作 × 资产形状逐一比较两边结果，任何一侧单独修改都会失败。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::mail::MailNotificationTemplate;
use auction_storage::delivery_admin_send_category;
use serde_json::{Value, json};

use crate::support;

/// 投递来源：通知来源、结算来源，以及闭集外的值（应落入 `manual`）。
const ORIGINS: [&str; 9] = [
    "mail_intake",
    "settlement_recovery_action",
    "operations_alert",
    "lot_watch",
    "seller_summary",
    "season_reminder",
    "settlement",
    "manual",
    "",
];

/// 通知模板全集，加结算模板与缺失、非字符串的模板值。
fn templates() -> Vec<Value> {
    let mut templates = MailNotificationTemplate::ALL
        .into_iter()
        .map(|template| json!(template.as_str()))
        .collect::<Vec<_>>();
    templates.extend([
        json!("settlement_winner_equipment_cod"),
        json!("settlement_payment_carrier"),
        json!("settlement_seller_payout"),
        json!(null),
        json!(42),
    ]);
    templates
}

/// 资产形状：无资产、装备引用、入站附件，以及非数组的损坏值。
fn asset_shapes() -> [(Value, Value); 4] {
    [
        (json!([]), json!([])),
        (json!([{ "kind": "auction_equipment_key", "id": "fixture" }]), json!([])),
        (json!([]), json!(["4242"])),
        (json!("not-an-array"), json!({ "4242": true })),
    ]
}

fn payloads() -> Vec<Value> {
    let mut payloads = vec![json!({}), json!([]), json!("send")];
    for operation in [json!("send"), json!("return"), json!("take"), json!(null), json!(1)] {
        for template in templates() {
            for (equipment_secret_refs, attachment_ids) in asset_shapes() {
                payloads.push(json!({
                    "operation": operation,
                    "template": template,
                    "equipment_secret_refs": equipment_secret_refs,
                    "attachment_ids": attachment_ids,
                }));
            }
        }
    }
    payloads
}

/// Rust 投影与 SQL 函数对每个来源 × 载荷组合给出同一类别（含 `NULL`）。
#[tokio::test]
async fn postgres_send_category_projection_matches_sql_function() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let mut origins = Vec::new();
    let mut cases = Vec::new();
    for origin in ORIGINS {
        for payload in payloads() {
            origins.push(origin.to_owned());
            cases.push(payload);
        }
    }

    let categories: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT public.delivery_send_category(c.origin, c.payload)
         FROM unnest($1::text[], $2::jsonb[]) WITH ORDINALITY AS c(origin, payload, ordinal)
         ORDER BY c.ordinal",
    )
    .bind(&origins)
    .bind(&cases)
    .fetch_all(store.postgres_pool())
    .await
    .expect("批量读取 SQL 发送类别");

    assert_eq!(categories.len(), cases.len());
    for ((origin, payload), sql_category) in origins.iter().zip(&cases).zip(categories) {
        assert_eq!(
            delivery_admin_send_category(origin, payload),
            sql_category,
            "origin={origin:?} payload={payload}"
        );
    }
    // 夹具必须覆盖每个类别，避免组合退化成只比较 NULL。
    for expected in
        ["account", "equipment", "intake", "manual", "operations", "outbid", "settlement"]
    {
        assert!(
            origins.iter().zip(&cases).any(|(origin, payload)| {
                delivery_admin_send_category(origin, payload).as_deref() == Some(expected)
            }),
            "夹具未覆盖类别 {expected}"
        );
    }
    store.postgres_pool().close().await;
}
