//! 4C 前缀 CAS、第 4 期 acquire_return 与两个旧 owner 失败关闭的 PG 行为。

use auction_application::consignment::{
    AdminDepositAction, AdminDepositQuery, AppendQuarantineObservationCommand, ConsignmentError,
    DepositActor, DepositBusinessRejection, DepositCommandService, DepositRead, EnvelopeStore,
    ObservationReceipt, QuarantineState, RecheckQuarantineCommand, ReleaseQuarantineCommand,
};
use auction_application::core::RequestHash;
use auction_application::mail::{
    DispositionRetryReason, DispositionRoundRequest, QuarantineRecheckReadError,
    QuarantineRecheckReader, QuarantineRecheckRound, QuarantineRecheckService,
    QuarantineRecheckStore, QuarantineReobservation,
};
use auction_domain::CommandId;
use auction_domain::consignment::deposit::ReleaseKind;
use sqlx::Acquire;
use sqlx::Row;
use uuid::Uuid;

use super::envelopes::{observation, quarantine_command, repository, seed_base, worker_command};
use crate::support::TestDatabase;

fn at(value: &str) -> auction_time::Timestamp {
    auction_time::Timestamp::parse_from_rfc3339(value).expect("固定时刻")
}

fn release_request(
    command_id: CommandId,
    quarantine_id: Uuid,
    kind: ReleaseKind,
    expected_version: u64,
    observation_id: Uuid,
    actor: DepositActor,
    request_hash: RequestHash,
) -> ReleaseQuarantineCommand {
    ReleaseQuarantineCommand {
        command_id,
        actor,
        quarantine_id,
        expected_version,
        observation_id,
        proposed_kind: kind,
        request_hash,
        request_id: None,
        auto_binding: None,
    }
}

async fn append_release_observation(
    repository: &auction_storage::DepositRepository,
    pool: &sqlx::PgPool,
    quarantine_id: Uuid,
    quarantine_version: u64,
) -> Uuid {
    append_release_observation_with(repository, pool, quarantine_id, quarantine_version, false)
        .await
}

async fn append_release_observation_with(
    repository: &auction_storage::DepositRepository,
    pool: &sqlx::PgPool,
    quarantine_id: Uuid,
    _quarantine_version: u64,
    empty: bool,
) -> Uuid {
    // 首次隔离已在同一事务自动排入系统重检；补证测试直接领取该工作。
    let _claim = repository
        .claim_due(
            "quarantine-worker".to_owned(),
            at("2099-01-01T00:00:00Z"),
            at("2099-01-02T00:00:00Z"),
        )
        .await
        .expect("领取重检")
        .expect("应有重检工作");
    let source_id = Uuid::new_v4();
    let (manifest, derived, raw_count) = if empty {
        (
            serde_json::json!([]),
            serde_json::json!({
                "subject_is_consign": true,
                "sender_identity": "verified",
                "list_detail_consistent": true,
                "duplicate_raw_eid": false,
                "canonical_alias_ambiguous": false,
                "unknown_attachment": false,
                "system_rts": false,
                "legacy_auction_owner": null,
                "received_minute_raw": "2026-01-01T05:00",
                "asset_count": 0,
                "items": [],
            }),
            None,
        )
    } else {
        (
            serde_json::json!([{
                "ordinal": 1,
                "kind": "equipment",
                "raw_eid": "42",
                "name": "Fake Gear",
                "canonical_eid": "42",
                "amount": null,
                "protected_ref": "83000000-0000-0000-0000-000000000001",
            }]),
            serde_json::json!({
                "subject_is_consign": true,
                "sender_identity": "verified",
                "list_detail_consistent": true,
                "duplicate_raw_eid": true,
                "canonical_alias_ambiguous": false,
                "unknown_attachment": false,
                "system_rts": false,
                "legacy_auction_owner": null,
                "received_minute_raw": "2026-01-01T05:00",
                "items": [{
                    "ordinal": 1,
                    "world": "main",
                    "tradeable": true,
                    "history_ok": true,
                }],
            }),
            Some(1_i32),
        )
    };
    sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ($1, '12345', 'main', '555', 'release-observation', $2, 'fake-mailbox',
                 NULL, NULL, '2026-01-01T05:00:00Z', 'parser-release', NULL,
                 decode(repeat('72', 32), 'hex'), decode(repeat('73', 32), 'hex'),
                 decode(repeat('6a', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, $3, $4, $5,
                 NULL)",
    )
    .bind(source_id)
    .bind(Uuid::parse_str(super::support::SESSION).expect("会话"))
    .bind(raw_count)
    .bind(&manifest)
    .bind(&derived)
    .execute(pool)
    .await
    .expect("写入补证观察");
    let mut observation = super::envelopes::observation(!empty);
    observation.observation_id = source_id;
    observation.manifest_digest = [0x6a; 32];
    observation.parser_version = "parser-release".to_owned();
    if empty {
        observation.asset_count = Some(0);
        observation.takeable = Some(false);
        observation.items.clear();
    }
    let receipt = repository
        .append_quarantine_observation(AppendQuarantineObservationCommand {
            command_id: worker_command("append-release-observation"),
            request_hash: RequestHash::new([42; 32]),
            quarantine_id,
            remote_observation_id: "release-observation".to_owned(),
            observed_at: at("2026-01-01T05:00:00Z"),
            observation,
            evidence_ref: None,
        })
        .await
        .expect("追加补证观察");
    receipt.observation_id
}

async fn insert_source_observation(
    pool: &sqlx::PgPool,
    observation_id: Uuid,
    remote_observation_id: &str,
    digest: &str,
    observed_at: &str,
) {
    sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ($1, '12345', 'main', '555', $2, $3, 'fake-mailbox', NULL, NULL,
                 $4, 'parser-v1', NULL, decode(repeat('72', 32), 'hex'),
                 decode(repeat('73', 32), 'hex'), decode(repeat($5, 32), 'hex'),
                 'FakeSellerOne', 'FakeBot', true, true, 0, 0, 1,
                 '[{\"ordinal\":1,\"kind\":\"equipment\",\"name\":\"Fake Gear\",
                    \"raw_eid\":\"42\",\"canonical_eid\":\"42\",
                    \"protected_ref\":\"83000000-0000-0000-0000-000000000001\"}]'::jsonb,
                 '{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T05:00\",
                   \"items\":[{\"ordinal\":1,\"world\":\"main\",\"tradeable\":true,
                               \"history_ok\":true}]}'::jsonb,
                 NULL)",
    )
    .bind(observation_id)
    .bind(remote_observation_id)
    .bind(Uuid::parse_str(super::support::SESSION).expect("会话"))
    .bind(at(observed_at))
    .bind(digest)
    .execute(pool)
    .await
    .expect("写入源观察");
}

