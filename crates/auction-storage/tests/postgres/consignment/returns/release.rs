//! 隔离解除为 `acquire_return`：与直接接管同一个写端、同一事务；来源读不出就继续隔离。

use std::sync::Arc;

use auction_application::consignment::{
    AcquireReceipt, ConsignmentError, DecideEnvelopeCommand, DepositActor,
    DepositBusinessRejection, EnvelopeStore, ReceiptDisposition, ReleaseQuarantineCommand,
    ReleaseReceipt,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeDecision, EnvelopeObservation, LockedEnvelopeFacts,
    QuarantineReason, ReleaseKind, SenderIdentity,
};
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::envelopes::repository;
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::REFUND_ROUND;
use super::fixture::{
    MANIFEST, Rts, acquire, quarantine_of, quarantined_rts_sql, seed_refund_rts, service,
};
use crate::support::TestDatabase;

fn release(rts: Rts, byte: u8) -> ReleaseQuarantineCommand {
    ReleaseQuarantineCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        actor: DepositActor::Admin("fake-admin".to_owned()),
        quarantine_id: quarantine_of(rts),
        expected_version: 2,
        observation_id: rts.observation(),
        proposed_kind: ReleaseKind::AcquireReturn,
        request_hash: RequestHash::new([byte; 32]),
        request_id: None,
        auto_binding: None,
    }
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
}

#[tokio::test]
async fn postgres_consignment_returns_quarantine_release_acquires_through_the_same_write_end() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = database.store().await.expect("运行账号");
    let pool = owner.postgres_pool();
    seed_refund_rts(pool, &store, Rts { n: 1 }).await;
    let (good, unknown) = (Rts { n: 5 }, Rts { n: 6 });
    succeeds(pool, &quarantined_rts_sql(good, REFUND_ROUND)).await;
    succeeds(pool, &quarantined_rts_sql(unknown, &Uuid::from_u128(9).to_string())).await;
    let repository = repository(&database).await;

    // 反例：RTS 正文的派发引用指不到任何持久的轮，来源不明，继续隔离，零接管行、零解除行。
    let error = repository.release_quarantine(release(unknown, 31)).await.expect_err("不得放行");
    assert_eq!(error, ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_return_intakes").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);

    // 正例：引用指认原轮时，解除、终局、intake 与唯一 Take 工作同事务提交。
    let receipt = repository.release_quarantine(release(good, 32)).await.expect("解除并接管");
    assert_eq!(receipt.decision_kind, ReleaseKind::AcquireReturn);
    let intake = receipt.return_intake_id.expect("回执带 intake");
    let (work, round): (Uuid, i32) = sqlx::query_as(
        "SELECT take_work_id, original_round FROM public.consignment_return_intakes WHERE id = $1",
    )
    .bind(intake)
    .fetch_one(pool)
    .await
    .expect("读取 intake");
    assert_eq!(
        (receipt.take_work_id, receipt.original_round),
        (Some(work), u32::try_from(round).ok())
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_return_intakes").await, 1);
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.mail_quarantine_releases WHERE kind = 'acquire_return'"
        )
        .await,
        1
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.mail_quarantines WHERE state = 'released'").await,
        1
    );
}

/// 隔离时的 RTS 观察事实；首回执重放只按命令账本命中，事实仅需自洽。
fn rts_observation(rts: Rts) -> EnvelopeObservation {
    EnvelopeObservation {
        observation_id: rts.observation(),
        session_binding_digest: [0x33; 32],
        manifest_digest: MANIFEST,
        parser_version: "fake-rts-parser".to_owned(),
        subject_is_consign: true,
        sender_identity: SenderIdentity::Verified,
        list_detail_consistent: true,
        asset_count: Some(0),
        duplicate_raw_eid: false,
        canonical_alias_ambiguous: false,
        unknown_attachment: false,
        credits: Some(0),
        cod: Some(0),
        returnable: true,
        takeable: Some(true),
        system_rts: true,
        legacy_auction_owner: None,
        items: Vec::new(),
    }
}

/// 旧拍卖案件与预约消费侧的快照：旧案件证据 / 案件行数、预约总数与非 active 预约数（消费 / 退役）、寄存件行数。
/// 夹具的购买前置会自带一条预约，故按前后快照相等断言“零消费”，不写绝对零。
async fn old_owner_snapshot(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.settlement_return_evidence),
                (SELECT count(*) FROM public.settlement_return_cases),
                (SELECT count(*) FROM public.consignment_deposit_reservations),
                (SELECT count(*) FROM public.consignment_deposit_reservations
                 WHERE state <> 'active'),
                (SELECT count(*) FROM public.consignment_deposit_items)",
    )
    .fetch_one(pool)
    .await
    .expect("读取旧 owner 快照")
}

