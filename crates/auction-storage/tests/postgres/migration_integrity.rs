//! 迁移 000127 与 000131 的升级夹具：告警批次只放行原始 prepared delivery，资金来源累计发行可拆分但不能超额，冲正世界由复合外键约束。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_storage::postgres_migrate_to;
use auction_time::{Duration, Timestamp};
use serde_json::json;
use uuid::Uuid;

use crate::support;

async fn repeatable_read_issue(
    pool: sqlx::PgPool,
    barrier: std::sync::Arc<tokio::sync::Barrier>,
    source_id: Uuid,
    allocation_id: Uuid,
    uid: &'static str,
) -> bool {
    let mut transaction = pool
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .await
        .expect("开启 Repeatable Read 资金事务");
    let issued: i64 = sqlx::query_scalar(
        "SELECT issued_amount FROM public.settlement_funding_sources WHERE id = $1",
    )
    .bind(source_id)
    .fetch_one(&mut *transaction)
    .await
    .expect("建立一致快照");
    assert_eq!(issued, 0);
    barrier.wait().await;
    let inserted = sqlx::query(
        "INSERT INTO public.settlement_funding_entries
            (id, command_id, uid, credit_id, source_id, amount, kind, proof_digest)
         VALUES ($1, $2, $3, $4, $5, 60, 'issue', decode(repeat('35', 32), 'hex'))",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(uid)
    .bind(allocation_id)
    .bind(source_id)
    .execute(&mut *transaction)
    .await;
    if inserted.is_err() {
        transaction.rollback().await.expect("回滚竞争发行事务");
        return false;
    }
    transaction.commit().await.is_ok()
}

async fn insert_alert_batch(
    pool: &sqlx::PgPool,
    status: &str,
    delivery_status: &str,
    valid_identity: bool,
) -> Uuid {
    let now = Timestamp::now();
    let batch_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let request_digest = [0x31_u8; 32];
    let origin_id = if valid_identity {
        format!("operations_alert:{batch_id}")
    } else {
        format!("operations_alert:{}", Uuid::new_v4())
    };
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "external_mail_id": format!("operations-alert:{batch_id}"),
        "template": "operations_alert",
        "result_code": "operations_alert",
        "user_key_user_id": null,
        "credits": 0,
        "cod": 0,
        "attachment_ids": [],
        "body": "migration fixture",
    });
    let terminal = delivery_status == "succeeded";
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at)
         VALUES ($1, $2, $3, 'operations_alert', $4, 'alerts', $5, $6,
                 $7, $8, $9, $10, $11, $12, $13, $13)",
    )
    .bind(delivery_id)
    .bind(command_id)
    .bind(request_digest.as_slice())
    .bind(origin_id)
    .bind(payload)
    .bind(delivery_status)
    .bind(i64::from(terminal))
    .bind(i32::from(terminal))
    .bind(terminal.then_some(now))
    .bind(terminal.then_some(now))
    .bind(terminal.then_some(now))
    .bind(terminal.then(|| "mooglemail:main:send:910001".to_owned()))
    .bind(now)
    .execute(pool)
    .await
    .expect("写入告警 delivery fixture");
    let claimed = status == "claimed";
    sqlx::query(
        "INSERT INTO public.operations_alert_batches
            (id, command_id, delivery_id, request_digest, status, claim_token,
             claim_fencing, claimed_at, claim_expires_at, next_attempt_at, attempt_count,
             item_count, body_digest, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, 1, $7, $8, $9, 1, 1, $10, $11, $11)",
    )
    .bind(batch_id)
    .bind(command_id)
    .bind(delivery_id)
    .bind(request_digest.as_slice())
    .bind(status)
    .bind(claimed.then(Uuid::new_v4))
    .bind(claimed.then_some(now))
    .bind(claimed.then_some(now + Duration::minutes(2)))
    .bind((!claimed).then_some(now + Duration::minutes(1)))
    .bind([0x41_u8; 32].as_slice())
    .bind(now)
    .execute(pool)
    .await
    .expect("写入告警批次 fixture");
    batch_id
}

