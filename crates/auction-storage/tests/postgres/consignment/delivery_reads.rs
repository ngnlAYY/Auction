//! Admin 投递读模型对寄售处置投递真实形状的读取与失败关闭。
//!
//! 处置链把起写许可记在 `mail_disposition_write_starts`，不写 `sending_at`；终态从
//! `claimed` 直接进入 `succeeded/manual_reconcile`。合法行必须能被 Admin 列表与详情读出，
//! 补上处置链不可能产生的 `sending_at` 后必须按 `StoredResultCorrupt` 失败关闭。

use std::sync::Arc;

use auction_application::consignment::DispositionAction;
use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryAdminError, DeliveryAdminListRequest, DeliveryAdminPort,
};
use auction_application::mail::{
    DispositionManifest, DispositionManifestAttachment, DispositionRetryReason,
    DispositionWorkClaim, DispositionWorkStore,
};
use auction_domain::SettlementWorld;
use auction_storage::{
    DepositRepository, DepositSettings, ReservationSecretAad, ReservationSecretCrypto,
    ReservationSecretCryptoError, SealedReservationSecret,
};
use auction_time::{Duration, Timestamp};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support::TestDatabase;

/// 读模型用例不需要真实封装；预约签名不经过本适配器。
struct NoCrypto;

impl ReservationSecretCrypto for NoCrypto {
    fn seal(
        &self,
        _token: &auction_domain::consignment::deposit::TokenText,
        _aad: ReservationSecretAad,
    ) -> Result<SealedReservationSecret, ReservationSecretCryptoError> {
        Err(ReservationSecretCryptoError)
    }

    fn open(
        &self,
        _sealed: &SealedReservationSecret,
        _aad: ReservationSecretAad,
    ) -> Result<auction_domain::consignment::deposit::TokenText, ReservationSecretCryptoError> {
        Err(ReservationSecretCryptoError)
    }
}

fn at(value: &str) -> Timestamp {
    Timestamp::parse_from_rfc3339(value).expect("固定时刻")
}

fn payload(decision_id: Uuid, work_id: Uuid) -> serde_json::Value {
    json!({
        "schema_version": 2,
        "operation": "take",
        "world": "main",
        "external_mail_id": "555",
        "mail_key": {
            "bot_uid": "12345",
            "world_id": "main",
            "external_mid": "555",
        },
        "decision_id": decision_id,
        "work_id": work_id,
        "template": "consignment_take",
        "result_code": null,
        "body": "",
        "user_key_user_id": null,
        "credits": 0,
        "cod": 0,
        "attachment_ids": [],
    })
}

struct Inserted {
    delivery_id: Uuid,
}

async fn insert_prepared_via_release(
    pool: &PgPool,
    store: &auction_storage::DatabaseStore,
) -> Uuid {
    let delivery_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let decision_id = Uuid::new_v4();
    let work_id = Uuid::new_v4();
    let attempt_id = Uuid::new_v4();
    let claim_token = Uuid::new_v4();
    let created_at = at("2026-11-06T10:00:00Z");
    let claimed_at = at("2026-11-06T10:00:01Z");
    let lease_until = at("2026-11-06T10:05:00Z");
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, lease_token, lease_expires_at, fencing_version, attempt_count,
             claimed_at, sending_at, completed_at, created_at, updated_at)
         VALUES ($1, $2, decode(repeat('31', 32), 'hex'), 'consignment_disposition', $3,
                 'FakeBot', $4, 'claimed', $5, $6, 1, 1, $7, NULL, NULL, $8, $7)",
    )
    .bind(delivery_id)
    .bind(command_id)
    .bind(format!("consignment:{decision_id}:{work_id}"))
    .bind(payload(decision_id, work_id))
    .bind(claim_token)
    .bind(lease_until)
    .bind(claimed_at)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("写入待重试的寄售处置投递");
    let repository = DepositRepository::new(
        store.clone(),
        Arc::new(NoCrypto),
        DepositSettings::with_standard_price_bounds(
            auction_domain::consignment::FeeBps::new(500).expect("费率"),
            24,
            100,
        )
        .expect("价格上下界"),
    );
    let claim = DispositionWorkClaim {
        work_id,
        final_decision_id: decision_id,
        delivery_id,
        action: DispositionAction::Take,
        attempt_id,
        fence: 1,
        claim_token,
        attempt_count: 1,
        lease_until,
        manifest: DispositionManifest {
            world: SettlementWorld::Main,
            external_mail_id: "555".to_owned(),
            sender: "FakeSender".to_owned(),
            recipient: "FakeBot".to_owned(),
            subject: "consign".to_owned(),
            body_digest: [1; 32],
            credits: 0,
            cod: 0,
            received_minute_raw: "2026-11-06T10:00Z".to_owned(),
            received_at: created_at,
            attachments: vec![DispositionManifestAttachment {
                ordinal: 1,
                name: "Fake equipment".to_owned(),
                canonical_eid: Some("42".to_owned()),
            }],
            manifest_digest: [2; 32],
        },
    };
    repository
        .release_for_retry(
            &claim,
            DispositionRetryReason::TemporaryUnavailable,
            lease_until + Duration::minutes(1),
        )
        .await
        .expect("处置链重试释放");
    delivery_id
}

