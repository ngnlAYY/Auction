//! Admin 投递列表的缺省投影与扩展投影（`include=settlement_returns`）的 PostgreSQL 18 用例。
//!
//! 缺省列表要能被升级前已发布的 Admin 页面按闭集解码：结算退件 Take（来源
//! `settlement_return_action`）不在缺省投影内。排除必须在 SQL 里与计数同一条件，不能在页内事后
//! 过滤，否则总数与分页对不上。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::delivery::{DeliveryAdminListRequest, DeliveryAdminPort};
use chrono::{DateTime, Duration, TimeZone, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;

/// 升级前已发布 Admin 页面（v0.1.5）解码投递时认可的来源闭集（`valid_origin_kind`）；独立抄录，
/// 代表仍在浏览器里运行的旧 decoder。
const RELEASED_DECODER_ORIGINS: [&str; 9] = [
    "settlement",
    "settlement_recovery_action",
    "user_key",
    "manual",
    "mail_intake",
    "operations_alert",
    "lot_watch",
    "seller_summary",
    "season_reminder",
];

/// 数据库当前允许的全部来源；结算退件 Take 最后写入，按创建时间倒序排在第一页最前面。
const STORED_ORIGINS: [&str; 10] = [
    "settlement",
    "settlement_recovery_action",
    "user_key",
    "manual",
    "mail_intake",
    "operations_alert",
    "lot_watch",
    "seller_summary",
    "season_reminder",
    "settlement_return_action",
];

/// 写入一条 pending 投递；结算退件 Take 用系统退件发件人与 Take 载荷，其余为普通 Send。
async fn insert_pending(pool: &PgPool, origin_kind: &str, created_at: DateTime<Utc>) -> Uuid {
    let id = Uuid::new_v4();
    let (recipient, payload) = if origin_kind == "settlement_return_action" {
        ("MoogleMail", r#"{"schema_version":1,"operation":"take","world":"main"}"#)
    } else {
        (
            "Sample",
            r#"{"schema_version":1,"operation":"send","template":"register_key","credits":0,"cod":0,"attachment_ids":[]}"#,
        )
    };
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, created_at, updated_at)
         VALUES ($1, $2, decode(repeat('44', 32), 'hex'), $3, $4, $5, $6::jsonb,
                 'pending', $7, $7)",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(origin_kind)
    .bind(format!("projection-{id}"))
    .bind(recipient)
    .bind(payload)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("写入投递");
    id
}

/// 缺省列表的每个来源都在旧页面闭集内，结算退件 Take 不计入总数、不占分页；扩展投影列出它，
/// 按参考编号定位也遵守同一投影。
#[tokio::test]
async fn postgres_admin_delivery_list_default_projection_omits_settlement_return_takes() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let base = Utc.with_ymd_and_hms(2026, 11, 16, 0, 0, 0).single().expect("时间");
    let mut take_id = Uuid::nil();
    for (offset, origin) in (0_i64..).zip(STORED_ORIGINS) {
        let id = insert_pending(pool, origin, base + Duration::minutes(offset)).await;
        if origin == "settlement_return_action" {
            take_id = id;
        }
    }
    assert!(!take_id.is_nil());
    // 时间窗只框住本用例写入的行，不受模板库里其他投递影响。
    let request = |include: bool, reference: Option<Uuid>, limit: i64, offset: i64| {
        DeliveryAdminListRequest {
            reference,
            created_from: Some(base),
            created_to: Some(base + Duration::hours(1)),
            include_settlement_return_actions: include,
            limit,
            offset,
            ..DeliveryAdminListRequest::default()
        }
    };

    let default =
        DeliveryAdminPort::list(&store, request(false, None, 100, 0)).await.expect("缺省列表");
    assert_eq!(default.total, 9);
    assert_eq!(default.entries.len(), 9);
    assert!(
        default
            .entries
            .iter()
            .all(|entry| RELEASED_DECODER_ORIGINS.contains(&entry.origin_kind.as_str())),
        "{:?}",
        default.entries.iter().map(|entry| entry.origin_kind.as_str()).collect::<Vec<_>>()
    );

    // 最新的结算退件 Take 不占缺省第一页：页内条数与总数同一口径。
    let first_page =
        DeliveryAdminPort::list(&store, request(false, None, 4, 0)).await.expect("缺省第一页");
    assert_eq!(first_page.total, 9);
    assert_eq!(
        first_page.entries.iter().map(|entry| entry.origin_kind.as_str()).collect::<Vec<_>>(),
        ["season_reminder", "seller_summary", "lot_watch", "operations_alert"]
    );
    let last_page =
        DeliveryAdminPort::list(&store, request(false, None, 4, 8)).await.expect("缺省末页");
    assert_eq!(last_page.total, 9);
    assert_eq!(
        last_page.entries.iter().map(|entry| entry.origin_kind.as_str()).collect::<Vec<_>>(),
        ["settlement"]
    );

    let extended =
        DeliveryAdminPort::list(&store, request(true, None, 100, 0)).await.expect("扩展列表");
    assert_eq!(extended.total, 10);
    assert_eq!(extended.entries.first().map(|entry| entry.id), Some(take_id));
    assert_eq!(extended.entries[0].origin_kind, "settlement_return_action");

    let by_reference = DeliveryAdminPort::list(&store, request(false, Some(take_id), 50, 0))
        .await
        .expect("缺省投影按参考编号定位");
    assert_eq!(by_reference.total, 0);
    assert!(by_reference.entries.is_empty());
    let by_reference = DeliveryAdminPort::list(&store, request(true, Some(take_id), 50, 0))
        .await
        .expect("扩展投影按参考编号定位");
    assert_eq!(by_reference.total, 1);
    assert_eq!(by_reference.entries.first().map(|entry| entry.id), Some(take_id));

    store.postgres_pool().close().await;
}
