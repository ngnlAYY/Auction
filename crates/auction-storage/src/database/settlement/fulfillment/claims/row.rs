//! 结算履约领取行到 claim 的映射：执行身份错误与上下文事实错误分开归类，后者由领取事务转人工。

use auction_application::settlement::{
    SettlementFulfillmentClaim, SettlementFulfillmentDeliverySnapshot,
    SettlementFulfillmentOperation, SettlementFulfillmentState,
};
use auction_domain::{AuctionId, CommandId, EquipmentId, LotId, SettlementId, SubmissionId};
use auction_time::Timestamp;
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use super::pricing;
use crate::database::settlement::fulfillment::mapping::{
    ClaimMappingError, fulfillment_delivery_status, map_storage_error, obligation_kind,
    repository_error, request_hash_from_optional, settlement_world, string_column, uuid_column,
};
use crate::database::settlement::policy::initial_payment_deadline;

// SQL 行映射的参数分别对应 claim 的租约、版本和尝试次数；拆成共享大结构会越过存储 owner 边界。
#[allow(clippy::too_many_arguments)]
pub(super) fn map_claim(
    row: &PgRow,
    worker_id: &str,
    claim_token: Uuid,
    claim_fencing: i64,
    state_version: u64,
    claimed_at: Timestamp,
    lease_until: Timestamp,
    retry_count: u32,
) -> Result<SettlementFulfillmentClaim, ClaimMappingError> {
    let world = settlement_world(
        string_column(row, "dispatch_world").map_err(ClaimMappingError::Execution)?,
    )
    .map_err(ClaimMappingError::Execution)?;
    let equipment_world = row
        .try_get::<Option<String>, _>("equipment_world")
        .map_err(map_storage_error)
        .map_err(ClaimMappingError::Context)?
        .ok_or_else(|| ClaimMappingError::Context(repository_error("结算履约缺少装备来源世界")))
        .and_then(|value| settlement_world(value).map_err(ClaimMappingError::Context))?;
    let operation = SettlementFulfillmentOperation::from_name(
        &string_column(row, "delivery_operation").map_err(ClaimMappingError::Execution)?,
    )
    .ok_or_else(|| ClaimMappingError::Execution(repository_error("结算履约操作无效")))?;
    let kind =
        obligation_kind(string_column(row, "obligation_kind").map_err(ClaimMappingError::Context)?)
            .map_err(ClaimMappingError::Context)?;
    let recipient_user_id: Option<Uuid> = row
        .try_get("recipient_user_id")
        .map_err(map_storage_error)
        .map_err(ClaimMappingError::Context)?;
    let recipient_user_game_identity: Option<String> = row
        .try_get("recipient_user_game_identity")
        .map_err(map_storage_error)
        .map_err(ClaimMappingError::Context)?;
    let recipient_game_identity: Option<String> = row
        .try_get("recipient_game_identity")
        .map_err(map_storage_error)
        .map_err(ClaimMappingError::Context)?;
    let recipient = match (recipient_user_id, recipient_user_game_identity, recipient_game_identity)
    {
        (Some(_), Some(identity), None) if !identity.is_empty() && identity == identity.trim() => {
            identity
        }
        (None, None, Some(identity)) if !identity.is_empty() && identity == identity.trim() => {
            identity
        }
        _ => {
            return Err(ClaimMappingError::Context(repository_error("结算履约收件人身份不完整")));
        }
    };
    let submission_id: Option<Uuid> = row
        .try_get("submission_id")
        .map_err(map_storage_error)
        .map_err(ClaimMappingError::Context)?;
    let prerequisite_id: Option<Uuid> = row
        .try_get("prerequisite_obligation_id")
        .map_err(map_storage_error)
        .map_err(ClaimMappingError::Context)?;
    let fulfillment_delivery_id: Option<Uuid> = row
        .try_get("fulfillment_delivery_id")
        .map_err(map_storage_error)
        .map_err(ClaimMappingError::Execution)?;
    let explicit_reference_proof = match row
        .try_get::<Option<String>, _>("delivery_external_reference_proven")
        .map_err(map_storage_error)
        .map_err(ClaimMappingError::Execution)?
        .as_deref()
    {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => {
            return Err(ClaimMappingError::Execution(repository_error(
                "结算履约外部引用证明标记无效",
            )));
        }
    };
    let action_reference_proven = row
        .try_get::<Option<String>, _>("delivery_result_operation")
        .map_err(map_storage_error)
        .map_err(ClaimMappingError::Execution)?
        .as_deref()
        == Some("return")
        && row
            .try_get::<Option<String>, _>("delivery_result_proof")
            .map_err(map_storage_error)
            .map_err(ClaimMappingError::Execution)?
            .as_deref()
            == Some("action_removed_or_success_page");
    let delivery_snapshot_id: Option<Uuid> = row
        .try_get("delivery_snapshot_id")
        .map_err(map_storage_error)
        .map_err(ClaimMappingError::Execution)?;
    let delivery_snapshot = match delivery_snapshot_id {
        None => None,
        Some(id) => {
            let prepare_command_id = row
                .try_get::<Option<Uuid>, _>("delivery_prepare_command_id")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?
                .map(CommandId::from_uuid)
                .ok_or_else(|| {
                    ClaimMappingError::Execution(repository_error(
                        "结算履约 delivery 缺少 prepare command",
                    ))
                })?;
            let prepare_request_hash = request_hash_from_optional(
                row.try_get("delivery_prepare_request_digest")
                    .map_err(map_storage_error)
                    .map_err(ClaimMappingError::Execution)?,
            )
            .map_err(ClaimMappingError::Execution)?
            .ok_or_else(|| {
                ClaimMappingError::Execution(repository_error(
                    "结算履约 delivery 缺少 prepare digest",
                ))
            })?;
            let status = fulfillment_delivery_status(
                row.try_get::<Option<String>, _>("delivery_status")
                    .map_err(map_storage_error)
                    .map_err(ClaimMappingError::Execution)?
                    .ok_or_else(|| {
                        ClaimMappingError::Execution(repository_error("结算履约 delivery 缺少状态"))
                    })?,
            )
            .map_err(ClaimMappingError::Execution)?;
            let origin_kind = row
                .try_get::<Option<String>, _>("delivery_origin_kind")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?
                .ok_or_else(|| {
                    ClaimMappingError::Execution(repository_error("结算履约 delivery 缺少 owner"))
                })?;
            let origin_id = row
                .try_get::<Option<String>, _>("delivery_origin_id")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?
                .ok_or_else(|| {
                    ClaimMappingError::Execution(repository_error(
                        "结算履约 delivery 缺少 origin identity",
                    ))
                })?;
            let delivery_recipient = row
                .try_get::<Option<String>, _>("delivery_recipient")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?
                .ok_or_else(|| {
                    ClaimMappingError::Execution(repository_error("结算履约 delivery 缺少收件人"))
                })?;
            let payload = row
                .try_get::<Option<serde_json::Value>, _>("delivery_payload")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?
                .ok_or_else(|| {
                    ClaimMappingError::Execution(repository_error("结算履约 delivery 缺少 payload"))
                })?;
            Some(SettlementFulfillmentDeliverySnapshot {
                id,
                status,
                prepare_command_id,
                prepare_request_hash,
                activate_command_id: row
                    .try_get::<Option<Uuid>, _>("delivery_activate_command_id")
                    .map_err(map_storage_error)
                    .map_err(ClaimMappingError::Execution)?
                    .map(CommandId::from_uuid),
                activate_request_hash: request_hash_from_optional(
                    row.try_get("delivery_activate_request_digest")
                        .map_err(map_storage_error)
                        .map_err(ClaimMappingError::Execution)?,
                )
                .map_err(ClaimMappingError::Execution)?,
                origin_kind,
                origin_id,
                recipient: delivery_recipient,
                payload,
                external_reference: row
                    .try_get("delivery_external_reference")
                    .map_err(map_storage_error)
                    .map_err(ClaimMappingError::Execution)?,
                external_reference_proven: explicit_reference_proof || action_reference_proven,
                failure_code: row
                    .try_get("delivery_failure_code")
                    .map_err(map_storage_error)
                    .map_err(ClaimMappingError::Execution)?,
                // 寄出时刻取数据库投递终态时间，供等待领取的轮询分档；应用层不另取时钟。
                completed_at: row
                    .try_get("delivery_completed_at")
                    .map_err(map_storage_error)
                    .map_err(ClaimMappingError::Execution)?,
            })
        }
    };
    Ok(SettlementFulfillmentClaim {
        obligation_id: SettlementId::from_uuid(
            uuid_column(row, "obligation_id").map_err(ClaimMappingError::Execution)?,
        ),
        auction_id: AuctionId::from_uuid(
            uuid_column(row, "auction_id").map_err(ClaimMappingError::Context)?,
        ),
        lot_id: LotId::from_uuid(uuid_column(row, "lot_id").map_err(ClaimMappingError::Context)?),
        origin_command_id: CommandId::from_uuid(
            uuid_column(row, "origin_command_id").map_err(ClaimMappingError::Context)?,
        ),
        world,
        kind,
        state: SettlementFulfillmentState::from_name(
            &string_column(row, "execution_state").map_err(ClaimMappingError::Execution)?,
        )
        .ok_or_else(|| ClaimMappingError::Execution(repository_error("结算履约阶段无效")))?,
        operation,
        recipient_user_id,
        recipient,
        amount: row
            .try_get("amount")
            .map_err(map_storage_error)
            .map_err(ClaimMappingError::Context)?,
        carrier_credit: u32::try_from(
            row.try_get::<i64, _>("carrier_credit")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Context)?,
        )
        .map_err(|_| {
            ClaimMappingError::Context(repository_error("结算履约 carrier credit 超出范围"))
        })?,
        equipment_id: EquipmentId::from_uuid(
            uuid_column(row, "equipment_id").map_err(ClaimMappingError::Context)?,
        ),
        equipment_world,
        external_equipment_id: string_column(row, "external_equipment_id")
            .map_err(ClaimMappingError::Context)?,
        equipment_name: string_column(row, "equipment_name").map_err(ClaimMappingError::Context)?,
        settlement_line: pricing::map_settlement_line(row).map_err(ClaimMappingError::Context)?,
        settlement_mode: pricing::map_settlement_mode(row).map_err(ClaimMappingError::Context)?,
        // 只有 recovery case 覆盖的付款义务才有期限；历史义务缺列时两者都保持 None。
        payment_deadline_at: row
            .try_get("recovery_payment_deadline_at")
            .map_err(map_storage_error)
            .map_err(ClaimMappingError::Context)?,
        initial_payment_deadline_at: initial_payment_deadline(
            row.try_get("recovery_created_at")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Context)?,
            row.try_get("recovery_policy_version")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Context)?,
        )
        .map_err(|message| ClaimMappingError::Context(repository_error(message)))?,
        submission_id: submission_id.map(SubmissionId::from_uuid),
        submission_source_kind: row
            .try_get("submission_source_kind")
            .map_err(map_storage_error)
            .map_err(ClaimMappingError::Context)?,
        submission_external_mail_id: row
            .try_get("submission_external_mail_id")
            .map_err(map_storage_error)
            .map_err(ClaimMappingError::Context)?,
        prerequisite_id: prerequisite_id.map(SettlementId::from_uuid),
        dispatch_no: u32::try_from(
            row.try_get::<i32, _>("dispatch_no")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?,
        )
        .ok()
        .filter(|value| *value >= auction_domain::FIRST_DISPATCH_NO)
        .ok_or_else(|| ClaimMappingError::Execution(repository_error("结算履约派发编号无效")))?,
        delivery_id: fulfillment_delivery_id,
        prepare_command_id: row
            .try_get::<Option<Uuid>, _>("prepare_command_id")
            .map_err(map_storage_error)
            .map_err(ClaimMappingError::Execution)?
            .map(CommandId::from_uuid),
        prepare_request_hash: request_hash_from_optional(
            row.try_get("prepare_request_digest")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?,
        )
        .map_err(ClaimMappingError::Execution)?,
        activate_command_id: row
            .try_get::<Option<Uuid>, _>("activate_command_id")
            .map_err(map_storage_error)
            .map_err(ClaimMappingError::Execution)?
            .map(CommandId::from_uuid),
        activate_request_hash: request_hash_from_optional(
            row.try_get("activate_request_digest")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?,
        )
        .map_err(ClaimMappingError::Execution)?,
        delivery_snapshot,
        claim_token,
        claim_fencing,
        state_version,
        worker_id: worker_id.to_owned(),
        claimed_at,
        lease_until,
        retry_count,
    })
}
