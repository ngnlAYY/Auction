//! isekai 处罚存量构造器：第 6 期写端造出的“同一买家、同一有效周期、三张不同 isekai 合同的未付载体退回”，
//! 最早一份 `returned_at` 为 t0−89 天；第三份在季界前退回、季后才补证，补证提交即首次合格（t0），其 Take 已起写、
//! 待核未消费。isekai 处罚与本金例、第 6→7 期跨界例共用（D9 §11.1“处罚及季界”“第 6→7 期跨过 90 天边界”）。
//!
//! 合同经合同图写端、返回事实经 6D3 获证写端、载体本金经资金端口的测试入口造出，不卸触发器、不放宽约束。
//! 接管入口对 isekai 只得待判事实（可交付性无可读事实，S2b 现状），所以第三份先以接管同款证据提交、再以补证重裁。

use auction_application::settlement::{CashLeg, CashPhase, FundingError, SettleCash};
use auction_domain::consignment::funding::{BlockedLeg, FundingMode, OpeningBlock};
use auction_domain::consignment::refusal::{
    Deliverability, PendingReason, Qualification, RefusalKind,
};
use auction_storage::{DatabaseStore, RefusalOutcome, ReturnEvidence, ReturnFactSubmission};
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_3};
use super::super::deposit::support::{CREDENTIAL, SESSION, succeeds};
use super::super::funding::fixture::{CONSUMPTION, consumption_sql, mutation};
use super::fixture::{PendingSanctionStock, at, buyer, command, good_evidence, intake};
use super::isekai::{
    MAIN_CREDENTIAL, MAIN_SESSION, minutes_before_now, seed_intake, seed_isekai_purchased_with,
    switch_to_main_mail,
};

/// 成交冻结的资金 UID（严格现金）。
pub const UID: &str = "7001";
/// 严格现金账户的初始自有余额。
pub const OWN: i64 = 10;
/// 三张合同冻结的 isekai 拒收阈值（D9 默认值）。
const REFUSAL_LIMIT: u8 = 3;
/// 三张合同的接管行编号：都落在现金本金腿（[`intake`] 按编号取腿）。
const INTAKES: [u32; 3] = [3, 8, 13];
/// 第三张合同（Take 待核的那一封）的下标。
const THIRD: usize = 2;

/// 第三封相对季界（有效交割关闭界点）的退回时刻。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThirdReturn {
    /// 季界前两天退回、季后补证：合格，凑足阈值并触发处罚。
    BeforeClose,
    /// 恰在季界退回（关闭等号）：即使会凑足阈值也不处罚，只待判。
    AtClose,
}

/// 构造好的存量。
pub struct IsekaiStock {
    /// 季界：三张合同共用的冻结季窗季末。
    pub close: Timestamp,
    /// 最早一份退回的发生时刻（构造时为数据库当前时刻前 89 天的整分钟）。
    pub earliest_returned_at: Timestamp,
    /// 达阈值时同事务固定的待执行处罚；关闭等号起退回时没有。
    pub sanction: Option<PendingSanctionStock>,
    /// 第三封的接管行。
    pub third_intake_id: Uuid,
    third_contract: String,
    third_carrier: String,
    third_delivery: String,
}

fn submission(n: u32, close: Timestamp, evidence: ReturnEvidence) -> ReturnFactSubmission {
    ReturnFactSubmission {
        intake_id: Uuid::parse_str(&intake(n).intake()).expect("接管行 UUID"),
        kind: RefusalKind::IsekaiUnpaidCarrierReturn,
        proven_at: Timestamp::now().duration_trunc(Duration::microseconds(1)).expect("截断到微秒"),
        delivery_close_at: Some(close),
        evidence,
    }
}

async fn submit(
    store: &DatabaseStore,
    lock_key: u128,
    n: u32,
    close: Timestamp,
    evidence: ReturnEvidence,
) -> RefusalOutcome {
    store
        .test_refusal_submit(command(lock_key), &[buyer()], &submission(n, close, evidence), None)
        .await
        .expect("拒收提交应成功")
}

/// 第三封载体本金 1 按真实派发顺序预留、在途、支出（严格现金由原 UID 自有借出 1）。
async fn spend_third_principal(
    store: &DatabaseStore,
    contract: &str,
    carrier: &str,
    delivery: &str,
) {
    let id = |text: &str| Uuid::parse_str(text).expect("夹具 UUID");
    for (byte, phase) in
        [(0x41, CashPhase::Reserve), (0x42, CashPhase::InFlight), (0x43, CashPhase::Spent)]
    {
        store
            .test_settle_cash(SettleCash {
                mutation: mutation(byte),
                contract_id: id(contract),
                obligation_id: id(carrier),
                delivery_id: id(delivery),
                leg: CashLeg::CarrierPrincipal,
                phase,
                amount: 1,
            })
            .await
            .expect("载体本金状态迁移");
    }
}

