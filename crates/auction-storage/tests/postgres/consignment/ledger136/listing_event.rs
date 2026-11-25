//! 挂单事件核对：000136 为新增 `prove_handoff` 改写了 closed 分支的括号形状，本模块钉住
//! withdraw / admin_delist 回执仍须核对 `listing_id`、`listing_version`、`state` 与 `disposition = 'closed'`。

use super::super::deposit::support::{
    LISTING, USER_1, check, fails, listing_awaiting_deposit, succeeds, users_and_key,
};
use crate::support::TestDatabase;

const WITHDRAW_COMMAND: &str = "c6d00000-0000-4000-8000-000000000001";
const CLOSED_AT: &str = "2026-01-01T06:00:00Z";

/// 卖家撤回尚未收件的挂单（awaiting_deposit -> closed，版本 3），回执为 v2；
/// `receipt_version` 写进回执的 `listing_version`，None 表示回执缺该字段。
fn withdraw_sql(receipt_version: Option<i32>) -> String {
    let version = receipt_version
        .map_or_else(String::new, |version| format!("\"listing_version\":{version},"));
    format!(
        "UPDATE public.consignment_listings
         SET state = 'closed', state_version = 3, reason_code = 'seller_cancelled',
             updated_at = '{CLOSED_AT}'
         WHERE id = '{LISTING}';
         INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{WITHDRAW_COMMAND}', 2, 'public_user', '{USER_1}', 'withdraw', '{LISTING}',
                 decode(repeat('b3', 32), 'hex'), 'listing', '{LISTING}', '{LISTING}', 200,
                 'ok',
                 '{{\"schema_version\":2,\"operation\":\"withdraw\",\"disposition\":\"closed\",
                    \"command_id\":\"{WITHDRAW_COMMAND}\",\"listing_id\":\"{LISTING}\",
                    {version}\"state\":\"closed\"}}', '{CLOSED_AT}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 3, '{WITHDRAW_COMMAND}', 'public_user', 'closed',
                 'awaiting_deposit', 'closed', 'seller_cancelled', 101, '{CLOSED_AT}');"
    )
}

#[tokio::test]
async fn postgres_consignment_ledger136_withdraw_receipt_must_carry_listing_version() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_awaiting_deposit())).await;

    // 反例：回执的 listing_version 与挂单新版本不符。回执形状校验只认版本格式，
    // 与挂单版本的比对只由同事务事件核对完成（旧优先级下 withdraw 分支会跳过它）。
    check(pool, &withdraw_sql(Some(99)), "consignment_listing_version_event").await;
    // 反例：回执缺 listing_version，被回执形状校验拒绝，仍不得提交。
    fails(pool, &withdraw_sql(None), "23514", None).await;
    // 正例：回执核对 listing_id、listing_version、state 与 disposition 后提交。
    succeeds(pool, &withdraw_sql(Some(3))).await;
}
