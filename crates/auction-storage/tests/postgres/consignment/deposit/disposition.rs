//! 4C 起写许可、清单变化零起写与取得证明逐件应用的 PG 行为。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration as StdDuration, Instant};

use auction_application::consignment::{
    ApplyMailProofCommand, ConsignmentFuture, DepositCommandService, DispositionAction,
    DispositionStore, EnvelopeStore, MailProofKind, StartDispositionWriteCommand,
    WriteStartOutcome,
};
use auction_application::core::RequestHash;
use auction_application::mail::{
    ConsignmentDispositionExecution, ConsignmentDispositionProof, ConsignmentDispositionRequest,
    ConsignmentDispositionTransport, ConsignmentNotSent, DispositionMaintenanceService,
    DispositionReconcileReason, DispositionRetryReason, DispositionRound, DispositionRoundRequest,
    DispositionWorkStore, PreparedDisposition,
};
use auction_domain::CommandId;
use sqlx::AssertSqlSafe;
use sqlx::PgPool;
use uuid::Uuid;

use super::envelopes::{readiness, repository};
use super::support::{
    ACCEPTED, ATTEMPT, CREDENTIAL, DECIDE, DECISION, DELIVERY, OBSERVATION, SESSION, WORK,
    accepted_envelope, listing_awaiting_deposit, mail_session, observation_and_asset,
    reservation_issued, succeeds, users_and_key,
};
use crate::support::TestDatabase;

fn command(text: &str) -> CommandId {
    CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, text.as_bytes()))
}

fn at(value: &str) -> auction_time::Timestamp {
    auction_time::Timestamp::parse_from_rfc3339(value).expect("固定时刻")
}

async fn control(pool: &PgPool) {
    succeeds(
        pool,
        "UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1;",
    )
    .await;
}

async fn must_succeed(pool: &PgPool, sql: &str) {
    let mut transaction = pool.begin().await.expect("开启夹具事务");
    if let Err(error) =
        sqlx::raw_sql(AssertSqlSafe(sql.to_owned())).execute(&mut *transaction).await
    {
        let detail = error
            .as_database_error()
            .map_or_else(|| error.to_string(), |database| database.message().to_owned());
        panic!("夹具失败：{detail}\nSQL：{sql}");
    }
    transaction.commit().await.expect("提交夹具");
}

async fn seed_attempt_and_start(owner: &PgPool, with_start: bool, digest: &str) {
    let start_sql = if with_start {
        format!(
            "INSERT INTO public.mail_disposition_write_starts
                (work_id, decision_id, delivery_id, attempt_id, fence, started_at, command_id)
             VALUES ('{WORK}', '{DECISION}', '{DELIVERY}', '{ATTEMPT}', 1, '{ACCEPTED}',
                     '{DECIDE}');"
        )
    } else {
        String::new()
    };
    succeeds(
        owner,
        &format!(
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('{ATTEMPT}', '{WORK}', '{DELIVERY}', 1, '{CREDENTIAL}', 1, 1,
                     '{SESSION}', decode(repeat('{digest}', 32), 'hex'), '{ACCEPTED}');
             UPDATE public.deliveries
             SET status = 'claimed', lease_token = '00000000-0000-4000-8000-0000000000aa',
                 lease_expires_at = timestamptz '2099-01-01T00:00:00Z',
                 fencing_version = 1, attempt_count = 1
             WHERE id = '{DELIVERY}';
             {start_sql}"
        ),
    )
    .await;
}

#[tokio::test]
async fn postgres_take_proof_uses_original_start_fence() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    control(pool).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    seed_attempt_and_start(owner.postgres_pool(), true, "68").await;
    // 当前投递 fence 已前进，证明仍必须引用原成功起写的 attempt/fence。
    sqlx::query(
        "UPDATE public.deliveries
         SET fencing_version = 2, attempt_count = 2
         WHERE id = $1",
    )
    .bind(Uuid::parse_str(DELIVERY).expect("投递"))
    .execute(owner.postgres_pool())
    .await
    .expect("推进当前 fence");
    let repository = repository(&database).await;
    let receipt = repository
        .apply_mail_proof(ApplyMailProofCommand {
            command_id: command("apply-take-original-fence"),
            request_hash: RequestHash::new([36; 32]),
            original_attempt_id: Uuid::parse_str(ATTEMPT).expect("attempt"),
            proof_kind: MailProofKind::ExactTake,
            final_decision_id: Uuid::parse_str(DECISION).expect("决定"),
            action: DispositionAction::Take,
            delivery_id: Uuid::parse_str(DELIVERY).expect("投递"),
            fence: 1,
            observation_id: Uuid::parse_str(OBSERVATION).expect("观察"),
            validator_version: "validator-test".to_owned(),
            proof_digest: [0xa8; 32],
        })
        .await
        .expect("原 fence 证明应成功");
    assert_eq!(receipt.custody_changes.len(), 1);
    let error = repository
        .apply_mail_proof(ApplyMailProofCommand {
            command_id: command("apply-take-current-fence"),
            request_hash: RequestHash::new([37; 32]),
            original_attempt_id: Uuid::parse_str(ATTEMPT).expect("attempt"),
            proof_kind: MailProofKind::ExactTake,
            final_decision_id: Uuid::parse_str(DECISION).expect("决定"),
            action: DispositionAction::Take,
            delivery_id: Uuid::parse_str(DELIVERY).expect("投递"),
            fence: 2,
            observation_id: Uuid::parse_str(OBSERVATION).expect("观察"),
            validator_version: "validator-test".to_owned(),
            proof_digest: [0xa9; 32],
        })
        .await
        .expect_err("不能用当前 fence 冒充原 fence");
    assert_eq!(
        error,
        auction_application::consignment::ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::FenceMismatch
        )
    );
    let proofs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_disposition_proofs WHERE conclusion='take_proven'",
    )
    .fetch_one(pool)
    .await
    .expect("读取证明数");
    assert_eq!(proofs, 1);
}

