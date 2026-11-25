//! 损失证据、案件、卖家回执与人工结果的存储原语：证据不足被拒、案件与回执各只产生一次、
//! 同命令重放返回原结果且同命令异请求冲突、人工结果追加后资金与发行类表零增量（不变量 1、6、8，决定 22）。
//!
//! 夹具：main 代管 position 与一份损失事实；原语经运行账号连接执行，命令账本里已有的命令行供外键引用
//! （账本对新操作的放开归 7M-2，本文件只验证原语本身）。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, ListingDelistReason, LossCaseOutcome, loss_proof_id,
};
use auction_domain::CommandId;
use auction_storage::{DatabaseStore, OpenedLossCase, TestLossProof};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::seed_isekai_custody;
use super::super::deposit::support::{POSITION, USER_1, USER_2, succeeds};
use super::sale_recovery::unknown_refund_sql;
use super::{CASE, FACT, PROOF, command_ids, fact_sql, proof_sql, seed};
use crate::support::TestDatabase;

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("夹具 UUID")
}

fn command(text: &str) -> CommandId {
    CommandId::from_uuid(uuid(text))
}

fn note(text: &str) -> ListingDelistReason {
    ListingDelistReason::new(text.to_owned()).expect("夹具说明")
}

fn proof<'a>(source_kind: &'a str, observation_ref: &'a str) -> TestLossProof<'a> {
    TestLossProof {
        position_id: uuid(POSITION),
        source_kind,
        observation_ref,
        last_known_ref: "fake-last-known",
    }
}

async fn scalar(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取标量")
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM public.{table}")))
        .fetch_one(pool)
        .await
        .expect("读取行数")
}

/// 损失五表之外全部基表的“行数 + 全表内容摘要”快照，另含补偿池余额与版本；
/// 人工结果追加不得让它们有任何变化。内容摘要覆盖所有列（含余额与金额合计所在列），不只比行数。
async fn non_loss_snapshot(pool: &PgPool) -> (Vec<(String, i64, String)>, Vec<(String, i64, i64)>) {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
           AND table_name !~ '^consignment_loss_' ORDER BY table_name",
    )
    .fetch_all(pool)
    .await
    .expect("读取基表");
    let mut snapshot = Vec::new();
    for table in tables {
        let rows = count(pool, &table).await;
        let content: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '') \
             FROM public.{table} AS t"
        )))
        .fetch_one(pool)
        .await
        .expect("读取内容摘要");
        snapshot.push((table, rows, content));
    }
    let pools = sqlx::query_as(
        "SELECT singleton::text, balance, state_version FROM public.compensation_pool",
    )
    .fetch_all(pool)
    .await
    .expect("读取补偿池余额");
    (snapshot, pools)
}

fn rejection(error: ConsignmentError) -> DepositBusinessRejection {
    match error {
        ConsignmentError::Deposit(rejection) => rejection,
        other => panic!("期望确定业务拒绝，实际 {other:?}"),
    }
}

async fn open_case(runtime: &DatabaseStore, pool: &PgPool, commands: &[String]) -> OpenedLossCase {
    succeeds(
        pool,
        &(proof_sql(PROOF, "fake-observation-1", &commands[0])
            + &fact_sql(FACT, PROOF, &commands[1])),
    )
    .await;
    let ids = OpenedLossCase {
        case_id: uuid(CASE),
        outcome_id: uuid("7c700000-0000-4000-8000-000000000001"),
        seller_receipt_id: uuid("7c800000-0000-4000-8000-000000000001"),
    };
    runtime
        .test_open_loss_case(command(&commands[1]), uuid(FACT), uuid(POSITION), uuid(USER_1), ids)
        .await
        .expect("开案")
}

