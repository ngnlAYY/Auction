//! 公开目录读取的场次投影与分类计数：两者在同一条查询里取得，计数口径只由场次状态决定。
//!
//! 计数用例的页码取在全部拍品之后，当前页为空，只校验投影、分类计数、总数与它们的一致性
//! （`PublicLotPage::validate` 全部执行），不依赖装备展示映射。分类用例给各大类种互不相同的
//! 件数，另外核对 SQL 中的分类统计、分类筛选与竞价日志 lot 代码搜索，与领域大类映射及 Rust
//! 生成的 lot 代码逐项一致（含未知分类）。当前场次快照另核对冻结策略的代理默认模式与缺值时的
//! 失败关闭。维护账号写入种子，运行账号经公开端口读取。仅在 `test-db` feature 下编译，入口见
//! `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use auction_application::auction::{
    AuctionReader, PublicBidLogListRequest, PublicBidLogReader, PublicLotListRequest,
    PublicLotReader, PublicPreviewLotSummary,
};
use auction_application::public::lot_code;
use auction_domain::ProxyMode;
use auction_domain::equipment::taxonomy::EquipmentCategory;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;
use crate::support::fixtures::{policy_snapshot, seed_user, stable_id};

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
    .bind(policy_snapshot(deadline, 50_000, 30_000))
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
        // 带等级与品质的最小属性，使分类用例可以完整映射公开装备投影。
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ($1, $2, $3, $4, $5, '{\"quality\": \"Legendary\", \"level\": 300}', $6)",
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

/// 带外改写冻结策略的代理默认模式；形状 CHECK 拒绝闭集外的字符串，但对 JSON null 放行。
async fn set_proxy_default_mode(pool: &PgPool, auction_id: Uuid, value: &Value) {
    sqlx::query(
        "UPDATE public.auctions
         SET policy_snapshot = jsonb_set(policy_snapshot, '{proxy_default_mode}', CAST($2 AS jsonb))
         WHERE id = $1",
    )
    .bind(auction_id)
    .bind(value.to_string())
    .execute(pool)
    .await
    .expect("改写冻结策略的代理默认模式");
}

/// 当前场次快照（含预览场次）带出该场冻结策略的代理默认模式；该字段被带外改成 JSON null 时
/// 当前场次与实时预检都失败关闭，不以 fast 兜底，恢复后照常读取。
#[tokio::test]
async fn postgres_current_auction_carries_the_frozen_proxy_default_mode() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let seed_store = database.maintenance_store().await.expect("维护账号连接");
    let pool = seed_store.postgres_pool();
    let live = seed_auction(pool, "current-live", "main", 1, "live", &[("Cloth", "live")]).await;
    let preview =
        seed_auction(pool, "current-preview", "isekai", 1, "preview", &[("Staff", "pending")])
            .await;
    let store = database.store().await.expect("运行账号连接");

    let main = store.current_auction("main".to_owned()).await.expect("读取").expect("进行中场次");
    assert_eq!((main.id, main.proxy_default_mode), (live, ProxyMode::Fast));
    let isekai = store.current_auction("isekai".to_owned()).await.expect("读取").expect("预览场次");
    assert_eq!((isekai.id, isekai.proxy_default_mode), (preview, ProxyMode::Fast));

    set_proxy_default_mode(pool, live, &json!("slow")).await;
    let main = store.current_auction("main".to_owned()).await.expect("读取").expect("进行中场次");
    assert_eq!(main.proxy_default_mode, ProxyMode::Slow, "取冻结策略的值");

    set_proxy_default_mode(pool, live, &Value::Null).await;
    assert!(store.current_auction("main".to_owned()).await.is_err(), "缺值必须失败关闭");
    assert!(store.public_realtime_auction_status(live).await.is_err(), "实时预检同样失败关闭");

    set_proxy_default_mode(pool, live, &json!("fast")).await;
    let main = store.current_auction("main".to_owned()).await.expect("读取").expect("进行中场次");
    assert_eq!(main.proxy_default_mode, ProxyMode::Fast);
}

/// 分类用例的种子名；lot UUID 为 `stable_id(CATEGORY_SEED, "lot-{index}")`。
const CATEGORY_SEED: &str = "categories";

/// 按 `EquipmentCategory::ALL` 顺序，第 i 个大类种 i + 1 件，最后加一件不在闭集内的分类。
/// 各大类件数互不相同，SQL 统计列的分类字面量或按位置读取的列任意两类互换都会改变结果；
/// 未知分类只计入 `all`。
fn category_samples() -> Vec<&'static str> {
    let mut samples: Vec<&'static str> = EquipmentCategory::ALL
        .iter()
        .enumerate()
        .flat_map(|(index, category)| std::iter::repeat_n(category.as_str(), index + 1))
        .collect();
    samples.push("Unknown");
    samples
}

