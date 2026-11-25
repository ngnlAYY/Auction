//! 按 RTS 的 K 接管：恰一个 intake、一个 Take 工作、一个终局；异来源与同腿第二个 MID 的冲突。

use std::sync::Arc;

use auction_application::consignment::{
    AcquireReturn, ConsignmentError, DepositBusinessRejection, ReturnIntakeStore,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{OBLIGATION_1, OBLIGATION_3};
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{PAY_DELIVERY, PAY_MID, original_dispatch_sql};
use super::fixture::{Rts, acquire, archived_empty_final_sql, seed_refund_rts, service};
use crate::support::TestDatabase;

async fn count(pool: &PgPool, table: &str) -> i64 {
    let sql = match table {
        "intake" => "SELECT count(*) FROM public.consignment_return_intakes",
        "work" => {
            "SELECT count(*) FROM public.mail_disposition_work
                   WHERE decision_id IN (SELECT decision_id FROM public.consignment_return_intakes)"
        }
        "final" => {
            "SELECT count(*) FROM public.mail_final_dispositions WHERE kind = 'acquire_return'"
        }
        "initial" => {
            "SELECT count(*) FROM public.mail_initial_decisions WHERE kind = 'acquire_return'"
        }
        "conflict" => "SELECT count(*) FROM public.consignment_return_conflicts",
        "command" => {
            "SELECT count(*) FROM public.consignment_commands WHERE operation = 'acquire_return'"
        }
        other => panic!("未知计数：{other}"),
    };
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
}

fn rejection(error: &ConsignmentError) -> Option<DepositBusinessRejection> {
    match error {
        ConsignmentError::Deposit(rejection) => Some(*rejection),
        _ => None,
    }
}

#[tokio::test]
async fn postgres_consignment_returns_rts_acquires_one_intake_one_take_work_one_final() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_refund_rts(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);

    let receipt = acquire(&service, Rts { n: 1 }).await.expect("接管");
    assert!(!receipt.replayed);
    // 正例：恰一个 intake、一个 Take 工作、一个终局（与首次决定同提）、一条账本回执。
    for table in ["intake", "work", "final", "initial", "command"] {
        assert_eq!(count(pool, table).await, 1, "{table} 恰一行");
    }
    let (intake, work, decision): (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT id, take_work_id, decision_id FROM public.consignment_return_intakes",
    )
    .fetch_one(pool)
    .await
    .expect("读取 intake");
    assert_eq!((receipt.intake_id, receipt.work_id, receipt.decision_id), (intake, work, decision));

    // 重放：同 K 同命令得原回执，零新增行。
    let replay = acquire(&service, Rts { n: 1 }).await.expect("重放");
    assert!(replay.replayed);
    assert_eq!((replay.intake_id, replay.work_id), (intake, work));
    for table in ["intake", "work", "final", "command"] {
        assert_eq!(count(pool, table).await, 1, "{table} 重放不新增");
    }
}

#[tokio::test]
async fn postgres_consignment_returns_second_mid_foreign_origin_and_unmatched_manifest_never_acquire()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_refund_rts(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);
    acquire(&service, Rts { n: 1 }).await.expect("第一封接管");

    // 反例一：同一轮同一腿第二个不同 MID 进冲突案，不接管、不建工作。
    succeeds(pool, &Rts { n: 2 }.sql(0, 0)).await;
    let second = acquire(&service, Rts { n: 2 }).await.expect_err("第二个 MID 不得接管");
    assert_eq!(rejection(&second), Some(DepositBusinessRejection::DispositionConflict));
    assert_eq!(count(pool, "conflict").await, 1, "冲突案追加一行");
    assert_eq!((count(pool, "intake").await, count(pool, "work").await), (1, 1));

    // 反例二：清单与首派冻结身份不符（Credits 不同）只隔离，零行。
    succeeds(pool, &Rts { n: 3 }.sql(7, 0)).await;
    let unmatched = acquire(&service, Rts { n: 3 }).await.expect_err("清单不符不得接管");
    assert_eq!(rejection(&unmatched), Some(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!((count(pool, "intake").await, count(pool, "final").await), (1, 1));

    // 反例三：同一 K 以另一义务的来源再次到达（异来源）是处置冲突，不改写既有接管。
    succeeds(pool, &original_dispatch_sql()).await;
    let mut foreign = Rts { n: 1 }.facts(OBLIGATION_1);
    foreign.original_attempt.delivery_id = Uuid::parse_str(PAY_DELIVERY).expect("投递");
    foreign.sent_mid = PAY_MID.to_owned();
    let (command_id, request_hash) = (Uuid::new_v4(), RequestHash::new([9; 32]));
    let error = store
        .acquire_return(AcquireReturn {
            command_id: CommandId::from_uuid(command_id),
            request_hash,
            facts: foreign,
        })
        .await
        .expect_err("异来源不得改写接管");
    assert_eq!(rejection(&error), Some(DepositBusinessRejection::DispositionConflict));
    assert_eq!((count(pool, "intake").await, count(pool, "work").await), (1, 1));
}

/// (终局数, 其中接管终局数, 接管 Take 投递数, 任意工作数, intake 数, 冲突案数)：接管整封的全部落点。
async fn footprint(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64) {
    async fn one(pool: &PgPool, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
    }
    (
        one(pool, "SELECT count(*) FROM public.mail_final_dispositions").await,
        one(
            pool,
            "SELECT count(*) FROM public.mail_final_dispositions WHERE kind = 'acquire_return'",
        )
        .await,
        one(
            pool,
            "SELECT count(*) FROM public.deliveries WHERE origin_kind = 'consignment_disposition'",
        )
        .await,
        one(pool, "SELECT count(*) FROM public.mail_disposition_work").await,
        one(pool, "SELECT count(*) FROM public.consignment_return_intakes").await,
        one(pool, "SELECT count(*) FROM public.consignment_return_conflicts").await,
    )
}

/// K 已先以另一种终局封口：RTS 接管得 `disposition_conflict`，零 intake、零 Take 工作、零新增终局。
#[tokio::test]
async fn postgres_consignment_returns_k_with_opposite_final_disposition_is_a_conflict() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_refund_rts(pool, &store, Rts { n: 1 }).await;
    let base = footprint(pool).await;
    succeeds(pool, &archived_empty_final_sql(Rts { n: 1 })).await;
    let before = footprint(pool).await;
    assert_eq!(
        before,
        (base.0 + 1, base.1, base.2, base.3, base.4, base.5),
        "只多一个空信归档终局"
    );
    let service = service(&store);

    let error = acquire(&service, Rts { n: 1 }).await.expect_err("相反终局不得接管");
    assert_eq!(rejection(&error), Some(DepositBusinessRejection::DispositionConflict));
    assert_eq!(footprint(pool).await, before, "零 intake、零 Take 工作、零新增终局行");
    // 终局仍是原来的那一个，没有被改写。
    let kind: String = sqlx::query_scalar(
        "SELECT kind FROM public.mail_final_dispositions WHERE external_mid = '601'",
    )
    .fetch_one(pool)
    .await
    .expect("读取终局");
    assert_eq!(kind, "archived_empty");
}

