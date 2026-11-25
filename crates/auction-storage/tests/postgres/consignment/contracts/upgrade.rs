//! 从 134 升级到 135：既有拍卖义务、履约与投递行逐字节不变，新列为空，旧行满足来源 CHECK。

use auction_storage::postgres_migrate_to;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;

use super::super::deposit::support::{fails, succeeds};
use crate::support::{TestDatabase, fixtures};

const AUCTION: &str = "c6000000-0000-4000-8000-000000000001";
pub(crate) const LOT: &str = "c6000000-0000-4000-8000-000000000002";
const EQUIPMENT: &str = "c6000000-0000-4000-8000-000000000003";
const COMMAND: &str = "c6000000-0000-4000-8000-000000000004";
const OBLIGATION: &str = "c6000000-0000-4000-8000-000000000005";
const DELIVERY: &str = "c6000000-0000-4000-8000-000000000006";
const DELIVERY_COMMAND: &str = "c6000000-0000-4000-8000-000000000007";

pub(crate) async fn seed_legacy_settlement(pool: &PgPool) {
    let deadline = Timestamp::now() + Duration::hours(6);
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1::uuid, 'main', 1, 'Legacy settlement', 'live', $2, $2, $2, $2, $3)",
    )
    .bind(AUCTION)
    .bind(deadline)
    .bind(fixtures::policy_snapshot(deadline, 50_000, 30_000))
    .execute(pool)
    .await
    .expect("写入旧场次");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1::uuid, 'settlement-test', $2::uuid, decode(repeat('01', 32), 'hex'), 'accepted')",
    )
    .bind(COMMAND)
    .bind(AUCTION)
    .execute(pool)
    .await
    .expect("写入旧命令");
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ('{EQUIPMENT}', 'main', '900', 'Legacy gear', 'Weapon', '{{}}'::jsonb,
                     '2026-01-01T00:00:00Z');
             INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price,
                 category_key, category_number)
             VALUES ('{LOT}', '{AUCTION}', '{EQUIPMENT}', 1, 'unsold', 100, 'Weapon', 1);
             INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('{DELIVERY}', '{DELIVERY_COMMAND}', decode(repeat('18', 32), 'hex'),
                     'settlement', '{OBLIGATION}', 'FakeSeller', '{{}}'::jsonb, 'prepared', 0, 0,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, recipient_game_identity, amount,
                 carrier_credit, equipment_id)
             VALUES ('{OBLIGATION}', '{AUCTION}', '{LOT}', 'main', '{COMMAND}', 1,
                     'unsold_return', 'ready', 'FakeSeller', 0, 0, '{EQUIPMENT}');
             INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, dispatch_world, delivery_operation, delivery_id)
             VALUES ('{OBLIGATION}', 'pending', 'main', 'send', '{DELIVERY}');"
        ),
    )
    .await;
}

/// 旧表既有列的整行文本；新增的三个来源列不在其中，用于逐字节比较。
pub(crate) async fn legacy_rows(pool: &PgPool) -> (String, String, String) {
    sqlx::query_as(
        "SELECT (SELECT to_jsonb(row) - 'contract_id' - 'contract_kind' - 'custody_equipment_id'
                 FROM public.settlement_obligations AS row)::text,
                (SELECT to_jsonb(row) FROM public.settlement_obligation_fulfillments AS row)::text,
                (SELECT to_jsonb(row) FROM public.deliveries AS row)::text",
    )
    .fetch_one(pool)
    .await
    .expect("读取旧行")
}

#[tokio::test]
async fn postgres_consignment_contract_upgrade_from_134_keeps_legacy_settlement_rows() {
    let database = TestDatabase::create_empty().await.expect("创建空库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    postgres_migrate_to(pool, 134).await.expect("迁移到 134");
    seed_legacy_settlement(pool).await;
    let before = legacy_rows(pool).await;

    postgres_migrate_to(pool, 135).await.expect("迁移到 135");

    assert_eq!(legacy_rows(pool).await, before, "既有义务、履约与投递行必须逐字节不变");
    let origin: (bool, bool) = sqlx::query_as(
        "SELECT contract_id IS NULL AND contract_kind IS NULL AND custody_equipment_id IS NULL,
                auction_id IS NOT NULL AND lot_id IS NOT NULL AND origin_command_id IS NOT NULL
                    AND equipment_id IS NOT NULL
         FROM public.settlement_obligations",
    )
    .fetch_one(pool)
    .await
    .expect("读取升级后的来源列");
    assert_eq!(origin, (true, true), "旧行是纯拍卖来源");
}

/// 来源兼容（D15）：拍卖来源的义务带不存在的 `submission_id` 时被外键拒绝；同一行不带投稿即可提交
/// （不变量 6：来源不明一律失败关闭）。
#[tokio::test]
async fn postgres_consignment_auction_obligation_rejects_an_unknown_submission() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_legacy_settlement(pool).await;
    let obligation = |submission: &str| {
        format!(
            "INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, recipient_game_identity, amount,
                 carrier_credit, equipment_id, submission_id)
             VALUES ('c6000000-0000-4000-8000-0000000000a1', '{AUCTION}', '{LOT}', 'main',
                     '{COMMAND}', 1, 'withdrawn_return', 'ready', 'FakeSeller', 0, 0,
                     '{EQUIPMENT}', {submission});"
        )
    };

    // 反例：投稿 ID 在库里不存在。
    fails(
        pool,
        &obligation("'c6000000-0000-4000-8000-0000000000a2'"),
        "23503",
        Some("settlement_obligations_submission_id_fkey"),
    )
    .await;
    // 正例：同一行不带投稿。
    succeeds(pool, &obligation("NULL")).await;
}