async fn insert_history_blocking_equipment(pool: &sqlx::PgPool) {
    // 同一规范 EID 的历史装备行没有 lot，4B 判定为 HistoryUnproven；不建 lot 可避免
    // 与 seed_base 已有的寄售强占用触发器争用同一实物。
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, 'main', '42', 'Fake Historical Gear', 'Weapon', '{}'::jsonb, $2)",
    )
    .bind(Uuid::new_v4())
    .bind(at("2026-01-01T06:00:00Z"))
    .execute(pool)
    .await
    .expect("写入观察后的历史装备行");
}

/// 模拟重检读取器：远端身份显式给出，用来复现旧读取器按领取换身份的输入。
struct ContentReader {
    observation_id: Option<Uuid>,
    manifest_digest: [u8; 32],
    remote_observation_id: String,
}

impl QuarantineRecheckReader for ContentReader {
    fn reread(
        &self,
        claim: &auction_application::mail::QuarantineRecheckClaim,
        _deadline: std::time::Instant,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        Result<QuarantineReobservation, QuarantineRecheckReadError>,
    > {
        let mut observed = observation(true);
        observed.observation_id = self.observation_id.unwrap_or(claim.previous_observation_id);
        observed.manifest_digest = self.manifest_digest;
        let remote_observation_id = self.remote_observation_id.clone();
        Box::pin(async move {
            Ok(Ok(QuarantineReobservation {
                remote_observation_id,
                observed_at: at("2026-01-01T05:00:00Z"),
                observation: observed,
                evidence_ref: None,
            }))
        })
    }
}

fn recheck_service(
    repository: &auction_storage::DepositRepository,
    reader: std::sync::Arc<dyn QuarantineRecheckReader>,
) -> QuarantineRecheckService {
    let commands = std::sync::Arc::new(DepositCommandService::new(
        std::sync::Arc::new(repository.clone()),
        std::sync::Arc::new(repository.clone()),
        std::sync::Arc::new(repository.clone()),
        std::sync::Arc::new(repository.clone()),
        std::sync::Arc::new(repository.clone()),
    ));
    QuarantineRecheckService::new(commands, std::sync::Arc::new(repository.clone()), reader)
}

async fn round_request(pool: &sqlx::PgPool) -> DispositionRoundRequest {
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时刻");
    DispositionRoundRequest {
        worker_id: "recheck-identity".to_owned(),
        now,
        lease_until: now + auction_time::Duration::hours(1),
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
    }
}

async fn queue_admin_recheck(
    repository: &auction_storage::DepositRepository,
    quarantine_id: Uuid,
    expected_version: u64,
) {
    repository
        .recheck_quarantine(RecheckQuarantineCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            admin_actor: "fake-admin".to_owned(),
            quarantine_id,
            expected_version,
            request_hash: RequestHash::new([0x5b; 32]),
            request_id: None,
        })
        .await
        .expect("排 Admin 重检");
}

/// 收口首次隔离自动排入的系统重检，让 Admin 重检可以排队。
async fn drain_queued_recheck(
    repository: &auction_storage::DepositRepository,
    pool: &sqlx::PgPool,
    quarantine_id: Uuid,
) {
    let request = round_request(pool).await;
    let claim = repository
        .claim_due(request.worker_id, request.now, request.lease_until)
        .await
        .expect("领取自动重检")
        .expect("应有自动重检");
    let receipt = ObservationReceipt::appended(
        CommandId::from_uuid(Uuid::new_v4()),
        quarantine_id,
        claim.quarantine_version,
        claim.previous_observation_id,
        [0x5d; 32],
    );
    repository.complete(&claim, &receipt).await.expect("收口自动重检");
}

#[tokio::test]
async fn postgres_first_observation_replay_keeps_first_quarantine_version() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-first-version-replay")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    assert_eq!(quarantine.version, 1);
    // 首次隔离自动排队，无需再排一条 Admin 重检。
    let _claim = repository
        .claim_due(
            "quarantine-worker".to_owned(),
            at("2099-01-01T00:00:00Z"),
            at("2099-01-02T00:00:00Z"),
        )
        .await
        .expect("领取重检")
        .expect("应有重检工作");
    let first_source = Uuid::new_v4();
    insert_source_observation(pool, first_source, "obs-a", "6a", "2026-01-01T05:00:00Z").await;
    let mut first_observation = observation(true);
    first_observation.observation_id = first_source;
    first_observation.manifest_digest = [0x6a; 32];
    let append_a = repository
        .append_quarantine_observation(AppendQuarantineObservationCommand {
            command_id: worker_command("append-observation-a"),
            request_hash: RequestHash::new([72; 32]),
            quarantine_id: quarantine.quarantine_id,
            remote_observation_id: "obs-a".to_owned(),
            observed_at: at("2026-01-01T05:00:00Z"),
            observation: first_observation.clone(),
            evidence_ref: None,
        })
        .await
        .expect("追加 A");
    assert_eq!(append_a.quarantine_version, 2);
    let second_source = Uuid::new_v4();
    insert_source_observation(pool, second_source, "obs-b", "6b", "2026-01-01T06:00:00Z").await;
    let mut second_observation = observation(true);
    second_observation.observation_id = second_source;
    second_observation.manifest_digest = [0x6b; 32];
    let append_b = repository
        .append_quarantine_observation(AppendQuarantineObservationCommand {
            command_id: worker_command("append-observation-b"),
            request_hash: RequestHash::new([73; 32]),
            quarantine_id: quarantine.quarantine_id,
            remote_observation_id: "obs-b".to_owned(),
            observed_at: at("2026-01-01T06:00:00Z"),
            observation: second_observation,
            evidence_ref: None,
        })
        .await
        .expect("追加 B");
    assert_eq!(append_b.quarantine_version, 3);
    let replay = repository
        .append_quarantine_observation(AppendQuarantineObservationCommand {
            command_id: worker_command("append-observation-a-replay"),
            request_hash: RequestHash::new([74; 32]),
            quarantine_id: quarantine.quarantine_id,
            remote_observation_id: "obs-a".to_owned(),
            observed_at: at("2026-01-01T05:00:00Z"),
            observation: first_observation,
            evidence_ref: None,
        })
        .await
        .expect("以新命令重放 A");
    assert_eq!(replay.quarantine_version, 2);
    assert_eq!(replay.observation_id, append_a.observation_id);
    let version: i64 =
        sqlx::query_scalar("SELECT version FROM public.mail_quarantines WHERE id=$1")
            .bind(quarantine.quarantine_id)
            .fetch_one(pool)
            .await
            .expect("读取隔离版本");
    assert_eq!(version, 3);
}

