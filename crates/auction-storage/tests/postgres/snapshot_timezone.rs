//! 行快照证明的时区无关性。
//!
//! 历史 legacy 自寄与装备损失快照由 `to_jsonb(行)` 在 `TimeZone=UTC` 会话下写入。旧整行
//! 比较把时间戳渲染成会话时区的文本，因此只有 UTC 会话下相等；本测试固定同一批行，
//! 切换会话 `TimeZone` 反复求值新的比较式，并同时证明旧式在非 UTC 会话下翻转。
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_storage::{
    delivery_snapshot_equality_sql, trusted_delivery_predicate_sql, trusted_obligation_equality_sql,
};
use sqlx::{AssertSqlSafe, PgConnection, Row};
use uuid::Uuid;

#[allow(dead_code)]
use crate::support;

/// UTC、跨日期变更线正偏移、负偏移、非整点偏移与使用夏令时的地区。
const ZONES: [&str; 5] =
    ["UTC", "Pacific/Kiritimati", "America/Los_Angeles", "Asia/Kathmandu", "Europe/Berlin"];

/// 时区名只来自本文件的固定常量，不接受外部输入。
async fn set_time_zone(connection: &mut PgConnection, zone: &str) {
    sqlx::query(AssertSqlSafe(format!("SET TimeZone = '{zone}'")))
        .execute(&mut *connection)
        .await
        .expect("切换会话时区");
}

async fn evaluate(connection: &mut PgConnection, sql: &str) -> bool {
    sqlx::query(AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut *connection)
        .await
        .expect("求值快照证明")
        .get::<bool, _>(0)
}

struct Fixture {
    delivery_id: Uuid,
    obligation_id: Uuid,
    command_id: Uuid,
}

