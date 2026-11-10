//! `deliveries.payload` 的 world/operation/template 热键列化（迁移 `000094`）。
//!
//! 三类断言：
//!   1. 生成列只从 `payload` 派生，不存在第二份可写入的事实：插入覆盖缺失键、JSON null 与
//!      普通字符串的 payload 后，列值与 `payload ->> …` 逐行相等；更新 `payload` 后列随之
//!      重算；直接写生成列被数据库拒绝；全表漂移计数为 0。
//!   2. 整行证据比较不受派生列影响：按升级前形状（不含三列）构造的冻结快照仍然匹配，
//!      而不剔除三列的整行比较必然失败，证明剔除语句确实生效。
//!   3. 维护扫描的部分索引在新的列形式上可被使用（`EXPLAIN` 断言），且两种写法给出同一集合。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[allow(dead_code)]
use crate::support;

use auction_application::recovery::{
    EXTERNAL_RECOVERY_AMBIGUOUS_REASON, ExternalRecoverySendEvidence, ExternalRecoveryStore,
    ExternalRecoveryUnresolvedBlock,
};
use auction_storage::delivery_snapshot_equality_sql;
use sqlx::{AssertSqlSafe, PgConnection, Row};
use uuid::Uuid;

/// 生成列清单；与本迁移提升的三个热键一一对应。
const DERIVED_COLUMNS: [&str; 3] = ["payload_world", "payload_operation", "payload_template"];

/// 插入一条最小可用的 pending 投递；`payload` 是唯一写入面。
async fn insert_delivery(connection: &mut PgConnection, payload: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, decode(repeat('11', 32), 'hex'), 'settlement', $3, $4, $5::jsonb, 'pending')",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(format!("w12-6:{id}"))
    .bind(format!("player-{id}"))
    .bind(payload)
    .execute(&mut *connection)
    .await
    .expect("写入投递");
    id
}

/// 读取一行的三列与对应的 `payload ->>` 结果，逐列断言相等。
async fn assert_columns_track_payload(connection: &mut PgConnection, id: Uuid) {
    let row = sqlx::query(
        "SELECT payload_world, payload ->> 'world' AS world_key,
                payload_operation, payload ->> 'operation' AS operation_key,
                payload_template, payload ->> 'template' AS template_key
         FROM public.deliveries WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *connection)
    .await
    .expect("读取投递列值");
    for (column, key) in
        [("world", "world_key"), ("operation", "operation_key"), ("template", "template_key")]
    {
        let derived: Option<String> =
            row.try_get(format!("payload_{column}").as_str()).expect("读取生成列");
        let expected: Option<String> = row.try_get(key).expect("读取 payload 键");
        assert_eq!(derived, expected, "payload_{column} 与 payload ->> '{column}' 必须逐行相等");
    }
}