#[tokio::test]
async fn postgres_recheck_same_remote_content_reuses_observation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("recheck-identity")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let initial_observation: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.mail_envelope_observations WHERE quarantine_id=$1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("隔离内观察");

    // 第一轮：首次隔离自动排入的系统重检。远端身份模拟旧读取器按领取变化。
    let service = recheck_service(
        &repository,
        std::sync::Arc::new(ContentReader {
            observation_id: None,
            manifest_digest: [0x7b; 32],
            remote_observation_id: "legacy-claim-1".to_owned(),
        }),
    );
    let round = service.run_once(round_request(pool).await).await.expect("第一轮重检");
    assert_eq!(
        round,
        QuarantineRecheckRound::Appended {
            quarantine_version: 1,
            observation_id: initial_observation,
        }
    );
    let state: (i64, i64) = sqlx::query_as(
        "SELECT version,
                (SELECT count(*) FROM public.mail_envelope_observations
                 WHERE quarantine_id = q.id)
         FROM public.mail_quarantines AS q WHERE q.id = $1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("第一轮观察事实");
    assert_eq!(state, (1, 1));

    // 第二轮：同样内容、不同领取身份，仍复用同一观察，不增版本。
    queue_admin_recheck(&repository, quarantine.quarantine_id, 1).await;
    let service = recheck_service(
        &repository,
        std::sync::Arc::new(ContentReader {
            observation_id: None,
            manifest_digest: [0x7b; 32],
            remote_observation_id: "legacy-claim-2".to_owned(),
        }),
    );
    let round = service.run_once(round_request(pool).await).await.expect("第二轮重检");
    assert_eq!(
        round,
        QuarantineRecheckRound::Appended {
            quarantine_version: 1,
            observation_id: initial_observation,
        }
    );
    let state: (i64, i64) = sqlx::query_as(
        "SELECT version,
                (SELECT count(*) FROM public.mail_envelope_observations
                 WHERE quarantine_id = q.id)
         FROM public.mail_quarantines AS q WHERE q.id = $1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("第二轮观察事实");
    assert_eq!(state, (1, 1));

    // 第三轮：内容不同才追加一条观察并抬高隔离版本。
    let changed_source = Uuid::new_v4();
    insert_source_observation(pool, changed_source, "obs-changed", "6c", "2026-01-01T06:00:00Z")
        .await;
    queue_admin_recheck(&repository, quarantine.quarantine_id, 1).await;
    let service = recheck_service(
        &repository,
        std::sync::Arc::new(ContentReader {
            observation_id: Some(changed_source),
            manifest_digest: [0x6c; 32],
            remote_observation_id: "legacy-claim-3".to_owned(),
        }),
    );
    let round = service.run_once(round_request(pool).await).await.expect("第三轮重检");
    assert!(matches!(round, QuarantineRecheckRound::Appended { quarantine_version: 2, .. }));
    let state: (i64, i64) = sqlx::query_as(
        "SELECT version,
                (SELECT count(*) FROM public.mail_envelope_observations
                 WHERE quarantine_id = q.id)
         FROM public.mail_quarantines AS q WHERE q.id = $1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("第三轮观察事实");
    assert_eq!(state, (2, 2));
}

#[tokio::test]
async fn postgres_admin_recheck_is_idempotent() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("admin-recheck-idempotent")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    // 首次隔离自动排入的系统重检先收口，Admin 才能排新的重检。
    drain_queued_recheck(&repository, pool, quarantine.quarantine_id).await;

    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let request_hash = RequestHash::new([0x5e; 32]);
    let command = RecheckQuarantineCommand {
        command_id,
        admin_actor: "fake-admin".to_owned(),
        quarantine_id: quarantine.quarantine_id,
        expected_version: 1,
        request_hash,
        request_id: None,
    };
    let queued = repository.recheck_quarantine(command.clone()).await.expect("Admin 重检排队");
    assert_eq!(queued.command_id, command_id);
    assert_eq!(queued.quarantine_id, quarantine.quarantine_id);

    // 同 ID 同请求原样重放，不新增工作。
    let replay = repository.recheck_quarantine(command.clone()).await.expect("同 ID 重放");
    assert_eq!(replay, queued);

    // 同 ID 异请求冲突，结论不覆盖。
    let conflict = repository
        .recheck_quarantine(RecheckQuarantineCommand {
            request_hash: RequestHash::new([0x5f; 32]),
            ..command
        })
        .await;
    assert_eq!(conflict, Err(ConsignmentError::CommandIdConflict));

    let queued_works: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_quarantine_rechecks
         WHERE quarantine_id=$1 AND state='queued'",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取排队重检");
    assert_eq!(queued_works, 1);
}

