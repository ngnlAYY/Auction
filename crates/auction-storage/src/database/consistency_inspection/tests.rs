//! 显式 PostgreSQL 18 测试；只写连接私有临时表，永不修改 public 业务表。

use sqlx::{Connection, Executor, Row};

use super::queries::CHECKS;

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
    equipment_id uuid, obligation_kind text, business_state text
);
CREATE TEMP TABLE settlement_obligation_fulfillments (
    obligation_id uuid, execution_state text, delivery_id uuid, proof_digest bytea
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
INSERT INTO realtime_streams VALUES ('auction:test', 3);
-- W12.1：已分配事件 + 一条刚提交、尚未分配序号的竞价事件（正常状态，不得误报）。
INSERT INTO realtime_events VALUES
    ('auction:test', 1, now() - interval '2 minutes'),
    ('auction:test', 2, now() - interval '1 minute'),
    ('auction:test', NULL, now());
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
DELETE FROM pg_temp.realtime_events WHERE stream_sequence = 1;
-- 未分配序号滞留超过阈值说明分配器/relay 停摆，事件无法进入回放。
INSERT INTO pg_temp.realtime_events VALUES (
    'auction:test', NULL, now() - interval '6 minutes'
);
"#;