/// 自动复判：系统 actor 的解除命令，命令 ID 取 UUIDv5（账本约束）。
fn automatic_release(rts: Rts, byte: u8) -> ReleaseQuarantineCommand {
    let mut command = release(rts, byte);
    command.command_id =
        CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, b"pit2-b8-automatic-release"));
    command.actor = DepositActor::System;
    command
}

/// 对序列化冲突 / 死锁（应用层归为 Unavailable）有界重试，与生产的有界重试同形；其余错误原样返回。
async fn release_retry(
    repository: &auction_storage::DepositRepository,
    command: ReleaseQuarantineCommand,
) -> Result<ReleaseReceipt, ConsignmentError> {
    let mut last = ConsignmentError::Unavailable;
    for _ in 0..4 {
        match repository.release_quarantine(command.clone()).await {
            Err(ConsignmentError::Unavailable) => {
                last = ConsignmentError::Unavailable;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            other => return other,
        }
    }
    Err(last)
}

async fn acquire_retry(
    store: &Arc<auction_storage::DatabaseStore>,
    rts: Rts,
) -> Result<AcquireReceipt, ConsignmentError> {
    let service = service(store);
    let mut last = ConsignmentError::Unavailable;
    for _ in 0..4 {
        match acquire(&service, rts).await {
            Err(ConsignmentError::Unavailable) => {
                last = ConsignmentError::Unavailable;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            other => return other,
        }
    }
    Err(last)
}

/// 同 K 的终局、intake、Take 工作与接管命令计数：(终局, intake, 工作)。
async fn terminal_counts(pool: &PgPool, rts: Rts) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.mail_final_dispositions
                 WHERE bot_uid = '12345' AND world_id = 'main' AND external_mid = $1),
                (SELECT count(*) FROM public.consignment_return_intakes
                 WHERE bot_uid = '12345' AND world_id = 'main' AND external_mid = $1),
                (SELECT count(*) FROM public.mail_disposition_work
                 WHERE bot_uid = '12345' AND world_id = 'main' AND external_mid = $1)",
    )
    .bind(rts.mid())
    .fetch_one(pool)
    .await
    .expect("读取终局计数")
}

/// 保护不变量 2、3：同一原信键 K 只有一个终局。Admin 解除、自动复判与直接接管三方并发，
/// 恰一个终局、一个 intake、一个 Take 工作；胜者之外要么幂等回到同一结果、要么以冲突失败，不产生第二个终局。
/// 寄售退款原轮不进旧拍卖案件，也不消费预约。
#[tokio::test]
async fn postgres_consignment_returns_release_auto_and_direct_acquire_race_has_one_terminal() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    let rts = Rts { n: 5 };
    seed_refund_rts(pool, &store, Rts { n: 1 }).await;
    succeeds(pool, &quarantined_rts_sql(rts, REFUND_ROUND)).await;
    let repository = repository(&database).await;
    let old_owner_before = old_owner_snapshot(pool).await;
    assert_eq!(&old_owner_before.0, &0);
    assert_eq!(&old_owner_before.1, &0);

    let (admin, automatic, direct) = tokio::join!(
        release_retry(&repository, release(rts, 41)),
        release_retry(&repository, automatic_release(rts, 42)),
        acquire_retry(&store, rts),
    );

    // 至少有一方成功；失败方只能是处置冲突 / 证据不成立这类业务拒绝，不能是未归类的失败。
    for result in [admin.as_ref().err(), automatic.as_ref().err(), direct.as_ref().err()]
        .into_iter()
        .flatten()
    {
        assert!(
            matches!(
                result,
                ConsignmentError::Deposit(
                    DepositBusinessRejection::DispositionConflict
                        | DepositBusinessRejection::EvidenceUnproven
                )
            ),
            "败者必须是业务冲突：{result:?}"
        );
    }
    assert!(admin.is_ok() || automatic.is_ok() || direct.is_ok(), "三方不得全部失败");

    // 恰一个终局、一个 intake、一个 Take 工作。
    assert_eq!(terminal_counts(pool, rts).await, (1, 1, 1));
    let (intake, work, decision): (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT id, take_work_id, decision_id FROM public.consignment_return_intakes",
    )
    .fetch_one(pool)
    .await
    .expect("读取 intake");
    // 所有成功方指向同一个终局；解除回执的 intake 字段另见 loser 回执用例。
    for receipt in [admin, automatic].into_iter().flatten() {
        assert_eq!(receipt.decision_id, decision);
    }
    if let Ok(receipt) = direct {
        assert_eq!(
            (receipt.intake_id, receipt.work_id, receipt.decision_id),
            (intake, work, decision)
        );
    }
    assert!(
        count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await <= 1,
        "同一隔离至多一条解除行"
    );
    assert_eq!(old_owner_snapshot(pool).await, old_owner_before);

    // 反例：竞争结束后换一个新命令再解除，不得产生第二个终局、intake 或工作。
    drop(repository.release_quarantine(release(rts, 43)).await);
    assert_eq!(terminal_counts(pool, rts).await, (1, 1, 1));
    assert_eq!(old_owner_snapshot(pool).await, old_owner_before);
}