#[tokio::test]
async fn postgres_release_quarantine_return_commits_one_owner_and_replays() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-for-return-release")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let observation_id =
        append_release_observation(&repository, pool, quarantine.quarantine_id, 1).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let request_hash = RequestHash::new([43; 32]);
    let released = repository
        .release_quarantine(ReleaseQuarantineCommand {
            command_id,
            actor: DepositActor::Admin("fake-admin".to_owned()),
            quarantine_id: quarantine.quarantine_id,
            expected_version: 2,
            observation_id,
            proposed_kind: ReleaseKind::Return,
            request_hash,
            request_id: None,
            auto_binding: None,
        })
        .await
        .expect("解除并整封退回");
    assert_eq!(released.decision_kind, ReleaseKind::Return);
    let replay = repository
        .release_quarantine(ReleaseQuarantineCommand {
            command_id,
            actor: DepositActor::Admin("fake-admin".to_owned()),
            quarantine_id: quarantine.quarantine_id,
            expected_version: 2,
            observation_id,
            proposed_kind: ReleaseKind::Return,
            request_hash,
            request_id: None,
            auto_binding: None,
        })
        .await
        .expect("同命令重放");
    assert_eq!(replay, released);
    let compatible = repository
        .release_quarantine(ReleaseQuarantineCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor: DepositActor::Admin("fake-admin".to_owned()),
            quarantine_id: quarantine.quarantine_id,
            expected_version: 3,
            observation_id,
            proposed_kind: ReleaseKind::Return,
            request_hash: RequestHash::new([44; 32]),
            request_id: None,
            auto_binding: None,
        })
        .await
        .expect("相容重放返回原 owner");
    assert_eq!(compatible.decision_id, released.decision_id);
    let conflict = repository
        .release_quarantine(ReleaseQuarantineCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor: DepositActor::Admin("fake-admin".to_owned()),
            quarantine_id: quarantine.quarantine_id,
            expected_version: 3,
            observation_id,
            proposed_kind: ReleaseKind::ArchiveEmpty,
            request_hash: RequestHash::new([45; 32]),
            request_id: None,
            auto_binding: None,
        })
        .await;
    assert!(conflict.is_err());
    let facts: (String, i64, i64, i64) = sqlx::query_as(
        "SELECT q.state, q.version,
                (SELECT count(*) FROM public.mail_quarantine_releases WHERE quarantine_id=q.id),
                (SELECT count(*) FROM public.mail_disposition_work w
                 JOIN public.mail_final_dispositions d ON d.id=w.decision_id
                 WHERE d.bot_uid=q.bot_uid AND d.world_id=q.world_id
                   AND d.external_mid=q.external_mid AND w.action='return')
         FROM public.mail_quarantines q WHERE q.id=$1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取解除事实");
    assert_eq!(facts, ("released".to_owned(), 3, 1, 1));
}

#[tokio::test]
async fn postgres_release_quarantine_accept_commits_full_envelope() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-for-accept-release")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let observation_id =
        append_release_observation(&repository, pool, quarantine.quarantine_id, 1).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let released = repository
        .release_quarantine(release_request(
            command_id,
            quarantine.quarantine_id,
            ReleaseKind::Accept,
            2,
            observation_id,
            DepositActor::Admin("fake-admin".to_owned()),
            RequestHash::new([60; 32]),
        ))
        .await
        .expect("Admin 解除并整封接受");
    assert_eq!(released.decision_kind, ReleaseKind::Accept);
    assert_eq!(released.listing_changes.len(), 1);
    assert_eq!(
        released.listing_changes[0].state,
        auction_domain::consignment::ListingState::DepositAccepted
    );
    assert_eq!(released.listing_changes[0].listing_version, 4);
    let facts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.consignment_deposit_items WHERE decision_id=$1),
            (SELECT count(*) FROM public.consignment_custody_positions p
             JOIN public.consignment_deposit_items i ON i.id=p.item_id
             WHERE i.decision_id=$1),
            (SELECT count(*) FROM public.consignment_equipment_claims c
             JOIN public.consignment_custody_positions p ON p.id=c.position_id
             JOIN public.consignment_deposit_items i ON i.id=p.item_id
             WHERE i.decision_id=$1),
            (SELECT count(*) FROM public.mail_disposition_work
             WHERE decision_id=$1 AND action='take')",
    )
    .bind(released.decision_id)
    .fetch_one(pool)
    .await
    .expect("读取接受事实");
    assert_eq!(facts, (1, 1, 1, 1));
    let position: (String, i64) = sqlx::query_as(
        "SELECT p.state, p.version FROM public.consignment_custody_positions p
         JOIN public.consignment_deposit_items i ON i.id=p.item_id WHERE i.decision_id=$1",
    )
    .bind(released.decision_id)
    .fetch_one(pool)
    .await
    .expect("读取 position");
    assert_eq!(position, ("awaiting_take".to_owned(), 1));
    let event: (Uuid, String) = sqlx::query_as(
        "SELECT command_id, actor_kind FROM public.consignment_events
         WHERE listing_id=$1 AND state_version=4",
    )
    .bind(Uuid::parse_str(super::support::LISTING).expect("挂单"))
    .fetch_one(pool)
    .await
    .expect("读取接受事件");
    assert_eq!(event, (command_id.as_uuid(), "admin".to_owned()));
    let quarantine_state: (String, i64) =
        sqlx::query_as("SELECT state, version FROM public.mail_quarantines WHERE id=$1")
            .bind(quarantine.quarantine_id)
            .fetch_one(pool)
            .await
            .expect("读取隔离");
    assert_eq!(quarantine_state, ("released".to_owned(), 3));
}

#[tokio::test]
async fn postgres_decide_envelope_rechecks_history_inside_lock() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    // 观察时历史通过；提交决定前出现同一实物的历史行，4B 判定转为 HistoryUnproven。
    insert_history_blocking_equipment(pool).await;
    let error = repository
        .decide_envelope(super::envelopes::accepted_command(worker_command(
            "decide-history-changed",
        )))
        .await
        .expect_err("锁内历史不通过时不得按观察时的接受决定提交");
    assert_eq!(error, ConsignmentError::Deposit(DepositBusinessRejection::DispositionConflict));
    let facts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.mail_initial_decisions),
            (SELECT count(*) FROM public.mail_final_dispositions),
            (SELECT count(*) FROM public.consignment_deposit_items),
            (SELECT count(*) FROM public.consignment_custody_positions),
            (SELECT count(*) FROM public.mail_disposition_work)",
    )
    .fetch_one(pool)
    .await
    .expect("读取零事实");
    assert_eq!(facts, (0, 0, 0, 0, 0));
    let listing: (String, i64) =
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1")
            .bind(Uuid::parse_str(super::support::LISTING).expect("挂单"))
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!(listing, ("awaiting_deposit".to_owned(), 3));
}