/// 写入一条会要求 legacy 自寄重开的失败投递，并在 UTC 会话下按历史写入方式生成快照。
async fn seed(connection: &mut PgConnection) -> Fixture {
    let fixture = Fixture {
        delivery_id: Uuid::new_v4(),
        obligation_id: Uuid::new_v4(),
        command_id: Uuid::new_v4(),
    };
    let digest = [7_u8; 32];
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, completed_at, failure_code, result_metadata)
         VALUES ($1, $2, $3, 'settlement', $4, 'Player One', '{\"operation\":\"send\"}'::jsonb,
                 'failed', 5, transaction_timestamp(), 'recipient_is_sender',
                 jsonb_build_object(
                     'compose_discard', jsonb_build_object('outcome', 'discarded'),
                     'legacy_self_reconciliation', jsonb_build_object('command_id', $5::text)))",
    )
    .bind(fixture.delivery_id)
    .bind(fixture.command_id)
    .bind(digest.as_slice())
    .bind(fixture.obligation_id.to_string())
    .bind(fixture.command_id)
    .execute(&mut *connection)
    .await
    .expect("写入失败投递");

    // 义务用临时表承载：证明比较式只依赖行本身，不需要完整结算夹具。
    sqlx::query(
        "CREATE TEMP TABLE snapshot_o (
             id uuid PRIMARY KEY,
             policy_version integer NOT NULL,
             obligation_kind text NOT NULL,
             amount bigint NOT NULL,
             created_at timestamptz NOT NULL)",
    )
    .execute(&mut *connection)
    .await
    .expect("创建义务临时表");
    sqlx::query(
        "INSERT INTO snapshot_o (id, policy_version, obligation_kind, amount, created_at)
         VALUES ($1, 1, 'winner_equipment_cod', 300000, '2026-11-02 12:34:56.123456+00')",
    )
    .bind(fixture.obligation_id)
    .execute(&mut *connection)
    .await
    .expect("写入义务快照行");

    sqlx::query(
        "CREATE TEMP TABLE snapshot_r (
             delivery_id uuid PRIMARY KEY,
             obligation_id uuid NOT NULL,
             action text NOT NULL,
             command_id uuid NOT NULL,
             uid text NOT NULL,
             expected_delivery_fencing bigint NOT NULL,
             original_snapshot jsonb NOT NULL,
             evidence jsonb NOT NULL)",
    )
    .execute(&mut *connection)
    .await
    .expect("创建自寄修复临时表");
    // 快照按历史写入语义生成：`to_jsonb(行)` 在 UTC 会话下渲染时间列。
    sqlx::query(
        "INSERT INTO snapshot_r
            (delivery_id, obligation_id, action, command_id, uid, expected_delivery_fencing,
             original_snapshot, evidence)
         SELECT d.id, $2::uuid, 'reopen_payment', $3::uuid, '12345', d.fencing_version - 1,
                jsonb_build_object(
                    'delivery', to_jsonb(d) || jsonb_build_object(
                        'status', 'manual_reconcile',
                        'result_metadata', d.result_metadata - 'legacy_self_reconciliation'),
                    'obligation', (SELECT to_jsonb(o) FROM snapshot_o o),
                    'fulfillment', jsonb_build_object(
                        'obligation_id', $2::text,
                        'delivery_id', d.id::text,
                        'execution_state', 'manual_reconcile')),
                jsonb_build_object(
                    'identity', jsonb_build_object('uid', '12345', 'player_name', d.recipient))
         FROM public.deliveries d WHERE d.id = $1",
    )
    .bind(fixture.delivery_id)
    .bind(fixture.obligation_id)
    .bind(fixture.command_id)
    .execute(&mut *connection)
    .await
    .expect("写入自寄修复快照");

    sqlx::query(
        "CREATE TEMP TABLE snapshot_x AS
         SELECT jsonb_build_object(
             'delivery', to_jsonb(d) || jsonb_build_object(
                 'status', 'manual_reconcile',
                 'failure_code', 'equipment_destroyed',
                 'result_metadata', d.result_metadata - 'legacy_self_reconciliation'))
             AS original_snapshot
         FROM public.deliveries d WHERE d.id = $1",
    )
    .bind(fixture.delivery_id)
    .execute(&mut *connection)
    .await
    .expect("写入装备损失快照");
    fixture
}

fn legacy_predicate(repair_table: &str) -> String {
    format!(
        "SELECT ({predicate}) IS TRUE AS trusted
         FROM public.deliveries d JOIN {repair_table} r ON r.delivery_id = d.id",
        predicate = trusted_delivery_predicate_sql()
    )
}

fn obligation_equality() -> String {
    format!(
        "SELECT ({equality}) IS TRUE AS trusted FROM snapshot_o o CROSS JOIN snapshot_r r",
        equality = trusted_obligation_equality_sql()
    )
}

fn equipment_loss_equality(delivery_id: Uuid) -> String {
    format!(
        "SELECT (TRUE {equality}) IS TRUE AS trusted
         FROM public.deliveries d CROSS JOIN snapshot_x x WHERE d.id = '{delivery_id}'::uuid",
        equality = delivery_snapshot_equality_sql()
    )
}

/// 旧式整行比较，仅用于证明该写法确实随会话时区翻转；正式代码已不再使用。
fn previous_row_comparison() -> String {
    "SELECT ((to_jsonb(d) - 'status' - 'fencing_version' - 'updated_at' - 'result_metadata')
             = ((r.original_snapshot->'delivery')
                - 'status' - 'fencing_version' - 'updated_at' - 'result_metadata'))
            IS TRUE AS trusted
     FROM public.deliveries d JOIN snapshot_r r ON r.delivery_id = d.id"
        .to_owned()
}

