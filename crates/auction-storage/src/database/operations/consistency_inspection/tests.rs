//! 显式 PostgreSQL 18 测试；只写连接私有临时表，永不修改 public 业务表。

use std::collections::BTreeSet;

use sqlx::{Connection, Executor, Row};

use super::queries::CHECKS;
use crate::database::retention_policy_rule_duration_days;

/// realtime 序号检查的完整窗口（天），与 SQL 字面量 `realtime_complete_window!()` 对应。
const REALTIME_COMPLETE_WINDOW_DAYS: i64 = 29;

/// 巡检完整窗口 = 保留策略 realtime_event_age（30 天）− 1 天余量；SQL 字面量与常量一致。
/// 策略期限或窗口任一侧变化时本断言提醒同步两处。
#[test]
fn realtime_complete_window_tracks_retention_policy() {
    let retention = retention_policy_rule_duration_days("realtime_event_age")
        .expect("策略包含 realtime_event_age");
    assert_eq!(retention, 30);
    assert_eq!(REALTIME_COMPLETE_WINDOW_DAYS, retention - 1);
    assert_eq!(REALTIME_COMPLETE_WINDOW_DAYS, 29);
    let realtime = realtime_check();
    assert!(
        realtime.contains(&format!("interval '{REALTIME_COMPLETE_WINDOW_DAYS} days'")),
        "SQL 窗口字面量必须与常量一致"
    );
}

fn realtime_check() -> &'static str {
    CHECKS
        .iter()
        .find(|(invariant, _)| *invariant == "realtime_sequence")
        .map(|(_, sql)| *sql)
        .expect("realtime_sequence 检查")
}

/// 保留清理后的序号口径：窗口外缺号与尾部超前不报告；窗口内缺号、尾部落后或窗口内尾部
/// 不等于最大序号 + 1 都报告。窗口边界附近序号更大的旧事件计入后缀，不误报缺号。
#[tokio::test]
#[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
async fn postgres_realtime_continuity_allows_purged_prefix_only()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
    let mut connection = sqlx::PgConnection::connect(&url).await?;
    let mut transaction = connection.begin().await?;
    transaction.execute(REALTIME_PURGE_FIXTURE).await?;
    let rows = sqlx::query(sqlx::AssertSqlSafe(realtime_check().replace("public.", "pg_temp.")))
        .bind(100_i64)
        .fetch_all(&mut *transaction)
        .await?;
    let found = rows
        .iter()
        .map(|row| (row.get::<String, _>("entity_key"), row.get::<String, _>("reason")))
        .collect::<BTreeSet<_>>();
    let gap = "realtime_sequence_gap_or_tail_mismatch".to_owned();
    let expected = ["auction:tail-ahead", "auction:tail-behind", "auction:window-gap"]
        .into_iter()
        .map(|stream| (stream.to_owned(), gap.clone()))
        .collect::<BTreeSet<_>>();
    assert_eq!(found, expected);
    transaction.rollback().await?;
    Ok(())
}

const REALTIME_PURGE_FIXTURE: &str = r#"
CREATE TEMP TABLE realtime_streams (stream_key text, next_sequence bigint);
CREATE TEMP TABLE realtime_events (stream_key text, stream_sequence bigint, created_at timestamptz);
CREATE TEMP TABLE realtime_consumer_offsets (consumer_key text, stream_key text, last_sequence bigint);
INSERT INTO realtime_streams VALUES
    ('auction:purged', 9), ('auction:emptied', 4), ('auction:old-tail', 10),
    ('auction:boundary', 6), ('auction:window-gap', 9), ('auction:tail-behind', 3),
    ('auction:tail-ahead', 10);
INSERT INTO realtime_events VALUES
    -- 清理后保留被失败引用的 5 与窗口内 6..8：正常。
    ('auction:purged', 5, now() - interval '40 days'),
    ('auction:purged', 6, now() - interval '2 days'),
    ('auction:purged', 7, now() - interval '1 day'),
    ('auction:purged', 8, now() - interval '1 hour'),
    -- 窗口外只剩 5，尾部超前是更新的旧事件被清理：正常。
    ('auction:old-tail', 5, now() - interval '40 days'),
    -- 跨窗口边界的长事务：序号 4 的创建时间早于序号 3，仍计入窗口后缀：正常。
    ('auction:boundary', 3, now() - interval '28 days 23 hours'),
    ('auction:boundary', 4, now() - interval '29 days 1 hour'),
    ('auction:boundary', 5, now()),
    -- 窗口内缺 7：报告。
    ('auction:window-gap', 5, now() - interval '40 days'),
    ('auction:window-gap', 6, now() - interval '2 days'),
    ('auction:window-gap', 8, now() - interval '1 hour'),
    -- 尾部落后于已有最大序号（即使事件全在窗口外）：报告。
    ('auction:tail-behind', 4, now() - interval '40 days'),
    -- 窗口内尾部不等于最大序号 + 1：报告。
    ('auction:tail-ahead', 6, now() - interval '2 days'),
    ('auction:tail-ahead', 7, now() - interval '1 day'),
    ('auction:tail-ahead', 8, now() - interval '1 hour');
