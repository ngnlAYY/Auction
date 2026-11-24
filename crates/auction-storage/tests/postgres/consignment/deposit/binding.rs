//! 投递尝试与处分证明的 K、投递、会话及凭据代次绑定。

use super::schema::additional_rows;
use super::support::{
    ACCEPTED, ATTEMPT, CREDENTIAL, DECISION, DELIVERY, ISSUED, LISTING, OBSERVATION,
    RELEASE_DECISION, SESSION, START, USER_2, WORK, attempt_start_proof_and_custody, check,
    listing_awaiting_deposit, mail_session, quarantine_release_fixture, seed_accepted, succeeds,
    users_and_key,
};
use crate::support::TestDatabase;

#[tokio::test]
async fn postgres_consignment_deposit_reservation_rejects_listing_seller_mismatch() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_awaiting_deposit() + &mail_session())).await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_deposit_reservations
                (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
                 canonical_eid, season_window_id, generation, service_epoch, session_id,
                 token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
             VALUES ('82000000-0000-0000-0000-000000000099', '{LISTING}', '{USER_2}',
                     'FakeSellerTwo', '12345', 'main', '42', NULL, 1, 1, '{SESSION}',
                     decode(repeat('5c', 32), 'hex'), '{ISSUED}',
                     '2026-01-01T12:00:00Z', 'active',
                     '40000000-0000-4000-8000-000000000001', NULL, NULL);"
        ),
        "c4_res_listing_seller",
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_deposit_attempt_binds_the_original_delivery_session_and_credential() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &additional_rows()).await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('d0000000-0000-4000-8000-000000000001',
                     'd1000000-0000-4000-8000-000000000001',
                     decode(repeat('d1', 32), 'hex'), 'manual', 'fake-origin', 'FakeBot',
                     '{{}}'::jsonb, 'prepared', 0, 0, '{START}', '{START}');"
        ),
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('d2000000-0000-4000-8000-000000000001', '{WORK}',
                     'd0000000-0000-4000-8000-000000000001', 2, '{CREDENTIAL}', 1, 1,
                     '{SESSION}', decode(repeat('d2', 32), 'hex'), '{ACCEPTED}');"
        ),
        "c4_attempt_delivery",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('d2000000-0000-4000-8000-000000000002', '{WORK}', '{DELIVERY}', 2,
                     '{CREDENTIAL}', 1, 1, '81000000-0000-0000-0000-000000000002',
                     decode(repeat('d3', 32), 'hex'), '{ACCEPTED}');"
        ),
        "c4_attempt_session",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('d2000000-0000-4000-8000-000000000003', '{WORK}', '{DELIVERY}', 2,
                     '{CREDENTIAL}', 2, 1, '{SESSION}', decode(repeat('d4', 32), 'hex'),
                     '{ACCEPTED}');"
        ),
        "c4_attempt_session",
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_deposit_proof_binds_work_observation_and_decision_k() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &additional_rows()).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    succeeds(pool, &quarantine_release_fixture()).await;
    check(
        pool,
        &format!(
            "INSERT INTO public.mail_disposition_proofs
                (id, work_id, decision_id, delivery_id, fence, attempt_id, kind, asset_leg,
                 observation_id, validator_version, digest, conclusion, proven_at)
             VALUES ('d3000000-0000-4000-8000-000000000001', '{WORK}', '{DECISION}',
                     '{DELIVERY}', 1, '{ATTEMPT}', 'exact_return', 'carrier',
                     '8f000000-0000-0000-0000-000000000001', 'v1',
                     decode(repeat('d5', 32), 'hex'), 'return_proven', '{ACCEPTED}');"
        ),
        "c4_proof_observation_k",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.mail_disposition_proofs
                (id, work_id, decision_id, delivery_id, fence, attempt_id, kind, asset_leg,
                 observation_id, validator_version, digest, conclusion, proven_at)
             VALUES ('d3000000-0000-4000-8000-000000000002', '{WORK}', '{RELEASE_DECISION}',
                     '{DELIVERY}', 1, '{ATTEMPT}', 'exact_return', 'cash', '{OBSERVATION}',
                     'v1', decode(repeat('d6', 32), 'hex'), 'return_proven', '{ACCEPTED}');"
        ),
        "c4_proof_decision_k",
    )
    .await;
}