#[tokio::test]
async fn postgres_start_rejects_wrong_claim_without_start_row() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    control(pool).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    seed_attempt_and_start(owner.postgres_pool(), false, "68").await;
    let repository = repository(&database).await;
    let mut wrong = start_command(
        command("start-wrong-claim"),
        Uuid::parse_str(ATTEMPT).expect("attempt"),
        1,
        1,
    );
    wrong.claim_token = Uuid::new_v4();
    let error = repository.start_disposition_write(wrong).await.expect_err("错误 claim 必须拒绝");
    assert_eq!(
        error,
        auction_application::consignment::ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::FenceMismatch
        )
    );
    let starts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.mail_disposition_write_starts")
            .fetch_one(pool)
            .await
            .expect("读取起写数");
    assert_eq!(starts, 0);
}

#[tokio::test]
async fn postgres_disposition_write_permit_is_issued_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    control(pool).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    seed_attempt_and_start(owner.postgres_pool(), false, "68").await;
    let repository = repository(&database).await;
    let first_command = command("start-permit");
    let first = repository
        .start_disposition_write(start_command(
            first_command,
            Uuid::parse_str(ATTEMPT).expect("attempt"),
            1,
            1,
        ))
        .await
        .expect("首次起写");
    assert!(matches!(first, WriteStartOutcome::FirstCommitted(_)));
    // 模拟提交后连接断开、客户端以同 ID 重试：新仓储实例只能读账本得到原回执。
    let reopened = super::envelopes::repository(&database).await;
    let replay = reopened
        .start_disposition_write(start_command(
            first_command,
            Uuid::parse_str(ATTEMPT).expect("attempt"),
            1,
            1,
        ))
        .await
        .expect("同命令重放");
    assert!(matches!(replay, WriteStartOutcome::AlreadyCommitted(_)));
    let changed_fence = reopened
        .start_disposition_write(start_command(
            command("start-permit-new-fence"),
            Uuid::new_v4(),
            2,
            2,
        ))
        .await
        .expect("换 fence 重放");
    assert!(matches!(changed_fence, WriteStartOutcome::AlreadyCommitted(_)));
    let starts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.mail_disposition_write_starts")
            .fetch_one(pool)
            .await
            .expect("读取起写数");
    assert_eq!(starts, 1);
}

/// 起写许可（资产外写的并发安全边界）：旧 fence 换租后起写失败且零起写行，新 fence 恰首发一次；
/// 租约过期而 token 与 fence 都对时同样失败（覆盖 `lease_active`）。
#[tokio::test]
async fn postgres_stale_fence_start_fails_then_new_fence_starts_once() {
    const NEW_ATTEMPT: &str = "86000000-0000-0000-0000-0000000000b2";
    const NEW_TOKEN: &str = "00000000-0000-4000-8000-0000000000bb";
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    control(pool).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    seed_attempt_and_start(owner.postgres_pool(), false, "68").await;
    let repository = repository(&database).await;
    let fence_mismatch = auction_application::consignment::ConsignmentError::Deposit(
        auction_application::consignment::DepositBusinessRejection::FenceMismatch,
    );
    let starts = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM public.mail_disposition_write_starts")
            .fetch_one(pool)
            .await
            .expect("读取起写数")
    };
    let old_attempt = Uuid::parse_str(ATTEMPT).expect("attempt");

    // 分支一：token 与 fence 都对，但租约已过期。
    succeeds(
        owner.postgres_pool(),
        &format!(
            "UPDATE public.deliveries SET lease_expires_at = clock_timestamp() - interval '1 minute'
             WHERE id = '{DELIVERY}';"
        ),
    )
    .await;
    let expired = repository
        .start_disposition_write(start_command(command("start-expired-lease"), old_attempt, 1, 1))
        .await
        .expect_err("过租的持有者不得起写");
    assert_eq!(expired, fence_mismatch);
    assert_eq!(starts().await, 0);

    // 分支二：租约有效、token 相同，但投递的 fencing_version 已前进。
    succeeds(
        owner.postgres_pool(),
        &format!(
            "UPDATE public.deliveries
             SET lease_expires_at = timestamptz '2099-01-01T00:00:00Z', fencing_version = 2
             WHERE id = '{DELIVERY}';"
        ),
    )
    .await;
    let bumped = repository
        .start_disposition_write(start_command(command("start-bumped-fence"), old_attempt, 1, 1))
        .await
        .expect_err("fencing_version 已前进，旧 fence 不得起写");
    assert_eq!(bumped, fence_mismatch);
    assert_eq!(starts().await, 0);

    // 换租：B 持有新 token 与 fence 2，并有自己的 attempt 行。
    succeeds(
        owner.postgres_pool(),
        &format!(
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('{NEW_ATTEMPT}', '{WORK}', '{DELIVERY}', 2, '{CREDENTIAL}', 1, 1,
                     '{SESSION}', decode(repeat('68', 32), 'hex'), '{ACCEPTED}');
             UPDATE public.deliveries
             SET lease_token = '{NEW_TOKEN}', attempt_count = 2 WHERE id = '{DELIVERY}';"
        ),
    )
    .await;
    let stale = start_command(command("start-stale-a"), old_attempt, 1, 1);
    let stale_error = repository
        .start_disposition_write(stale.clone())
        .await
        .expect_err("旧 fence A 换租后必须失败");
    assert_eq!(stale_error, fence_mismatch);
    assert_eq!(starts().await, 0);

    let mut fresh = start_command(
        command("start-fresh-b"),
        Uuid::parse_str(NEW_ATTEMPT).expect("attempt"),
        2,
        2,
    );
    fresh.claim_token = Uuid::parse_str(NEW_TOKEN).expect("token");
    // A 的同命令重放仍失败（失败不入账本，不会被当成已完成）。
    let replay_error =
        repository.start_disposition_write(stale).await.expect_err("A 的同命令重放仍失败");
    assert_eq!(replay_error, fence_mismatch);
    assert_eq!(starts().await, 0);

    let first = repository.start_disposition_write(fresh).await.expect("新 fence 起写");
    assert!(matches!(first, WriteStartOutcome::FirstCommitted(_)));
    assert_eq!(starts().await, 1);
    let started_fence: i64 =
        sqlx::query_scalar("SELECT fence FROM public.mail_disposition_write_starts")
            .fetch_one(pool)
            .await
            .expect("读取起写 fence");
    assert_eq!(started_fence, 2);
}