#[tokio::test]
async fn postgres_migration_127_keeps_prepared_pending_and_claimed_batches_claimable() {
    let database = support::TestDatabase::create_empty().await.expect("创建未迁移测试库");
    let store = database.maintenance_store().await.expect("连接测试库 owner");
    let pool = store.postgres_pool();
    postgres_migrate_to(pool, 126).await.expect("迁移到 126");

    let pending_batch = insert_alert_batch(pool, "pending", "prepared", true).await;
    let claimed_batch = insert_alert_batch(pool, "claimed", "prepared", true).await;
    let terminal_batch = insert_alert_batch(pool, "claimed", "succeeded", true).await;
    postgres_migrate_to(pool, 127).await.expect("迁移 127 应接受原始 prepared 批次");

    let pending_status: String =
        sqlx::query_scalar("SELECT status FROM public.operations_alert_batches WHERE id = $1")
            .bind(pending_batch)
            .fetch_one(pool)
            .await
            .expect("读取 pending 批次");
    assert_eq!(pending_status, "pending");
    let claimed_status: String =
        sqlx::query_scalar("SELECT status FROM public.operations_alert_batches WHERE id = $1")
            .bind(claimed_batch)
            .fetch_one(pool)
            .await
            .expect("读取 claimed 批次");
    assert_eq!(claimed_status, "claimed");
    let terminal_status: String =
        sqlx::query_scalar("SELECT status FROM public.operations_alert_batches WHERE id = $1")
            .bind(terminal_batch)
            .fetch_one(pool)
            .await
            .expect("读取终态批次");
    assert_eq!(terminal_status, "succeeded");
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_migration_127_rejects_prepared_delivery_identity_mismatch() {
    let database = support::TestDatabase::create_empty().await.expect("创建未迁移测试库");
    let store = database.maintenance_store().await.expect("连接测试库 owner");
    let pool = store.postgres_pool();
    postgres_migrate_to(pool, 126).await.expect("迁移到 126");
    insert_alert_batch(pool, "claimed", "prepared", false).await;

    assert!(
        postgres_migrate_to(pool, 127).await.is_err(),
        "身份不一致的 prepared delivery 应阻止升级"
    );
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_migration_131_backfills_and_atomically_caps_split_funding_issues() {
    let database = support::TestDatabase::create_empty().await.expect("创建未迁移测试库");
    let maintenance = database.maintenance_store().await.expect("连接测试库 owner");
    let maintenance_pool = maintenance.postgres_pool();
    postgres_migrate_to(maintenance_pool, 130).await.expect("迁移到 130");

    let uid = "910001";
    let source_id = Uuid::new_v4();
    let allocation_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts
            (uid, accounting_mode, status, state_version)
         VALUES ($1, 'strict_cash', 'ready', 1)",
    )
    .bind(uid)
    .execute(maintenance_pool)
    .await
    .expect("写入资金账户");
    sqlx::query(
        "INSERT INTO public.settlement_funding_allocations
            (id, uid, purpose, amount, state, state_version)
         VALUES ($1, $2, 'proprietary', 0, 'available', 1)",
    )
    .bind(allocation_id)
    .bind(uid)
    .execute(maintenance_pool)
    .await
    .expect("写入资金分配");
    sqlx::query(
        "INSERT INTO public.settlement_funding_sources
            (id, uid, kind, source_key, amount, proof_digest)
         VALUES ($1, $2, 'reconciliation', $3, 100, decode(repeat('31', 32), 'hex'))",
    )
    .bind(source_id)
    .bind(uid)
    .bind(Uuid::new_v4())
    .execute(maintenance_pool)
    .await
    .expect("写入待迁移资金来源");
    for amount in [30_i64, 20] {
        sqlx::query(
            "INSERT INTO public.settlement_funding_entries
                (id, command_id, uid, credit_id, source_id, amount, kind, proof_digest)
             VALUES ($1, $2, $3, $4, $5, $6, 'issue', decode(repeat('32', 32), 'hex'))",
        )
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(uid)
        .bind(allocation_id)
        .bind(source_id)
        .bind(amount)
        .execute(maintenance_pool)
        .await
        .expect("写入既有拆分发行");
    }
    postgres_migrate_to(maintenance_pool, 131).await.expect("迁移 131");
    sqlx::query("SELECT public.apply_auction_runtime_privileges()")
        .fetch_one(maintenance_pool)
        .await
        .expect("重放权限授权后仍收紧 funding sources");

    let tracked: i64 = sqlx::query_scalar(
        "SELECT issued_amount FROM public.settlement_funding_sources WHERE id = $1",
    )
    .bind(source_id)
    .fetch_one(maintenance_pool)
    .await
    .expect("读取迁移回填累计额");
    assert_eq!(tracked, 50);

    let runtime = database.store().await.expect("连接运行账号");
    let runtime_pool = runtime.postgres_pool();
    let direct_counter_update = sqlx::query(
        "UPDATE public.settlement_funding_sources SET issued_amount = 51 WHERE id = $1",
    )
    .bind(source_id)
    .execute(runtime_pool)
    .await;
    assert!(direct_counter_update.is_err(), "运行账号不得直接改写累计发行额");
    let direct_counter_insert = sqlx::query(
        "INSERT INTO public.settlement_funding_sources
            (id, uid, kind, source_key, amount, proof_digest, issued_amount)
         VALUES ($1, $2, 'reconciliation', $3, 1, decode(repeat('37', 32), 'hex'), 1)",
    )
    .bind(Uuid::new_v4())
    .bind(uid)
    .bind(Uuid::new_v4())
    .execute(runtime_pool)
    .await;
    assert!(direct_counter_insert.is_err(), "运行账号不得在新来源上指定累计发行额");
    for amount in [25_i64, 25] {
        sqlx::query(
            "INSERT INTO public.settlement_funding_entries
                (id, command_id, uid, credit_id, source_id, amount, kind, proof_digest)
             VALUES ($1, $2, $3, $4, $5, $6, 'issue', decode(repeat('33', 32), 'hex'))",
        )
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(uid)
        .bind(allocation_id)
        .bind(source_id)
        .bind(amount)
        .execute(runtime_pool)
        .await
        .expect("允许同一来源的合法拆分发行");
    }
    let runtime_tracked: i64 = sqlx::query_scalar(
        "SELECT issued_amount FROM public.settlement_funding_sources WHERE id = $1",
    )
    .bind(source_id)
    .fetch_one(runtime_pool)
    .await
    .expect("运行账号读取累计发行额");
    assert_eq!(runtime_tracked, 100, "来源分录触发器必须推进受保护的计数器");
    let over_cap = sqlx::query(
        "INSERT INTO public.settlement_funding_entries
            (id, command_id, uid, credit_id, source_id, amount, kind, proof_digest)
         VALUES ($1, $2, $3, $4, $5, 1, 'issue', decode(repeat('34', 32), 'hex'))",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(uid)
    .bind(allocation_id)
    .bind(source_id)
    .execute(runtime_pool)
    .await;
    assert!(over_cap.is_err(), "累计发行超过来源额度必须失败");

    let amount_change =
        sqlx::query("UPDATE public.settlement_funding_sources SET amount = 101 WHERE id = $1")
            .bind(source_id)
            .execute(maintenance_pool)
            .await;
    assert!(amount_change.is_err(), "已使用来源的 amount 不得改写");
    let tracked: i64 = sqlx::query_scalar(
        "SELECT issued_amount FROM public.settlement_funding_sources WHERE id = $1",
    )
    .bind(source_id)
    .fetch_one(maintenance_pool)
    .await
    .expect("读取累计发行");
    assert_eq!(tracked, 100);
    let race_source_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_funding_sources
            (id, uid, kind, source_key, amount, proof_digest)
         VALUES ($1, $2, 'reconciliation', $3, 60, decode(repeat('36', 32), 'hex'))",
    )
    .bind(race_source_id)
    .bind(uid)
    .bind(Uuid::new_v4())
    .execute(maintenance_pool)
    .await
    .expect("写入并发发行来源");
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let (first, second) = tokio::join!(
        repeatable_read_issue(
            runtime_pool.clone(),
            barrier.clone(),
            race_source_id,
            allocation_id,
            uid,
        ),
        repeatable_read_issue(runtime_pool.clone(), barrier, race_source_id, allocation_id, uid,),
    );
    assert_ne!(first, second, "Repeatable Read 并发竞争最多允许一个足额发行提交");
    let race_totals: (i64, i64) = sqlx::query_as(
        "SELECT source.issued_amount, count(entry.id)::bigint
         FROM public.settlement_funding_sources AS source
         LEFT JOIN public.settlement_funding_entries AS entry
           ON entry.source_id = source.id AND entry.kind = 'issue'
         WHERE source.id = $1
         GROUP BY source.issued_amount",
    )
    .bind(race_source_id)
    .fetch_one(maintenance_pool)
    .await
    .expect("读取并发发行结果");
    assert_eq!(race_totals, (60, 1));
    runtime_pool.close().await;
    maintenance_pool.close().await;
}