/// 构造存量；`owner` 是维护账号池，`store` 是运行账号。
pub async fn seed_isekai_sanction(
    owner: &PgPool,
    store: &DatabaseStore,
    third: ThirdReturn,
) -> IsekaiStock {
    let seeded = seed_isekai_purchased_with(owner, 3, REFUSAL_LIMIT, true).await;
    switch_to_main_mail(owner).await;
    succeeds(
        owner,
        &format!(
            "INSERT INTO public.settlement_funding_allocations (id, uid, purpose, amount)
             VALUES (gen_random_uuid(), '{UID}', 'proprietary', {OWN});"
        ),
    )
    .await;
    let (contract, carrier, delivery, _mid) = seeded.contracts[THIRD].clone();
    spend_third_principal(store, &contract, &carrier, &delivery).await;

    let earliest = minutes_before_now(owner, 89, 0).await;
    let close_text = minutes_before_now(owner, 1, 0).await;
    let third_at = match third {
        ThirdReturn::BeforeClose => minutes_before_now(owner, 2, 0).await,
        ThirdReturn::AtClose => close_text.clone(),
    };
    let close = at(&close_text);
    let returned = [earliest.clone(), minutes_before_now(owner, 30, 0).await, third_at];
    for (k, (n, rts_at)) in INTAKES.iter().zip(&returned).enumerate() {
        seed_intake(owner, &seeded, k, *n, rts_at).await;
    }

    // 前两份：季前退回、可交付，获证即合格；只有两份，未达冻结阈值 3。
    for n in &INTAKES[..THIRD] {
        let outcome = submit(store, u128::from(*n), *n, close, good_evidence()).await;
        assert_eq!(outcome.qualification, Qualification::Qualified);
        assert!(outcome.sanction.is_none(), "两份不同合同未达阈值 3");
    }
    // 第三份先以接管入口同款证据提交（可交付性有争议）：只待判，不凑数、不处罚。
    let n = INTAKES[THIRD];
    let disputed = ReturnEvidence { deliverability: Deliverability::Disputed, ..good_evidence() };
    let pending = submit(store, u128::from(n), n, close, disputed).await;
    assert_eq!(
        pending.qualification,
        Qualification::Pending(PendingReason::DeliverabilityDisputed)
    );
    assert!(pending.sanction.is_none());

    // 第三封 Take 起写：同事务建立该 UID 的现金退件阻断；之后一直待核、不消费。
    let third_intake = intake(n);
    let start = third_intake
        .start_sql()
        .replace(CREDENTIAL, MAIN_CREDENTIAL)
        .replace(SESSION, MAIN_SESSION);
    let third_intake_id = Uuid::parse_str(&third_intake.intake()).expect("接管行 UUID");
    let block = OpeningBlock::new(
        UID.to_owned(),
        third_intake_id,
        BlockedLeg::CarrierPrincipal,
        Uuid::parse_str(&delivery).expect("原载体投递"),
        1,
        FundingMode::StrictCash,
    )
    .expect("阻断记录");
    store.test_open_cash_return_block(&start, &block).await.expect("Take 起写同事务建立阻断");

    // 季后补证：同一获证写端以可交付证据重裁。
    let proven = submit(store, 100 + u128::from(n), n, close, good_evidence()).await;
    let sanction = proven.sanction.map(|record| PendingSanctionStock {
        cycle_id: record.cycle_id,
        sanction_id: record.sanction_id,
        account_work_id: record.account_work_id,
    });
    IsekaiStock {
        close,
        earliest_returned_at: at(&earliest),
        sanction,
        third_intake_id,
        third_contract: contract,
        third_carrier: carrier,
        third_delivery: delivery,
    }
}

impl IsekaiStock {
    /// 迟到的精确 Take：证明、消费行与本金回补同一事务提交（`byte` 决定资金命令 ID，同值即重放）。
    pub async fn consume_third_take(
        &self,
        store: &DatabaseStore,
        byte: u8,
    ) -> Result<(), FundingError> {
        let id = |text: &str| Uuid::parse_str(text).expect("夹具 UUID");
        let proof_and_consumption = consumption_sql(&intake(INTAKES[THIRD]))
            .replace(CONTRACT, &self.third_contract)
            .replace(OBLIGATION_3, &self.third_carrier);
        store
            .test_return_cash(
                &proof_and_consumption,
                mutation(byte),
                [
                    id(&self.third_contract),
                    self.third_intake_id,
                    id(CONSUMPTION),
                    id(&self.third_delivery),
                ],
                BlockedLeg::CarrierPrincipal,
                1,
                [7; 32],
            )
            .await
    }

    /// 同一资金命令的重放：不再写证明与消费行。
    pub async fn replay_third_take(
        &self,
        store: &DatabaseStore,
        byte: u8,
    ) -> Result<(), FundingError> {
        let id = |text: &str| Uuid::parse_str(text).expect("夹具 UUID");
        store
            .test_return_cash(
                "",
                mutation(byte),
                [
                    id(&self.third_contract),
                    self.third_intake_id,
                    id(CONSUMPTION),
                    id(&self.third_delivery),
                ],
                BlockedLeg::CarrierPrincipal,
                1,
                [7; 32],
            )
            .await
    }
}

/// 原 UID 的自有余额。
pub async fn own(owner: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(amount), 0)::bigint FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'proprietary'",
    )
    .bind(UID)
    .fetch_one(owner)
    .await
    .expect("自有余额")
}

/// 第三封的本金回补事实：(资金回补数, return 分录金额合计, rts_principal 结论数, 未解除阻断数)。
pub async fn principal_effects(owner: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_funding_returns),
                (SELECT COALESCE(sum(amount), 0)::bigint FROM public.settlement_funding_entries
                 WHERE uid = $1 AND kind = 'return'),
                (SELECT count(*) FROM public.consignment_principal_facts
                 WHERE conclusion = 'rts_principal'),
                (SELECT count(*) FROM public.consignment_cash_return_blocks WHERE state = 'open')",
    )
    .bind(UID)
    .fetch_one(owner)
    .await
    .expect("本金回补事实")
}