#[tokio::test]
async fn postgres_delivery_payload_columns_derive_from_payload_only() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let mut connection = pool.acquire().await.expect("获取测试连接");

    // 三列必须是数据库生成的 STORED 列：没有任何写入面可以独立设置它们。
    let generated = sqlx::query(
        "SELECT column_name, is_generated, generation_expression
         FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = 'deliveries'
           AND column_name = ANY($1::text[])
         ORDER BY column_name",
    )
    .bind(DERIVED_COLUMNS.map(str::to_owned).to_vec())
    .fetch_all(&mut *connection)
    .await
    .expect("读取列定义");
    assert_eq!(generated.len(), DERIVED_COLUMNS.len(), "三个生成列必须都存在");
    for row in &generated {
        let column: String = row.get("column_name");
        assert_eq!(row.get::<String, _>("is_generated"), "ALWAYS", "{column} 必须是生成列");
        assert!(
            row.get::<String, _>("generation_expression").contains("payload"),
            "{column} 的生成表达式必须只读 payload"
        );
    }

    // 缺失键、JSON null 与普通字符串三种形状；`->>` 对前两者都返回 NULL。
    let complete = insert_delivery(
        &mut connection,
        r#"{"world":"main","operation":"send","template":"lot_watch","credits":"0"}"#,
    )
    .await;
    let partial = insert_delivery(&mut connection, r#"{"operation":"take"}"#).await;
    let nullish = insert_delivery(&mut connection, r#"{"world":null,"template":null}"#).await;
    for id in [complete, partial, nullish] {
        assert_columns_track_payload(&mut connection, id).await;
    }

    // 更新 payload 后生成列随推导重算；payload 仍是唯一写入面。
    sqlx::query("UPDATE public.deliveries SET payload = payload || '{\"world\":\"isekai\"}'::jsonb WHERE id = $1")
        .bind(partial)
        .execute(&mut *connection)
        .await
        .expect("追加 payload 键");
    assert_columns_track_payload(&mut connection, partial).await;

    // 直接写生成列必须被数据库拒绝（SQLSTATE 428C9：不能给生成列赋非 DEFAULT 值）。
    let error = sqlx::query("UPDATE public.deliveries SET payload_world = 'main' WHERE id = $1")
        .bind(complete)
        .execute(&mut *connection)
        .await
        .expect_err("直接写生成列必须失败");
    assert_eq!(error.as_database_error().and_then(|error| error.code()).as_deref(), Some("428C9"));

    // 迁移回填等价性：全表没有任何一行与 payload 派生结果不一致。
    let drift: i64 = sqlx::query(
        "SELECT count(*) FROM public.deliveries
         WHERE payload_world IS DISTINCT FROM payload ->> 'world'
            OR payload_operation IS DISTINCT FROM payload ->> 'operation'
            OR payload_template IS DISTINCT FROM payload ->> 'template'",
    )
    .fetch_one(&mut *connection)
    .await
    .expect("统计漂移行")
    .get(0);
    assert_eq!(drift, 0, "生成列不得与 payload 漂移");

    // 表级授权沿用 000081：运行账号不需要为新列单独授权。
    let readable: bool = sqlx::query(
        "SELECT has_column_privilege('auction_runtime', 'public.deliveries', 'payload_world', 'SELECT')
            AND has_column_privilege('auction_runtime', 'public.deliveries', 'payload_operation', 'SELECT')
            AND has_column_privilege('auction_runtime', 'public.deliveries', 'payload_template', 'SELECT')",
    )
    .fetch_one(&mut *connection)
    .await
    .expect("读取运行账号列权限")
    .get(0);
    assert!(readable, "auction_runtime 必须能读取新列");

    drop(connection);
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_delivery_snapshot_proof_ignores_derived_columns() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let mut connection = pool.acquire().await.expect("获取测试连接");

    let id = insert_delivery(
        &mut connection,
        r#"{"world":"main","operation":"send","template":"settlement_winner_equipment_cod",
            "schema_version":1,"credits":"0","cod":"0"}"#,
    )
    .await;

    // 升级前形状的冻结快照：整行 JSON 不含三个派生列，其余列与升级前 `to_jsonb(行)` 一致
    // （比较式要求时间键存在于快照中，因此不能在快照里删掉它们）。
    sqlx::query(
        "CREATE TEMP TABLE pre_upgrade_snapshot AS
         SELECT jsonb_build_object(
             'delivery',
             to_jsonb(d) - 'payload_world' - 'payload_operation' - 'payload_template'
         ) AS original_snapshot
         FROM public.deliveries d WHERE d.id = $1",
    )
    .bind(id)
    .execute(&mut *connection)
    .await
    .expect("构造升级前形状快照");

    let equality = delivery_snapshot_equality_sql();
    let matches_pre_upgrade: bool = sqlx::query(AssertSqlSafe(format!(
        "SELECT (TRUE {equality}) IS TRUE
         FROM public.deliveries d CROSS JOIN pre_upgrade_snapshot x WHERE d.id = $1"
    )))
    .bind(id)
    .fetch_one(&mut *connection)
    .await
    .expect("求值升级前快照")
    .get(0);
    assert!(matches_pre_upgrade, "升级前写入的整行证据必须继续成立");

    // 反例：派生列确实存在于整行 JSON 中，因此上面的成立来自比较式的剔除而不是空集或恒真。
    let row_has_derived: bool = sqlx::query(
        "SELECT (to_jsonb(d) - 'payload_world' - 'payload_operation' - 'payload_template')
                IS DISTINCT FROM to_jsonb(d)
         FROM public.deliveries d WHERE d.id = $1",
    )
    .bind(id)
    .fetch_one(&mut *connection)
    .await
    .expect("比较整行 JSON")
    .get(0);
    assert!(row_has_derived, "整行 JSON 必须包含三个派生列");

    drop(connection);
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_delivery_sweep_index_uses_derived_columns() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let mut connection = pool.acquire().await.expect("获取测试连接");

    // 维护态投递：部分索引谓词要求 external_reference 为空、sending_at/completed_at 非空。
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             failure_code, sending_at, completed_at)
         SELECT gen_random_uuid(), gen_random_uuid(), decode(repeat('22', 32), 'hex'),
                'settlement', 'w12-6:sweep:' || g, 'sweep' || g,
                jsonb_build_object(
                    'world', CASE WHEN g % 2 = 0 THEN 'main' ELSE 'isekai' END,
                    'operation', CASE WHEN g % 3 = 0 THEN 'take' ELSE 'send' END,
                    'template', 'settlement_seller_payout',
                    'variables', jsonb_build_object('body', repeat('x', 200))),
                'manual_reconcile', 'write_unknown',
                now() - interval '2 hours', now() - interval '1 hour'
         FROM generate_series(1, 400) AS g",
    )
    .execute(&mut *connection)
    .await
    .expect("写入维护态投递");
    sqlx::query("ANALYZE public.deliveries").execute(&mut *connection).await.expect("更新统计");

    let sweep = "SELECT d.id FROM public.deliveries d
                 WHERE d.status='manual_reconcile' AND d.payload_operation IN ('send','take')
                   AND d.external_reference IS NULL AND d.sending_at IS NOT NULL
                   AND d.completed_at IS NOT NULL
                   AND COALESCE(d.payload_world,'main')='main'
                 ORDER BY COALESCE(d.payload_world,'main'), d.created_at, d.id LIMIT 50";
    let plan: Vec<String> = sqlx::query(AssertSqlSafe(format!("EXPLAIN {sweep}")))
        .fetch_all(&mut *connection)
        .await
        .expect("读取维护扫描执行计划")
        .iter()
        .map(|row| row.get::<String, _>(0))
        .collect();
    let plan_text = plan.join("\n");
    assert!(
        plan_text.contains("deliveries_mailbox_sweep_order"),
        "维护扫描必须能使用列形式的部分索引，实际计划：{plan_text}"
    );

    // 列形式与 payload 表达式在既有数据集上必须给出同一集合。
    let column_ids: Vec<Uuid> = sqlx::query(AssertSqlSafe(sweep.to_owned()))
        .fetch_all(&mut *connection)
        .await
        .expect("列形式维护扫描")
        .iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect();
    let payload_ids: Vec<Uuid> = sqlx::query(
        "SELECT d.id FROM public.deliveries d
         WHERE d.status='manual_reconcile' AND d.payload ->> 'operation' IN ('send','take')
           AND d.external_reference IS NULL AND d.sending_at IS NOT NULL
           AND d.completed_at IS NOT NULL
           AND COALESCE(d.payload ->> 'world','main')='main'
         ORDER BY COALESCE(d.payload ->> 'world','main'), d.created_at, d.id LIMIT 50",
    )
    .fetch_all(&mut *connection)
    .await
    .expect("payload 表达式维护扫描")
    .iter()
    .map(|row| row.get::<Uuid, _>("id"))
    .collect();
    assert_eq!(column_ids, payload_ids, "两种写法必须选出同一批投递");
    assert!(!column_ids.is_empty(), "夹具必须命中维护态投递");

    drop(connection);
    store.postgres_pool().close().await;
}

