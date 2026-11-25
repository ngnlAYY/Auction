//! 退件接管与消费：同一腿只接一件、同一 intake 只消费一次、新轮必须引用封闭上轮；
//! 现金退件阻断同 UID 的 opening，直到原资金消费同事务解除。

use sqlx::PgPool;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_3};
use super::super::deposit::support::{fails, succeeds};
use super::fixture::{Intake, PURCHASED_AT, REFUND_DELIVERY, refund_dispatch_sql, seed_purchased};
use crate::support::TestDatabase;

const UID: &str = "7001";
const OTHER_UID: &str = "7002";
pub(in crate::consignment) const CONSUMPTION: &str = "c6a00000-0000-4000-8000-000000000001";
const BLOCK: &str = "c6b00000-0000-4000-8000-000000000001";
const FUNDING_RETURN: &str = "c6b00000-0000-4000-8000-000000000002";
const RETURNED_SOURCE: &str = "c6b00000-0000-4000-8000-000000000003";
const SPENT_ENTRY: &str = "c6b00000-0000-4000-8000-000000000004";
const RETURN_ENTRY: &str = "c6b00000-0000-4000-8000-000000000005";
const REFUND_ALLOCATION: &str = "c6b00000-0000-4000-8000-000000000006";

pub(in crate::consignment) async fn seed(pool: &PgPool) {
    seed_purchased(pool).await;
    succeeds(pool, &refund_dispatch_sql()).await;
}

/// 精确 Take 证明加消费：证明与消费在同一事务（消费守卫要求本事务写入的 take_proven 证明）。
pub(in crate::consignment) fn proof_sql(intake: &Intake) -> String {
    format!(
        "INSERT INTO public.mail_disposition_proofs
            (id, work_id, decision_id, delivery_id, fence, attempt_id, kind, asset_leg,
             observation_id, validator_version, digest, conclusion, proven_at)
         VALUES ('{proof}', '{work}', '{decision}', '{take}', 1, '{attempt}', 'exact_take',
                 'cash', '{observation}', 'v1', decode(repeat('a7', 32), 'hex'), 'take_proven',
                 '{PURCHASED_AT}');",
        proof = intake.proof(),
        work = intake.work(),
        decision = intake.decision(),
        take = intake.take_delivery(),
        attempt = intake.attempt(),
        observation = intake.observation(),
    )
}

pub(in crate::consignment) fn consumption_sql(intake: &Intake, consumption: &str) -> String {
    format!(
        "INSERT INTO public.consignment_return_consumptions
            (id, intake_id, contract_id, obligation_id, original_round, leg, bot_uid, world_id,
             external_mid, work_id, delivery_id, attempt_id, fence, proof_id)
         VALUES ('{consumption}', '{intake_id}', '{CONTRACT}', '{OBLIGATION_3}', 1, '{leg}',
                 '12345', 'main', '{mid}', '{work}', '{take}', '{attempt}', 1, '{proof}');",
        intake_id = intake.intake(),
        leg = intake.leg,
        mid = intake.mid(),
        work = intake.work(),
        take = intake.take_delivery(),
        attempt = intake.attempt(),
        proof = intake.proof(),
    )
}

/// 退款腿的新一轮派发：number 是轮号，authorization 是 (上轮 intake, 上轮消费)。
fn round_sql(number: i32, authorization: Option<(&str, &str)>, round_id: &str) -> String {
    let delivery = round_id.replacen("c66", "c75", 1);
    let (prior, intake, consumption) = match authorization {
        Some((intake, consumption)) => {
            (format!("{}", number - 1), format!("'{intake}'"), format!("'{consumption}'"))
        }
        None => ("NULL".to_owned(), "NULL".to_owned(), "NULL".to_owned()),
    };
    format!(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, created_at, updated_at)
         VALUES ('{delivery}', '{delivery}', decode(repeat('c5', 32), 'hex'), 'settlement',
                 '{OBLIGATION_3}', 'FakeSellerTwo', '{{}}'::jsonb, 'prepared', 0, 0,
                 '{PURCHASED_AT}', '{PURCHASED_AT}');
         INSERT INTO public.consignment_dispatch_rounds
            (contract_id, obligation_id, round, round_id, delivery_id, rights_holder_identity,
             bot_uid, world_id, subject, template, body_digest, round_ref, d0, credits, cod,
             prior_round, authorized_by_intake, authorized_by_consumption)
         VALUES ('{CONTRACT}', '{OBLIGATION_3}', {number}, '{round_id}', '{delivery}',
                 'FakeSellerTwo', '12345', 'main', 'fake subject', 'consignment_cod',
                 decode(repeat('d1', 32), 'hex'), '{round_id}', '{PURCHASED_AT}', 0, 101,
                 {prior}, {intake}, {consumption});"
    )
}

