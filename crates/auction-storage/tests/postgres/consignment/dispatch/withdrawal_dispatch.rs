//! 撤回件派发（PIWX）：两条真实撤回入口（P7C2 直接撤回、撤回意愿在取得时兑现）同提撤回件首轮，经生产的
//! 领取、轮衔接、尝试与起写得到唯一一次 POST 许可，position `in_custody → in_transit`，Sent 证据把投递收口为
//! `succeeded`；撤回件被退回并精确消费后 position 回到 `in_custody`，退回处置不触发再售。
//!
//! 反例：Sale 合同不能借撤回件的 position 边；非撤回合同不能走撤回首轮原语。
//! 保护不变量 1、2、3、4、6：起写许可只发一次、重放不新增；合同与首轮同提；非法边失败关闭。

use std::sync::Arc;

use auction_application::consignment::{
    AutoRelistSetting, ConsignmentError, CustodyControlStore, DispatchService, DispatchWorkStore,
    RecordEvidenceInput, ReturnDispositionService, ReturnDispositionStore, WithdrawDepositCommand,
    WriteStart,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::dispatch::{AttemptKey, EvidenceCategory, dispatch_round_id};
use auction_domain::consignment::recovery::{ListingEffect, RelistEligibility};
use auction_domain::consignment::returns::{ReturnIntakeFacts, ReturnLeg};
use auction_domain::consignment::{FeeBps, ListingPrice, ListingVersion};
use auction_domain::{CommandId, UserId};
use auction_storage::{DatabaseStore, DepositRepository, DepositSettings};
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{
    ATTEMPT, DECISION, LISTING, POSITION, PROOF, PROOF_COMMAND, SESSION, USER_1, seed_accepted,
    succeeds,
};
use super::super::returns::fixture::{
    Claim, MANIFEST, Rts, SIGNED_VALIDATOR, claim_take, consume_input, seed_original_started,
    service as return_service, start, take_delivery,
};
use super::fixture::{
    CONTENT, Seeded, at, binding, prepare_send_credential, sale_graph, work_store,
};
use crate::support::TestDatabase;

/// Sent 绑定的远端 MID（明显的假值）。
const SENT_MID: &str = "9001";

/// 玩家 UUIDv4 命令 ID。
fn command_id(seed: u128) -> CommandId {
    CommandId::from_uuid(Uuid::from_u128(0x9a10_0000_0000_4000_8000_0000_0000_0000 + seed))
}

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("夹具 UUID")
}

fn withdraw(seed: u128, version: u64) -> WithdrawDepositCommand {
    WithdrawDepositCommand {
        command_id: command_id(seed),
        seller_user_id: UserId::from_uuid(uuid(USER_1)),
        listing_id: uuid(LISTING),
        expected_version: ListingVersion::new(version).expect("版本"),
        request_hash: RequestHash::new([0x9a; 32]),
        request_id: None,
    }
}

/// 撤回入口。
#[derive(Clone, Copy)]
enum Entry {
    /// 取得后上架，再直接撤回（available 版本 5）。
    Direct,
    /// 先登记撤回意愿（版本 4），取得事务兑现。
    IntentAtTake,
}

struct World {
    _database: TestDatabase,
    owner: PgPool,
    runtime: DatabaseStore,
}

/// 撤回件派发的结果：合同、返还义务、投递与起写的尝试。
struct Dispatched {
    contract_id: Uuid,
    obligation_id: Uuid,
    attempt: AttemptKey,
}

impl World {
    /// main 件已接受、寄售闸门打开；`with_content` 为假时收件仓库不注入派发内容来源（失败关闭占位）。
    async fn accepted(with_content: bool) -> (Self, DepositRepository) {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let runtime = database.store().await.expect("运行账号");
        seed_accepted(runtime.postgres_pool()).await;
        let owner = database.maintenance_store().await.expect("维护账号").postgres_pool().clone();
        succeeds(
            &owner,
            "UPDATE public.consignment_lookup_control
             SET admitting = true, enabled_epoch = 1,
                 worker_ready_until = clock_timestamp() + interval '1 hour' WHERE id = 1;",
        )
        .await;
        let deposit = DepositRepository::without_reservation_crypto(
            runtime.clone(),
            DepositSettings::new(
                FeeBps::new(500).expect("费率"),
                ListingPrice::new(ListingPrice::MIN).expect("下限"),
                ListingPrice::new(ListingPrice::MAX).expect("上限"),
                24,
                100,
            ),
        );
        let deposit =
            if with_content { deposit.with_dispatch_content(Arc::new(CONTENT)) } else { deposit };
        (Self { _database: database, owner, runtime }, deposit)
    }