#[tokio::test]
async fn postgres_release_accept_rechecks_history_inside_lock() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-for-history-release")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let observation_id =
        append_release_observation(&repository, pool, quarantine.quarantine_id, 1).await;
    // 补证观察仍记录 history_ok=true；历史实际在同实物上变为不通过。
    insert_history_blocking_equipment(pool).await;
    let error = repository
        .release_quarantine(release_request(
            CommandId::from_uuid(Uuid::new_v4()),
            quarantine.quarantine_id,
            ReleaseKind::Accept,
            2,
            observation_id,
            DepositActor::Admin("fake-admin".to_owned()),
            RequestHash::new([61; 32]),
        ))
        .await
        .expect_err("锁内历史回退时不得接受解除");
    assert_eq!(error, ConsignmentError::Deposit(DepositBusinessRejection::DispositionConflict));
    let facts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.mail_final_dispositions),
            (SELECT count(*) FROM public.mail_quarantine_releases),
            (SELECT count(*) FROM public.consignment_deposit_items),
            (SELECT count(*) FROM public.consignment_custody_positions),
            (SELECT count(*) FROM public.mail_disposition_work)",
    )
    .fetch_one(pool)
    .await
    .expect("读取零事实");
    assert_eq!(facts, (0, 0, 0, 0, 0));
    let quarantine_state: (String, i64) =
        sqlx::query_as("SELECT state, version FROM public.mail_quarantines WHERE id=$1")
            .bind(quarantine.quarantine_id)
            .fetch_one(pool)
            .await
            .expect("读取隔离");
    assert_eq!(quarantine_state, ("active".to_owned(), 2));
    let listing: (String, i64) =
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1")
            .bind(Uuid::parse_str(super::support::LISTING).expect("挂单"))
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!(listing, ("awaiting_deposit".to_owned(), 3));
}

#[tokio::test]
async fn postgres_release_quarantine_archive_empty_uses_observation_asset_count() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-for-empty-release")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let observation_id =
        append_release_observation_with(&repository, pool, quarantine.quarantine_id, 1, true).await;
    let released = repository
        .release_quarantine(release_request(
            CommandId::from_uuid(Uuid::new_v4()),
            quarantine.quarantine_id,
            ReleaseKind::ArchiveEmpty,
            2,
            observation_id,
            DepositActor::Admin("fake-admin".to_owned()),
            RequestHash::new([75; 32]),
        ))
        .await
        .expect("Admin 解除空信");
    assert_eq!(released.decision_kind, ReleaseKind::ArchiveEmpty);
    assert!(released.listing_changes.is_empty());
    let facts: (String, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT kind FROM public.mail_final_dispositions WHERE id=$1),
            (SELECT count(*) FROM public.mail_disposition_work WHERE decision_id=$1),
            (SELECT count(*) FROM public.consignment_deposit_items WHERE decision_id=$1),
            (SELECT count(*) FROM public.mail_disposition_proofs WHERE decision_id=$1)",
    )
    .bind(released.decision_id)
    .fetch_one(pool)
    .await
    .expect("读取空信解除事实");
    assert_eq!(facts, ("archived_empty".to_owned(), 0, 0, 0));
    let quarantine_state: (String, i64) =
        sqlx::query_as("SELECT state, version FROM public.mail_quarantines WHERE id=$1")
            .bind(quarantine.quarantine_id)
            .fetch_one(pool)
            .await
            .expect("读取隔离");
    assert_eq!(quarantine_state, ("released".to_owned(), 3));
}

#[tokio::test]
async fn postgres_quarantine_read_reports_total_release_and_fails_closed() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-for-read")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let observation_id =
        append_release_observation(&repository, pool, quarantine.quarantine_id, 1).await;
    let detail = repository
        .quarantine(AdminDepositQuery {
            quarantine_id: quarantine.quarantine_id,
            cursor: None,
            limit: 10,
        })
        .await
        .expect("读取隔离详情")
        .expect("隔离存在");
    assert_eq!(detail.state, QuarantineState::Active);
    assert_eq!(detail.observation_total, 2);
    assert_eq!(detail.observations.len(), 2);
    assert!(detail.allowed_actions.contains(&AdminDepositAction::Recheck));
    assert!(detail.allowed_actions.contains(&AdminDepositAction::ReleaseQuarantine));
    assert!(detail.final_decision.is_none());
    let first_page = repository
        .quarantine(AdminDepositQuery {
            quarantine_id: quarantine.quarantine_id,
            cursor: None,
            limit: 1,
        })
        .await
        .expect("读取第一页")
        .expect("隔离存在");
    assert_eq!(first_page.observation_total, 2);
    assert_eq!(first_page.observations.len(), 1);
    let second_page = repository
        .quarantine(AdminDepositQuery {
            quarantine_id: quarantine.quarantine_id,
            cursor: Some(first_page.observations[0].observation_id),
            limit: 1,
        })
        .await
        .expect("读取第二页")
        .expect("隔离存在");
    assert_eq!(second_page.observations.len(), 1);
    assert_ne!(
        second_page.observations[0].observation_id,
        first_page.observations[0].observation_id
    );
    repository
        .release_quarantine(ReleaseQuarantineCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor: DepositActor::Admin("fake-admin".to_owned()),
            quarantine_id: quarantine.quarantine_id,
            expected_version: 2,
            observation_id,
            proposed_kind: ReleaseKind::Return,
            request_hash: RequestHash::new([51; 32]),
            request_id: None,
            auto_binding: None,
        })
        .await
        .expect("解除");
    let released = repository
        .quarantine(AdminDepositQuery {
            quarantine_id: quarantine.quarantine_id,
            cursor: None,
            limit: 10,
        })
        .await
        .expect("读取已解除详情")
        .expect("隔离存在");
    assert_eq!(released.state, QuarantineState::Released);
    assert!(released.allowed_actions.is_empty());
    assert_eq!(
        released.final_decision.as_ref().map(|decision| decision.kind),
        Some(ReleaseKind::Return)
    );
    assert_eq!(released.observation_total, 2);
    // 非法游标不能被静默当成空页或补造字段。
    assert!(
        repository
            .quarantine(AdminDepositQuery {
                quarantine_id: quarantine.quarantine_id,
                cursor: Some(Uuid::new_v4()),
                limit: 10,
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn postgres_append_without_active_recheck_lease_is_rejected() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-without-lease")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let error = repository
        .append_quarantine_observation(AppendQuarantineObservationCommand {
            command_id: worker_command("append-without-lease"),
            request_hash: RequestHash::new([52; 32]),
            quarantine_id: quarantine.quarantine_id,
            remote_observation_id: "no-lease".to_owned(),
            observed_at: at("2026-01-01T06:00:00Z"),
            observation: observation(true),
            evidence_ref: None,
        })
        .await
        .expect_err("没有在租重检工作时不能追加");
    assert_eq!(error, auction_application::consignment::ConsignmentError::LeaseLost);
    let facts: (i64, i64) = sqlx::query_as(
        "SELECT q.version,
                (SELECT count(*) FROM public.mail_envelope_observations
                 WHERE quarantine_id=q.id)
         FROM public.mail_quarantines AS q WHERE q.id=$1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取零追加事实");
    assert_eq!(facts, (1, 1));
}

#[tokio::test]
async fn postgres_observation_conflict_keeps_quarantine_version() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-conflicting-observation")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    append_release_observation(&repository, pool, quarantine.quarantine_id, 1).await;
    let mut conflicting = observation(true);
    conflicting.manifest_digest = [0x6b; 32];
    conflicting.parser_version = "parser-conflict".to_owned();
    let error = repository
        .append_quarantine_observation(AppendQuarantineObservationCommand {
            command_id: worker_command("append-conflicting-observation"),
            request_hash: RequestHash::new([59; 32]),
            quarantine_id: quarantine.quarantine_id,
            remote_observation_id: "release-observation".to_owned(),
            observed_at: at("2026-01-01T07:00:00Z"),
            observation: conflicting,
            evidence_ref: None,
        })
        .await
        .expect_err("同观察身份不同摘要必须拒绝");
    assert_eq!(
        error,
        auction_application::consignment::ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::EvidenceConflict
        )
    );
    let facts: (i64, i64, i64) = sqlx::query_as(
        "SELECT q.version,
                (SELECT count(*) FROM public.mail_envelope_observations
                 WHERE quarantine_id=q.id),
                (SELECT count(*) FROM public.mail_observation_conflicts)
         FROM public.mail_quarantines AS q WHERE q.id=$1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取冲突事实");
    assert_eq!(facts, (2, 2, 1));
}

#[tokio::test]
async fn postgres_recheck_stale_version_is_rejected_without_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-stale-recheck")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let error = repository
        .recheck_quarantine(auction_application::consignment::RecheckQuarantineCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            admin_actor: "fake-admin".to_owned(),
            quarantine_id: quarantine.quarantine_id,
            expected_version: 2,
            request_hash: RequestHash::new([53; 32]),
            request_id: None,
        })
        .await
        .expect_err("过期版本必须拒绝");
    assert_eq!(
        error,
        auction_application::consignment::ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::StaleVersion
        )
    );
    let works: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_quarantine_rechecks WHERE quarantine_id=$1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取重检数");
    // 首次隔离自动排入的一条仍存在；过期 Admin 命令本身不得新增工作。
    assert_eq!(works, 1);
}