fn start_command(
    command_id: CommandId,
    attempt_id: Uuid,
    fence: u64,
    attempt_count: u32,
) -> StartDispositionWriteCommand {
    let _ = attempt_count;
    StartDispositionWriteCommand {
        command_id,
        request_hash: RequestHash::new([31; 32]),
        final_decision_id: Uuid::parse_str(DECISION).expect("决定"),
        action: DispositionAction::Take,
        delivery_id: Uuid::parse_str(DELIVERY).expect("投递"),
        fence,
        work_id: Uuid::parse_str(WORK).expect("工作"),
        attempt_id,
        claim_token: Uuid::parse_str("00000000-0000-4000-8000-0000000000aa").expect("token"),
        session_binding_digest: [0x33; 32],
        expected_manifest_digest: [0x68; 32],
    }
}

#[tokio::test]
async fn postgres_changed_return_manifest_has_zero_posts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    let owner = database.maintenance_store().await.expect("维护账号");
    let return_fixture = users_and_key()
        + &listing_awaiting_deposit().replace("2026-01-02T00:00:00Z", "2099-01-01T00:00:00Z")
        + &mail_session()
        + &reservation_issued().replace("2026-01-01T12:00:00Z", "2099-01-01T00:00:00Z")
        + &observation_and_asset()
        + &super::support::decide_command()
        + &format!(
            "INSERT INTO public.mail_initial_decisions
                (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
                 quarantine_id, decided_at)
             VALUES ('12345', 'main', '555', '86000000-0000-0000-0000-000000000002',
                     '{OBSERVATION}', '{DECIDE}', 'return_decided', NULL, '{ACCEPTED}');
             INSERT INTO public.mail_final_dispositions
                (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
                 decided_at)
             VALUES ('12345', 'main', '555', '{DECISION}', 'return_decided',
                     '{OBSERVATION}', '{DECIDE}', '{ACCEPTED}');
             INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('{DELIVERY}', '8b000000-0000-0000-0000-0000000000c1',
                     decode(repeat('69', 32), 'hex'), 'consignment_disposition',
                     'consignment:{DECISION}:{WORK}', 'FakeBot',
                     '{{\"schema_version\":2,\"operation\":\"return\",\"world\":\"main\",\
                       \"external_mail_id\":\"555\",\"mail_key\":\
                       {{\"bot_uid\":\"12345\",\"world_id\":\"main\",\"external_mid\":\"555\"}},\
                       \"decision_id\":\"{DECISION}\",\"work_id\":\"{WORK}\"}}'::jsonb,
                     'prepared', 0, 0, '{ACCEPTED}', '{ACCEPTED}');
             INSERT INTO public.mail_disposition_work
                (id, bot_uid, world_id, external_mid, decision_id, notice_id, action,
                 delivery_id, expected_observation_id, manifest_digest, created_at)
             VALUES ('{WORK}', '12345', 'main', '555', '{DECISION}', NULL, 'return',
                     '{DELIVERY}', '{OBSERVATION}', decode(repeat('68', 32), 'hex'),
                     '{ACCEPTED}');"
        );
    must_succeed(pool, &return_fixture).await;
    must_succeed(
        owner.postgres_pool(),
        &format!(
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('{ATTEMPT}', '{WORK}', '{DELIVERY}', 1, '{CREDENTIAL}', 1, 1,
                     '{SESSION}', decode(repeat('68', 32), 'hex'), '{ACCEPTED}');
             UPDATE public.deliveries
             SET status='claimed', lease_token='00000000-0000-4000-8000-0000000000aa',
                 lease_expires_at=timestamptz '2099-01-01T00:00:00Z',
                 fencing_version=1, attempt_count=1
             WHERE id='{DELIVERY}';"
        ),
    )
    .await;
    // 命令期望摘要 0x67 与工作的 0x68 不同，必须零起写。
    let repository = repository(&database).await;
    let error = repository
        .start_disposition_write(StartDispositionWriteCommand {
            command_id: command("start-changed-return"),
            request_hash: RequestHash::new([32; 32]),
            final_decision_id: Uuid::parse_str(DECISION).expect("决定"),
            action: DispositionAction::Return,
            delivery_id: Uuid::parse_str(DELIVERY).expect("投递"),
            fence: 1,
            work_id: Uuid::parse_str(WORK).expect("工作"),
            attempt_id: Uuid::parse_str(ATTEMPT).expect("attempt"),
            claim_token: Uuid::parse_str("00000000-0000-4000-8000-0000000000aa").expect("token"),
            session_binding_digest: [0x33; 32],
            expected_manifest_digest: [0x67; 32],
        })
        .await;
    assert!(error.is_err());
    let starts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.mail_disposition_write_starts")
            .fetch_one(pool)
            .await
            .expect("读取起写数");
    assert_eq!(starts, 0);
}

