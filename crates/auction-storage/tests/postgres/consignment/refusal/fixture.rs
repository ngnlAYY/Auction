//! 拒收测试夹具：同一合同的多个退件接管行、提交入口，以及交给第 7 期复用的“达阈值待执行处罚”存量构造器。

use auction_domain::UserId;
use auction_domain::consignment::refusal::{Deliverability, RefusalKind};
use auction_storage::{DatabaseStore, RefusalOutcome, ReturnEvidence, ReturnFactSubmission};
use auction_time::Timestamp;
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use super::super::deposit::support::{USER_1, USER_2, succeeds};
use super::super::ledger136::fixture::{Intake, PURCHASED_AT, refund_dispatch_sql, seed_purchased};

/// 五条资产腿，同一合同同一原轮每腿至多一个接管行；前三条凑 main 阈值 3。
pub const LEGS: [&str; 5] = ["cash_refund", "cash_payout", "cash_principal", "original", "carrier"];

pub fn buyer() -> UserId {
    UserId::from_uuid(Uuid::parse_str(USER_2).expect("夹具买家 UUID"))
}

pub fn seller() -> UserId {
    UserId::from_uuid(Uuid::parse_str(USER_1).expect("夹具卖家 UUID"))
}

/// 每次调用一个不同的命令锁键（只需非空且互不相同）。
pub fn command(n: u128) -> Uuid {
    Uuid::from_u128(0xc6c0_0000_0000_4000_8000_0000_0000_0000 + n)
}

pub fn at(text: &str) -> Timestamp {
    Timestamp::parse_from_rfc3339(text).expect("夹具时刻")
}

/// 第 n 个接管行（n 从 1 起，腿取 [`LEGS`]）。
pub fn intake(n: u32) -> Intake {
    Intake { n, leg: LEGS[(usize::try_from(n).expect("n") - 1) % LEGS.len()] }
}

/// 把夹具接管行的 RTS 发生时刻改成 `rts_at`（默认是购买时刻）。
pub fn intake_sql_at(intake: &Intake, rts_at: &str) -> String {
    let sql = intake.sql();
    let from = format!(
        "'{}', '{}', '{}', decode(repeat('e1'",
        intake.leg,
        intake.observation(),
        PURCHASED_AT
    );
    let to =
        format!("'{}', '{}', '{}', decode(repeat('e1'", intake.leg, intake.observation(), rts_at);
    assert!(sql.contains(&from), "夹具 SQL 形状变了，无法改写 RTS 时刻");
    sql.replace(&from, &to)
}

/// 已购买、有退款腿首派的基础夹具，不含任何接管行。
pub async fn seed_base(owner: &PgPool) {
    seed_purchased(owner).await;
    succeeds(owner, &refund_dispatch_sql()).await;
}

/// 在基础夹具上建 `count` 个接管行，RTS 都发生在购买时刻。
pub async fn seed_intakes(owner: &PgPool, count: u32) {
    for n in 1..=count {
        succeeds(owner, &intake(n).sql()).await;
    }
}

/// 建一个指定 RTS 发生时刻的接管行。
pub async fn seed_intake_at(owner: &PgPool, n: u32, rts_at: &str) {
    succeeds(owner, &intake_sql_at(&intake(n), rts_at)).await;
}

pub fn good_evidence() -> ReturnEvidence {
    ReturnEvidence {
        deliverability: Deliverability::Deliverable,
        paid_or_opposite: false,
        asset_matches: true,
        duplicate: false,
        ownership_proven: true,
    }
}

pub fn submission(n: u32, evidence: ReturnEvidence) -> ReturnFactSubmission {
    ReturnFactSubmission {
        intake_id: Uuid::parse_str(&intake(n).intake()).expect("接管行 UUID"),
        kind: RefusalKind::MainCodReturn,
        proven_at: at("2026-01-01T05:00:00Z"),
        delivery_close_at: None,
        evidence,
    }
}

/// 在买家锁下提交第 n 个接管行的退回事实（证据全部成立）。
pub async fn submit(store: &DatabaseStore, n: u32) -> RefusalOutcome {
    submit_with(store, n, good_evidence()).await
}

pub async fn submit_with(
    store: &DatabaseStore,
    n: u32,
    evidence: ReturnEvidence,
) -> RefusalOutcome {
    store
        .test_refusal_submit(command(u128::from(n)), &[buyer()], &submission(n, evidence), None)
        .await
        .expect("拒收提交应成功")
}

/// 第 7 期复用的存量：买家在 main 合同上已有三份合格拒收，达到冻结阈值 3，
/// 已落一项 pending 处罚、三个构成项与唯一待执行账号工作，新购被阻断。
pub struct PendingSanctionStock {
    pub cycle_id: Uuid,
    pub sanction_id: Uuid,
    pub account_work_id: Uuid,
}

/// 构造“达阈值待执行处罚”存量；`owner` 是维护账号池，`store` 是运行账号。
pub async fn seed_pending_sanction(owner: &PgPool, store: &DatabaseStore) -> PendingSanctionStock {
    seed_base(owner).await;
    seed_intakes(owner, 3).await;
    let mut last = None;
    for n in 1..=3 {
        last = Some(submit(store, n).await);
    }
    let sanction = last.and_then(|outcome| outcome.sanction).expect("第三份拒收应触发处罚");
    PendingSanctionStock {
        cycle_id: sanction.cycle_id,
        sanction_id: sanction.sanction_id,
        account_work_id: sanction.account_work_id,
    }
}

pub async fn count(owner: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM public.{table}")))
        .fetch_one(owner)
        .await
        .expect("计数")
}