#[tokio::test]
async fn postgres_recheck_claim_retry_and_expired_lease_takeover() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-recheck-lease")))
        .await
        .expect("首次隔离");
    assert!(first.quarantine.is_some());
    // 首次隔离自动排队，无需再排一条 Admin 重检。
    let db_now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时刻");
    let first_claim = repository
        .claim_due("recheck-a".to_owned(), db_now, db_now + auction_time::Duration::hours(1))
        .await
        .expect("首次领取")
        .expect("应有重检");
    assert_eq!(first_claim.attempt_count, 1);
    assert_eq!(first_claim.fence, 1);
    repository
        .release_for_retry(
            &first_claim,
            DispositionRetryReason::TemporaryUnavailable,
            db_now + auction_time::Duration::hours(1),
        )
        .await
        .expect("释放重试");
    assert!(
        repository
            .claim_due(
                "recheck-b".to_owned(),
                db_now + auction_time::Duration::hours(2),
                db_now + auction_time::Duration::hours(3),
            )
            .await
            .expect("提前领取")
            .is_none()
    );
    sqlx::query(
        "UPDATE public.mail_quarantine_rechecks
         SET available_at = clock_timestamp() - interval '1 second'
         WHERE id = $1",
    )
    .bind(first_claim.work_id)
    .execute(pool)
    .await
    .expect("使重检 retry_at 到期");
    let second_claim = repository
        .claim_due("recheck-b".to_owned(), db_now - auction_time::Duration::hours(1), db_now)
        .await
        .expect("重试领取")
        .expect("retry_at 后可领取");
    assert_eq!(second_claim.attempt_count, 2);
    assert_eq!(second_claim.fence, 2);
    assert!(repository.check_current(&first_claim).await.is_err());
    sqlx::query(
        "UPDATE public.mail_quarantine_rechecks
         SET lease_until = clock_timestamp() - interval '1 minute'
         WHERE id = $1",
    )
    .bind(second_claim.work_id)
    .execute(pool)
    .await
    .expect("模拟过租");
    assert!(repository.check_current(&second_claim).await.is_err());
    let third_claim = repository
        .claim_due("recheck-c".to_owned(), db_now - auction_time::Duration::hours(1), db_now)
        .await
        .expect("过租领取")
        .expect("过租工作应可被他人领取");
    assert_eq!(third_claim.attempt_count, 3);
    assert_eq!(third_claim.fence, 3);
    assert!(repository.check_current(&second_claim).await.is_err());
    repository.check_current(&third_claim).await.expect("新租约有效");
}

#[tokio::test]
async fn postgres_release_same_command_with_other_request_is_rejected() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-command-conflict")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let observation_id =
        append_release_observation(&repository, pool, quarantine.quarantine_id, 1).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());
    repository
        .release_quarantine(release_request(
            command_id,
            quarantine.quarantine_id,
            ReleaseKind::Return,
            2,
            observation_id,
            DepositActor::Admin("fake-admin".to_owned()),
            RequestHash::new([54; 32]),
        ))
        .await
        .expect("首次解除");
    let error = repository
        .release_quarantine(release_request(
            command_id,
            quarantine.quarantine_id,
            ReleaseKind::Return,
            2,
            observation_id,
            DepositActor::Admin("fake-admin".to_owned()),
            RequestHash::new([55; 32]),
        ))
        .await
        .expect_err("同命令异请求必须冲突");
    assert_eq!(error, auction_application::consignment::ConsignmentError::CommandIdConflict);
    let releases: i64 = sqlx::query_scalar("SELECT count(*) FROM public.mail_quarantine_releases")
        .fetch_one(pool)
        .await
        .expect("读取解除数");
    assert_eq!(releases, 1);
}