#[tokio::test]
async fn postgres_row_snapshot_proofs_ignore_session_time_zone() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    // 固定使用同一连接：临时快照需要跨语句存续，会话时区也必须显式切换。
    let mut connection = pool.acquire().await.expect("获取测试连接");

    set_time_zone(&mut connection, "UTC").await;
    let fixture = seed(&mut connection).await;

    let legacy = legacy_predicate("snapshot_r");
    let obligation = obligation_equality();
    let equipment_loss = equipment_loss_equality(fixture.delivery_id);

    for zone in ZONES {
        set_time_zone(&mut connection, zone).await;
        assert!(
            evaluate(&mut connection, &legacy).await,
            "legacy 自寄证明在 {zone} 会话下必须成立"
        );
        assert!(
            evaluate(&mut connection, &obligation).await,
            "义务整行比较在 {zone} 会话下必须成立"
        );
        assert!(
            evaluate(&mut connection, &equipment_loss).await,
            "装备损失整行比较在 {zone} 会话下必须成立"
        );
    }

    // 对照：旧式整行比较只在 UTC 会话下相等，正是本工作项要消除的耦合。
    set_time_zone(&mut connection, "UTC").await;
    assert!(evaluate(&mut connection, &previous_row_comparison()).await, "旧式在 UTC 会话下相等");
    set_time_zone(&mut connection, "Pacific/Kiritimati").await;
    assert!(
        !evaluate(&mut connection, &previous_row_comparison()).await,
        "旧式整行比较把时间戳渲染成会话时区文本，非 UTC 会话下必须不相等"
    );

    // 快照里的时间文本改成带 +09:00 偏移的同一瞬间，结论不变。
    sqlx::query(
        "CREATE TEMP TABLE snapshot_r_offset AS
         SELECT delivery_id, obligation_id, action, command_id, uid, expected_delivery_fencing,
                evidence,
                jsonb_set(
                    original_snapshot,
                    '{delivery,created_at}',
                    to_jsonb(to_char(
                        (original_snapshot#>>'{delivery,created_at}')::timestamptz
                            AT TIME ZONE 'Asia/Tokyo',
                        'YYYY-MM-DD\"T\"HH24:MI:SS.US') || '+09:00')) AS original_snapshot
         FROM snapshot_r",
    )
    .execute(&mut *connection)
    .await
    .expect("生成带偏移时间文本的快照");
    let offset_legacy = legacy_predicate("snapshot_r_offset");
    for zone in ["America/Los_Angeles", "Asia/Kathmandu", "Europe/Berlin"] {
        set_time_zone(&mut connection, zone).await;
        assert!(
            evaluate(&mut connection, &offset_legacy).await,
            "同一瞬间的非 UTC 偏移时间文本在 {zone} 会话下也必须成立"
        );
    }

    // 不等：实际行的时间列与快照差 1 秒时，任何会话时区下都必须不成立。
    sqlx::query(
        "UPDATE public.deliveries SET created_at = created_at - interval '1 second' WHERE id = $1",
    )
    .bind(fixture.delivery_id)
    .execute(&mut *connection)
    .await
    .expect("改动实际行时间列");
    for zone in ["UTC", "Pacific/Kiritimati", "Asia/Kathmandu"] {
        set_time_zone(&mut connection, zone).await;
        assert!(
            !evaluate(&mut connection, &legacy).await,
            "实际行时间列偏差在 {zone} 会话下必须判为不匹配"
        );
        assert!(
            !evaluate(&mut connection, &equipment_loss).await,
            "装备损失比较在实际行时间列偏差后 {zone} 会话下必须判为不匹配"
        );
    }

    // 恢复实际行，改为改动快照时间文本：同样在任何会话时区下判为不匹配。
    sqlx::query(
        "UPDATE public.deliveries SET created_at = created_at + interval '1 second' WHERE id = $1",
    )
    .bind(fixture.delivery_id)
    .execute(&mut *connection)
    .await
    .expect("恢复实际行时间列");
    sqlx::query(
        "UPDATE snapshot_r
         SET original_snapshot = jsonb_set(
             original_snapshot,
             '{delivery,created_at}',
             to_jsonb(((original_snapshot#>>'{delivery,created_at}')::timestamptz
                       + interval '1 second') AT TIME ZONE 'UTC'))",
    )
    .execute(&mut *connection)
    .await
    .expect("改动快照时间文本");
    for zone in ["UTC", "Pacific/Kiritimati", "America/Los_Angeles"] {
        set_time_zone(&mut connection, zone).await;
        assert!(
            !evaluate(&mut connection, &legacy).await,
            "快照时间文本偏差在 {zone} 会话下必须判为不匹配"
        );
    }

    drop(connection);
    store.postgres_pool().close().await;
}

/// 归一化清单必须覆盖两张表中参与整行比较的每个 `timestamptz` 列；新增时间列而漏改
/// 比较式会在此失败。已被排除列（各自单独断言或允许变动）不参与整行比较，因此不要求归一化。
#[tokio::test]
async fn postgres_snapshot_normalization_covers_every_timestamptz_column() {
    // 与两个比较式的排除列保持一致；`updated_at` 在两者中都被剔除，`completed_at`
    // 只在装备损失比较式中被剔除。
    const LEGACY_EXCLUDED: [&str; 4] =
        ["status", "fencing_version", "updated_at", "result_metadata"];
    const LOSS_EXCLUDED: [&str; 6] = [
        "status",
        "failure_code",
        "fencing_version",
        "completed_at",
        "updated_at",
        "result_metadata",
    ];
    const OBLIGATION_EXCLUDED: [&str; 3] = ["state_version", "updated_at", "business_state"];

    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let rows = sqlx::query(
        "SELECT table_name, column_name FROM information_schema.columns
         WHERE table_schema = 'public'
           AND table_name IN ('deliveries', 'settlement_obligations')
           AND data_type = 'timestamp with time zone'
         ORDER BY table_name, column_name",
    )
    .fetch_all(store.postgres_pool())
    .await
    .expect("读取时间列清单");

    let delivery_sql = trusted_delivery_predicate_sql();
    let loss_sql = delivery_snapshot_equality_sql();
    let obligation_sql = trusted_obligation_equality_sql();
    let mut deliveries = 0;
    let mut obligations = 0;
    for row in rows {
        let table: String = row.get("table_name");
        let column: String = row.get("column_name");
        match table.as_str() {
            "deliveries" => {
                deliveries += 1;
                let normalized = format!("d.{column} AT TIME ZONE 'UTC'");
                if !LEGACY_EXCLUDED.contains(&column.as_str()) {
                    assert!(
                        delivery_sql.contains(&normalized),
                        "legacy 自寄比较式缺少 deliveries.{column} 的 UTC 归一化"
                    );
                    assert!(
                        delivery_sql.contains(&format!("'{column}'")),
                        "legacy 自寄比较式的存在性守卫缺少 deliveries.{column}"
                    );
                }
                if !LOSS_EXCLUDED.contains(&column.as_str()) {
                    assert!(
                        loss_sql.contains(&normalized),
                        "装备损失比较式缺少 deliveries.{column} 的 UTC 归一化"
                    );
                    assert!(
                        loss_sql.contains(&format!("'{column}'")),
                        "装备损失比较式的存在性守卫缺少 deliveries.{column}"
                    );
                }
            }
            "settlement_obligations" => {
                obligations += 1;
                if !OBLIGATION_EXCLUDED.contains(&column.as_str()) {
                    assert!(
                        obligation_sql.contains(&format!("o.{column} AT TIME ZONE 'UTC'")),
                        "义务比较式缺少 settlement_obligations.{column} 的 UTC 归一化"
                    );
                }
            }
            other => panic!("未预期的表：{other}"),
        }
    }
    assert_eq!(deliveries, 7, "deliveries 的时间列数量与比较式清单不一致");
    assert_eq!(obligations, 2, "settlement_obligations 的时间列数量与比较式清单不一致");

    store.postgres_pool().close().await;
}