/// 接管整封全有全无：Take 工作或 intake 写入失败，终局、决定、工作、账本都回滚；同命令重试成功。
#[tokio::test]
async fn postgres_consignment_returns_acquire_rolls_back_the_whole_envelope_on_failure() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_refund_rts(pool, &store, Rts { n: 1 }).await;
    let base = footprint(pool).await;
    let service = service(&store);

    for table in ["mail_disposition_work", "consignment_return_intakes"] {
        succeeds(
            pool,
            &format!(
                "CREATE FUNCTION public.fixture_fail_write() RETURNS trigger LANGUAGE plpgsql AS
                 $f$ BEGIN RAISE EXCEPTION '夹具：写入失败'; END $f$;
                 CREATE TRIGGER fixture_fail_write BEFORE INSERT ON public.{table}
                 FOR EACH ROW EXECUTE FUNCTION public.fixture_fail_write();"
            ),
        )
        .await;
        assert!(acquire(&service, Rts { n: 1 }).await.is_err(), "{table} 写入失败必须失败");
        assert_eq!(footprint(pool).await, base, "{table} 失败后终局与工作全回滚");
        for name in ["initial", "command"] {
            assert_eq!(count(pool, name).await, 0, "{table} 失败后 {name} 为零");
        }
        succeeds(
            pool,
            &format!(
                "DROP TRIGGER fixture_fail_write ON public.{table};
                 DROP FUNCTION public.fixture_fail_write();"
            ),
        )
        .await;
    }

    let receipt = acquire(&service, Rts { n: 1 }).await.expect("同一命令重试成功");
    assert!(!receipt.replayed);
    assert_eq!(
        footprint(pool).await,
        (base.0 + 1, base.1 + 1, base.2 + 1, base.3 + 1, base.4 + 1, base.5)
    );
}

/// 同 K 异决定参数（Sent MID、冻结清单）不是重放：同命令 ID 异摘要得 `command_id_conflict`，不改写接管。
#[tokio::test]
async fn postgres_consignment_returns_acquire_with_other_decision_parameters_conflicts_by_digest() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_refund_rts(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);
    let first = acquire(&service, Rts { n: 1 }).await.expect("接管");
    let before = footprint(pool).await;

    let mut other_mid = Rts { n: 1 }.facts(OBLIGATION_3);
    other_mid.sent_mid = "9999".to_owned();
    let mut other_manifest = Rts { n: 1 }.facts(OBLIGATION_3);
    other_manifest.frozen_manifest_digest = [0x11; 32];
    for facts in [other_mid, other_manifest] {
        let error = service.acquire(facts).await.expect_err("异决定参数不得当作重放");
        assert!(matches!(error, ConsignmentError::CommandIdConflict), "得到 {error:?}");
    }
    assert_eq!(footprint(pool).await, before);

    // 原请求仍可重放。
    let replay = acquire(&service, Rts { n: 1 }).await.expect("重放");
    assert!(replay.replayed);
    assert_eq!(replay.intake_id, first.intake_id);
}