#[tokio::test]
async fn postgres_concurrent_admin_and_system_release_has_one_final() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-release-race")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let observation_id =
        append_release_observation(&repository, pool, quarantine.quarantine_id, 1).await;
    let admin = repository.release_quarantine(release_request(
        CommandId::from_uuid(Uuid::new_v4()),
        quarantine.quarantine_id,
        ReleaseKind::Return,
        2,
        observation_id,
        DepositActor::Admin("fake-admin".to_owned()),
        RequestHash::new([56; 32]),
    ));
    let system = repository.release_quarantine(release_request(
        worker_command("automatic-release-race"),
        quarantine.quarantine_id,
        ReleaseKind::ArchiveEmpty,
        2,
        observation_id,
        DepositActor::System,
        RequestHash::new([57; 32]),
    ));
    let (admin, system) = tokio::join!(admin, system);
    let loser = match (admin, system) {
        (Ok(_), Err(error)) | (Err(error), Ok(_)) => error,
        (Ok(_), Ok(_)) => panic!("同一 K 不能产生两个解除"),
        (Err(admin), Err(system)) => panic!("两个解除都失败：{admin:?} / {system:?}"),
    };
    assert!(matches!(
        loser,
        auction_application::consignment::ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::DispositionConflict
                | auction_application::consignment::DepositBusinessRejection::EvidenceUnproven
        )
    ));
    let facts: (i64, i64, i64, String) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.mail_quarantine_releases WHERE quarantine_id=$1),
            (SELECT count(*) FROM public.mail_final_dispositions),
            (SELECT count(*) FROM public.mail_disposition_work),
            (SELECT state FROM public.mail_quarantines WHERE id=$1)",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取并发解除事实");
    assert_eq!(facts.0, 1);
    assert_eq!(facts.1, 1);
    assert_eq!(facts.2, 1);
    assert_eq!(facts.3, "released");
}

#[tokio::test]
async fn postgres_quarantine_read_missing_release_fails_closed() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    let owner = database.maintenance_store().await.expect("维护账号");
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-read-missing-release")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    sqlx::query(
        "UPDATE public.mail_quarantines
         SET state = 'released', version = version + 1, updated_at = clock_timestamp()
         WHERE id = $1",
    )
    .bind(quarantine.quarantine_id)
    .execute(owner.postgres_pool())
    .await
    .expect("构造缺少解除行的已解除隔离");
    let error = repository
        .quarantine(AdminDepositQuery {
            quarantine_id: quarantine.quarantine_id,
            cursor: None,
            limit: 10,
        })
        .await
        .expect_err("已解除但缺 release_id 必须失败关闭");
    assert_eq!(error, auction_application::consignment::ConsignmentError::Unavailable);
}

#[tokio::test]
async fn postgres_quarantine_prefix_advances_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    let token = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.mail_poll_state
            (mailbox_key, cursor_value, last_poll_at, next_poll_at, consecutive_failures,
             paused, state_version, updated_at, claim_token, claim_owner, claim_expires_at)
         VALUES ('prefix-mailbox', '10', clock_timestamp(), clock_timestamp(), 0, false, 1,
                 clock_timestamp(), $1, 'worker-a', clock_timestamp() + interval '1 hour')",
    )
    .bind(token)
    .execute(pool)
    .await
    .expect("写入轮询租约");
    let mut first = pool.acquire().await.expect("第一连接");
    let mut second = pool.acquire().await.expect("第二连接");
    let repository = auction_storage::DepositRepository::new(
        runtime.clone(),
        std::sync::Arc::new(super::envelopes::NoCrypto),
        auction_storage::DepositSettings::new(
            auction_domain::consignment::FeeBps::new(500).expect("费率"),
            auction_domain::consignment::ListingPrice::new(1).expect("下限"),
            auction_domain::consignment::ListingPrice::new(1_000_000_000).expect("上限"),
            24,
            100,
        ),
    );
    let mut first_tx = first.begin().await.expect("第一事务");
    repository
        .test_advance_prefix(
            &mut first_tx,
            "prefix-mailbox",
            token,
            "worker-a",
            1,
            Some("10"),
            "11",
            at("2026-11-22T00:00:00Z"),
            at("2026-11-22T01:00:00Z"),
        )
        .await
        .expect("首次推进");
    let mut second_tx = second.begin().await.expect("第二事务");
    let (commit, blocked) = tokio::join!(
        first_tx.commit(),
        repository.test_advance_prefix(
            &mut second_tx,
            "prefix-mailbox",
            token,
            "worker-a",
            1,
            Some("10"),
            "11",
            at("2026-11-22T00:00:00Z"),
            at("2026-11-22T01:00:00Z"),
        )
    );
    commit.expect("提交首次推进");
    assert!(blocked.is_err());
    second_tx.rollback().await.expect("回滚第二事务");
    let row = sqlx::query("SELECT cursor_value, state_version FROM public.mail_poll_state WHERE mailbox_key='prefix-mailbox'")
        .fetch_one(pool)
        .await
        .expect("读取前缀");
    assert_eq!(row.get::<String, _>("cursor_value"), "11");
    assert_eq!(row.get::<i64, _>("state_version"), 2);
    let mut third = pool.acquire().await.expect("第三连接");
    let mut third_tx = third.begin().await.expect("第三事务");
    let rollback = repository
        .test_advance_prefix(
            &mut third_tx,
            "prefix-mailbox",
            token,
            "worker-a",
            2,
            Some("11"),
            "10",
            at("2026-11-22T00:00:00Z"),
            at("2026-11-22T01:00:00Z"),
        )
        .await;
    assert!(rollback.is_err());
    third_tx.rollback().await.expect("回滚倒退尝试");
    let cursor: String = sqlx::query_scalar(
        "SELECT cursor_value FROM public.mail_poll_state WHERE mailbox_key='prefix-mailbox'",
    )
    .fetch_one(pool)
    .await
    .expect("读取最终前缀");
    assert_eq!(cursor, "11");
}

#[tokio::test]
async fn postgres_quarantine_prefix_version_guard_rejects_stale_claim() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    let token = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.mail_poll_state
            (mailbox_key, cursor_value, last_poll_at, next_poll_at, consecutive_failures,
             paused, state_version, updated_at, claim_token, claim_owner, claim_expires_at)
         VALUES ('prefix-version-mailbox', '10', clock_timestamp(), clock_timestamp(), 0, false,
                 1, clock_timestamp(), $1, 'worker-a', clock_timestamp() + interval '1 hour')",
    )
    .bind(token)
    .execute(pool)
    .await
    .expect("写入轮询租约");
    sqlx::query(
        "UPDATE public.mail_poll_state
         SET state_version = 2, updated_at = clock_timestamp()
         WHERE mailbox_key = 'prefix-version-mailbox'",
    )
    .execute(pool)
    .await
    .expect("只递增版本、不改游标");
    let repository = auction_storage::DepositRepository::new(
        runtime.clone(),
        std::sync::Arc::new(super::envelopes::NoCrypto),
        auction_storage::DepositSettings::new(
            auction_domain::consignment::FeeBps::new(500).expect("费率"),
            auction_domain::consignment::ListingPrice::new(1).expect("下限"),
            auction_domain::consignment::ListingPrice::new(1_000_000_000).expect("上限"),
            24,
            100,
        ),
    );
    let mut connection = pool.acquire().await.expect("连接");
    let mut transaction = connection.begin().await.expect("事务");
    let result = repository
        .test_advance_prefix(
            &mut transaction,
            "prefix-version-mailbox",
            token,
            "worker-a",
            1,
            Some("10"),
            "11",
            at("2026-11-22T00:00:00Z"),
            at("2026-11-22T01:00:00Z"),
        )
        .await;
    assert_eq!(result.expect_err("旧 state_version 不能推进前缀"), "LeaseLost");
    transaction.rollback().await.expect("回滚");
    let row: (String, i64) = sqlx::query_as(
        "SELECT cursor_value, state_version FROM public.mail_poll_state
         WHERE mailbox_key = 'prefix-version-mailbox'",
    )
    .fetch_one(pool)
    .await
    .expect("读取前缀");
    assert_eq!(row, ("10".to_owned(), 2));
}