    /// 整封精确 Take 的取得事务。
    async fn take(deposit: &DepositRepository) {
        deposit
            .test_apply_take(
                uuid(PROOF_COMMAND),
                uuid(DECISION),
                uuid(PROOF),
                uuid(ATTEMPT),
                uuid(LISTING),
            )
            .await
            .expect("取得");
    }

    /// main 件已接受；经 `entry` 指定的真实写口建立 Withdrawal（撤回件首轮同提）。
    async fn withdrawn(entry: Entry) -> Self {
        let (world, deposit) = Self::accepted(true).await;
        match entry {
            Entry::Direct => {
                Self::take(&deposit).await;
                let receipt = deposit.request_withdrawal(withdraw(1, 5)).await.expect("直接撤回");
                assert_eq!(receipt.disposition.as_str(), "withdrawal_accepted");
            }
            Entry::IntentAtTake => {
                deposit.request_withdrawal(withdraw(2, 4)).await.expect("登记意愿");
                Self::take(&deposit).await;
            }
        }
        prepare_send_credential(&world.owner).await;
        world
    }

    fn service(&self) -> DispatchService {
        DispatchService::new(Arc::new(self.runtime.clone()))
    }

    async fn count(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.owner).await.expect("计数")
    }

    async fn position_state(&self) -> String {
        sqlx::query_scalar("SELECT state FROM public.consignment_custody_positions WHERE id = $1")
            .bind(uuid(POSITION))
            .fetch_one(&self.owner)
            .await
            .expect("position 状态")
    }

    /// (起写数即 POST 许可数, 寄售尝试数, 撤回件首轮数)。
    async fn tallies(&self) -> (i64, i64, i64) {
        (
            self.count("SELECT count(*) FROM public.delivery_send_attempts").await,
            self.count("SELECT count(*) FROM public.consignment_dispatch_attempts").await,
            self.count(
                "SELECT count(*) FROM public.consignment_dispatch_rounds
                 WHERE template = 'consignment_equipment_withdrawal'",
            )
            .await,
        )
    }

    /// 生产领取 → 衔接 → 登记尝试 → 起写 → Sent 证据。
    async fn dispatch_to_sent(&self) -> Dispatched {
        let (contract_id, obligation_id): (Uuid, Uuid) = sqlx::query_as(
            "SELECT contract.contract_id, obligation.id
             FROM public.consignment_contracts AS contract
             JOIN public.settlement_obligations AS obligation
               ON obligation.contract_id = contract.contract_id
             WHERE contract.kind = 'withdrawal' AND obligation.obligation_kind = 'withdrawn_return'",
        )
        .fetch_one(&self.owner)
        .await
        .expect("撤回合同与返还义务");
        assert_eq!(self.tallies().await, (0, 0, 1), "撤回同提首轮，尚无起写");
        let now = Timestamp::now();
        let work = work_store(&self.runtime, CONTENT)
            .claim_dispatch_work(now, now + Duration::minutes(5))
            .await
            .expect("领取")
            .expect("撤回轮可领取");
        assert_eq!((work.key.contract_id, work.key.obligation_id), (contract_id, obligation_id));
        assert_eq!((work.frozen.cod, work.frozen.credits), (0, 0), "撤回件零 COD、零 Credits");
        let (delivery, token, fence) = self
            .runtime
            .test_lease_dispatch_round(
                work.obligation_id,
                work.claim_token,
                work.claim_fencing,
                dispatch_round_id(&work.key),
            )
            .await
            .expect("衔接")
            .expect("取得投递租约");
        let attempt = AttemptKey { delivery_id: delivery, fence };
        let service = self.service();
        service.start_attempt(work.key, attempt, token, work.frozen.clone()).await.expect("登记");
        let started =
            service.mark_write_started(attempt, token, Some(binding())).await.expect("起写");
        assert!(matches!(started, WriteStart::FirstCommitted { .. }), "{started:?}");
        assert_eq!(self.position_state().await, "in_transit");
        // 重放：读原回执，不新增起写许可。
        let replay =
            service.mark_write_started(attempt, token, Some(binding())).await.expect("重放");
        assert!(matches!(replay, WriteStart::AlreadyCommitted { .. }), "{replay:?}");
        assert_eq!(self.tallies().await, (1, 1, 1), "恰一次 POST 许可");

        let sent = RecordEvidenceInput {
            attempt,
            category: EvidenceCategory::SentBinding,
            remote_observation_id: SENT_MID.to_owned(),
            observed_at: at("2026-01-01T05:00:00Z"),
            validator_version: "fake-validator-v1".to_owned(),
            evidence_digest: [7; 32],
        };
        assert!(!service.record_evidence(sent).await.expect("Sent 证据").replayed);
        let status: String =
            sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
                .bind(delivery)
                .fetch_one(&self.owner)
                .await
                .expect("投递状态");
        assert_eq!(status, "succeeded");
        Dispatched { contract_id, obligation_id, attempt }
    }
}