#[tokio::test]
async fn postgres_take_proof_applies_all_positions_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    control(pool).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    seed_attempt_and_start(owner.postgres_pool(), true, "68").await;
    let repository = repository(&database).await;
    let receipt = repository
        .apply_mail_proof(ApplyMailProofCommand {
            command_id: command("apply-take"),
            request_hash: RequestHash::new([33; 32]),
            original_attempt_id: Uuid::parse_str(ATTEMPT).expect("attempt"),
            proof_kind: MailProofKind::ExactTake,
            final_decision_id: Uuid::parse_str(DECISION).expect("决定"),
            action: DispositionAction::Take,
            delivery_id: Uuid::parse_str(DELIVERY).expect("投递"),
            fence: 1,
            observation_id: Uuid::parse_str(OBSERVATION).expect("观察"),
            validator_version: "validator-test".to_owned(),
            proof_digest: [0xa7; 32],
        })
        .await
        .expect("应用取得证明");
    assert_eq!(
        receipt.disposition,
        auction_application::consignment::ReceiptDisposition::TakeProven
    );
    assert_eq!(receipt.custody_changes.len(), 1);
    assert_eq!(receipt.listing_changes.len(), 1);
    let position: (String, i64) =
        sqlx::query_as("SELECT state, version FROM public.consignment_custody_positions")
            .fetch_one(pool)
            .await
            .expect("读取 position");
    assert_eq!(position, ("in_custody".to_owned(), 2));
    let proof: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_disposition_proofs WHERE conclusion='take_proven'",
    )
    .fetch_one(pool)
    .await
    .expect("读取证明数");
    assert_eq!(proof, 1);
}