#[tokio::test]
async fn postgres_consignment_ledger136_return_intake_and_consumption_happen_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let first = Intake { n: 1, leg: "cash_refund" };
    let second = Intake { n: 2, leg: "cash_refund" };

    // 反例：acquire_return 终局没有同事务 intake，提交时整笔回滚（不留终局决定与 Take 工作）。
    let sql = second.sql();
    let cut = sql.find("INSERT INTO public.consignment_return_intakes").expect("夹具含 intake");
    fails(pool, &sql[..cut], "23514", Some("c4_acquire_return_intake_required")).await;
    let finals: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_final_dispositions WHERE external_mid = '558'",
    )
    .fetch_one(pool)
    .await
    .expect("读取终局数");
    assert_eq!(finals, 0, "回滚后没有终局处置");

    // 正例：同一 K、同一轮同一腿恰一个 intake，随终局与 Take 工作同事务提交。
    succeeds(pool, &first.sql()).await;
    // 反例：第二封不同 MID 主张同一原轮同一腿，不能再接管。
    fails(pool, &second.sql(), "23505", Some("c6_intake_leg_uq")).await;
    succeeds(pool, &first.start_sql()).await;

    // 反例：没有本事务写入的 take_proven 证明，不能消费。
    fails(pool, &consumption_sql(&first, CONSUMPTION), "23514", Some("c6_consumption_take_proof"))
        .await;
    // 反例：同一事务对同一 intake 写第二个消费，被 intake 唯一键拒绝（证明只对应一次消费）。
    fails(
        pool,
        &(proof_sql(&first)
            + &consumption_sql(&first, CONSUMPTION)
            + &consumption_sql(&first, "c6a00000-0000-4000-8000-000000000002")),
        "23505",
        Some("c6_consumption_intake_uq"),
    )
    .await;
    // 正例：证明与消费同事务，恰一次消费。
    succeeds(pool, &(proof_sql(&first) + &consumption_sql(&first, CONSUMPTION))).await;
    let consumed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.consignment_return_consumptions")
            .fetch_one(pool)
            .await
            .expect("读取消费数");
    assert_eq!(consumed, 1);

    // 轮次：同一 (合同, 义务, 轮) 只有一次；新轮必须引用已消费上轮的 intake 与消费。
    let round_id = |n: u32| format!("c6600000-0000-4000-8000-0000000000a{n}");
    fails(pool, &round_sql(1, None, &round_id(1)), "23505", Some("c6_round_pk")).await;
    fails(pool, &round_sql(2, None, &round_id(2)), "23514", Some("c6_round_authorization_ck"))
        .await;
    fails(
        pool,
        &round_sql(
            2,
            Some((&first.intake(), "c6a00000-0000-4000-8000-0000000000ff")),
            &round_id(3),
        ),
        "23503",
        Some("c6_round_consumption_fk"),
    )
    .await;
    succeeds(pool, &round_sql(2, Some((&first.intake(), CONSUMPTION)), &round_id(4))).await;
}

fn block_sql() -> String {
    format!(
        "INSERT INTO public.consignment_cash_return_blocks
            (id, uid, intake_id, leg, original_delivery_id, amount, original_mode, state,
             opened_in_write_start)
         VALUES ('{BLOCK}', '{UID}', '{intake}', 'cash_refund', '{REFUND_DELIVERY}', 101,
                 'strict_cash', 'open', '{work}');",
        intake = Intake { n: 1, leg: "cash_refund" }.intake(),
        work = Intake { n: 1, leg: "cash_refund" }.work(),
    )
}

fn opening_source_sql(uid: &str, kind: &str, id: &str) -> String {
    format!(
        "INSERT INTO public.settlement_funding_accounts (uid, status) VALUES ('{uid}', 'ready')
            ON CONFLICT DO NOTHING;
         INSERT INTO public.settlement_funding_sources (id, uid, kind, source_key, amount, proof_digest)
         VALUES ('{id}', '{uid}', '{kind}', '{id}', 0, decode(repeat('05', 32), 'hex'));"
    )
}