#[tokio::test]
async fn postgres_consignment_loss_proof_rejects_insufficient_evidence_and_appends_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 2).await;

    // 反例：空库存、缺 MID、说明文字都不足以证明灭失，失败关闭且不留任何证据行。
    for source in ["empty_inventory", "missing_mid", "narrative"] {
        let error = runtime
            .test_append_loss_proof(command(&commands[0]), &proof(source, "fake-observation-1"))
            .await
            .expect_err("证据不足应被拒绝");
        assert_eq!(rejection(error), DepositBusinessRejection::EvidenceUnproven, "{source}");
    }
    assert_eq!(count(pool, "consignment_loss_proofs").await, 0);

    // 正例：独立来源的完整证据落库一次；同来源同观察同内容重放返回原证据。
    let first = runtime
        .test_append_loss_proof(command(&commands[0]), &proof("fake-source", "fake-observation-1"))
        .await
        .expect("追加证据");
    // 证据 ID 由系统按业务键派生，不由调用方给定。
    assert_eq!(first.proof_id, loss_proof_id(uuid(POSITION), "fake-source", "fake-observation-1"));
    let replay_input = proof("fake-source", "fake-observation-1");
    let replay =
        runtime.test_append_loss_proof(command(&commands[1]), &replay_input).await.expect("重放");
    assert_eq!(replay.proof_id, first.proof_id);
    assert_eq!(count(pool, "consignment_loss_proofs").await, 1);

    // 反例：同一远端观察内容矛盾（最后确定位置不同，推导的证据摘要随之不同），不覆盖先前证据，
    // 判 evidence_conflict。
    let mut contradicting = proof("fake-source", "fake-observation-1");
    contradicting.last_known_ref = "other-last-known";
    let error = runtime
        .test_append_loss_proof(command(&commands[1]), &contradicting)
        .await
        .expect_err("内容矛盾应被拒绝");
    assert_eq!(rejection(error), DepositBusinessRejection::EvidenceConflict);
    assert_eq!(count(pool, "consignment_loss_proofs").await, 1);

    // 反例（不变量 1）：同一命令 ID 换观察或任一字段都是异请求，判 command_id_conflict，不写第二条证据。
    let variants: [fn(&mut TestLossProof<'static>); 3] = [
        |p| p.observation_ref = "fake-observation-2",
        |p| p.source_kind = "other-source",
        |p| p.last_known_ref = "other-last-known",
    ];
    for (index, mutate) in variants.into_iter().enumerate() {
        let mut other = proof("fake-source", "fake-observation-1");
        mutate(&mut other);
        let error = runtime
            .test_append_loss_proof(command(&commands[0]), &other)
            .await
            .expect_err("同命令异请求应冲突");
        assert_eq!(error, ConsignmentError::CommandIdConflict, "变体 {index}");
        assert_eq!(count(pool, "consignment_loss_proofs").await, 1, "变体 {index}");
    }
    // 同命令同请求仍返回原证据。
    let again = runtime
        .test_append_loss_proof(command(&commands[0]), &proof("fake-source", "fake-observation-1"))
        .await
        .expect("同命令重放");
    assert_eq!(again, first);

    // 反例：最后确定位置不是合法安全文本（超长、控制字符、装备链接、key 形状），失败关闭。
    let long = "x".repeat(257);
    for bad in [
        long.as_str(),
        "line\nbreak",
        "https://example.invalid/equip/123/0123abcdef",
        "shelf 0123abcdef",
    ] {
        let mut other = proof("fake-source", "fake-observation-9");
        other.last_known_ref = bad;
        let error = runtime
            .test_append_loss_proof(command(&commands[1]), &other)
            .await
            .expect_err("非法位置文本应被拒绝");
        assert_eq!(rejection(error), DepositBusinessRejection::EvidenceUnproven);
    }
    // 反例：观察身份同样不得携带 key 形状（装备 key 不能借观察身份进入证据行）。
    let error = runtime
        .test_append_loss_proof(command(&commands[1]), &proof("fake-source", "obs-0123abcdef"))
        .await
        .expect_err("观察身份含 key 形状应被拒绝");
    assert_eq!(rejection(error), DepositBusinessRejection::EvidenceUnproven);
    assert_eq!(count(pool, "consignment_loss_proofs").await, 1);
}

#[tokio::test]
async fn postgres_consignment_loss_proof_takes_world_and_season_from_the_position() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed_isekai_custody(pool, "transaction_timestamp() + interval '10 days'").await;
    let commands = command_ids(pool, 1).await;

    runtime
        .test_append_loss_proof(command(&commands[0]), &proof("fake-source", "fake-observation-1"))
        .await
        .expect("追加证据");
    let (world, season): (String, Option<String>) =
        sqlx::query_as("SELECT world_id, season_id FROM public.consignment_loss_proofs")
            .fetch_one(pool)
            .await
            .expect("读取证据");
    assert_eq!((world.as_str(), season.as_deref()), ("isekai", Some("fake-season")));
}

/// 排除在途由锁内派发事实推导：该 position 的合同存在结果未知的派发（已起写、无 Sent 绑定）时不能排除，
/// 证据失败关闭为 `evidence_unproven`、不写行；请求里没有任何摘要字段，摘要只能来自存储推导。
#[tokio::test]
async fn postgres_consignment_loss_proof_cannot_exclude_an_unknown_dispatch() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 2).await;
    runtime
        .test_append_loss_proof(command(&commands[0]), &proof("fake-source", "fake-observation-1"))
        .await
        .expect("无未知派发时可以排除");

    succeeds(pool, &unknown_refund_sql()).await;
    let error = runtime
        .test_append_loss_proof(command(&commands[1]), &proof("fake-source", "fake-observation-2"))
        .await
        .expect_err("存在未知派发时不能排除在途");
    assert_eq!(rejection(error), DepositBusinessRejection::EvidenceUnproven);
    assert_eq!(count(pool, "consignment_loss_proofs").await, 1);
}