"#;

#[tokio::test]
#[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
async fn postgres_consistency_invariants() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
    let mut connection = sqlx::PgConnection::connect(&url).await?;
    let version: i32 = sqlx::query_scalar("SELECT current_setting('server_version_num')::integer")
        .fetch_one(&mut connection)
        .await?;
    assert_eq!(version / 10_000, 18);
    let mut transaction = connection.begin().await?;
    transaction.execute(FIXTURE).await?;
    for &(invariant, sql) in CHECKS {
        // 仅把静态 SQL 的固定 schema 改为连接私有临时表，不插入外部文本。
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql.replace("public.", "pg_temp.")))
            .bind(100_i64)
            .fetch_all(&mut *transaction)
            .await?;
        assert!(rows.is_empty(), "正常样本误报 {invariant}: {rows:?}");
    }
    transaction
        .execute(
            "UPDATE pg_temp.settlement_obligations
         SET prerequisite_obligation_id = '00000000-0000-0000-0000-000000000003'
         WHERE obligation_kind = 'winner_equipment_cod'",
        )
        .await?;
    let cycle_rows = sqlx::query(sqlx::AssertSqlSafe(CHECKS[2].1.replace("public.", "pg_temp.")))
        .bind(100_i64)
        .fetch_all(&mut *transaction)
        .await?;
    assert!(cycle_rows.iter().any(|row| row.get::<String, _>("reason") == "obligation_cycle"));
    transaction
        .execute(
            "UPDATE pg_temp.settlement_obligations SET prerequisite_obligation_id = NULL
         WHERE obligation_kind = 'winner_equipment_cod'",
        )
        .await?;
    // 已发送、等待收取/收口的 enqueued 是正常状态；不能误判成已满足或投递不一致。
    transaction
        .execute(
            "INSERT INTO pg_temp.deliveries VALUES (
             '00000000-0000-0000-0000-000000000040', 'succeeded', 'settlement',
             '00000000-0000-0000-0000-000000000003');
         UPDATE pg_temp.settlement_obligation_fulfillments SET execution_state = 'enqueued',
             delivery_id = '00000000-0000-0000-0000-000000000040'
         WHERE obligation_id = '00000000-0000-0000-0000-000000000003'",
        )
        .await?;
    let waiting_rows = sqlx::query(sqlx::AssertSqlSafe(CHECKS[3].1.replace("public.", "pg_temp.")))
        .bind(100_i64)
        .fetch_all(&mut *transaction)
        .await?;
    assert!(waiting_rows.is_empty());
    // 内部持有证明可在原投递失败后满足义务；不得仅因 delivery 非 succeeded 误报。
    transaction
        .execute(
            "INSERT INTO pg_temp.deliveries VALUES (
             '00000000-0000-0000-0000-000000000041', 'failed', 'settlement',
             '00000000-0000-0000-0000-000000000003');
         UPDATE pg_temp.settlement_obligation_fulfillments
         SET execution_state = 'satisfied', delivery_id = '00000000-0000-0000-0000-000000000041',
             proof_digest = '\x01'::bytea, proof_kind = 'bot_internal_v1'
         WHERE obligation_id = '00000000-0000-0000-0000-000000000003'",
        )
        .await?;
    let internal_proof_rows =
        sqlx::query(sqlx::AssertSqlSafe(CHECKS[3].1.replace("public.", "pg_temp.")))
            .bind(100_i64)
            .fetch_all(&mut *transaction)
            .await?;
    assert!(internal_proof_rows.is_empty(), "内部证明被误报: {internal_proof_rows:?}");
    transaction.execute("DELETE FROM pg_temp.deliveries").await?;
    transaction.execute(
        "DELETE FROM pg_temp.deliveries;
         UPDATE pg_temp.settlement_obligation_fulfillments SET execution_state = 'pending', delivery_id = NULL
         WHERE obligation_id = '00000000-0000-0000-0000-000000000003'",
    ).await?;
    transaction.execute(CORRUPTION).await?;
    for &(invariant, sql) in CHECKS {
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql.replace("public.", "pg_temp.")))
            .bind(100_i64)
            .fetch_all(&mut *transaction)
            .await?;
        assert!(!rows.is_empty(), "未发现损坏样本 {invariant}");
        let total: i64 = rows[0].try_get("violation_count")?;
        assert!(total > 0, "差异总数无效 {invariant}");
    }
    // 样本达到上限仍须返回完整计数，不能把截断当作全量通过。
    transaction
        .execute(
            "INSERT INTO pg_temp.lots
         SELECT md5(n::text)::uuid, 10, NULL, false, 1, 'live', now() + interval '1 day', NULL, NULL
         FROM generate_series(1, 150) n",
        )
        .await?;
    let rows = sqlx::query(sqlx::AssertSqlSafe(CHECKS[0].1.replace("public.", "pg_temp.")))
        .bind(100_i64)
        .fetch_all(&mut *transaction)
        .await?;
    assert_eq!(rows.len(), 100);
    assert_eq!(rows[0].try_get::<i64, _>("violation_count")?, 151);
    transaction.rollback().await?;
    Ok(())
}

