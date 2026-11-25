//! 核对记录的命令绑定：`consignment_loss_reconciles` 的延迟触发器 `c7_reconcile_command_binding` 只放行
//! “同一 position 上被拒为 `evidence_conflict` 的 `confirm_position_loss` 命令”。
//!
//! 保护的不变量：4、6、8（核对案与命令账本一一对应，不可绕过确认路径伪造）。每个反例各用一条伪造行直接
//! 插入表，用运行账号连接（不关触发器、不摘约束），提交时被拒，表中零行。每个绑定条件（position、操作、
//! 拒绝码）各有独立反例，放宽任一条件都会让对应反例失败。

use auction_application::consignment::PositionLossStore;
use uuid::Uuid;

use super::super::super::deposit::support::{POSITION, fails};
use super::super::sale_recovery::{Closure, World};
use super::{admin_command_id, confirm, record_proof};

const OTHER_POSITION: &str = "7c4b0000-0000-4000-8000-0000000000b1";
const FORGED_OTHER_POSITION: &str = "7c4b0000-0000-4000-8000-0000000000a1";
const FORGED_OTHER_OPERATION: &str = "7c4b0000-0000-4000-8000-0000000000a2";
const MISSING_COMMAND: &str = "7c4b0000-0000-4000-8000-0000000000a3";

/// 一条伪造的账本拒绝回执（409 `evidence_conflict`），再加指向给定 position 的核对记录。
fn forged_rejection_and_reconcile(
    command: &str,
    operation: &str,
    command_target: &str,
    reconcile_position: &str,
) -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{command}', 2, 'admin', 'fake-admin', '{operation}', '{command_target}',
                 decode(repeat('d7', 32), 'hex'), 'position', '{command_target}', NULL, 409,
                 'evidence_conflict',
                 '{{\"schema_version\": 2, \"operation\": \"{operation}\",
                   \"disposition\": \"rejected\", \"command_id\": \"{command}\",
                   \"code\": \"evidence_conflict\"}}',
                 '2026-01-03T00:00:00Z');
         INSERT INTO public.consignment_loss_reconciles (command_id, position_id, recorded_at)
         VALUES ('{command}', '{reconcile_position}', '2026-01-03T00:00:00Z');"
    )
}

fn bare_reconcile(command: &str) -> String {
    format!(
        "INSERT INTO public.consignment_loss_reconciles (command_id, position_id, recorded_at)
         VALUES ('{command}', '{POSITION}', '2026-01-03T00:00:00Z');"
    )
}

/// 反例（表驱动）：别的 position 的命令、别的操作的命令、已成功的确认命令、不存在的命令。
#[tokio::test]
async fn postgres_consignment_loss_reconcile_binding_rejects_forged_rows() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    // 真实的成功确认：它是 200 回执，不是 evidence_conflict 拒绝。
    let proof_id = record_proof(&world, 1).await;
    world.deposit.confirm_position_loss(confirm(1, 3, proof_id, 0x41)).await.expect("确认灭失");
    let success: Uuid = admin_command_id(1).as_uuid();

    let cases = [
        (
            "别的 position 的命令",
            forged_rejection_and_reconcile(
                FORGED_OTHER_POSITION,
                "confirm_position_loss",
                OTHER_POSITION,
                POSITION,
            ),
            "23514",
            "c7_reconcile_command",
        ),
        (
            "别的操作的命令",
            forged_rejection_and_reconcile(
                FORGED_OTHER_OPERATION,
                "record_loss_case_outcome",
                POSITION,
                POSITION,
            ),
            "23514",
            "c7_reconcile_command",
        ),
        ("已成功的确认命令", bare_reconcile(&success.to_string()), "23514", "c7_reconcile_command"),
        // 不存在的命令：命令外键（同样延迟到提交时）先于触发器拒绝。
        ("不存在的命令", bare_reconcile(MISSING_COMMAND), "23503", "c7_reconcile_command_fk"),
    ];
    for (name, sql, state, constraint) in cases {
        eprintln!("伪造核对记录：{name}");
        fails(&world.pool, &sql, state, Some(constraint)).await;
        assert_eq!(world.count("consignment_loss_reconciles").await, 0, "{name}");
    }
}