/// 保护不变量 1：解除重放（同命令 ID 同请求）原样返回同一 intake / Take 工作，零新增行；
/// 首次隔离回执重放仍是隔离，不因后续的解除而变成接管。
#[tokio::test]
async fn postgres_consignment_returns_release_replay_keeps_same_intake_and_first_receipt_stays_quarantined()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    let rts = Rts { n: 5 };
    seed_refund_rts(pool, &store, Rts { n: 1 }).await;
    succeeds(pool, &quarantined_rts_sql(rts, REFUND_ROUND)).await;
    let repository = repository(&database).await;
    let old_owner_before = old_owner_snapshot(pool).await;

    let command = release(rts, 44);
    let first = repository.release_quarantine(command.clone()).await.expect("解除并接管");
    let (intake, work) =
        (first.return_intake_id.expect("intake"), first.take_work_id.expect("工作"));
    let ledger = "SELECT count(*) FROM public.consignment_commands";
    let commands_before = count(pool, ledger).await;

    // 正例：同命令同请求重放，回执逐字段相同，账本与终局零新增。
    let replay = repository.release_quarantine(command.clone()).await.expect("重放");
    assert_eq!(replay, first);
    assert_eq!((replay.return_intake_id, replay.take_work_id), (Some(intake), Some(work)));
    assert_eq!(count(pool, ledger).await, commands_before);
    assert_eq!(terminal_counts(pool, rts).await, (1, 1, 1));

    // 反例：同命令 ID 异请求（摘要不同）是冲突，不得返回原回执、不改写既有接管。
    let mut changed = command;
    changed.request_hash = RequestHash::new([45; 32]);
    let conflict = repository.release_quarantine(changed).await.expect_err("异请求必须冲突");
    assert_eq!(conflict, ConsignmentError::CommandIdConflict);
    assert_eq!(terminal_counts(pool, rts).await, (1, 1, 1));

    // 首回执重放：隔离时提交的首次决定命令（夹具账本里的 c7c0… / 摘要 e5）重放，仍是隔离且带原引用。
    let quarantine = DecideEnvelopeCommand {
        command_id: CommandId::from_uuid(Uuid::from_u128(
            0xc7c0_0000_0000_5000_8000_0000_0000_0000 + u128::from(rts.n),
        )),
        request_hash: RequestHash::new([0xe5; 32]),
        mail_key: rts.key(),
        facts: LockedEnvelopeFacts {
            mail_key: Some(rts.key()),
            observation: rts_observation(rts),
            reservations: Vec::new(),
            envelope_seller_user_id: None,
            capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
            read_failure: false,
            already_decided: false,
            now: Timestamp::UNIX_EPOCH,
        },
        decision: EnvelopeDecision::Quarantine(QuarantineReason::DuplicateRawEid),
    };
    let first_receipt: Result<_, ConsignmentError> =
        EnvelopeStore::decide_envelope(&repository, quarantine).await;
    let first_receipt = first_receipt.expect("首回执重放");
    assert_eq!(first_receipt.disposition, ReceiptDisposition::Quarantined);
    assert_eq!(first_receipt.quarantine.map(|q| q.quarantine_id), Some(quarantine_of(rts)));
    assert!(first_receipt.return_intake.is_none());
    assert_eq!(terminal_counts(pool, rts).await, (1, 1, 1));
    assert_eq!(old_owner_snapshot(pool).await, old_owner_before);
}

/// 保护不变量 1、6：同一隔离已被某命令解除为接管后，另一个命令提议同种结果得到的回执
/// 必须带同一个 intake / 轮 / Take 工作 / 投递（回执形状由应用层 `validate_shape` 校验，缺字段会被判不可用）。
/// 复现：已知生产缺陷，见报告（`release_receipt_from_existing` 不回填 intake 字段）。
#[tokio::test]
async fn postgres_consignment_returns_release_second_command_receipt_carries_same_intake() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    let rts = Rts { n: 5 };
    seed_refund_rts(pool, &store, Rts { n: 1 }).await;
    succeeds(pool, &quarantined_rts_sql(rts, REFUND_ROUND)).await;
    let repository = repository(&database).await;

    let first = repository.release_quarantine(release(rts, 46)).await.expect("首个解除");
    let second = repository.release_quarantine(release(rts, 47)).await.expect("同结果的第二命令");
    assert_eq!(second.release_id, first.release_id);
    assert_eq!(second.validate_shape(), Ok(()), "第二命令的回执必须是合法的接管回执");
    assert_eq!(
        (second.return_intake_id, second.take_work_id, second.take_delivery_id),
        (first.return_intake_id, first.take_work_id, first.take_delivery_id)
    );
    assert_eq!(terminal_counts(pool, rts).await, (1, 1, 1));
}
