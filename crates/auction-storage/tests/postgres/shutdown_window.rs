//! 验收：停机时段门禁的数据库证据。
//!
//! `collect_shutdown_window` 是 `auction-cli service stop/restart` 拒绝停机的唯一事实来源：
//! 所有 Final 场次无条件保护；在拍 lot 只要有效截止不晚于保护窗口末端（含已过截止仍 live）
//! 就保护；窗口外截止与非 live 的 lot 不阻挡。以运行账号读取，证明只读快照不需要写权限。
//! 入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::support;

const WINDOW_MINUTES: u32 = 60;

fn policy(deadline: DateTime<Utc>) -> serde_json::Value {
    json!({
        "schema_version": 5,
        "bidding_version": 2,
        "start_price": 50_000,
        "bid_increment_ppm": 0,
        "minimum_increment": 1_000,
        "soft_close_window_seconds": 900,
        "soft_close_extension_seconds": 900,
        "proxy_enabled": true,
        "proxy_default_mode": "fast",
        "proxy_slow_interval_seconds": 600,
        "proxy_max_outbid_uses": 0,
        "final_phase_lead_seconds": 900,
        "scheduled_end_at": deadline,
        "settlement": {
            "version": 1,
            "mode": "hold_curve_v1",
            "compensation_pool_enabled": true,
            "anchors": []
        },
        "recovery": {"version": 1, "mode": "remind_v1"}
    })
}

/// 每个世界同时只能有一场活动场次（`auctions_one_active_world_idx`），调用方按世界区分。
async fn seed_auction(
    pool: &sqlx::PgPool,
    world_id: &str,
    sequence: i32,
    status: &str,
    deadline: DateTime<Utc>,
) -> Uuid {
    let auction_id = Uuid::new_v4();
    let opened = deadline - Duration::hours(4);
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1, $8, $2, $3, $4, $5, $5, $6, $6, $7)",
    )
    .bind(auction_id)
    .bind(sequence)
    .bind(format!("shutdown window {sequence}"))
    .bind(status)
    .bind(opened)
    .bind(deadline)
    .bind(policy(deadline))
    .bind(world_id)
    .execute(pool)
    .await
    .expect("写入测试场次");
    auction_id
}

async fn seed_lot(
    pool: &sqlx::PgPool,
    auction_id: Uuid,
    number: i32,
    status: &str,
    leader: Option<Uuid>,
    effective_end_at: DateTime<Utc>,
) -> Uuid {
    let equipment_id = Uuid::new_v4();
    let lot_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, collected_at)
         VALUES ($1, 'main', $2, $3, 'weapon', $4)",
    )
    .bind(equipment_id)
    .bind(format!("shutdown-{auction_id}-{number}"))
    .bind(format!("shutdown equipment {number}"))
    .bind(effective_end_at - Duration::hours(5))
    .execute(pool)
    .await
    .expect("写入测试装备");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             leader_user_id, effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, $4, $5, 50000, $6, $7, $8, 'weapon', $4)",
    )
    .bind(lot_id)
    .bind(auction_id)
    .bind(equipment_id)
    .bind(number)
    .bind(status)
    .bind(leader.map(|_| 120_000_i64))
    .bind(leader)
    .bind(effective_end_at)
    .execute(pool)
    .await
    .expect("写入测试 lot");
    lot_id
}

#[tokio::test]
async fn postgres_shutdown_window_protects_final_and_lots_ending_inside_the_window() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("以维护账号写入夹具");
    let pool = maintenance.postgres_pool();
    let now = Utc::now();

    let leader = Uuid::new_v4();
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(leader)
        .bind("shutdown-window-leader")
        .bind([9_u8; 32].as_slice())
        .execute(pool)
        .await
        .expect("写入测试用户");

    // 在拍场次：已过截止仍 live、窗口内、窗口外三个 live lot，以及窗口内已流拍的 lot。
    let live_auction = seed_auction(pool, "main", 1, "live", now + Duration::hours(3)).await;
    let overdue =
        seed_lot(pool, live_auction, 1, "live", Some(leader), now - Duration::minutes(10)).await;
    let ending =
        seed_lot(pool, live_auction, 2, "live", Some(leader), now + Duration::minutes(30)).await;
    let later =
        seed_lot(pool, live_auction, 3, "live", Some(leader), now + Duration::hours(3)).await;
    let closed = seed_lot(pool, live_auction, 4, "unsold", None, now + Duration::minutes(20)).await;
    // Final 场次（另一世界）：即使截止远在窗口之外也无条件保护。
    let final_auction = seed_auction(pool, "isekai", 1, "final", now + Duration::hours(5)).await;
    // 已结算场次不保护。
    let settled_auction = seed_auction(pool, "main", 2, "settled", now - Duration::days(1)).await;

    let runtime = database.store().await.expect("以运行账号读取");
    let postgres = &runtime;
    let window = postgres.collect_shutdown_window(WINDOW_MINUTES).await.expect("读取停机时段证据");

    assert_eq!(
        window.protected_until - window.checked_at,
        Duration::minutes(i64::from(WINDOW_MINUTES)),
        "窗口按数据库时钟计算"
    );
    let finals: Vec<Uuid> = window.final_auctions.iter().map(|item| item.auction_id).collect();
    assert_eq!(finals, [final_auction], "只有 Final 场次无条件保护");
    assert!(!finals.contains(&settled_auction));
    let lots: Vec<Uuid> = window.ending_lots.iter().map(|lot| lot.lot_id).collect();
    assert_eq!(lots, [overdue, ending], "按截止排序：已过截止仍 live 的在前，窗口内的在后");
    assert!(!lots.contains(&later), "窗口外截止不阻挡");
    assert!(!lots.contains(&closed), "非 live 的 lot 不阻挡");
    assert!(window.ending_lots.iter().all(|lot| lot.auction_id == live_auction));

    runtime.postgres_pool().close().await;
    maintenance.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_shutdown_window_is_empty_when_nothing_is_in_progress() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号读取");
    let postgres = &runtime;
    let window = postgres.collect_shutdown_window(WINDOW_MINUTES).await.expect("读取停机时段证据");
    assert!(window.final_auctions.is_empty() && window.ending_lots.is_empty(), "空库允许停机");
    runtime.postgres_pool().close().await;
}