const FIXTURE: &str = r#"
CREATE TEMP TABLE lots (
    id uuid, current_price bigint, leader_user_id uuid, leader_proxy_last boolean,
    state_version bigint, status text, effective_end_at timestamptz,
    slow_authorization_id uuid, slow_due_at timestamptz
);
CREATE TEMP TABLE bids (
    lot_id uuid, bidder_user_id uuid, amount bigint, mechanism text, accepted_state_version bigint
);
CREATE TEMP TABLE audit_records (occurred_at timestamptz, metadata jsonb, action text, entity_id text);
CREATE TEMP TABLE compensation_pool (balance bigint, state_version bigint);
CREATE TEMP TABLE compensation_ledger (
    id uuid, auction_id uuid, entry_sequence integer, movement_kind text, amount bigint, balance_after bigint
);
CREATE TEMP TABLE auction_settlements (
    auction_id uuid, opening_pool_balance bigint, closing_pool_balance bigint,
    pool_enabled boolean, tax_total bigint, compensation_total bigint,
    opening_pool_version bigint, closing_pool_version bigint, settled_at timestamptz
);
CREATE TEMP TABLE compensation_pool_reversals (
    id uuid, opening_pool_version bigint, closing_pool_version bigint,
    opening_balance bigint, closing_balance bigint,
    compensation_amount bigint, tax_reversed bigint, created_at timestamptz
);
CREATE TEMP TABLE settlement_obligations (
    id uuid, prerequisite_obligation_id uuid, auction_id uuid, lot_id uuid,
    equipment_id uuid, obligation_kind text, business_state text, contract_id uuid
);
CREATE TEMP TABLE settlement_obligation_fulfillments (
    obligation_id uuid, execution_state text, delivery_id uuid, proof_digest bytea, proof_kind text
);
CREATE TEMP TABLE settlement_recoveries (
    id uuid, auction_id uuid, lot_id uuid, equipment_id uuid, world_id text,
    payment_obligation_id uuid, seller_payout_obligation_id uuid,
    buyer_refund_obligation_id uuid, equipment_obligation_id uuid
);
CREATE TEMP TABLE deliveries (id uuid, status text, origin_kind text, origin_id text);
CREATE TEMP TABLE realtime_streams (stream_key text, next_sequence bigint);
CREATE TEMP TABLE realtime_events (stream_key text, stream_sequence bigint, created_at timestamptz);
CREATE TEMP TABLE realtime_consumer_offsets (consumer_key text, stream_key text, last_sequence bigint);
INSERT INTO lots VALUES (
    '00000000-0000-0000-0000-000000000001', 100,
    '00000000-0000-0000-0000-000000000010', true, 5, 'live', now() + interval '1 day',
    '00000000-0000-0000-0000-000000000011', now() + interval '1 hour'
);
INSERT INTO bids VALUES (
    '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000010', 100, 'proxy', 4
);
INSERT INTO audit_records VALUES (
    now() - interval '1 day', '{"balance": 100, "state_version": 1}', 'auction.compensation.merge', 'global'
);
INSERT INTO compensation_pool VALUES (115, 3);
INSERT INTO auction_settlements VALUES (
    '00000000-0000-0000-0000-000000000020', 100, 110, true, 20, 10, 1, 2, now() - interval '1 hour'
);
INSERT INTO compensation_ledger
SELECT md5(sequence::text)::uuid, '00000000-0000-0000-0000-000000000020', sequence, kind, amount, balance
FROM (VALUES (1, 'opening', 100, 100), (2, 'tax', 20, 120),
             (3, 'compensation', -10, 110), (4, 'closing', 0, 110)) v(sequence, kind, amount, balance);
