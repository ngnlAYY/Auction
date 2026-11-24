//! 通用投递路径对寄售处置投递的来源级排除：领取、过期回收与 Admin 动作。
//!
//! 保护不变量 4：外部写入只在本地提交后执行；结果未知只转人工核对，绝不自动重发。
//! 寄售处置的 Take/Return 只能由寄售专用链在 `mail_disposition_write_starts` 的一次性
//! 起写许可下执行，通用 claim、sending 围栏、租约释放、过期回收、邮箱巡检与 Admin
//! 投递动作都不得接管或重新激活它。

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminPort,
    DeliveryClaimExclusions, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::support::TestDatabase;

const WORKER: &str = "consignment-guard-worker";

/// 寄售处置投递的最小合法载荷：通用外壳键 + 迁移 000134 要求的寄售专有键。
fn consignment_payload(operation: &str, external_mid: &str) -> Value {
    json!({
        "schema_version": 2,
        "operation": operation,
        "world": "main",
        "external_mail_id": external_mid,
        "mail_key": {
            "bot_uid": "12345",
            "world_id": "main",
            "external_mid": external_mid,
        },
        "decision_id": "87000000-0000-0000-0000-000000000001",
        "work_id": "8c000000-0000-0000-0000-000000000001",
    })
}

async fn insert_consignment(
    pool: &PgPool,
    status: &str,
    operation: &str,
    external_mid: &str,
    lease_expires_at: Option<Timestamp>,
) -> Uuid {
    let delivery_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let lease_token = matches!(status, "claimed" | "sending").then(Uuid::new_v4);
    // 处置链把起写许可记在 `mail_disposition_write_starts`，终态没有 sending_at。
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, lease_token, lease_expires_at, fencing_version, attempt_count,
             claimed_at, sending_at, completed_at, failure_code, created_at, updated_at)
         VALUES ($1, $2, decode(repeat('31', 32), 'hex'), 'consignment_disposition', $3,
                 'FakeBot', $4, $5, $6, $7,
                 CASE WHEN $5 IN ('claimed', 'sending', 'manual_reconcile') THEN 1 ELSE 0 END,
                 CASE WHEN $5 IN ('claimed', 'sending', 'manual_reconcile') THEN 1 ELSE 0 END,
                 CASE WHEN $5 IN ('claimed', 'sending', 'manual_reconcile')
                      THEN clock_timestamp() - interval '10 minutes' END,
                 CASE WHEN $5 = 'sending'
                      THEN clock_timestamp() - interval '5 minutes' END,
                 CASE WHEN $5 = 'manual_reconcile'
                      THEN clock_timestamp() - interval '1 minute' END,
                 CASE WHEN $5 = 'manual_reconcile' THEN 'outcome_unknown' END,
                 clock_timestamp() - interval '1 hour', clock_timestamp())",
    )
    .bind(delivery_id)
    .bind(command_id)
    .bind(format!("consignment:{command_id}:{delivery_id}"))
    .bind(consignment_payload(operation, external_mid))
    .bind(status)
    .bind(lease_token)
    .bind(lease_expires_at)
    .execute(pool)
    .await
    .expect("写入寄售处置投递");
    delivery_id
}

async fn insert_manual(pool: &PgPool, status: &str) -> Uuid {
    let delivery_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let lease_token = matches!(status, "claimed" | "sending").then(Uuid::new_v4);
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "external_mail_id": delivery_id.to_string(),
        "template": "manual_notice",
        "result_code": "test",
        "user_key_user_id": null,
        "credits": 0,
        "cod": 0,
        "attachment_ids": [],
        "body": "Frozen body",
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, lease_token, lease_expires_at, fencing_version, attempt_count,
             claimed_at, sending_at, completed_at, failure_code, created_at, updated_at)
         VALUES ($1, $2, decode(repeat('32', 32), 'hex'), 'manual', $3, 'SampleMailbox', $4,
                 $5, $6,
                 CASE WHEN $5 IN ('claimed', 'sending')
                      THEN clock_timestamp() - interval '1 minute' END,
                 CASE WHEN $5 IN ('claimed', 'sending', 'manual_reconcile') THEN 1 ELSE 0 END,
                 CASE WHEN $5 IN ('claimed', 'sending', 'manual_reconcile') THEN 1 ELSE 0 END,
                 CASE WHEN $5 IN ('claimed', 'sending', 'manual_reconcile')
                      THEN clock_timestamp() - interval '10 minutes' END,
                 CASE WHEN $5 IN ('sending', 'manual_reconcile')
                      THEN clock_timestamp() - interval '5 minutes' END,
                 CASE WHEN $5 = 'manual_reconcile'
                      THEN clock_timestamp() - interval '1 minute' END,
                 CASE WHEN $5 = 'manual_reconcile' THEN 'outcome_unknown' END,
                 clock_timestamp() - interval '1 hour', clock_timestamp())",
    )
    .bind(delivery_id)
    .bind(command_id)
    .bind(format!("manual:{delivery_id}"))
    .bind(payload)
    .bind(status)
    .bind(lease_token)
    .execute(pool)
    .await
    .expect("写入普通投递");
    delivery_id
}

