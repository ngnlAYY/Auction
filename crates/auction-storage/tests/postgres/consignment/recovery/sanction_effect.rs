//! 处罚效果与账号工作结果：只在处罚与账号工作投影同事务前进、且 applied 带该买家的拉黑命令时落库。
//!
//! 保护的不变量：1（同一处罚业务键只对应一条账号命令）、8（效果追加后不可改写）、6（失败关闭）。

use uuid::Uuid;

use super::super::deposit::support::{USER_2, fails, succeeds};
use super::super::refusal::fixture::seed_pending_sanction;
use crate::support::TestDatabase;

const ACCOUNT_COMMAND: &str = "c7a00000-0000-4000-8000-000000000001";
const APPLIED_AT: &str = "2026-02-01T00:00:00Z";

fn account_command() -> String {
    format!(
        "INSERT INTO public.user_admin_commands
            (command_id, command_kind, request_digest, user_id, result_payload, created_at)
         VALUES ('{ACCOUNT_COMMAND}', 'blacklist', decode(repeat('e7', 32), 'hex'), '{USER_2}',
                 '{{}}'::jsonb, '{APPLIED_AT}');"
    )
}

fn projections(sanction: Uuid, work: Uuid) -> String {
    format!(
        "UPDATE public.consignment_sanctions SET state = 'applied', version = 2
         WHERE id = '{sanction}';
         UPDATE public.consignment_account_work SET state = 'completed', version = 3
         WHERE id = '{work}';"
    )
}

fn effect(sanction: Uuid, command: &str) -> String {
    format!(
        "INSERT INTO public.consignment_sanction_effects
            (sanction_id, buyer_user_id, state, account_command_id, applied_at)
         VALUES ('{sanction}', '{USER_2}', 'applied', {command}, '{APPLIED_AT}');"
    )
}

fn work_result(work: Uuid, sanction: Uuid) -> String {
    format!(
        "INSERT INTO public.consignment_sanction_work_results
            (work_id, sanction_id, result, recorded_at)
         VALUES ('{work}', '{sanction}', 'applied', '{APPLIED_AT}');"
    )
}

#[tokio::test]
async fn postgres_consignment_recovery_sanction_effect_follows_projections_and_account_command() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let stock = seed_pending_sanction(pool, &store).await;
    let (sanction, work) = (stock.sanction_id, stock.account_work_id);
    succeeds(
        pool,
        &format!(
            "UPDATE public.consignment_account_work SET state = 'claimed', version = 2
             WHERE id = '{work}';"
        ),
    )
    .await;

    // 反例：处罚仍是 pending 时写 applied 效果。
    fails(
        pool,
        &(account_command() + &effect(sanction, &format!("'{ACCOUNT_COMMAND}'"))),
        "23514",
        Some("c7_effect_projection"),
    )
    .await;
    // 反例：applied 没有账号命令。
    fails(
        pool,
        &(projections(sanction, work) + &effect(sanction, "NULL")),
        "23514",
        Some("c7_effect_command_ck"),
    )
    .await;
    // 反例：效果在但账号工作没有同事务走到 completed，结果行被拒。
    fails(
        pool,
        &(account_command()
            + "UPDATE public.consignment_sanctions SET state = 'applied', version = 2
               WHERE id = '"
            + &sanction.to_string()
            + "';"
            + &effect(sanction, &format!("'{ACCOUNT_COMMAND}'"))
            + &work_result(work, sanction)),
        "23514",
        Some("c7_work_result_projection"),
    )
    .await;

    // 正例：账号命令、处罚与账号工作投影、效果、结果同一事务。
    succeeds(
        pool,
        &(account_command()
            + &projections(sanction, work)
            + &effect(sanction, &format!("'{ACCOUNT_COMMAND}'"))
            + &work_result(work, sanction)),
    )
    .await;
    // 效果不可改写（追加式）。
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_sanction_effects SET state = 'cycle_closed'
             WHERE sanction_id = '{sanction}';"
        ),
        "55000",
        None,
    )
    .await;
}