/// 撤回件的系统 RTS：Credits、COD 为零，附件就是被寄还的规范装备（与冻结发送身份一致）；接收分钟取
/// 当前时刻之后，晚于真实时钟下的 Sent。
fn withdrawal_rts_sql(rts: Rts) -> String {
    let later = (Timestamp::now() + Duration::minutes(2))
        .to_rfc3339_opts(auction_time::SecondsFormat::Secs, true);
    let minute = &later[..16];
    let sql = rts.sql(0, 0);
    let with_item = sql.replace(
        "true, true, 0, 0, NULL, '[]'::jsonb,",
        "true, true, 0, 0, 1,
         '[{\"ordinal\":1,\"kind\":\"equipment\",\"name\":\"Fake Gear\",
            \"raw_eid\":\"42\",\"canonical_eid\":\"42\",
            \"protected_ref\":\"83000000-0000-0000-0000-000000000001\"}]'::jsonb,",
    );
    assert_ne!(with_item, sql, "夹具应带上被寄还的装备");
    with_item.replace("2026-01-01T05:00", minute)
}

/// 两条真实撤回入口都能派发：首次起写 `FirstCommitted`、恰一次 POST 许可、position 进入 `in_transit`，
/// Sent 证据收口为 `succeeded`。
#[tokio::test]
async fn postgres_withdrawal_dispatch_both_entries_reach_first_write_and_sent() {
    for entry in [Entry::Direct, Entry::IntentAtTake] {
        let world = World::withdrawn(entry).await;
        assert_eq!(world.position_state().await, "in_custody");
        world.dispatch_to_sent().await;
        assert_eq!(world.position_state().await, "in_transit");
    }
}

/// 撤回件被退回：精确消费后 position 回到 `in_custody`（不是 `committed`），退回处置只恢复原受益权，
/// 禁止再售，即使冻结开关为开；没有重列决定、没有新处分代，挂单仍是 `withdrawal_pending`。
#[tokio::test]
async fn postgres_withdrawal_dispatch_returned_original_goes_back_to_custody_without_relist() {
    let world = World::withdrawn(Entry::Direct).await;
    let dispatched = world.dispatch_to_sent().await;
    let rts = Rts { n: 1 };
    succeeds(&world.owner, &withdrawal_rts_sql(rts)).await;
    let store = Arc::new(world.runtime.clone());
    let returns = return_service(&store);
    let receipt = returns
        .acquire(ReturnIntakeFacts {
            k: rts.key(),
            contract_id: dispatched.contract_id,
            obligation_id: dispatched.obligation_id,
            original_round: 1,
            original_attempt: dispatched.attempt,
            sent_mid: SENT_MID.to_owned(),
            rts_observation_id: rts.observation(),
            rts_returned_at: Timestamp::UNIX_EPOCH,
            leg: ReturnLeg::Original,
            frozen_manifest_digest: MANIFEST,
            session_id: uuid(SESSION),
        })
        .await
        .expect("接管撤回件退件");
    let claim = Claim::new(21, 1, true);
    claim_take(&world.owner, &receipt, take_delivery(&world.owner, &receipt).await, claim).await;
    let started = start(&returns, &world.owner, &receipt, claim).await.expect("Take 起写");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "{started:?}");
    returns
        .consume(consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("精确消费");
    assert_eq!(world.position_state().await, "in_custody", "撤回件回库，不回 committed");

    let port: Arc<dyn ReturnDispositionStore> = store.clone();
    let dispositions = ReturnDispositionService::new(port);
    let pending = dispositions.pending(None, 10).await.expect("待处置").items;
    assert_eq!(pending.len(), 1);
    let setting = AutoRelistSetting { enabled: true, config_digest: [9; 32] };
    let resolved = dispositions.resolve(pending[0], setting).await.expect("处置");
    assert_eq!(resolved.decision.listing, ListingEffect::Unchanged);
    assert_eq!(resolved.decision.relist, RelistEligibility::Forbidden, "撤回件被退不触发再售");
    assert_eq!(world.count("SELECT count(*) FROM public.consignment_relist_decisions").await, 0);
    assert_eq!(
        world.count("SELECT count(*) FROM public.consignment_disposition_generations").await,
        1
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM public.consignment_listings
                 WHERE state = 'withdrawal_pending'"
            )
            .await,
        1
    );
}