#[tokio::test]
async fn postgres_phase4_quarantine_cannot_acquire_without_real_round() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let receipt = repository
        .decide_envelope(quarantine_command(worker_command("quarantine-for-acquire")))
        .await
        .expect("首次隔离");
    let quarantine = receipt.quarantine.expect("隔离引用");
    let observation_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.mail_envelope_observations WHERE quarantine_id=$1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取隔离内观察");
    let error = repository
        .release_quarantine(ReleaseQuarantineCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor: DepositActor::Admin("fake-admin".to_owned()),
            quarantine_id: quarantine.quarantine_id,
            expected_version: 1,
            observation_id,
            proposed_kind: ReleaseKind::AcquireReturn,
            request_hash: RequestHash::new([21; 32]),
            request_id: None,
            auto_binding: None,
        })
        .await
        .expect_err("第 4 期不能成功接管");
    assert_eq!(
        error,
        auction_application::consignment::ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::EvidenceUnproven
        )
    );
    let state: (String, i64) =
        sqlx::query_as("SELECT state, version FROM public.mail_quarantines WHERE id=$1")
            .bind(quarantine.quarantine_id)
            .fetch_one(pool)
            .await
            .expect("读取隔离");
    assert_eq!(state, ("active".to_owned(), 1));
    let releases: i64 = sqlx::query_scalar("SELECT count(*) FROM public.mail_quarantine_releases")
        .fetch_one(pool)
        .await
        .expect("读取解除数");
    assert_eq!(releases, 0);
}

async fn release_legacy(
    repository: &auction_storage::DepositRepository,
    owner_pool: &sqlx::PgPool,
    owner_kind: &str,
) -> (Uuid, Uuid) {
    let receipt = repository
        .decide_envelope(quarantine_command(worker_command(&format!(
            "quarantine-legacy-{owner_kind}"
        ))))
        .await
        .expect("首次隔离");
    let quarantine = receipt.quarantine.expect("隔离引用");
    let observation_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.mail_envelope_observations WHERE quarantine_id=$1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(owner_pool)
    .await
    .expect("读取隔离内观察");
    let locator = "{\"v\":2,\"b\":\"fake-mailbox\"}";
    let advanced_observation: Uuid = sqlx::query_scalar(
        "WITH advanced AS (
             UPDATE public.mail_quarantines
             SET reread_locator = $2, version = version + 1, updated_at = clock_timestamp()
             WHERE id = $1
             RETURNING id, version
         )
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         SELECT $3, o.bot_uid, o.world_id, o.external_mid, $4, o.session_id, o.mailbox_key,
                advanced.id, advanced.version, o.observed_at, o.parser_version,
                o.evidence_ref, o.subject_digest, o.body_digest, o.manifest_digest,
                o.sender, o.recipient, o.returnable, o.takeable, o.credits, o.cod,
                o.raw_count, o.manifest,
                jsonb_set(o.derived_facts, '{legacy_auction_owner}', to_jsonb($6::text), false),
                o.season_window_id
         FROM public.mail_envelope_observations AS o, advanced
         WHERE o.id = $5
         RETURNING id",
    )
    .bind(quarantine.quarantine_id)
    .bind(locator)
    .bind(Uuid::new_v4())
    .bind(format!("legacy-{}", Uuid::new_v4()))
    .bind(observation_id)
    .bind(owner_kind)
    .fetch_one(owner_pool)
    .await
    .expect("更新隔离来源");
    (quarantine.quarantine_id, advanced_observation)
}

#[tokio::test]
async fn postgres_old_owner_handoffs_fail_closed_and_keep_quarantine() {
    for (owner_kind, kind) in [
        ("buyer_refund", ReleaseKind::AuctionReturnCase),
        ("winner_equipment_cod", ReleaseKind::AuctionRecoveryReturn),
    ] {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let runtime = database.store().await.expect("运行账号");
        let pool = runtime.postgres_pool();
        let owner = database.maintenance_store().await.expect("维护账号");
        seed_base(pool).await;
        let repository = repository(&database).await;
        let (quarantine_id, observation_id) =
            release_legacy(&repository, owner.postgres_pool(), owner_kind).await;
        let error = repository
            .release_quarantine(ReleaseQuarantineCommand {
                command_id: CommandId::from_uuid(Uuid::new_v4()),
                actor: DepositActor::Admin("fake-admin".to_owned()),
                quarantine_id,
                expected_version: 2,
                observation_id,
                proposed_kind: kind,
                request_hash: RequestHash::new([22; 32]),
                request_id: None,
                auto_binding: None,
            })
            .await
            .expect_err("旧 owner 写端未接入必须失败关闭");
        assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
        let state: (String, i64) =
            sqlx::query_as("SELECT state, version FROM public.mail_quarantines WHERE id=$1")
                .bind(quarantine_id)
                .fetch_one(pool)
                .await
                .expect("读取隔离");
        assert_eq!(state, ("active".to_owned(), 2));
        let facts: (i64, i64, i64) = sqlx::query_as(
            "SELECT
                (SELECT count(*) FROM public.mail_quarantine_releases WHERE quarantine_id=$1),
                (SELECT count(*) FROM public.mail_final_dispositions),
                (SELECT count(*) FROM public.mail_disposition_work)",
        )
        .bind(quarantine_id)
        .fetch_one(pool)
        .await
        .expect("读取零事实");
        assert_eq!(facts, (0, 0, 0));
    }
}