async fn seed_second_listing(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
    let listing_id = Uuid::new_v4();
    let snapshot_id = Uuid::new_v4();
    let register_id = Uuid::new_v4();
    let publish_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, b"second-listing-publish");
    let listing = listing_awaiting_deposit()
        .replace("2026-01-02T00:00:00Z", "2099-01-01T00:00:00Z")
        .replace(super::support::LISTING, &listing_id.to_string())
        .replace("30000000-0000-0000-0000-000000000001", &snapshot_id.to_string())
        .replace("40000000-0000-4000-8000-000000000001", &register_id.to_string())
        .replace("41000000-0000-5000-8000-000000000001", &publish_id.to_string())
        .replace("'42'", "'43'");
    succeeds(pool, &listing).await;
    let reservation_id = Uuid::new_v4();
    let asset_id = Uuid::new_v4();
    let observation_id = Uuid::new_v4();
    let mut transaction = pool.begin().await.expect("开启第二件事务");
    sqlx::query(
        "INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id, canonical_eid,
             season_window_id, generation, service_epoch, session_id, token_digest, issued_at,
             expires_at, state, last_command_id, retired_at, reason)
         VALUES ($1, $2, $3, 'FakeSellerOne', '12345', 'main', '43', NULL, 1, 1,
                 $4, decode(repeat('71', 32), 'hex'), $5, timestamptz '2099-01-01T00:00:00Z',
                 'active', $6, NULL, NULL)",
    )
    .bind(reservation_id)
    .bind(listing_id)
    .bind(Uuid::parse_str(super::support::USER_1).expect("卖家"))
    .bind(Uuid::parse_str(SESSION).expect("会话"))
    .bind(at(ACCEPTED))
    .bind(register_id)
    .execute(&mut *transaction)
    .await
    .expect("写入第二件预约");
    sqlx::query(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '43', 1, $1, NULL, $2, $1)",
    )
    .bind(reservation_id)
    .bind(at(ACCEPTED))
    .execute(&mut *transaction)
    .await
    .expect("写入第二件强占用");
    sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ($1, '12345', 'main', '555', 'obs-two-items', $2, 'fake-mailbox', NULL, NULL,
                 timestamptz '2026-01-01T05:00:00Z', 'parser-v1', NULL,
                 decode(repeat('72', 32), 'hex'), decode(repeat('73', 32), 'hex'),
                 decode(repeat('7f', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, 2,
                 '[{\"ordinal\":1,\"kind\":\"equipment\",\"name\":\"Fake Gear\",
                    \"raw_eid\":\"42\",\"canonical_eid\":\"42\",
                    \"protected_ref\":\"83000000-0000-0000-0000-000000000001\"},
                   {\"ordinal\":2,\"kind\":\"equipment\",\"name\":\"Fake Gear Two\",
                    \"raw_eid\":\"43\",\"canonical_eid\":\"43\",
                    \"protected_ref\":\"83000000-0000-0000-0000-000000000001\"}]'::jsonb,
                 '{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T05:00\",
                   \"asset_count\":2,
                   \"items\":[{\"ordinal\":1,\"world\":\"main\",\"tradeable\":true,
                               \"history_ok\":true},
                              {\"ordinal\":2,\"world\":\"main\",\"tradeable\":true,
                               \"history_ok\":true}]}'::jsonb,
                 NULL)",
    )
    .bind(observation_id)
    .bind(Uuid::parse_str(SESSION).expect("会话"))
    .execute(&mut *transaction)
    .await
    .expect("写入两件观察");
    sqlx::query(
        "INSERT INTO public.consignment_custody_assets
            (equipment_id, world_id, canonical_eid, season_id, first_observation_id,
             auction_equipment_id)
         VALUES ($1, 'main', '43', NULL, $2, NULL)",
    )
    .bind(asset_id)
    .bind(observation_id)
    .execute(&mut *transaction)
    .await
    .expect("写入第二件资产");
    transaction.commit().await.expect("提交第二件夹具");
    (listing_id, reservation_id, observation_id)
}

#[tokio::test]
async fn postgres_take_proof_applies_all_positions_once_for_two_listings() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    super::envelopes::seed_base(pool).await;
    let (second_listing, second_reservation, second_observation) = seed_second_listing(pool).await;
    let repository = repository(&database).await;
    let mut decide = super::envelopes::accepted_command(command("decide-two-items"));
    decide.request_hash = RequestHash::new([39; 32]);
    decide.facts.observation.observation_id = second_observation;
    decide.facts.observation.manifest_digest = [0x7f; 32];
    decide.facts.observation.asset_count = Some(2);
    let mut second_item = decide.facts.observation.items[0].clone();
    second_item.ordinal = 2;
    second_item.display_name = Some("Fake Gear Two".to_owned());
    second_item.raw_eid = Some("43".to_owned());
    second_item.canonical_eid = Some("43".to_owned());
    second_item.protected_ref = Some(Uuid::parse_str(super::support::ENVELOPE).expect("凭证"));
    decide.facts.observation.items.push(second_item);
    decide.facts.reservations.push(auction_domain::consignment::deposit::ReservationBinding {
        reservation_id: second_reservation,
        generation: 1,
        listing_id: second_listing,
        listing_version: auction_domain::consignment::ListingVersion::new(2).expect("版本"),
        seller_user_id: auction_domain::UserId::from_uuid(
            Uuid::parse_str(super::support::USER_1).expect("卖家"),
        ),
        equipment: auction_domain::consignment::ListingEquipmentIdentity::new("main", "43")
            .expect("装备"),
        expires_at: at("2099-01-01T00:00:00Z"),
        state: auction_domain::consignment::deposit::ReservationState::Active,
    });
    let mut second_matched = match &decide.decision {
        auction_domain::consignment::deposit::EnvelopeDecision::Accept(items) => items[0].clone(),
        _ => panic!("夹具应为接受"),
    };
    second_matched.ordinal = 2;
    second_matched.listing_id = second_listing;
    second_matched.listing_version =
        auction_domain::consignment::ListingVersion::new(2).expect("版本");
    second_matched.reservation_id = second_reservation;
    second_matched.equipment =
        auction_domain::consignment::ListingEquipmentIdentity::new("main", "43").expect("装备");
    decide.decision = auction_domain::consignment::deposit::EnvelopeDecision::Accept(vec![
        match &decide.decision {
            auction_domain::consignment::deposit::EnvelopeDecision::Accept(items) => {
                items[0].clone()
            }
            _ => panic!("夹具应为接受"),
        },
        second_matched,
    ]);
    let receipt = repository.decide_envelope(decide).await.expect("两件整封接受");
    assert_eq!(receipt.listing_changes.len(), 2);
    let (work_id, delivery_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT id, delivery_id FROM public.mail_disposition_work WHERE decision_id=$1",
    )
    .bind(receipt.decision_id)
    .fetch_one(pool)
    .await
    .expect("读取工作");
    let attempt_id = Uuid::new_v4();
    let token = Uuid::parse_str("00000000-0000-4000-8000-0000000000ab").expect("token");
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('{attempt_id}', '{work_id}', '{delivery_id}', 1, '{CREDENTIAL}', 1, 1,
                     '{SESSION}', decode(repeat('7f', 32), 'hex'), '{ACCEPTED}');
             UPDATE public.deliveries
             SET status='claimed', lease_token='{token}',
                 lease_expires_at=timestamptz '2099-01-01T00:00:00Z',
                 fencing_version=1, attempt_count=1
             WHERE id='{delivery_id}';
             INSERT INTO public.mail_disposition_write_starts
                (work_id, decision_id, delivery_id, attempt_id, fence, started_at, command_id)
             VALUES ('{work_id}', '{}', '{delivery_id}', '{attempt_id}', 1, '{ACCEPTED}',
                     '{}');",
            receipt.decision_id,
            receipt.command_id.as_uuid()
        ),
    )
    .await;
    let proof = repository
        .apply_mail_proof(ApplyMailProofCommand {
            command_id: command("apply-two-items"),
            request_hash: RequestHash::new([40; 32]),
            original_attempt_id: attempt_id,
            proof_kind: MailProofKind::ExactTake,
            final_decision_id: receipt.decision_id,
            action: DispositionAction::Take,
            delivery_id,
            fence: 1,
            observation_id: second_observation,
            validator_version: "validator-test".to_owned(),
            proof_digest: [0xad; 32],
        })
        .await
        .expect("两件取得证明");
    assert_eq!(proof.custody_changes.len(), 2);
    assert_eq!(proof.listing_changes.len(), 2);
    let positions: Vec<(String, i64)> = sqlx::query_as(
        "SELECT p.state, p.version FROM public.consignment_custody_positions p
         JOIN public.consignment_deposit_items i ON i.id=p.item_id
         WHERE i.decision_id=$1 ORDER BY i.ordinal",
    )
    .bind(receipt.decision_id)
    .fetch_all(pool)
    .await
    .expect("读取逐件 position");
    assert_eq!(positions, vec![("in_custody".to_owned(), 2), ("in_custody".to_owned(), 2)]);
    let listings: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT l.id, l.state, l.state_version FROM public.consignment_listings l
         WHERE l.id = ANY($1)",
    )
    .bind(vec![Uuid::parse_str(super::support::LISTING).expect("挂单"), second_listing])
    .fetch_all(pool)
    .await
    .expect("读取挂单版本");
    assert_eq!(listings.len(), 2);
    let changed: std::collections::BTreeMap<Uuid, u64> = proof
        .listing_changes
        .iter()
        .map(|change| (change.listing_id, change.listing_version))
        .collect();
    assert_eq!(changed.len(), 2);
    for (listing_id, state, version) in &listings {
        assert_ne!(state, "awaiting_deposit");
        assert_eq!(*version as u64, changed[listing_id]);
    }
    let _ = work_id;
}