INSERT INTO compensation_pool_reversals VALUES (
    '00000000-0000-0000-0000-000000000021', 2, 3, 110, 115, 10, 5, now()
);
INSERT INTO settlement_obligations VALUES
    ('00000000-0000-0000-0000-000000000002', NULL,
     '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-000000000001',
     '00000000-0000-0000-0000-000000000030', 'winner_equipment_cod', 'ready'),
    ('00000000-0000-0000-0000-000000000003', '00000000-0000-0000-0000-000000000002',
     '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-000000000001',
     '00000000-0000-0000-0000-000000000030', 'seller_payout', 'ready');
INSERT INTO settlement_obligation_fulfillments VALUES
    ('00000000-0000-0000-0000-000000000002', 'satisfied', NULL, '\x01'),
    ('00000000-0000-0000-0000-000000000003', 'pending', NULL, NULL);
INSERT INTO settlement_obligations VALUES (
    '00000000-0000-0000-0000-000000000004', '00000000-0000-0000-0000-000000000002',
    '00000000-0000-0000-0000-000000000020', '00000000-0000-0000-0000-000000000001',
    '00000000-0000-0000-0000-000000000030', 'buyer_refund', 'contingent'
);
INSERT INTO settlement_obligation_fulfillments VALUES (
    '00000000-0000-0000-0000-000000000004', 'pending', NULL, NULL
);
INSERT INTO settlement_recoveries VALUES (
    '00000000-0000-0000-0000-000000000050', '00000000-0000-0000-0000-000000000020',
    '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000030', 'main',
    '00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000003',
    '00000000-0000-0000-0000-000000000004', NULL
);
INSERT INTO realtime_streams VALUES ('auction:test', 4), ('auction:purged', 9);
-- 已分配事件 + 一条刚提交、尚未分配序号的竞价事件（正常状态，不得误报）；
-- 另一流的旧前缀已按保留策略清理，只剩被引用的旧事件与窗口内后缀（正常状态）。
INSERT INTO realtime_events VALUES
    ('auction:test', 1, now() - interval '3 minutes'),
    ('auction:test', 2, now() - interval '2 minutes'),
    ('auction:test', 3, now() - interval '1 minute'),
    ('auction:test', NULL, now()),
    ('auction:purged', 5, now() - interval '40 days'),
    ('auction:purged', 6, now() - interval '2 days'),
    ('auction:purged', 7, now() - interval '1 day'),
    ('auction:purged', 8, now() - interval '1 hour');
INSERT INTO realtime_consumer_offsets VALUES ('consumer', 'auction:test', 1);
"#;

const CORRUPTION: &str = r#"
UPDATE pg_temp.lots SET current_price = 101, effective_end_at = now() - interval '1 second';
UPDATE pg_temp.compensation_ledger SET amount = 21 WHERE movement_kind = 'tax';
UPDATE pg_temp.settlement_obligations SET equipment_id = '00000000-0000-0000-0000-000000000031'
WHERE obligation_kind = 'seller_payout';
INSERT INTO pg_temp.deliveries VALUES (
    '00000000-0000-0000-0000-000000000040', 'pending', 'settlement', '00000000-0000-0000-0000-000000000003'
);
UPDATE pg_temp.settlement_obligation_fulfillments SET execution_state = 'prepared',
    delivery_id = '00000000-0000-0000-0000-000000000040'
WHERE obligation_id = '00000000-0000-0000-0000-000000000003';
-- 保留窗口内缺号：窗口外允许清理，窗口内的空号仍是损坏。
DELETE FROM pg_temp.realtime_events WHERE stream_key = 'auction:test' AND stream_sequence = 2;
-- 未分配序号滞留超过阈值说明分配器/relay 停摆，事件无法进入回放。
INSERT INTO pg_temp.realtime_events VALUES (
    'auction:test', NULL, now() - interval '6 minutes'
);
"#;