#[tokio::test]
async fn postgres_consignment_loss_case_and_seller_receipt_are_created_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 3).await;
    let opened = open_case(&runtime, pool, &commands).await;
    assert_eq!(count(pool, "consignment_loss_cases").await, 1);
    assert_eq!(count(pool, "consignment_loss_receipts").await, 1);
    let loss_rights = "SELECT count(*) FROM public.consignment_rights WHERE kind = 'loss_case'";
    assert_eq!(scalar(pool, loss_rights).await, 1, "开案同事务登记卖家的 loss_case 权利");

    // 正例：同命令重放返回原三个标识，不新增行（调用方提供的新 UUID 不被使用）。
    let fresh = OpenedLossCase {
        case_id: uuid("7c400000-0000-4000-8000-000000000009"),
        outcome_id: uuid("7c700000-0000-4000-8000-000000000009"),
        seller_receipt_id: uuid("7c800000-0000-4000-8000-000000000009"),
    };
    let replay = runtime
        .test_open_loss_case(command(&commands[1]), uuid(FACT), uuid(POSITION), uuid(USER_1), fresh)
        .await
        .expect("重放开案");
    assert_eq!(replay, opened);

    // 反例：另一条命令想对同一事实再开案，判 disposition_conflict，案件与回执仍各一。
    let error = runtime
        .test_open_loss_case(command(&commands[2]), uuid(FACT), uuid(POSITION), uuid(USER_1), fresh)
        .await
        .expect_err("第二条命令不得再开案");
    assert_eq!(rejection(error), DepositBusinessRejection::DispositionConflict);
    assert_eq!(count(pool, "consignment_loss_cases").await, 1);
    assert_eq!(count(pool, "consignment_loss_receipts").await, 1);
    assert_eq!(scalar(pool, loss_rights).await, 1, "重放与冲突都不再登记权利");

    // 卖家只能读到本人的案件回执；非本人与不存在不可区分。
    let own = runtime.test_seller_loss_case(uuid(USER_1), uuid(POSITION)).await.expect("读取");
    assert_eq!(own.expect("本人可见").receipt_id, opened.seller_receipt_id);
    assert!(
        runtime.test_seller_loss_case(uuid(USER_2), uuid(POSITION)).await.expect("读取").is_none()
    );
}