#[tokio::test]
async fn postgres_evidence_quarantine_does_not_burn_late_take_proof() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    control(pool).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    seed_attempt_and_start(owner.postgres_pool(), true, "68").await;
    let repository = repository(&database).await;
    let quarantined = repository
        .apply_mail_proof(ApplyMailProofCommand {
            command_id: command("apply-take-insufficient"),
            request_hash: RequestHash::new([34; 32]),
            original_attempt_id: Uuid::parse_str(ATTEMPT).expect("attempt"),
            proof_kind: MailProofKind::ExactTake,
            final_decision_id: Uuid::parse_str(DECISION).expect("决定"),
            action: DispositionAction::Take,
            delivery_id: Uuid::parse_str(DELIVERY).expect("投递"),
            fence: 1,
            observation_id: Uuid::parse_str(OBSERVATION).expect("观察"),
            validator_version: "validator-test".to_owned(),
            proof_digest: [0; 32],
        })
        .await
        .expect("证据不足只记待核");
    assert_eq!(
        quarantined.disposition,
        auction_application::consignment::ReceiptDisposition::EvidenceQuarantined
    );
    let position: (String, i64) =
        sqlx::query_as("SELECT state, version FROM public.consignment_custody_positions")
            .fetch_one(pool)
            .await
            .expect("读取待核 position");
    assert_eq!(position, ("awaiting_take".to_owned(), 1));
    let late = repository
        .apply_mail_proof(ApplyMailProofCommand {
            command_id: command("apply-take-late-proof"),
            request_hash: RequestHash::new([35; 32]),
            original_attempt_id: Uuid::parse_str(ATTEMPT).expect("attempt"),
            proof_kind: MailProofKind::ExactTake,
            final_decision_id: Uuid::parse_str(DECISION).expect("决定"),
            action: DispositionAction::Take,
            delivery_id: Uuid::parse_str(DELIVERY).expect("投递"),
            fence: 1,
            observation_id: Uuid::parse_str(OBSERVATION).expect("观察"),
            validator_version: "validator-test".to_owned(),
            proof_digest: [0xab; 32],
        })
        .await
        .expect("迟到正证照常推进");
    assert_eq!(late.disposition, auction_application::consignment::ReceiptDisposition::TakeProven);
    assert_eq!(late.custody_changes.len(), 1);
    let proofs: Vec<String> = sqlx::query_scalar(
        "SELECT conclusion FROM public.mail_disposition_proofs
         WHERE work_id=$1 ORDER BY conclusion",
    )
    .bind(Uuid::parse_str(WORK).expect("工作"))
    .fetch_all(pool)
    .await
    .expect("读取证明结论");
    assert_eq!(proofs, vec!["evidence_quarantined".to_owned(), "take_proven".to_owned()]);
}

#[tokio::test]
async fn postgres_opposite_conclusive_proof_is_rejected_with_zero_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    control(pool).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    seed_attempt_and_start(owner.postgres_pool(), true, "68").await;
    succeeds(
        owner.postgres_pool(),
        &format!(
            "INSERT INTO public.mail_disposition_proofs
                (id, work_id, decision_id, delivery_id, fence, attempt_id, kind, asset_leg,
                 observation_id, validator_version, digest, conclusion, proven_at)
             VALUES ('9b000000-0000-4000-8000-0000000000aa', '{WORK}', '{DECISION}',
                     '{DELIVERY}', 1, '{ATTEMPT}', 'exact_return', 'original', '{OBSERVATION}',
                     'validator-test', decode(repeat('ac', 32), 'hex'), 'return_proven',
                     '{ACCEPTED}');"
        ),
    )
    .await;
    let repository = repository(&database).await;
    let error = repository
        .apply_mail_proof(ApplyMailProofCommand {
            command_id: command("apply-take-opposite-conclusion"),
            request_hash: RequestHash::new([38; 32]),
            original_attempt_id: Uuid::parse_str(ATTEMPT).expect("attempt"),
            proof_kind: MailProofKind::ExactTake,
            final_decision_id: Uuid::parse_str(DECISION).expect("决定"),
            action: DispositionAction::Take,
            delivery_id: Uuid::parse_str(DELIVERY).expect("投递"),
            fence: 1,
            observation_id: Uuid::parse_str(OBSERVATION).expect("观察"),
            validator_version: "validator-test".to_owned(),
            proof_digest: [0xac; 32],
        })
        .await
        .expect_err("相反结论必须冲突");
    assert_eq!(
        error,
        auction_application::consignment::ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::EvidenceConflict
        )
    );
    let facts: (i64, String, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.mail_disposition_proofs WHERE work_id=$1),
            (SELECT state FROM public.consignment_custody_positions),
            (SELECT version FROM public.consignment_custody_positions)",
    )
    .bind(Uuid::parse_str(WORK).expect("工作"))
    .fetch_one(pool)
    .await
    .expect("读取零新事实");
    assert_eq!(facts, (1, "awaiting_take".to_owned(), 1));
}

#[tokio::test]
async fn postgres_concurrent_accept_return_has_one_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    super::envelopes::seed_base(pool).await;
    let repository = repository(&database).await;
    let accept = repository.decide_envelope(super::envelopes::accepted_command(
        super::envelopes::worker_command("race-accept"),
    ));
    let return_decision = repository.decide_envelope(super::envelopes::return_command(
        super::envelopes::worker_command("race-quarantine"),
    ));
    let (first, second) = tokio::join!(accept, return_decision);
    assert_ne!(first.is_ok(), second.is_ok());
    let finals: i64 = sqlx::query_scalar("SELECT count(*) FROM public.mail_final_dispositions")
        .fetch_one(pool)
        .await
        .expect("读取终局数");
    assert_eq!(finals, 1);
    let works: i64 = sqlx::query_scalar("SELECT count(*) FROM public.mail_disposition_work")
        .fetch_one(pool)
        .await
        .expect("读取工作数");
    assert_eq!(works, 1);
    let _ = (runtime, DECIDE);
}