async fn claim_due(store: &DatabaseStore) -> Vec<DeliveryWorkClaim> {
    let now = Timestamp::now();
    let sending_grace = std::time::Duration::from_secs(60);
    store.begin_claim_cycle(now, sending_grace).await.expect("执行领取前清理");
    store
        .claim_due(
            WORKER.to_owned(),
            now,
            now + Duration::minutes(2),
            sending_grace,
            DeliveryClaimExclusions::default(),
            10,
        )
        .await
        .expect("领取投递")
}

async fn state(pool: &PgPool, delivery_id: Uuid) -> (String, i64) {
    sqlx::query_as("SELECT status, fencing_version FROM public.deliveries WHERE id = $1")
        .bind(delivery_id)
        .fetch_one(pool)
        .await
        .expect("读取投递状态")
}

fn admin_command(
    delivery_id: Uuid,
    action: DeliveryAdminAction,
    external_reference: Option<&str>,
) -> DeliveryAdminCommand {
    DeliveryAdminCommand {
        command_id: Uuid::new_v4(),
        delivery_id,
        expected_fencing_version: 1,
        action,
        actor_id: "admin".to_owned(),
        failure_code: None,
        external_reference: external_reference.map(str::to_owned),
        request_id: None,
        request_hash: action.request_hash(delivery_id, 1, external_reference, None, "admin"),
    }
}

#[tokio::test]
async fn postgres_generic_claim_excludes_consignment_disposition() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let pending = insert_consignment(pool, "pending", "take", "551", None).await;
    let expired_claim = insert_consignment(
        pool,
        "claimed",
        "return",
        "552",
        Some(Timestamp::now() - Duration::minutes(5)),
    )
    .await;
    let generic = insert_manual(pool, "pending").await;

    let claims = claim_due(&store).await;
    assert_eq!(claims.len(), 1, "通用领取只应拿到普通投递");
    assert_eq!(claims[0].delivery_id, generic);
    assert_eq!(state(pool, pending).await, ("pending".to_owned(), 0));
    assert_eq!(state(pool, expired_claim).await, ("claimed".to_owned(), 1));
}

#[tokio::test]
async fn postgres_consignment_admin_cannot_requeue_disposition() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let consignment = insert_consignment(pool, "manual_reconcile", "take", "553", None).await;

    for command in [
        admin_command(consignment, DeliveryAdminAction::ReconcileRetry, None),
        admin_command(
            consignment,
            DeliveryAdminAction::ReconcileSucceeded,
            Some("admin-reference"),
        ),
    ] {
        let action = command.action;
        let error = store.execute(command).await.expect_err("寄售处置不能被 Admin 重新激活");
        match &error {
            ApplicationError::DeliveryAdmin(DeliveryAdminError::Invalid) => {}
            other => panic!("动作 {action:?} 必须以既有安全错误拒绝，实际 {other:?}"),
        }
        assert_eq!(state(pool, consignment).await, ("manual_reconcile".to_owned(), 1));
    }

    // 对照：同一状态的普通投递仍可重排，证明阻断只按来源生效。
    let generic = insert_manual(pool, "manual_reconcile").await;
    let result = store
        .execute(admin_command(generic, DeliveryAdminAction::ReconcileRetry, None))
        .await
        .expect("普通投递可以重排");
    assert_eq!(result.snapshot.status, "pending");
    assert_eq!(state(pool, generic).await.0, "pending");
}

#[tokio::test]
async fn postgres_expired_sweep_skips_consignment_disposition() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let consignment = insert_consignment(
        pool,
        "sending",
        "return",
        "554",
        Some(Timestamp::now() - Duration::minutes(5)),
    )
    .await;
    let generic = insert_manual(pool, "sending").await;

    store
        .begin_claim_cycle(Timestamp::now(), std::time::Duration::from_secs(60))
        .await
        .expect("运行过期回收");

    assert_eq!(state(pool, consignment).await, ("sending".to_owned(), 1));
    let (generic_status, generic_fence) = state(pool, generic).await;
    assert_eq!(generic_status, "manual_reconcile");
    assert_eq!(generic_fence, 2);
}