async fn insert_disposition(
    pool: &PgPool,
    status: &str,
    claimed_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
    external_reference: Option<String>,
    failure_code: Option<&str>,
) -> Inserted {
    let delivery_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let decision_id = Uuid::new_v4();
    let work_id = Uuid::new_v4();
    let created_at = at("2026-11-06T10:00:00Z");
    let updated_at = at("2026-11-06T10:00:05Z");
    let lease_token = (status == "claimed").then(Uuid::new_v4);
    let lease_expires_at = (status == "claimed").then(|| at("2026-11-06T10:05:00Z"));
    let fencing_version = if status == "prepared" { 0 } else { 1 };
    let attempt_count = if status == "prepared" { 0 } else { 1 };
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, lease_token, lease_expires_at, fencing_version, attempt_count,
             claimed_at, sending_at, completed_at, external_reference, failure_code,
             created_at, updated_at)
         VALUES ($1, $2, decode(repeat('31', 32), 'hex'), 'consignment_disposition', $3,
                 'FakeBot', $4, $5, $6, $7, $8, $9, $10, NULL, $11, $12, $13, $14, $15)",
    )
    .bind(delivery_id)
    .bind(command_id)
    .bind(format!("consignment:{decision_id}:{work_id}"))
    .bind(payload(decision_id, work_id))
    .bind(status)
    .bind(lease_token)
    .bind(lease_expires_at)
    .bind(fencing_version)
    .bind(attempt_count)
    .bind(claimed_at)
    .bind(completed_at)
    .bind(external_reference)
    .bind(failure_code)
    .bind(created_at)
    .bind(updated_at)
    .execute(pool)
    .await
    .expect("写入寄售处置投递");
    Inserted { delivery_id }
}

fn list_request() -> DeliveryAdminListRequest {
    DeliveryAdminListRequest { limit: 100, offset: 0, ..DeliveryAdminListRequest::default() }
}

#[tokio::test]
async fn postgres_admin_delivery_reads_consignment_disposition_rows() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let claimed_at = at("2026-11-06T10:00:01Z");
    let completed_at = at("2026-11-06T10:00:05Z");

    let prepared = insert_prepared_via_release(pool, &store).await;
    let claimed = insert_disposition(pool, "claimed", Some(claimed_at), None, None, None).await;
    let succeeded = insert_disposition(
        pool,
        "succeeded",
        Some(claimed_at),
        Some(completed_at),
        Some("ab".repeat(32)),
        None,
    )
    .await;
    let manual = insert_disposition(
        pool,
        "manual_reconcile",
        Some(claimed_at),
        Some(completed_at),
        None,
        Some("remote_write_unknown"),
    )
    .await;

    // 重试释放必须同时清掉 claimed_at，否则 Admin 读模型会把合法的 prepared 行判坏。
    let prepared_state: (String, Option<Timestamp>, Option<Timestamp>) = sqlx::query_as(
        "SELECT status, claimed_at, lease_expires_at FROM public.deliveries WHERE id = $1",
    )
    .bind(prepared)
    .fetch_one(pool)
    .await
    .expect("读取重试释放后的投递");
    assert_eq!(prepared_state, ("prepared".to_owned(), None, None));

    let page = store.list(list_request()).await.expect("Admin 列表读出四种合法形状");
    assert_eq!(page.total, 4);
    let mut statuses = page.entries.iter().map(|row| row.status.as_str()).collect::<Vec<_>>();
    statuses.sort_unstable();
    assert_eq!(statuses, ["claimed", "manual_reconcile", "prepared", "succeeded"]);
    for (delivery_id, expected) in [
        (prepared, "prepared"),
        (claimed.delivery_id, "claimed"),
        (succeeded.delivery_id, "succeeded"),
        (manual.delivery_id, "manual_reconcile"),
    ] {
        let detail = store.detail(delivery_id).await.expect("Admin 详情读出合法形状");
        assert_eq!(detail.delivery.status, expected);
        assert_eq!(detail.delivery.sending_at, None);
    }

    // 处置链不会给该来源写 sending_at；补上后列表与详情都必须失败关闭。
    sqlx::query("UPDATE public.deliveries SET sending_at = $2 WHERE id = $1")
        .bind(succeeded.delivery_id)
        .bind(claimed_at + Duration::seconds(1))
        .execute(pool)
        .await
        .expect("注入不可能的 sending_at");
    let list_error = store.list(list_request()).await.expect_err("列表必须拒绝非法时间线");
    assert!(matches!(
        list_error,
        ApplicationError::DeliveryAdmin(DeliveryAdminError::StoredResultCorrupt)
    ));
    let detail_error =
        store.detail(succeeded.delivery_id).await.expect_err("详情必须拒绝非法时间线");
    assert!(matches!(
        detail_error,
        ApplicationError::DeliveryAdmin(DeliveryAdminError::StoredResultCorrupt)
    ));
}
