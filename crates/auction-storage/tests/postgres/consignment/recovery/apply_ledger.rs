//! 处罚执行与解禁周期重置的账本闭集（000137 ⑫）：回执形状、Rust 与 SQL 闭集对照，以及“已禁、仅记录”效果的约束。
//!
//! 保护的不变量：6（失败关闭：缺字段、形状不符一律拒绝）、1（账本闭集与应用闭集逐项对应）、8（效果追加）。

use auction_application::consignment::RecoveryOperation;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{USER_2, fails};
use super::sanction_handoff::Env;

fn receipt(operation: &str, disposition: &str, extra: Value) -> Value {
    let mut body = json!({
        "schema_version": 2,
        "operation": operation,
        "disposition": disposition,
        "command_id": COMMAND,
    });
    if let (Some(target), Value::Object(fields)) = (body.as_object_mut(), extra) {
        target.extend(fields);
    }
    body
}

const COMMAND: &str = "7b200000-0000-5000-8000-000000000001";
const ID_A: &str = "7b200000-0000-4000-8000-0000000000a1";
const ID_B: &str = "7b200000-0000-4000-8000-0000000000b2";
const ID_C: &str = "7b200000-0000-4000-8000-0000000000c3";

async fn valid(pool: &PgPool, body: &Value, operation: &str) -> bool {
    sqlx::query_scalar("SELECT public.c4_valid_v2_receipt($1, $2, $3, 200)")
        .bind(body)
        .bind(Uuid::parse_str(COMMAND).expect("命令 ID"))
        .bind(operation)
        .fetch_one(pool)
        .await
        .expect("回执校验函数")
}

/// 回执形状：applied 必带账号命令，其余状态不带；reset_cycle 的关闭字段键必在、值为 UUID 或 null。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_ledger_receipt_shapes_fail_closed() {
    let env = Env::new().await;
    let pool = env.pool();
    let apply = |disposition: &str, extra: Value| receipt("apply_sanction", disposition, extra);
    let ok = |state: &str, account: Option<&str>| {
        let mut fields = json!({"sanction_id": ID_A, "effect_state": state});
        if let (Some(account), Some(map)) = (account, fields.as_object_mut()) {
            map.insert("account_command_id".into(), json!(account));
        }
        apply("sanction_applied", fields)
    };
    // 正例。
    assert!(valid(pool, &ok("applied", Some(ID_B)), "apply_sanction").await);
    assert!(valid(pool, &ok("cycle_closed", None), "apply_sanction").await);
    assert!(valid(pool, &ok("already_blacklisted", None), "apply_sanction").await);
    // 反例：applied 缺命令；非 applied 带命令；未知状态；错误处置。
    assert!(!valid(pool, &ok("applied", None), "apply_sanction").await);
    assert!(!valid(pool, &ok("cycle_closed", Some(ID_B)), "apply_sanction").await);
    assert!(!valid(pool, &ok("already_blacklisted", Some(ID_B)), "apply_sanction").await);
    assert!(!valid(pool, &ok("unknown_state", None), "apply_sanction").await);
    assert!(
        !valid(
            pool,
            &apply("cycle_opened", json!({"sanction_id": ID_A, "effect_state": "cycle_closed"})),
            "apply_sanction"
        )
        .await
    );

    let reset = |extra: Value| receipt("reset_cycle", "cycle_opened", extra);
    let full = json!({"buyer_user_id": USER_2, "closed_cycle_id": ID_A, "opened_cycle_id": ID_B,
                      "closed_sanction_id": ID_C});
    assert!(valid(pool, &reset(full.clone()), "reset_cycle").await);
    let nothing_closed = json!({"buyer_user_id": USER_2, "closed_cycle_id": null,
                                "opened_cycle_id": ID_B, "closed_sanction_id": null});
    assert!(valid(pool, &reset(nothing_closed), "reset_cycle").await);
    // 反例：有关闭的处罚却没有关闭的周期；缺键；错误处置。
    let orphan = json!({"buyer_user_id": USER_2, "closed_cycle_id": null,
                        "opened_cycle_id": ID_B, "closed_sanction_id": ID_C});
    assert!(!valid(pool, &reset(orphan), "reset_cycle").await);
    let missing = json!({"buyer_user_id": USER_2, "opened_cycle_id": ID_B});
    assert!(!valid(pool, &reset(missing), "reset_cycle").await);
    assert!(!valid(pool, &receipt("reset_cycle", "sanction_applied", full), "reset_cycle").await);
}

/// Rust 的 `RecoveryOperation` 闭集与 SQL 账本 CHECK 逐项对照：每个操作码都出现在 actor 与 target 约束里，
/// 且新目标种类 sanction 在类型化目标约束里。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_ledger_closed_sets_match_application() {
    let env = Env::new().await;
    let definition = |name: &'static str| {
        let pool = env.pool().clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT pg_get_constraintdef(oid) FROM pg_constraint
                 WHERE conname = $1 AND conrelid = 'public.consignment_commands'::regclass",
            )
            .bind(name)
            .fetch_one(&pool)
            .await
            .expect("约束定义")
        }
    };
    let (actor, target, typed) = (
        definition("consignment_command_actor").await,
        definition("consignment_command_target").await,
        definition("c4_command_typed_target_ck").await,
    );
    for operation in RecoveryOperation::ALL {
        let literal = format!("'{}'", operation.as_str());
        assert!(actor.contains(&literal), "actor 约束缺 {literal}");
        assert!(target.contains(&literal), "target 约束缺 {literal}");
    }
    assert!(typed.contains("'sanction'") && typed.contains("'buyer_cycle'"));
}

/// 反例：账号并未被禁时不能记“已禁、仅记录”（触发器核对账号当前确为 blacklisted）。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_ledger_already_blacklisted_needs_a_blacklisted_account()
 {
    let env = Env::new().await;
    let (sanction, work) = (env.stock.sanction_id, env.stock.account_work_id);
    let sql = format!(
        "UPDATE public.consignment_account_work SET state = 'claimed', version = 2 WHERE id = '{work}';
         UPDATE public.consignment_account_work SET state = 'completed', version = 3 WHERE id = '{work}';
         UPDATE public.consignment_sanctions SET state = 'applied', version = 2 WHERE id = '{sanction}';
         INSERT INTO public.consignment_sanction_effects
            (sanction_id, buyer_user_id, state, account_command_id, applied_at)
         VALUES ('{sanction}', '{USER_2}', 'already_blacklisted', NULL, now());
         INSERT INTO public.consignment_sanction_work_results
            (work_id, sanction_id, result, recorded_at)
         VALUES ('{work}', '{sanction}', 'already_blacklisted', now());"
    );
    fails(env.pool(), &sql, "23514", Some("c7_effect_projection")).await;
}