/// 原支出、returned source 一笔完整 return、原资金消费，以及阻断解除：同一事务。
fn funding_return_sql(clear: bool) -> String {
    let intake = Intake { n: 1, leg: "cash_refund" };
    let mut sql = format!(
        "INSERT INTO public.settlement_funding_allocations
            (id, uid, purpose, consignment_contract_id, amount)
         VALUES ('{REFUND_ALLOCATION}', '{UID}', 'refund', '{CONTRACT}', 0);
         INSERT INTO public.settlement_funding_entries
            (id, command_id, uid, debit_id, amount, kind, proof_digest)
         VALUES ('{SPENT_ENTRY}', '{SPENT_ENTRY}', '{UID}', '{REFUND_ALLOCATION}', 101, 'spent',
                 decode(repeat('06', 32), 'hex'));
         INSERT INTO public.settlement_funding_sources (id, uid, kind, source_key, amount, proof_digest)
         VALUES ('{RETURNED_SOURCE}', '{UID}', 'returned_delivery', '{RETURNED_SOURCE}', 101,
                 decode(repeat('07', 32), 'hex'));
         INSERT INTO public.settlement_funding_entries
            (id, command_id, uid, credit_id, source_id, amount, kind, proof_digest)
         VALUES ('{RETURN_ENTRY}', '{RETURN_ENTRY}', '{UID}', '{REFUND_ALLOCATION}',
                 '{RETURNED_SOURCE}', 101, 'return', decode(repeat('07', 32), 'hex'));
         INSERT INTO public.consignment_funding_returns
            (id, uid, contract_id, intake_id, consumption_id, leg, source_id,
             original_delivery_id, original_spent_entry_id, amount)
         VALUES ('{FUNDING_RETURN}', '{UID}', '{CONTRACT}', '{intake_id}', '{CONSUMPTION}',
                 'cash_refund', '{RETURNED_SOURCE}', '{REFUND_DELIVERY}', '{SPENT_ENTRY}', 101);",
        intake_id = intake.intake(),
    );
    if clear {
        sql += &format!(
            "UPDATE public.consignment_cash_return_blocks
             SET state = 'cleared', cleared_by = '{FUNDING_RETURN}', cleared_at = '{PURCHASED_AT}'
             WHERE id = '{BLOCK}';"
        );
    }
    sql
}

#[tokio::test]
async fn postgres_consignment_ledger136_cash_return_block_stops_opening_until_funding_return() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let first = Intake { n: 1, leg: "cash_refund" };
    succeeds(pool, &first.sql()).await;
    succeeds(pool, &first.start_sql()).await;
    // 现金 Take 起写后阻断该 UID 的 opening。
    succeeds(pool, &block_sql()).await;

    // 反例：同 UID 的 opening 来源（含零额建界、record_only）都被拒绝。
    for (kind, id) in [
        ("reconciliation", "c6b10000-0000-4000-8000-000000000001"),
        ("record_opening", "c6b10000-0000-4000-8000-000000000002"),
    ] {
        fails(
            pool,
            &opening_source_sql(UID, kind, id),
            "23514",
            Some("c6_opening_blocked_by_cash_return"),
        )
        .await;
    }
    // 正例：阻断只归本 UID，其他 UID 的 opening 不受影响。
    succeeds(
        pool,
        &opening_source_sql(OTHER_UID, "reconciliation", "c6b10000-0000-4000-8000-000000000003"),
    )
    .await;

    // 反例：没有同事务的原资金消费，阻断不能解除；只有 Take 证明与消费、没有资金消费也不能。
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_cash_return_blocks
             SET state = 'cleared', cleared_by = '{FUNDING_RETURN}', cleared_at = '{PURCHASED_AT}'
             WHERE id = '{BLOCK}';"
        ),
        "23514",
        Some("c6_block_transition"),
    )
    .await;
    fails(
        pool,
        &(proof_sql(&first)
            + &consumption_sql(&first, CONSUMPTION)
            + &format!(
                "UPDATE public.consignment_cash_return_blocks
                 SET state = 'cleared', cleared_by = '{FUNDING_RETURN}',
                     cleared_at = '{PURCHASED_AT}' WHERE id = '{BLOCK}';"
            )),
        "23514",
        Some("c6_block_transition"),
    )
    .await;
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_cash_return_blocks WHERE state = 'open'",
    )
    .fetch_one(pool)
    .await
    .expect("读取阻断数");
    assert_eq!(open, 1, "失败的解除不改变阻断");

    // 正例：证明、消费、原资金消费与解除同一事务，之后该 UID 才能建 opening。
    succeeds(
        pool,
        &(proof_sql(&first) + &consumption_sql(&first, CONSUMPTION) + &funding_return_sql(true)),
    )
    .await;
    succeeds(
        pool,
        &opening_source_sql(UID, "reconciliation", "c6b10000-0000-4000-8000-000000000004"),
    )
    .await;
}
