//! 公开目录读取的场次投影与分类计数：两者在同一条查询里取得，计数口径只由场次状态决定。
//!
//! 页码取在全部拍品之后，当前页为空，只校验投影、分类计数、总数与它们的一致性
//! （`PublicLotPage::validate` 全部执行），不依赖装备展示映射。维护账号写入种子，
//! 运行账号经公开端口读取。仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use auction_application::auction::{PublicLotListRequest, PublicLotReader};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;

fn stable_id(seed: &str, suffix: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("public-lots-{seed}-{suffix}").as_bytes())
}

fn policy(deadline: DateTime<Utc>) -> serde_json::Value {
    json!({
        "schema_version": 5,
        "bidding_version": 2,
        "start_price": 50_000,
        "bid_increment_ppm": 0,
        "minimum_increment": 30_000,
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

/// 写入一场指定状态的场次与若干（装备分类, lot 状态）拍品。
async fn seed_auction(
    pool: &PgPool,
    seed: &str,
    world: &str,
    sequence: i32,
    status: &str,
    lots: &[(&str, &str)],
) -> Uuid {
    let now = Utc::now();
    // 预览场次的开拍时间在将来，其余场次已开拍。
    let opens_at =
        if status == "preview" { now + Duration::hours(2) } else { now - Duration::hours(4) };
    let deadline = opens_at + Duration::hours(6);
    let auction_id = stable_id(seed, "auction");
    // 草稿/预览场次与本世界的收件目标由延迟约束触发器校验一致，必须同事务写入。
    let mut transaction = pool.begin().await.expect("开启种子事务");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, settled_at, policy_snapshot)
         VALUES ($1, $2, $3, $4, $5, $6, $6 - interval '1 hour', $7, $7,
                 CASE WHEN $5 = 'settled' THEN $7 END, $8)",
    )
    .bind(auction_id)
    .bind(world)
    .bind(sequence)
    .bind(format!("{seed} auction"))
    .bind(status)
    .bind(opens_at)
    .bind(deadline)
    .bind(policy(deadline))
    .execute(&mut *transaction)
    .await
    .expect("写入测试场次");
    if status == "preview" {
        sqlx::query(
            "INSERT INTO public.auction_intake_targets (world_id, auction_id, target_status)
             VALUES ($1, $2, 'preview')",
        )
        .bind(world)
        .bind(auction_id)
        .execute(&mut *transaction)
        .await
        .expect("写入预览收件目标");
    }
    transaction.commit().await.expect("提交种子事务");
    for (index, (category, lot_status)) in lots.iter().enumerate() {
        let equipment_id = stable_id(seed, &format!("equipment-{index}"));
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, collected_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(equipment_id)
        .bind(world)
        .bind(format!("{}{index}", 1_000 + sequence * 100))
        .bind(format!("{seed} equipment {index}"))
        .bind(*category)
        .bind(now - Duration::days(1))
        .execute(pool)
        .await
        .expect("写入测试装备");
        let number = i32::try_from(index + 1).expect("lot number");
        // 待定拍品必须挂在一条投稿上（`lots_pending_submission_check`），投稿又引用命令账本。
        let submission_id = if *lot_status == "pending" {
            let command_id = stable_id(seed, &format!("submission-command-{index}"));
            let mut request_hash = [0_u8; 32];
            request_hash[..16].copy_from_slice(command_id.as_bytes());
            sqlx::query(
                "INSERT INTO public.auction_commands
                    (command_id, command_kind, aggregate_id, request_hash, result_code)
                 VALUES ($1, 'mail_intake', $2, $3, 'accepted')",
            )
            .bind(command_id)
            .bind(auction_id)
            .bind(request_hash.as_slice())
            .execute(pool)
            .await
            .expect("写入测试投稿命令");
            let submission_id = stable_id(seed, &format!("submission-{index}"));
            sqlx::query(
                "INSERT INTO public.auction_submissions
                    (id, source_command_id, auction_id, mailbox_key, external_mail_id, seller,
                     received_at)
                 VALUES ($1, $2, $3, 'mooglemail:main', $4, 'seller', $5)",
            )
            .bind(submission_id)
            .bind(command_id)
            .bind(auction_id)
            .bind(format!("{}{index}", 2_000 + sequence * 100))
            .bind(now - Duration::days(1))
            .execute(pool)
            .await
            .expect("写入测试投稿");
            Some(submission_id)
        } else {
            None
        };
        sqlx::query(
            "INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
                 effective_end_at, category_key, category_number, submission_id)
             VALUES ($1, $2, $3, $4, $5, 50000, 50000, $6, $7, $4, $8)",
        )
        .bind(stable_id(seed, &format!("lot-{index}")))
        .bind(auction_id)
        .bind(equipment_id)
        .bind(number)
        .bind(*lot_status)
        .bind(deadline)
        .bind(*category)
        .bind(submission_id)
        .execute(pool)
        .await
        .expect("写入测试 lot");
    }
    auction_id
}