#[tokio::test]
async fn postgres_disposition_work_retry_count_and_lease_takeover() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    control(pool).await;
    succeeds(pool, &super::envelopes::readiness()).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    let manifest = serde_json::json!({
        "world": "main",
        "external_mail_id": "555",
        "sender": "FakeSellerOne",
        "recipient": "FakeBot",
        "subject": "consign",
        "body_digest": vec![0x67_u8; 32],
        "credits": 0,
        "cod": 0,
        "received_minute_raw": "2026-01-01T03:00",
        "received_at": "2026-01-01T03:00:00Z",
        "attachments": [{"ordinal": 1, "name": "42", "canonical_eid": "42"}],
        "manifest_digest": vec![0x68_u8; 32],
    });
    sqlx::query(
        "UPDATE public.deliveries
         SET payload = payload || jsonb_build_object('expected_manifest', $2)
         WHERE id = $1",
    )
    .bind(Uuid::parse_str(DELIVERY).expect("投递"))
    .bind(manifest)
    .execute(owner.postgres_pool())
    .await
    .expect("写入冻结清单");
    let repository = repository(&database).await;
    let db_now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时刻");
    let first = repository
        .claim_due("worker-a".to_owned(), db_now, db_now + auction_time::Duration::hours(1))
        .await
        .expect("首次领取")
        .expect("应有工作");
    assert_eq!(first.attempt_count, 1);
    assert_eq!(first.fence, 1);
    repository.check_current(&first).await.expect("首次租约有效");
    repository
        .release_for_retry(
            &first,
            DispositionRetryReason::TemporaryUnavailable,
            db_now + auction_time::Duration::hours(1),
        )
        .await
        .expect("释放重试");
    let before = repository
        .claim_due(
            "worker-b".to_owned(),
            db_now + auction_time::Duration::hours(2),
            db_now + auction_time::Duration::hours(3),
        )
        .await
        .expect("提前领取");
    assert!(before.is_none());
    sqlx::query(
        "UPDATE public.deliveries
         SET payload = payload || jsonb_build_object('retry_at',
             to_char((clock_timestamp() - interval '1 second') AT TIME ZONE 'UTC',
                     'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'))
         WHERE id = $1",
    )
    .bind(Uuid::parse_str(DELIVERY).expect("投递"))
    .execute(owner.postgres_pool())
    .await
    .expect("使 retry_at 到期");
    let second = repository
        .claim_due("worker-b".to_owned(), db_now - auction_time::Duration::hours(1), db_now)
        .await
        .expect("接管领取")
        .expect("retry_at 之后应可领取");
    assert_eq!(second.attempt_count, 2);
    assert_eq!(second.fence, 2);
    assert!(repository.check_current(&first).await.is_err());
    assert!(
        repository.reconcile(&first, DispositionReconcileReason::WriteUnknown, None).await.is_err()
    );
    repository.check_current(&second).await.expect("新租约有效");
    let updated: (String, i32) =
        sqlx::query_as("SELECT status, attempt_count FROM public.deliveries WHERE id=$1")
            .bind(second.delivery_id)
            .fetch_one(pool)
            .await
            .expect("读取投递状态");
    assert_eq!(updated, ("claimed".to_owned(), 2));
    sqlx::query(
        "UPDATE public.deliveries
         SET lease_expires_at = clock_timestamp() - interval '1 minute'
         WHERE id = $1",
    )
    .bind(second.delivery_id)
    .execute(owner.postgres_pool())
    .await
    .expect("模拟过租");
    assert!(repository.check_current(&second).await.is_err());
    let third = repository
        .claim_due("worker-c".to_owned(), db_now - auction_time::Duration::hours(1), db_now)
        .await
        .expect("过租后领取")
        .expect("过租工作应可被他人领取");
    assert_eq!(third.attempt_count, 3);
    assert_eq!(third.fence, 3);
    assert!(repository.check_current(&second).await.is_err());
    repository.check_current(&third).await.expect("新租约有效");
}

#[tokio::test]
async fn postgres_claim_due_uses_database_clock_for_retry_at() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    control(pool).await;
    succeeds(pool, &super::envelopes::readiness()).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    let manifest = serde_json::json!({
        "world": "main",
        "external_mail_id": "555",
        "sender": "FakeSellerOne",
        "recipient": "FakeBot",
        "subject": "consign",
        "body_digest": vec![0x67_u8; 32],
        "credits": 0,
        "cod": 0,
        "received_minute_raw": "2026-01-01T03:00",
        "received_at": "2026-01-01T03:00:00Z",
        "attachments": [{"ordinal": 1, "name": "Fake Gear", "canonical_eid": "42"}],
        "manifest_digest": vec![0x68_u8; 32],
    });
    sqlx::query(
        "UPDATE public.deliveries
         SET payload = payload || jsonb_build_object('expected_manifest', $2)
         WHERE id = $1",
    )
    .bind(Uuid::parse_str(DELIVERY).expect("投递"))
    .bind(manifest)
    .execute(owner.postgres_pool())
    .await
    .expect("写入冻结清单");
    let db_now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时刻");
    sqlx::query(
        "UPDATE public.deliveries
         SET payload = payload || jsonb_build_object('retry_at',
             to_char((clock_timestamp() + interval '1 hour') AT TIME ZONE 'UTC',
                     'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'))
         WHERE id = $1",
    )
    .bind(Uuid::parse_str(DELIVERY).expect("投递"))
    .execute(owner.postgres_pool())
    .await
    .expect("写入未来 retry_at");
    let repository = repository(&database).await;
    let caller_later = db_now + auction_time::Duration::hours(2);
    assert!(
        repository
            .claim_due(
                "worker-later".to_owned(),
                caller_later,
                caller_later + auction_time::Duration::hours(1),
            )
            .await
            .expect("调用方时钟超前领取")
            .is_none(),
        "数据库时刻尚未到 retry_at，不能领取"
    );
    sqlx::query(
        "UPDATE public.deliveries
         SET payload = payload || jsonb_build_object('retry_at',
             to_char((clock_timestamp() - interval '1 second') AT TIME ZONE 'UTC',
                     'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'))
         WHERE id = $1",
    )
    .bind(Uuid::parse_str(DELIVERY).expect("投递"))
    .execute(owner.postgres_pool())
    .await
    .expect("写入已到期 retry_at");
    let caller_earlier = db_now - auction_time::Duration::hours(1);
    let claimed = repository
        .claim_due(
            "worker-earlier".to_owned(),
            caller_earlier,
            caller_earlier + auction_time::Duration::hours(1),
        )
        .await
        .expect("调用方时钟落后领取")
        .expect("数据库时刻已到 retry_at，必须领取");
    assert_eq!(claimed.attempt_count, 1);
}