/// 反例一：Sale 的原件已在途时，position 不能借撤回件的回库边 `in_transit → in_custody` 离开交割
/// （守卫要求当前代合同是 Withdrawal 并有同事务原件腿消费），整句被拒、position 不变。
#[tokio::test]
async fn postgres_withdrawal_dispatch_sale_cannot_borrow_the_withdrawal_edges() {
    let (seeded, _started) = Seeded::first_dispatch_started().await;
    let pool = seeded.pool();
    let state = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM public.consignment_custody_positions WHERE id = $1",
        )
        .bind(uuid(POSITION))
        .fetch_one(pool)
        .await
        .expect("position 状态")
    };
    assert_eq!(state().await, "in_transit", "前提：Sale 原件腿已起写");
    let error = sqlx::query(
        "UPDATE public.consignment_custody_positions
         SET state = 'in_custody', version = version + 1, updated_at = clock_timestamp()
         WHERE id = $1",
    )
    .bind(uuid(POSITION))
    .execute(pool)
    .await
    .expect_err("Sale 不得借撤回件回库边");
    let constraint = error.as_database_error().and_then(|error| error.constraint());
    assert_eq!(constraint, Some("c7_position_withdrawal_return_fact"));
    assert_eq!(state().await, "in_transit");
}

/// 反例二：非撤回合同（Sale 图）不能走撤回件首轮原语：失败关闭，不新增任何轮或投递。
#[tokio::test]
async fn postgres_withdrawal_dispatch_round_rejects_a_sale_graph() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let before: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_dispatch_rounds),
                (SELECT count(*) FROM public.deliveries)",
    )
    .fetch_one(pool)
    .await
    .expect("计数");
    let result =
        seeded.runtime.test_create_withdrawal_round(&sale_graph(), Arc::new(CONTENT)).await;
    assert!(result.is_err(), "Sale 图不得建撤回件首轮：{result:?}");
    let after: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_dispatch_rounds),
                (SELECT count(*) FROM public.deliveries)",
    )
    .fetch_one(pool)
    .await
    .expect("计数");
    assert_eq!(after, before);
}

/// 撤回件首轮内容不可得（收件仓库未注入内容来源）：直接撤回整笔回滚为 `Unavailable`，合同、义务、投递、轮、
/// 命令账本、挂单与挂单事件逐项不变（不变量 3：合同与首轮全有全无）。
#[tokio::test]
async fn postgres_withdrawal_dispatch_missing_content_rolls_back_the_whole_withdrawal() {
    let (world, deposit) = World::accepted(false).await;
    World::take(&deposit).await;
    let snapshot = || async {
        let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM public.consignment_contracts),
                    (SELECT count(*) FROM public.settlement_obligations),
                    (SELECT count(*) FROM public.deliveries),
                    (SELECT count(*) FROM public.consignment_dispatch_rounds),
                    (SELECT count(*) FROM public.consignment_commands),
                    (SELECT count(*) FROM public.consignment_events)",
        )
        .fetch_one(&world.owner)
        .await
        .expect("计数");
        let listing: (String, i64) = sqlx::query_as(
            "SELECT state, state_version FROM public.consignment_listings WHERE id = $1",
        )
        .bind(uuid(LISTING))
        .fetch_one(&world.owner)
        .await
        .expect("挂单");
        (counts, listing, world.position_state().await)
    };
    let before = snapshot().await;
    assert_eq!(before.1, ("available".to_owned(), 5));

    let refused = deposit.request_withdrawal(withdraw(3, 5)).await;

    assert!(matches!(refused, Err(ConsignmentError::Unavailable)), "{refused:?}");
    assert_eq!(snapshot().await, before);
}