#[tokio::test]
async fn postgres_consignment_loss_outcome_is_append_only_idempotent_and_moves_no_money() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 5).await;
    let opened = open_case(&runtime, pool, &commands).await;
    let basis = uuid("7c900000-0000-4000-8000-000000000001");
    let ids = (
        uuid("7c700000-0000-4000-8000-000000000002"),
        uuid("7c800000-0000-4000-8000-000000000002"),
    );
    let before = non_loss_snapshot(pool).await;

    // 正例：版本 1 之后追加复核中，版本加一并带一份新的卖家回执。
    let receipt = runtime
        .test_append_loss_case_outcome(
            command(&commands[2]),
            opened.case_id,
            uuid(POSITION),
            1,
            LossCaseOutcome::Reviewing,
            basis,
            &note("fake review note"),
            ids,
        )
        .await
        .expect("追加复核中");
    assert_eq!((receipt.case_version, receipt.outcome_id), (2, ids.0));
    assert_eq!(count(pool, "consignment_loss_case_outcomes").await, 2);
    assert_eq!(count(pool, "consignment_loss_receipts").await, 2);
    // 金额与资金：除损失五表外，所有表的行数与内容、补偿池余额逐一不变。
    assert_eq!(non_loss_snapshot(pool).await, before);
    // 对照：快照确实能检出余额变化（维护账号可写，运行账号不可写，故在此直接制造差异）。
    sqlx::query("UPDATE public.compensation_pool SET balance = balance + 1")
        .execute(pool)
        .await
        .expect("制造余额差异");
    assert_ne!(non_loss_snapshot(pool).await, before, "快照必须能检出余额变化");
    sqlx::query("UPDATE public.compensation_pool SET balance = balance - 1")
        .execute(pool)
        .await
        .expect("恢复余额");
    assert_eq!(non_loss_snapshot(pool).await, before);

    // 正例：同命令同请求重放返回原回执，不新增行。
    let replay = runtime
        .test_append_loss_case_outcome(
            command(&commands[2]),
            opened.case_id,
            uuid(POSITION),
            1,
            LossCaseOutcome::Reviewing,
            basis,
            &note("fake review note"),
            (Uuid::new_v4(), Uuid::new_v4()),
        )
        .await
        .expect("重放");
    assert_eq!(replay, receipt);
    assert_eq!(count(pool, "consignment_loss_case_outcomes").await, 2);

    // 反例：同命令异请求（说明不同）冲突，不改历史。
    let conflict = runtime
        .test_append_loss_case_outcome(
            command(&commands[2]),
            opened.case_id,
            uuid(POSITION),
            1,
            LossCaseOutcome::Reviewing,
            basis,
            &note("another fake note"),
            (Uuid::new_v4(), Uuid::new_v4()),
        )
        .await
        .expect_err("同命令异请求应冲突");
    assert_eq!(conflict, ConsignmentError::CommandIdConflict);

    // 反例：同命令同说明但期望版本不同，同样是异请求，冲突且不改历史。
    let version_conflict = runtime
        .test_append_loss_case_outcome(
            command(&commands[2]),
            opened.case_id,
            uuid(POSITION),
            2,
            LossCaseOutcome::Reviewing,
            basis,
            &note("fake review note"),
            (Uuid::new_v4(), Uuid::new_v4()),
        )
        .await
        .expect_err("同命令异版本应冲突");
    assert_eq!(version_conflict, ConsignmentError::CommandIdConflict);
    assert_eq!(count(pool, "consignment_loss_case_outcomes").await, 2);

    // 反例：用已过期的案件版本追加，判 stale_version。
    let stale = runtime
        .test_append_loss_case_outcome(
            command(&commands[3]),
            opened.case_id,
            uuid(POSITION),
            1,
            LossCaseOutcome::Closed,
            basis,
            &note("fake close note"),
            (Uuid::new_v4(), Uuid::new_v4()),
        )
        .await
        .expect_err("过期版本应被拒绝");
    assert_eq!(rejection(stale), DepositBusinessRejection::StaleVersion);

    // 办结后不再接受结果：办结是终态。
    runtime
        .test_append_loss_case_outcome(
            command(&commands[3]),
            opened.case_id,
            uuid(POSITION),
            2,
            LossCaseOutcome::Closed,
            basis,
            &note("fake close note"),
            (Uuid::new_v4(), Uuid::new_v4()),
        )
        .await
        .expect("办结");
    let after_closed = runtime
        .test_append_loss_case_outcome(
            command(&commands[4]),
            opened.case_id,
            uuid(POSITION),
            3,
            LossCaseOutcome::Reviewing,
            basis,
            &note("fake late note"),
            (Uuid::new_v4(), Uuid::new_v4()),
        )
        .await
        .expect_err("办结后不得再追加");
    assert_eq!(rejection(after_closed), DepositBusinessRejection::ActionNotAllowed);
    assert_eq!(count(pool, "consignment_loss_case_outcomes").await, 3);

    // 读模型：卖家看到最新状态，Admin 看到按版本升序的全部记录。
    let seller = runtime
        .test_seller_loss_case(uuid(USER_1), uuid(POSITION))
        .await
        .expect("读取")
        .expect("本人可见");
    assert_eq!(seller.state, "closed");
    let admin = runtime.test_admin_loss_case(uuid(POSITION)).await.expect("读取").expect("案件");
    assert_eq!(admin.case_version, 3);
    assert_eq!(
        admin.outcomes.iter().map(|row| row.outcome.as_str()).collect::<Vec<_>>(),
        ["opened", "reviewing", "closed"]
    );
}

#[tokio::test]
async fn postgres_consignment_loss_outcome_same_version_concurrent_appends_commit_exactly_one() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 4).await;
    let opened = open_case(&runtime, pool, &commands).await;
    let basis = uuid("7c900000-0000-4000-8000-000000000001");
    let (first_note, second_note) = (note("fake concurrent one"), note("fake concurrent two"));

    // 两条不同命令、同一期望版本 1 并发追加：(case_id, case_version) 唯一约束与 Serializable 重试
    // 只放行一条，落败方重试后读到新版本，判 stale_version。
    let (first, second) = tokio::join!(
        runtime.test_append_loss_case_outcome(
            command(&commands[2]),
            opened.case_id,
            uuid(POSITION),
            1,
            LossCaseOutcome::Reviewing,
            basis,
            &first_note,
            (Uuid::new_v4(), Uuid::new_v4()),
        ),
        runtime.test_append_loss_case_outcome(
            command(&commands[3]),
            opened.case_id,
            uuid(POSITION),
            1,
            LossCaseOutcome::Reviewing,
            basis,
            &second_note,
            (Uuid::new_v4(), Uuid::new_v4()),
        ),
    );
    let results = [first, second];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1, "{results:?}");
    let loser = results.into_iter().find_map(Result::err).expect("落败方");
    assert_eq!(rejection(loser), DepositBusinessRejection::StaleVersion);
    assert_eq!(count(pool, "consignment_loss_case_outcomes").await, 2);
    assert_eq!(count(pool, "consignment_loss_receipts").await, 2);
}