/// 只要计数，不要拍品：offset 落在全部拍品之后。
fn counts_only(category: Option<&str>) -> PublicLotListRequest {
    PublicLotListRequest {
        category: category.map(str::to_owned),
        equipment_group: None,
        max_level: None,
        include_unleveled: true,
        limit: 20,
        offset: 1_000,
    }
}

fn expected(pairs: &[(&str, i64)]) -> BTreeMap<String, i64> {
    let mut counts: BTreeMap<String, i64> =
        ["all", "one", "two", "sta", "shd", "clo", "lig", "hea"]
            .into_iter()
            .map(|key| (key.to_owned(), 0))
            .collect();
    for (key, value) in pairs {
        counts.insert((*key).to_owned(), *value);
    }
    counts
}

#[tokio::test]
async fn postgres_public_lot_counts_follow_the_auction_status_in_one_query() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let seed_store = database.maintenance_store().await.expect("维护账号连接");
    let pool = seed_store.postgres_pool();

    // 进行中：live/sold/unsold 计入，pending 不计入。
    let live = seed_auction(
        pool,
        "live",
        "main",
        1,
        "live",
        &[
            ("One-Handed", "live"),
            ("One-Handed", "sold"),
            ("Cloth", "unsold"),
            ("Heavy", "pending"),
        ],
    )
    .await;
    // 预览（另一世界，避免被进行中场次遮住）：只计 pending。
    let preview = seed_auction(
        pool,
        "preview",
        "isekai",
        1,
        "preview",
        &[("Staff", "pending"), ("Shield", "pending"), ("Light", "withdrawn")],
    )
    .await;
    // 已结算：只计 sold/unsold。
    let settled = seed_auction(
        pool,
        "settled",
        "main",
        2,
        "settled",
        &[("Two-Handed", "sold"), ("Heavy", "unsold"), ("Cloth", "live")],
    )
    .await;

    let store = database.store().await.expect("运行账号连接");
    let reader = store.public_lot_reader(Vec::new());

    let page = reader.list_public_lots(live, counts_only(None)).await.expect("读取").expect("可见");
    assert_eq!(page.projection.projection_kind, "live");
    assert_eq!(page.category_counts, expected(&[("all", 3), ("one", 2), ("clo", 1)]));
    assert_eq!(page.total, 3);
    assert!(page.live.is_empty() && page.preview.is_empty());
    let one =
        reader.list_public_lots(live, counts_only(Some("one"))).await.expect("读取").expect("可见");
    assert_eq!(one.total, 2, "分类总数取对应分类计数");

    let page =
        reader.list_public_lots(preview, counts_only(None)).await.expect("读取").expect("可见");
    assert_eq!(page.projection.projection_kind, "preview");
    assert_eq!(page.category_counts, expected(&[("all", 2), ("sta", 1), ("shd", 1)]));
    assert_eq!(page.total, 2);

    let page =
        reader.list_public_lots(settled, counts_only(None)).await.expect("读取").expect("可见");
    assert_eq!(page.projection.auction_status, "settled");
    assert_eq!(page.category_counts, expected(&[("all", 2), ("two", 1), ("hea", 1)]));
    assert_eq!(page.total, 2);

    // 不存在的场次不可见，不返回投影或计数。
    assert!(
        reader
            .list_public_lots(stable_id("missing", "auction"), counts_only(None))
            .await
            .expect("读取")
            .is_none()
    );
}