/// 反例（种类守卫）：Sale 原件已在途，即使同一事务写入了该 Sale 原件腿的精确 Take 证明与退件消费，
/// position 也不能借撤回件的回库边 `in_transit → in_custody`；失败约束就是撤回件回库守卫。
#[tokio::test]
async fn postgres_withdrawal_dispatch_sale_consumption_cannot_take_the_withdrawal_return_edge() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号").postgres_pool().clone();
    let store = Arc::new(database.store().await.expect("运行账号"));
    let rts = Rts { n: 1 };
    let (receipt, claim) = seed_original_started(&owner, &store, rts).await;
    let mut tx = owner.begin().await.expect("事务");
    let proof = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.mail_disposition_proofs
            (id, work_id, decision_id, delivery_id, fence, attempt_id, kind, asset_leg,
             observation_id, validator_version, digest, conclusion, proven_at)
         SELECT $1, take_work_id, decision_id, take_delivery_id, $2, $3, 'exact_take', 'original',
                $4, $5, decode(repeat('a7', 32), 'hex'), 'take_proven', clock_timestamp()
         FROM public.consignment_return_intakes WHERE id = $6",
    )
    .bind(proof)
    .bind(i64::try_from(claim.fence).expect("fence"))
    .bind(claim.attempt)
    .bind(rts.observation())
    .bind(SIGNED_VALIDATOR)
    .bind(receipt.intake_id)
    .execute(&mut *tx)
    .await
    .expect("同事务精确 Take 证明");
    sqlx::query(
        "INSERT INTO public.consignment_return_consumptions
            (id, intake_id, contract_id, obligation_id, original_round, leg, bot_uid, world_id,
             external_mid, work_id, delivery_id, attempt_id, fence, proof_id)
         SELECT gen_random_uuid(), id, contract_id, obligation_id, original_round, 'original',
                bot_uid, world_id, external_mid, take_work_id, take_delivery_id, $1, $2, $3
         FROM public.consignment_return_intakes WHERE id = $4",
    )
    .bind(claim.attempt)
    .bind(i64::try_from(claim.fence).expect("fence"))
    .bind(proof)
    .bind(receipt.intake_id)
    .execute(&mut *tx)
    .await
    .expect("同事务 Sale 原件腿消费");
    let error = sqlx::query(
        "UPDATE public.consignment_custody_positions
         SET state = 'in_custody', version = version + 1, updated_at = clock_timestamp()
         WHERE id = $1",
    )
    .bind(uuid(POSITION))
    .execute(&mut *tx)
    .await
    .expect_err("Sale 不得借撤回件回库边");
    let constraint = error.as_database_error().and_then(|error| error.constraint());
    assert_eq!(constraint, Some("c7_position_withdrawal_return_fact"));
    tx.rollback().await.expect("回滚");
}

/// 反例（起写失败关闭）：Sale 的 position 处于非预期前态 `in_custody`（应为 `committed`）时，原件腿起写
/// 整笔回滚：零 POST 许可、零寄售尝试行，投递不进入 `sending`（不变量 4、6）。
#[tokio::test]
async fn postgres_withdrawal_dispatch_unexpected_position_state_fails_the_write_start_closed() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    prepare_send_credential(pool).await;
    // 存量破坏：绕过守卫把 Sale 的 position 置回 in_custody（只为构造非预期前态）。
    succeeds(
        pool,
        &format!(
            "ALTER TABLE public.consignment_custody_positions DISABLE TRIGGER USER;
             UPDATE public.consignment_custody_positions SET state = 'in_custody'
             WHERE id = '{POSITION}';
             ALTER TABLE public.consignment_custody_positions ENABLE TRIGGER USER;"
        ),
    )
    .await;
    let now = Timestamp::now();
    let work = work_store(&seeded.runtime, CONTENT)
        .claim_dispatch_work(now, now + Duration::minutes(5))
        .await
        .expect("领取")
        .expect("首轮可领取");
    let (delivery, token, fence) = seeded
        .runtime
        .test_lease_dispatch_round(
            work.obligation_id,
            work.claim_token,
            work.claim_fencing,
            dispatch_round_id(&work.key),
        )
        .await
        .expect("衔接")
        .expect("取得投递租约");
    let attempt = AttemptKey { delivery_id: delivery, fence };
    let service = seeded.service();
    service.start_attempt(work.key, attempt, token, work.frozen.clone()).await.expect("登记");

    let started = service.mark_write_started(attempt, token, Some(binding())).await;

    assert!(started.is_err(), "非预期前态不得发起写许可：{started:?}");
    let tallies: (i64, i64, String) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.delivery_send_attempts),
                (SELECT count(*) FROM public.consignment_dispatch_attempts),
                (SELECT status FROM public.deliveries WHERE id = $1)",
    )
    .bind(delivery)
    .fetch_one(pool)
    .await
    .expect("计数");
    assert_eq!(tallies, (0, 0, "claimed".to_owned()));
}
