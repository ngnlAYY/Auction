//! 拒收处罚：处罚、构成项与唯一账号工作同事务成形；pending 处罚就是新购阻断，周期同时至多一个 open。

use sqlx::PgPool;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_3};
use super::super::deposit::support::{USER_2, fails, succeeds};
use super::fixture::{Intake, PURCHASED_AT, refund_dispatch_sql, seed_purchased};
use crate::support::TestDatabase;

const CYCLE: &str = "c6c00000-0000-4000-8000-000000000001";
const SANCTION: &str = "c6c00000-0000-4000-8000-000000000002";
const ACCOUNT_WORK: &str = "c6c00000-0000-4000-8000-000000000003";

fn fact(n: u32) -> String {
    format!("c6d0000{n}-0000-4000-8000-000000000001")
}

fn cycle_sql() -> String {
    format!(
        "INSERT INTO public.consignment_refusal_cycles (id, buyer_user_id, ordinal, state, opened_at)
         VALUES ('{CYCLE}', '{USER_2}', 1, 'open', '{PURCHASED_AT}');"
    )
}

/// 一个合格拒收：返回事实、合格裁决与拒收。
fn refusal_sql(intake: &Intake) -> String {
    let fact = fact(intake.n);
    let verdict = format!("c6e0000{}-0000-4000-8000-000000000001", intake.n);
    format!(
        "INSERT INTO public.consignment_return_facts
            (return_fact_id, intake_id, contract_id, obligation_id, original_round, leg, kind,
             buyer_user_id, cycle_id, returned_at, returned_minute_end, proven_at,
             delivery_close_at)
         VALUES ('{fact}', '{intake_id}', '{CONTRACT}', '{OBLIGATION_3}', 1, '{leg}',
                 'main_cod_return', '{USER_2}', '{CYCLE}', '2026-01-01T04:00:00Z',
                 '2026-01-01T04:01:00Z', '{PURCHASED_AT}', NULL);
         INSERT INTO public.consignment_return_fact_verdicts
            (id, return_fact_id, verdict, reason, evaluated_at)
         VALUES ('{verdict}', '{fact}', 'qualified', NULL, '{PURCHASED_AT}');
         INSERT INTO public.consignment_refusals
            (id, return_fact_id, verdict_id, buyer_user_id, cycle_id, contract_id, kind,
             qualified_at)
         VALUES ('{verdict}', '{fact}', '{verdict}', '{USER_2}', '{CYCLE}', '{CONTRACT}',
                 'main_cod_return', '{PURCHASED_AT}');",
        intake_id = intake.intake(),
        leg = intake.leg,
    )
}

/// 处罚加 members 个构成项与可选的账号工作；阈值 2（main 默认第三次之外的最小合法值）。
fn sanction_sql(members: u32, work: bool) -> String {
    let mut sql = format!(
        "INSERT INTO public.consignment_sanctions
            (id, buyer_user_id, cycle_id, rule, t0, threshold, window_from, window_to,
             trigger_return_fact_id, state, version)
         VALUES ('{SANCTION}', '{USER_2}', '{CYCLE}', 'main_cod_returns', '{PURCHASED_AT}', 2,
                 NULL, NULL, '{}', 'pending', 1);",
        fact(1)
    );
    for n in 1..=members {
        sql += &format!(
            "INSERT INTO public.consignment_sanction_members (sanction_id, return_fact_id)
             VALUES ('{SANCTION}', '{}');",
            fact(n)
        );
    }
    if work {
        sql += &format!(
            "INSERT INTO public.consignment_account_work (id, sanction_id, buyer_user_id)
             VALUES ('{ACCOUNT_WORK}', '{SANCTION}', '{USER_2}');"
        );
    }
    sql
}

fn sale_probe() -> String {
    format!(
        "INSERT INTO public.consignment_sales
            (contract_id, buyer_user_id, frozen_buyer_identity, listing_version, price, fee,
             seller_amount, pricing_version, fee_bps, payment_deadline_days,
             payment_deadline_at, reminder_limit, cod_return_limit, isekai_refusal_limit,
             isekai_refusal_window_days, funding_uid, funding_mode, funding_boundary_at,
             funding_boundary_source_id, funding_account_version)
         VALUES ('c6f00000-0000-4000-8000-000000000001', '{USER_2}', 'FakeSellerTwo', 4, 101, 5,
                 96, 1, 500, 7, '2026-01-09T00:00:00Z', 1, 3, 5, 90, '7001', 'strict_cash',
                 '2026-01-02T00:00:00Z', NULL, 0);"
    )
}

async fn seed(pool: &PgPool) -> (Intake, Intake) {
    seed_purchased(pool).await;
    succeeds(pool, &refund_dispatch_sql()).await;
    let (first, second) =
        (Intake { n: 1, leg: "cash_refund" }, Intake { n: 2, leg: "cash_payout" });
    succeeds(pool, &first.sql()).await;
    succeeds(pool, &second.sql()).await;
    (first, second)
}

#[tokio::test]
async fn postgres_consignment_ledger136_sanction_commits_whole_and_blocks_new_purchases() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let (first, second) = seed(pool).await;
    let facts = cycle_sql() + &refusal_sql(&first) + &refusal_sql(&second);

    // 反例：构成项不足或缺唯一账号工作，提交时整笔回滚（含合格事实与周期）。
    fails(pool, &(facts.clone() + &sanction_sql(1, true)), "23514", Some("c6_sanction_shape"))
        .await;
    fails(pool, &(facts.clone() + &sanction_sql(2, false)), "23514", Some("c6_sanction_shape"))
        .await;
    let cycles: i64 = sqlx::query_scalar("SELECT count(*) FROM public.consignment_refusal_cycles")
        .fetch_one(pool)
        .await
        .expect("读取周期数");
    assert_eq!(cycles, 0, "失败的处罚事务不留周期");

    // 正例：合格事实、周期、处罚、两个构成项与唯一账号工作一次提交。
    succeeds(pool, &(facts + &sanction_sql(2, true))).await;

    // pending 处罚即新购阻断：新 Sale 在 BEFORE INSERT 触发器被拒（先于外键与延迟图校验）。
    fails(pool, &sale_probe(), "23514", Some("c6_sale_buyer_sanction_pending")).await;
    // 买家同时至多一个 open 周期。
    fails(
        pool,
        &cycle_sql()
            .replace(CYCLE, "c6c00000-0000-4000-8000-0000000000aa")
            .replace("1, 'open'", "2, 'open'"),
        "23505",
        Some("c6_cycle_open_uq"),
    )
    .await;
    // 处罚前进到 applied 后不再阻断：同一探针这次失败在外键（合同不存在），不是处罚阻断。
    succeeds(
        pool,
        &format!(
            "UPDATE public.consignment_sanctions SET state = 'applied', version = 2
             WHERE id = '{SANCTION}';"
        ),
    )
    .await;
    fails(pool, &sale_probe(), "23503", None).await;
}