#[derive(Default)]
struct CountingTransport {
    prepares: AtomicUsize,
    posts: AtomicUsize,
}

struct TestPrepared;

impl PreparedDisposition for TestPrepared {
    fn session_binding_digest(&self) -> [u8; 32] {
        [0x33; 32]
    }

    fn observed_manifest_digest(&self) -> [u8; 32] {
        [0x68; 32]
    }

    fn observation_id(&self) -> Uuid {
        Uuid::parse_str(OBSERVATION).expect("观察")
    }
}

impl ConsignmentDispositionTransport for CountingTransport {
    fn prepare(
        &self,
        _request: ConsignmentDispositionRequest,
        _deadline: Instant,
    ) -> ConsignmentFuture<'_, Result<Box<dyn PreparedDisposition>, ConsignmentNotSent>> {
        self.prepares.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(Ok(Box::new(TestPrepared) as Box<dyn PreparedDisposition>)) })
    }

    fn execute(
        &self,
        _prepared: Box<dyn PreparedDisposition>,
        _permit: auction_application::consignment::PostPermit,
        _deadline: Instant,
    ) -> ConsignmentFuture<'_, ConsignmentDispositionExecution> {
        self.posts.fetch_add(1, Ordering::Relaxed);
        Box::pin(async {
            Ok(ConsignmentDispositionExecution::Proven(
                ConsignmentDispositionProof::from_adapter(
                    DispositionAction::Take,
                    Uuid::parse_str(OBSERVATION).expect("观察"),
                    "validator-test".to_owned(),
                    [0xa7; 32],
                    true,
                )
                .expect("测试证明形状"),
            ))
        })
    }
}

#[tokio::test]
async fn postgres_disabled_service_continues_started_disposition() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_envelope()).await;
    succeeds(pool, &readiness()).await;
    let owner = database.maintenance_store().await.expect("维护账号");
    seed_attempt_and_start(owner.postgres_pool(), true, "68").await;
    let manifest = serde_json::json!({
        "world": "main",
        "external_mail_id": "555",
        "sender": "FakeSellerOne",
        "recipient": "FakeBot",
        "subject": "consign",
        "body_digest": vec![0x67_u8; 32],
        "credits": 0,
        "cod": 0,
        "received_minute_raw": "2026-01-01T03:00",
        "received_at": "2026-01-01T03:00:00Z",
        "attachments": [{"ordinal": 1, "name": "Fake Gear", "canonical_eid": "42"}],
        "manifest_digest": vec![0x68_u8; 32],
    });
    sqlx::query(
        "UPDATE public.deliveries
         SET payload = payload || jsonb_build_object('expected_manifest', $2)
         WHERE id = $1",
    )
    .bind(Uuid::parse_str(DELIVERY).expect("投递"))
    .bind(manifest)
    .execute(owner.postgres_pool())
    .await
    .expect("写入冻结清单");
    // 让该项重新可被处置维护领取，但已成功起写的 work 仍只有一条。
    succeeds(
        owner.postgres_pool(),
        &format!(
            "UPDATE public.deliveries
             SET status='prepared', lease_token=NULL, lease_expires_at=NULL
             WHERE id='{DELIVERY}';"
        ),
    )
    .await;
    // 关闭新业务不撤销已开始的资产处置；维护入口仍必须领取并识别原起写。
    succeeds(
        pool,
        "UPDATE public.consignment_lookup_control
         SET admitting=false, enabled_epoch=2 WHERE id=1;",
    )
    .await;
    assert!(runtime.has_consignment_work().await.expect("读取寄售存量"));

    let repository = Arc::new(repository(&database).await);
    let commands = Arc::new(DepositCommandService::new(
        repository.clone(),
        repository.clone(),
        repository.clone(),
        repository.clone(),
        repository.clone(),
    ));
    let transport = Arc::new(CountingTransport::default());
    let service = DispositionMaintenanceService::new(commands, repository, transport.clone());
    let now = auction_time::Timestamp::now();
    let round = service
        .run_once(DispositionRoundRequest {
            worker_id: "disabled-service-worker".to_owned(),
            now,
            lease_until: now + auction_time::Duration::minutes(2),
            deadline: Instant::now() + StdDuration::from_secs(60),
        })
        .await
        .expect("处置维护");
    assert_eq!(round, DispositionRound::Reconciled);
    assert_eq!(transport.prepares.load(Ordering::Relaxed), 1, "已开始的工作仍应被领取");
    assert_eq!(transport.posts.load(Ordering::Relaxed), 0, "停用后不得新增远端 POST");
    let starts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_disposition_write_starts WHERE work_id=$1",
    )
    .bind(Uuid::parse_str(WORK).expect("工作"))
    .fetch_one(pool)
    .await
    .expect("读取起写数");
    assert_eq!(starts, 1, "原起写行必须唯一保留");
    let delivery: (String, Option<String>) =
        sqlx::query_as("SELECT status, failure_code FROM public.deliveries WHERE id=$1")
            .bind(Uuid::parse_str(DELIVERY).expect("投递"))
            .fetch_one(pool)
            .await
            .expect("读取投递");
    assert_eq!(delivery, ("manual_reconcile".to_owned(), Some("already_committed".to_owned())));
    let facts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.mail_disposition_work WHERE id=$1),
                (SELECT count(*) FROM public.deliveries WHERE id=$2)",
    )
    .bind(Uuid::parse_str(WORK).expect("工作"))
    .bind(Uuid::parse_str(DELIVERY).expect("投递"))
    .fetch_one(pool)
    .await
    .expect("读取工作与投递");
    assert_eq!(facts, (1, 1), "工作与投递不得被删除或改回 pending");
}