fn page_request(category: Option<&str>, include_unleveled: bool) -> PublicLotListRequest {
    PublicLotListRequest {
        category: category.map(str::to_owned),
        equipment_group: None,
        max_level: None,
        include_unleveled,
        // 大于种子总数：分页不截掉任何一行，筛选漏掉的多余行也不会被 LIMIT 掩盖。
        limit: 50,
        offset: 0,
    }
}

/// 按 lot UUID 找回种子行，核对装备分类与 Rust 生成的 lot 代码。
fn assert_seeded_lot(seeded: &BTreeMap<Uuid, (&str, i64)>, lot: &PublicPreviewLotSummary) {
    let (category, number) = seeded.get(&lot.id).expect("返回的 lot 来自种子");
    assert_eq!(lot.equipment_category, *category, "{}", lot.code);
    assert_eq!(lot.code, lot_code(category, *number));
}

#[tokio::test]
async fn postgres_public_lot_categories_follow_the_domain_mapping() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let seed_store = database.maintenance_store().await.expect("维护账号连接");
    let samples = category_samples();
    let lots: Vec<(&str, &str)> = samples.iter().map(|category| (*category, "pending")).collect();
    let auction_id =
        seed_auction(seed_store.postgres_pool(), CATEGORY_SEED, "main", 1, "preview", &lots).await;
    let store = database.store().await.expect("运行账号连接");
    let reader = store.public_lot_reader(Vec::new());

    // 种子表：lot UUID → (装备分类, 类别内序号)；`seed_auction` 的类别内序号等于全局序号 index + 1。
    let seeded_ids: Vec<Uuid> =
        (0..samples.len()).map(|index| stable_id(CATEGORY_SEED, &format!("lot-{index}"))).collect();
    let seeded: BTreeMap<Uuid, (&str, i64)> = seeded_ids
        .iter()
        .zip(&samples)
        .zip(1_i64..)
        .map(|((lot_id, category), number)| (*lot_id, (*category, number)))
        .collect();
    // 期望统计逐项写死，不从领域映射推导。
    let expected_counts = expected(&[
        ("all", 29),
        ("one", 1),
        ("two", 2),
        ("sta", 3),
        ("shd", 4),
        ("clo", 5),
        ("lig", 6),
        ("hea", 7),
    ]);
    // 两条路径都要核对：包含无等级时由 SQL 统计与筛选，排除无等级时由 Rust 预筛选后按 UUID 取页。
    for include_unleveled in [true, false] {
        let page = reader
            .list_public_lots(auction_id, page_request(None, include_unleveled))
            .await
            .expect("读取")
            .expect("可见");
        assert_eq!(page.category_counts, expected_counts, "include_unleveled={include_unleveled}");
        // 全量页按全局序号排序，与种子顺序逐行一致。
        let ids: Vec<Uuid> = page.preview.iter().map(|lot| lot.id).collect();
        assert_eq!(ids, seeded_ids, "include_unleveled={include_unleveled}");
        for lot in &page.preview {
            assert_seeded_lot(&seeded, lot);
        }

        for (index, category) in EquipmentCategory::ALL.iter().enumerate() {
            let key = category.public_key();
            let page = reader
                .list_public_lots(auction_id, page_request(Some(key), include_unleveled))
                .await
                .expect("读取")
                .expect("可见");
            let count = index + 1;
            let context = format!("{key} include_unleveled={include_unleveled}");
            assert_eq!(page.total, i64::try_from(count).expect("分类件数"), "{context}");
            assert_eq!(page.preview.len(), count, "{context}");
            for lot in &page.preview {
                assert_eq!(lot.equipment_category, category.as_str(), "{context}");
                assert_seeded_lot(&seeded, lot);
            }
        }
    }
}