/// 回归：082 重定义发送类别函数时漏回 rekey，098 必须恢复 account 分类。
#[tokio::test]
async fn postgres_rekey_deliveries_stay_in_account_category() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let mut connection = pool.acquire().await.expect("获取测试连接");
    for template in ["rekey_key", "rekey_result"] {
        let category: Option<String> = sqlx::query_scalar(
            "SELECT public.delivery_send_category(
                'mail_intake',
                jsonb_build_object('operation', 'send', 'template', $1)
             )",
        )
        .bind(template)
        .fetch_one(&mut *connection)
        .await
        .expect("读取发送类别");
        assert_eq!(category.as_deref(), Some("account"), "{template}");
    }
}

/// 真实恢复队列的 pending 投递默认 `fencing_version=0`，且没有发送尝试。
/// 读取必须产出 payload 重建身份；无法唯一归属 Sent 时按原 fence=0 阻塞为人工对账，
/// 不能因为隔离证据表要求 fence>=1 而把整次核对打回，也不能静默改写 fence。
#[tokio::test]
async fn postgres_recovery_pending_fence_zero_is_read_and_blocked() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let mut connection = pool.acquire().await.expect("获取测试连接");
    let id = insert_delivery(
        &mut connection,
        r#"{"world":"main","operation":"send","template":"lot_watch","credits":0,"cod":0}"#,
    )
    .await;

    let expectations = store.pending_claimed_send_expectations().await.expect("读取恢复投递身份");
    assert_eq!(expectations.len(), 1);
    assert_eq!(expectations[0].delivery_id, id);
    assert_eq!(expectations[0].fencing_version, 0);
    assert_eq!(expectations[0].evidence, ExternalRecoverySendEvidence::RestoredPayload);

    let outcomes = store
        .block_unresolved_send_deliveries(
            Uuid::new_v4(),
            "ops".to_owned(),
            vec![ExternalRecoveryUnresolvedBlock {
                delivery_id: id,
                fencing_version: 0,
                reason: EXTERNAL_RECOVERY_AMBIGUOUS_REASON.to_owned(),
                candidate_mail_ids: vec!["123".to_owned()],
            }],
        )
        .await
        .expect("阻塞 pending fence=0 投递");
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].status, "blocked");
    let (status, fencing_version): (String, i64) =
        sqlx::query_as("SELECT status, fencing_version FROM public.deliveries WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *connection)
            .await
            .expect("读取阻塞后的投递");
    assert_eq!(status, "manual_reconcile");
    assert_eq!(fencing_version, 1, "阻塞推进一次 fence");
    drop(connection);
    store.postgres_pool().close().await;
}
