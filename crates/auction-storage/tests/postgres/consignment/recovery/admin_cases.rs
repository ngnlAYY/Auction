//! Admin 损失案件端口的存储实现：读案件、追加人工结果（版本仲裁、同命令重放）与失败关闭（不变量 1、2、8）。
//!
//! 人工结果经命令账本仲裁（7C-4b 合并端口后补上账本行）：每条结果用新的 UUIDv4 命令，账本回执与结果行同提。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, ListingDelistReason, LossCaseAdmin,
    LossCaseOutcome, LossCaseState, RecordLossCaseOutcomeCommand,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_storage::{DatabaseStore, OpenedLossCase};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{POSITION, USER_1, succeeds};
use super::{CASE, FACT, PROOF, command_ids, fact_sql, proof_sql, seed};
use crate::support::TestDatabase;

const BASIS: &str = "7c900000-0000-4000-8000-000000000001";
/// 人工结果的 Admin 命令（UUIDv4，账本里原本不存在）。
const OUTCOME_1: &str = "7ca00000-0000-4000-8000-000000000001";
const OUTCOME_2: &str = "7ca00000-0000-4000-8000-000000000002";

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("夹具 UUID")
}

fn outcome_command(
    command: &str,
    expected_case_version: u64,
    note: &str,
) -> RecordLossCaseOutcomeCommand {
    RecordLossCaseOutcomeCommand {
        admin_actor: "fixture-admin".to_owned(),
        command_id: CommandId::from_uuid(uuid(command)),
        position_id: uuid(POSITION),
        case_id: uuid(CASE),
        expected_case_version,
        outcome: LossCaseOutcome::Reviewing,
        basis_id: uuid(BASIS),
        private_note: ListingDelistReason::new(note.to_owned()).expect("夹具说明"),
        request_hash: RequestHash::new([7; 32]),
        request_id: None,
    }
}

async fn open_case(runtime: &DatabaseStore, pool: &PgPool, commands: &[String]) {
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
        .test_open_loss_case(
            CommandId::from_uuid(uuid(&commands[1])),
            uuid(FACT),
            uuid(POSITION),
            uuid(USER_1),
            ids,
        )
        .await
        .expect("开案");
}

async fn outcome_rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.consignment_loss_case_outcomes")
        .fetch_one(pool)
        .await
        .expect("读取行数")
}

#[tokio::test]
async fn postgres_consignment_loss_admin_port_appends_replays_and_arbitrates_versions() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 5).await;
    open_case(&runtime, pool, &commands).await;

    let opened = runtime.admin_loss_case(uuid(POSITION)).await.expect("读案件").expect("有案件");
    assert_eq!((opened.case_version, opened.state), (1, LossCaseState::Opened));

    // 正例：版本 1 之后追加人工结果，案件读到版本 2 与复核中。
    let receipt = runtime
        .record_loss_case_outcome(outcome_command(OUTCOME_1, 1, "fake review note"))
        .await
        .expect("追加复核中");
    assert_eq!(receipt.case_version, 2);
    let reviewing = runtime.admin_loss_case(uuid(POSITION)).await.expect("读案件").expect("有案件");
    assert_eq!((reviewing.case_version, reviewing.state), (2, LossCaseState::Reviewing));
    assert_eq!(reviewing.outcomes.len(), 2);

    // 正例：同命令同请求重放返回原回执，不新增行。
    let replay = runtime
        .record_loss_case_outcome(outcome_command(OUTCOME_1, 1, "fake review note"))
        .await
        .expect("重放");
    assert_eq!(replay, receipt);
    assert_eq!(outcome_rows(pool).await, 2);

    // 反例：另一条新命令仍按版本 1 提交，判 stale_version，不追加。
    let stale = runtime
        .record_loss_case_outcome(outcome_command(OUTCOME_2, 1, "fake other note"))
        .await
        .expect_err("版本落后");
    assert!(matches!(stale, ConsignmentError::Deposit(DepositBusinessRejection::StaleVersion)));
    assert_eq!(outcome_rows(pool).await, 2);
}

#[tokio::test]
async fn postgres_consignment_loss_admin_port_rejects_a_reused_command_id_with_another_request() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 5).await;
    open_case(&runtime, pool, &commands).await;
    runtime
        .record_loss_case_outcome(outcome_command(OUTCOME_1, 1, "fake review note"))
        .await
        .expect("追加复核中");

    // 同命令 ID 换请求摘要：command_id_conflict，不追加第二条结果。
    let mut other = outcome_command(OUTCOME_1, 1, "fake review note");
    other.request_hash = RequestHash::new([8; 32]);
    let conflict = runtime.record_loss_case_outcome(other).await.expect_err("同 ID 异请求");
    assert_eq!(conflict, ConsignmentError::CommandIdConflict);
    // 账本里已有的、操作不同的命令 ID 也不能被复用为人工结果命令。
    let reused = runtime
        .record_loss_case_outcome(outcome_command(&commands[2], 2, "fake other note"))
        .await
        .expect_err("复用他操作的命令 ID");
    assert_eq!(reused, ConsignmentError::CommandIdConflict);
    assert_eq!(outcome_rows(pool).await, 2);
}