/// 写入一场进行中的场次：每个（分类, 类别内序号）一件 lot，各有一条普通出价事实。
async fn seed_bid_log(pool: &PgPool, seed: &str, lots: &[(&str, i32)]) -> (Uuid, Vec<Uuid>) {
    let now = Utc::now();
    let deadline = now + Duration::hours(2);
    let auction_id = stable_id(seed, "auction");
    let bidder = stable_id(seed, "bidder");
    seed_user(pool, bidder, &format!("{seed}-bidder")).await;
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1, 'main', 1, $2, 'live', $3, $3, $4, $4, $5)",
    )
    .bind(auction_id)
    .bind(format!("{seed} auction"))
    .bind(now - Duration::hours(4))
    .bind(deadline)
    .bind(policy_snapshot(deadline, 50_000, 30_000))
    .execute(pool)
    .await
    .expect("写入测试场次");
    let mut lot_ids = Vec::with_capacity(lots.len());
    for (index, (category, category_number)) in lots.iter().enumerate() {
        let equipment_id = stable_id(seed, &format!("equipment-{index}"));
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, collected_at)
             VALUES ($1, 'main', $2, $3, $4, $5)",
        )
        .bind(equipment_id)
        .bind(format!("{}", 5_000 + index))
        .bind(format!("{seed} equipment {index}"))
        .bind(*category)
        .bind(now - Duration::days(1))
        .execute(pool)
        .await
        .expect("写入测试装备");
        let lot_id = stable_id(seed, &format!("lot-{index}"));
        sqlx::query(
            "INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
                 leader_user_id, effective_end_at, category_key, category_number)
             VALUES ($1, $2, $3, $4, 'live', 50000, 80000, $5, $6, $7, $8)",
        )
        .bind(lot_id)
        .bind(auction_id)
        .bind(equipment_id)
        .bind(i32::try_from(index + 1).expect("lot 序号"))
        .bind(bidder)
        .bind(deadline)
        .bind(*category)
        .bind(*category_number)
        .execute(pool)
        .await
        .expect("写入测试 lot");
        let command_id = stable_id(seed, &format!("command-{index}"));
        let mut request_hash = [0_u8; 32];
        request_hash[..16].copy_from_slice(command_id.as_bytes());
        sqlx::query(
            "INSERT INTO public.auction_commands
                (command_id, command_kind, aggregate_id, request_hash, result_code)
             VALUES ($1, 'place_bid', $2, $3, 'accepted')",
        )
        .bind(command_id)
        .bind(lot_id)
        .bind(request_hash.as_slice())
        .execute(pool)
        .await
        .expect("写入测试竞价命令");
        sqlx::query(
            "INSERT INTO public.bids
                (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
                 effective_end_at, accepted_state_version)
             VALUES ($1, $2, 1, $3, $4, 80000, 'ordinary', $5, 2)",
        )
        .bind(stable_id(seed, &format!("bid-{index}")))
        .bind(command_id)
        .bind(lot_id)
        .bind(bidder)
        .bind(deadline)
        .execute(pool)
        .await
        .expect("写入测试竞价事实");
        lot_ids.push(lot_id);
    }
    (auction_id, lot_ids)
}

fn bid_log_request(auction_id: Uuid, lot_code: &str) -> PublicBidLogListRequest {
    PublicBidLogListRequest {
        auction_id,
        world_id: None,
        lot_id: None,
        lot_code: Some(lot_code.to_owned()),
        limit: 20,
        offset: 0,
        cursor: None,
    }
}

#[tokio::test]
async fn postgres_bid_log_lot_code_search_matches_rust_lot_codes() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let seed_store = database.maintenance_store().await.expect("维护账号连接");
    // 类别内序号覆盖一位、两位与三位，检验两位补零；未知分类回退为 `Lot` 前缀。
    let lots = [
        ("One-Handed", 1),
        ("Two-Handed", 1),
        ("Staff", 1),
        ("Shield", 1),
        ("Cloth", 1),
        ("Cloth", 12),
        ("Light", 1),
        ("Heavy", 120),
        ("Unknown", 1),
    ];
    let (auction_id, lot_ids) =
        seed_bid_log(seed_store.postgres_pool(), "bid-log-codes", &lots).await;
    let store = database.store().await.expect("运行账号连接");

    for ((category, category_number), lot_id) in lots.iter().zip(&lot_ids) {
        let code = lot_code(category, *category_number);
        // SQL 搜索不区分大小写；结果还会在 Rust 侧用同一 lot 代码复核，任一侧漂移都会失败。
        for needle in [code.clone(), code.to_ascii_lowercase(), code.to_ascii_uppercase()] {
            let page = store
                .list_public_bid_log(bid_log_request(auction_id, &needle))
                .await
                .expect("读取")
                .expect("可见");
            let found: Vec<Uuid> = page.entries.iter().map(|entry| entry.lot_id).collect();
            assert_eq!(found, [*lot_id], "{needle}");
            assert_eq!(page.total, Some(1), "{needle}");
        }
    }
    assert_eq!(lot_code("Cloth", 12), "Clo12");
    assert_eq!(lot_code("Heavy", 120), "Hea120");
    assert_eq!(lot_code("Unknown", 1), "Lot01");
    // 前缀搜索覆盖同一分类的全部 lot。
    let page = store
        .list_public_bid_log(bid_log_request(auction_id, "clo"))
        .await
        .expect("读取")
        .expect("可见");
    assert_eq!(page.total, Some(2));
}
