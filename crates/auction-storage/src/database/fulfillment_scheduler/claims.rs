//! 结算履约 due claim 与 claim payload 映射。

use super::mapping::*;

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn claim_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<SettlementFulfillmentClaim>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    // 领取保持短事务；先按UUID锁义务，再锁execution，避免与满足事务的反向行锁。
    sqlx::query("SELECT id FROM public.settlement_obligations WHERE business_state='ready' ORDER BY id FOR UPDATE SKIP LOCKED")
        .fetch_all(&mut *transaction).await.map_err(map_storage_error)?;
    // ready obligation 可能尚未有 execution 行；只为缺失行补齐当前派发语义，
    // 不回写已有 delivery 的冻结操作。候选由锁保护，避免两个 Worker 同时补写同一行。
    sqlx::query(
        r#"WITH candidates AS MATERIALIZED (
               SELECT o.id, o.obligation_kind, o.world_id
               FROM public.settlement_obligations AS o
               LEFT JOIN public.settlement_obligation_fulfillments AS f
                   ON f.obligation_id = o.id
               LEFT JOIN public.settlement_obligation_fulfillments AS prerequisite
                   ON prerequisite.obligation_id = o.prerequisite_obligation_id
               WHERE o.business_state = 'ready'
                 AND f.obligation_id IS NULL
                 AND (
                     o.prerequisite_obligation_id IS NULL
                     OR prerequisite.execution_state = 'satisfied'
                     OR prerequisite.obligation_id IS NULL
                 )
               ORDER BY o.created_at, o.auction_id, o.lot_id, o.obligation_kind, o.id
               FOR UPDATE OF o SKIP LOCKED
               LIMIT $1
           )
           INSERT INTO public.settlement_obligation_fulfillments
               (obligation_id, execution_state, dispatch_world, delivery_operation)
           SELECT candidates.id,
                  'pending',
                  CASE
                      WHEN candidates.obligation_kind IN (
                          'winner_payment_carrier_cod', 'seller_payout', 'buyer_refund'
                      ) THEN 'main'
                      ELSE candidates.world_id
                  END,
                  'send'
           FROM candidates
           ON CONFLICT (obligation_id) DO NOTHING"#,
    )
    .bind(i64::from(limit))
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    // 上下文关系故意使用 LEFT JOIN 以暴露损坏事实；只有非空的 obligation/fulfillment owner
    // 参与锁定，nullable 关系留给后续校验并可在同一批次中隔离。这样既避免 outer-join 锁错误，
    // 又保持旧版按 obligation 顺序领取时的父子锁保护。
    let rows = sqlx::query(
        r#"SELECT f.obligation_id, f.execution_state, f.state_version,
                  f.claim_fencing, f.retry_count, f.dispatch_world,
                  f.delivery_operation,
                  f.delivery_id AS fulfillment_delivery_id,
                  f.prepare_command_id, f.prepare_request_digest,
                  f.activate_command_id, f.activate_request_digest,
                  delivery.id AS delivery_snapshot_id,
                  delivery.status AS delivery_status,
                  delivery.command_id AS delivery_prepare_command_id,
                  delivery.request_digest AS delivery_prepare_request_digest,
                  delivery.activation_command_id AS delivery_activate_command_id,
                  delivery.activation_request_digest AS delivery_activate_request_digest,
                  delivery.origin_kind AS delivery_origin_kind,
                  delivery.origin_id AS delivery_origin_id,
                  delivery.recipient AS delivery_recipient,
                  delivery.payload AS delivery_payload,
                  delivery.external_reference AS delivery_external_reference,
                  delivery.failure_code AS delivery_failure_code,
                  delivery.result_metadata ->> 'external_reference_proven'
                      AS delivery_external_reference_proven,
                  delivery.result_metadata ->> 'operation' AS delivery_result_operation,
                  delivery.result_metadata ->> 'proof' AS delivery_result_proof,
                  o.id AS obligation_row_id,
                  o.auction_id, o.lot_id, o.origin_command_id, o.obligation_kind,
                  o.world_id AS obligation_world,
                  o.recipient_user_id, recipient_user.username AS recipient_user_game_identity,
                  o.recipient_game_identity, o.amount, o.carrier_credit, o.equipment_id,
                  equipment.id AS equipment_row_id, equipment.world_id AS equipment_world,
                  equipment.external_equipment_id, equipment.name AS equipment_name,
                  o.submission_id, submission.source_kind AS submission_source_kind,
                  submission.source_reference AS submission_source_reference,
                  submission.mailbox_key AS submission_mailbox_key,
                  submission.external_mail_id AS submission_external_mail_id,
                  submission.seller AS submission_seller,
                  o.prerequisite_obligation_id,
                  prerequisite.obligation_id AS prerequisite_row_id
           FROM public.settlement_obligation_fulfillments AS f
           JOIN public.settlement_obligations AS o
               ON o.id = f.obligation_id
           LEFT JOIN public.equipment AS equipment
               ON equipment.id = o.equipment_id
           LEFT JOIN public.users AS recipient_user
               ON recipient_user.id = o.recipient_user_id
           LEFT JOIN public.auction_submissions AS submission
               ON submission.id = o.submission_id
           LEFT JOIN public.settlement_obligation_fulfillments AS prerequisite
               ON prerequisite.obligation_id = o.prerequisite_obligation_id
           LEFT JOIN public.deliveries AS delivery
               ON delivery.id = f.delivery_id
           WHERE f.execution_state IN ('pending', 'prepared', 'enqueued')
             AND (
                 (
                     f.claim_token IS NULL
                     AND (f.next_attempt_at IS NULL OR f.next_attempt_at <= $1)
                 )
                 OR f.claim_expires_at <= $1
             )
             AND o.business_state = 'ready'
             AND (
                 o.prerequisite_obligation_id IS NULL
                 OR prerequisite.execution_state = 'satisfied'
                 OR prerequisite.obligation_id IS NULL
             )
           ORDER BY o.created_at, o.auction_id, o.lot_id, o.obligation_kind, o.id
           LIMIT $2
           FOR UPDATE OF f SKIP LOCKED"#,
    )
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let execution_identity = map_claim_execution_identity(&row)?;
        let equipment_row_id: Option<Uuid> =
            row.try_get("equipment_row_id").map_err(map_storage_error)?;
        let prerequisite_id: Option<Uuid> =
            row.try_get("prerequisite_obligation_id").map_err(map_storage_error)?;
        let prerequisite_row_id: Option<Uuid> =
            row.try_get("prerequisite_row_id").map_err(map_storage_error)?;
        let relation_context_invalid = equipment_row_id.is_none()
            || prerequisite_id.is_some() && prerequisite_row_id.is_none();
        let obligation_id = execution_identity.obligation_id;
        let claim_token = Uuid::new_v4();
        let old_fencing: i64 = row.try_get("claim_fencing").map_err(map_storage_error)?;
        let claim_fencing = old_fencing
            .checked_add(1)
            .ok_or_else(|| repository_error_owned("结算履约 fencing 版本溢出"))?;
        if claim_fencing <= 0 {
            return Err(repository_error_owned("结算履约 fencing 版本无效"));
        }
        let retry_count: i32 = row.try_get("retry_count").map_err(map_storage_error)?;
        let retry_count = u32::try_from(retry_count)
            .map_err(|_| repository_error_owned("结算履约重试次数无效"))?;
        let state_version: i64 = row.try_get("state_version").map_err(map_storage_error)?;
        let next_state_version = state_version
            .checked_add(1)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| repository_error_owned("结算履约状态版本溢出"))?;
        let updated = sqlx::query(
            r#"UPDATE public.settlement_obligation_fulfillments
               SET claim_token = $2,
                   claimed_at = $3,
                   claim_expires_at = $4,
                   claim_fencing = $5,
                   state_version = state_version + 1,
                   next_attempt_at = NULL,
                   updated_at = CURRENT_TIMESTAMP
               WHERE obligation_id = $1
                 AND execution_state IN ('pending', 'prepared', 'enqueued')
                 AND (
                     (
                         claim_token IS NULL
                         AND (next_attempt_at IS NULL OR next_attempt_at <= $3)
                     )
                     OR claim_expires_at <= $3
                 )"#,
        )
        .bind(obligation_id)
        .bind(claim_token)
        .bind(now)
        .bind(lease_until)
        .bind(claim_fencing)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(repository_error_owned("结算履约领取时租约已变化"));
        }

        if relation_context_invalid || validate_claim_context(&row).is_err() {
            manual_reconcile_in_transaction(
                &mut transaction,
                execution_identity.obligation_id,
                &execution_identity.execution_state,
                next_state_version,
                claim_token,
                claim_fencing,
                "invalid_fulfillment_fact",
            )
            .await?;
            continue;
        }

        match map_claim(
            &row,
            worker_id,
            claim_token,
            claim_fencing,
            next_state_version,
            now,
            lease_until,
            retry_count,
        ) {
            Ok(claim) => claims.push(claim),
            Err(ClaimMappingError::Execution(error)) => return Err(error),
            Err(ClaimMappingError::Context(_error)) => {
                manual_reconcile_in_transaction(
                    &mut transaction,
                    execution_identity.obligation_id,
                    &execution_identity.execution_state,
                    next_state_version,
                    claim_token,
                    claim_fencing,
                    "invalid_fulfillment_fact",
                )
                .await?;
            }
        }
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(claims)
}

