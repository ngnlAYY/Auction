//! 代管事件的证明引用：延迟触发器 `c7_custody_event_proof` 取代原外键后，INSERT 与 UPDATE 都按事件种类核对
//! 证明存在（非 lost 认邮件证明，lost 认同 position 的损失证据）；被引用的证明不可删除或改键（原 RESTRICT）。
//!
//! 保护的不变量：3（证据引用必须存在）、8（证据只追加，引用不可改写）。

use super::super::contracts::fixture::seed_main_custody;
use super::super::deposit::support::{
    LISTING, POSITION, PROOF as MAIL_PROOF, PROOF_COMMAND, fails, succeeds,
};
use super::super::ledger136::fixture::available_sql;
use super::command_sql::{confirm_loss_sql, loss_proof_sql};
use super::{PROOF as LOSS_PROOF, T0};
use crate::support::TestDatabase;

const MISSING_PROOF: &str = "deadbeef-0000-4000-8000-00000000000c";

/// 版本 3 的 reconcile 事件，proof_id 可指定；命令借用已提交的取得证明命令。
fn reconcile_event(proof: &str) -> String {
    format!(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 3, '{PROOF_COMMAND}', 'reconcile', 'in_custody', 'reconcile',
                 '{proof}', '{LISTING}', 4, '{T0}');"
    )
}

/// 只关掉追加表触发器（按名字），保留本任务的引用触发器；模拟拥有者绕过追加保护的最坏情形。
fn without_append_only(table: &str, trigger: &str, sql: &str) -> String {
    format!("ALTER TABLE public.{table} DISABLE TRIGGER {trigger}; {sql}")
}

#[tokio::test]
async fn postgres_consignment_recovery_custody_event_mail_proof_must_exist() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;

    // 反例：非 lost 事件引用不存在的邮件证明。
    fails(pool, &reconcile_event(MISSING_PROOF), "23514", Some("c7_custody_event_mail_proof"))
        .await;
    // 正例：引用已存在的邮件证明。
    succeeds(pool, &reconcile_event(MAIL_PROOF)).await;
}

#[tokio::test]
async fn postgres_consignment_recovery_custody_event_proof_reference_is_kept_on_update_and_delete()
{
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &available_sql()).await;
    succeeds(pool, &loss_proof_sql()).await;
    let events = "consignment_custody_events";
    let events_trigger = "consignment_custody_events_append_only";

    // 反例（原 c4_custody_event_proof_fk 的 UPDATE 用例）：把已落库事件的 proof_id 改成不存在的证明。
    let retarget = |proof: &str| {
        format!(
            "UPDATE public.consignment_custody_events SET proof_id = '{proof}'
             WHERE position_id = '{POSITION}' AND version = 2;"
        )
    };
    fails(
        pool,
        &without_append_only(events, events_trigger, &retarget(MISSING_PROOF)),
        "23514",
        Some("c7_custody_event_mail_proof"),
    )
    .await;
    // 反例：改成损失证据（非 lost 事件必须引用邮件证明）。
    fails(
        pool,
        &without_append_only(events, events_trigger, &retarget(LOSS_PROOF)),
        "23514",
        Some("c7_custody_event_mail_proof"),
    )
    .await;
    // 正例：改回同一条已存在的邮件证明。
    succeeds(pool, &without_append_only(events, events_trigger, &retarget(MAIL_PROOF))).await;

    // 反例（原 ON DELETE RESTRICT）：被事件引用的邮件证明不可删除或改键。
    let mail_trigger = "mail_disposition_proofs_append_only";
    fails(
        pool,
        &without_append_only(
            "mail_disposition_proofs",
            mail_trigger,
            &format!("DELETE FROM public.mail_disposition_proofs WHERE id = '{MAIL_PROOF}';"),
        ),
        "23503",
        Some("c7_custody_event_proof_ref"),
    )
    .await;
    fails(
        pool,
        &without_append_only(
            "mail_disposition_proofs",
            mail_trigger,
            &format!(
                "UPDATE public.mail_disposition_proofs SET id = '{MISSING_PROOF}'
                 WHERE id = '{MAIL_PROOF}';"
            ),
        ),
        "23503",
        Some("c7_custody_event_proof_ref"),
    )
    .await;

    // lost 事件引用损失证据：先走完整灭失确认，再核对改写与删除。
    succeeds(pool, &confirm_loss_sql()).await;
    let lost_retarget = format!(
        "UPDATE public.consignment_custody_events SET proof_id = '{MAIL_PROOF}'
         WHERE position_id = '{POSITION}' AND event_kind = 'lost';"
    );
    fails(
        pool,
        &without_append_only(events, events_trigger, &lost_retarget),
        "23514",
        Some("c7_custody_event_loss_proof"),
    )
    .await;
    let loss_trigger = "c7_consignment_loss_proofs_append_only";
    fails(
        pool,
        &without_append_only(
            "consignment_loss_proofs",
            loss_trigger,
            &format!("DELETE FROM public.consignment_loss_proofs WHERE proof_id = '{LOSS_PROOF}';"),
        ),
        "23503",
        Some("c7_custody_event_proof_ref"),
    )
    .await;
}
