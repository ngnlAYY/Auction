//! 论坛回帖 lot 映射（`ForumReplyStore::lot_targets`）的 PostgreSQL 18 用例。
//!
//! 论坛 `start` 命令按映射携带的权威起拍价计算首口出价：每件 live/sold lot 都要带出自己的起拍价
//! （含不在信用网格上的历史值），lot 代码按分类编号生成；撤下的 lot 不在映射内。维护账号写入种子，
//! 运行账号经端口读取。仅在 `test-db` feature 下编译，入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::forum::ForumReplyStore;
use auction_domain::{AuctionId, BidAmount, LotId};
use auction_time::{Duration, Timestamp};

use crate::support;
use crate::support::fixtures::{policy_snapshot, seed_user, stable_id};

const SEED: &str = "forum-reply-targets";

#[tokio::test]
async fn postgres_forum_lot_targets_carry_authoritative_start_prices() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let pool = admin.postgres_pool();
    let now = Timestamp::now();
    let deadline = now + Duration::hours(6);
    let auction_id = stable_id(SEED, "auction");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1, 'main', 1, 'forum targets', 'live', $2, $2 - interval '1 hour', $3, $3, $4)",
    )
    .bind(auction_id)
    .bind(now - Duration::hours(1))
    .bind(deadline)
    .bind(policy_snapshot(deadline, 50_000, 30_000))
    .execute(pool)
    .await
    .expect("写入 live 场次");
    sqlx::query(
        "INSERT INTO public.auction_forum_sync (auction_id, status, thread_id, thread_url, title, body)
         VALUES ($1, 'published', '301', 'https://example.invalid/thread/301', 't', 'b')",
    )
    .bind(auction_id)
    .execute(pool)
    .await
    .expect("写入已发布论坛主题");

    let leader = stable_id(SEED, "leader");
    seed_user(pool, leader, "ForumTargetLeader").await;
    // (分类, 分类编号, 起拍价, 当前价与领先者, lot 状态)；1,500 是不在信用网格上的历史起拍价，
    // 已售 lot 的成交价高于起拍价，映射不得把当前价当作起拍价。
    let lots = [
        ("One-Handed", 1, 50_000, None, "live"),
        ("One-Handed", 2, 1_500, None, "live"),
        ("Staff", 1, 70_000, Some((90_000, leader)), "sold"),
        ("Light", 1, 30_000, None, "withdrawn"),
    ];
    let mut lot_ids = Vec::new();
    for (index, (category, category_number, start_price, bid, status)) in lots.iter().enumerate() {
        let equipment_id = stable_id(SEED, &format!("equipment-{index}"));
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ($1, 'main', $2, $3, $4, '{}', $5)",
        )
        .bind(equipment_id)
        .bind(format!("{}", 31_000 + index))
        .bind(format!("{SEED} equipment {index}"))
        .bind(*category)
        .bind(now - Duration::days(1))
        .execute(pool)
        .await
        .expect("写入测试装备");
        let lot_id = stable_id(SEED, &format!("lot-{index}"));
        sqlx::query(
            "INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
                 leader_user_id, effective_end_at, category_key, category_number)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(lot_id)
        .bind(auction_id)
        .bind(equipment_id)
        .bind(i32::try_from(index + 1).unwrap())
        .bind(*status)
        .bind(*start_price)
        .bind(bid.map_or(*start_price, |(price, _)| price))
        .bind(bid.map(|(_, user_id)| user_id))
        .bind(deadline)
        .bind(*category)
        .bind(*category_number)
        .execute(pool)
        .await
        .expect("写入测试 lot");
        lot_ids.push(LotId::from_uuid(lot_id));
    }

    let mut targets =
        store.lot_targets(AuctionId::from_uuid(auction_id)).await.expect("读取论坛回帖 lot 映射");
    targets.sort_by(|left, right| left.code.cmp(&right.code));
    let price = |value: i64| BidAmount::from_persisted_positive(value).unwrap();
    let actual = targets
        .iter()
        .map(|target| (target.code.as_str(), target.lot_id, target.start_price))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            ("One01", lot_ids[0], price(50_000)),
            ("One02", lot_ids[1], price(1_500)),
            ("Sta01", lot_ids[2], price(70_000)),
        ]
    );

    store.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}