#[cfg(feature = "postgres")]
// SQL 行映射的参数分别对应 claim 的租约、版本和尝试次数；拆成共享大结构会越过存储 owner 边界。
#[allow(clippy::too_many_arguments)]
pub(super) fn map_claim(
    row: &PgRow,
    worker_id: &str,
    claim_token: Uuid,
    claim_fencing: i64,
    state_version: u64,
    claimed_at: DateTime<Utc>,
    lease_until: DateTime<Utc>,
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
        .ok_or_else(|| {
            ClaimMappingError::Context(repository_error_owned("结算履约缺少装备来源世界"))
        })
        .and_then(|value| settlement_world(value).map_err(ClaimMappingError::Context))?;
    let operation = fulfillment_operation(
        string_column(row, "delivery_operation").map_err(ClaimMappingError::Execution)?,
    )
    .map_err(ClaimMappingError::Execution)?;
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
            return Err(ClaimMappingError::Context(repository_error_owned(
                "结算履约收件人身份不完整",
            )));
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
        None => false,
        Some("true") => true,
        Some("false") => false,
        Some(_) => {
            return Err(ClaimMappingError::Execution(repository_error_owned(
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
                    ClaimMappingError::Execution(repository_error_owned(
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
                ClaimMappingError::Execution(repository_error_owned(
                    "结算履约 delivery 缺少 prepare digest",
                ))
            })?;
            let status = fulfillment_delivery_status(
                row.try_get::<Option<String>, _>("delivery_status")
                    .map_err(map_storage_error)
                    .map_err(ClaimMappingError::Execution)?
                    .ok_or_else(|| {
                        ClaimMappingError::Execution(repository_error_owned(
                            "结算履约 delivery 缺少状态",
                        ))
                    })?,
            )
            .map_err(ClaimMappingError::Execution)?;
            let origin_kind = row
                .try_get::<Option<String>, _>("delivery_origin_kind")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?
                .ok_or_else(|| {
                    ClaimMappingError::Execution(repository_error_owned(
                        "结算履约 delivery 缺少 owner",
                    ))
                })?;
            let origin_id = row
                .try_get::<Option<String>, _>("delivery_origin_id")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?
                .ok_or_else(|| {
                    ClaimMappingError::Execution(repository_error_owned(
                        "结算履约 delivery 缺少 origin identity",
                    ))
                })?;
            let delivery_recipient = row
                .try_get::<Option<String>, _>("delivery_recipient")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?
                .ok_or_else(|| {
                    ClaimMappingError::Execution(repository_error_owned(
                        "结算履约 delivery 缺少收件人",
                    ))
                })?;
            let payload = row
                .try_get::<Option<serde_json::Value>, _>("delivery_payload")
                .map_err(map_storage_error)
                .map_err(ClaimMappingError::Execution)?
                .ok_or_else(|| {
                    ClaimMappingError::Execution(repository_error_owned(
                        "结算履约 delivery 缺少 payload",
                    ))
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
        state: match string_column(row, "execution_state")
            .map_err(ClaimMappingError::Execution)?
            .as_str()
        {
            "pending" => SettlementFulfillmentState::Pending,
            "prepared" => SettlementFulfillmentState::Prepared,
            "enqueued" => SettlementFulfillmentState::Enqueued,
            _ => {
                return Err(ClaimMappingError::Execution(repository_error_owned(
                    "结算履约阶段无效",
                )));
            }
        },
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
            ClaimMappingError::Context(repository_error_owned("结算履约 carrier credit 超出范围"))
        })?,
        equipment_id: EquipmentId::from_uuid(
            uuid_column(row, "equipment_id").map_err(ClaimMappingError::Context)?,
        ),
        equipment_world,
        external_equipment_id: string_column(row, "external_equipment_id")
            .map_err(ClaimMappingError::Context)?,
        equipment_name: string_column(row, "equipment_name").map_err(ClaimMappingError::Context)?,
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
