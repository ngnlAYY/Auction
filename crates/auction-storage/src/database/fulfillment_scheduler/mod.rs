//! PostgreSQL 结算履约领取与 prepared delivery outbox 适配器。
//!
//! 这一层只负责持久化 claim/fence 和 owner 间的 canonical payload。它不读取 secret、不发起
//! 外部 HTTP；delivery owner 必须在本地事务提交后再消费 `deliveries`，并把结果通过后续
//! fulfillment 观察流程回写。

use auction_application::{
    ApplicationError, SettlementDeliveryPreparation, SettlementFulfillmentClaim,
    SettlementFulfillmentProof, SettlementFulfillmentReconcileResult, SettlementFulfillmentResult,
    SettlementFulfillmentStore,
};
use auction_domain::CommandId;
use chrono::{DateTime, Utc};

#[cfg(feature = "postgres")]
use auction_application::{
    SettlementFulfillmentOperation, SettlementFulfillmentReconcileStatus,
    SettlementFulfillmentState,
};
#[cfg(feature = "postgres")]
use auction_domain::{
    AuctionId, EquipmentId, LotId, SettlementId, SettlementObligationKind, SettlementWorld,
    SubmissionId,
};
#[cfg(feature = "postgres")]
use uuid::Uuid;

use super::DatabaseStore;
#[cfg(feature = "postgres")]
use super::delivery_audit::{ACTIVATE, DeliveryAudit, PREPARE, append as append_delivery_audit};

#[cfg(feature = "postgres")]
use sqlx::{Postgres, Row, postgres::PgRow};

impl SettlementFulfillmentStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> auction_application::ApplicationFuture<'_, Vec<SettlementFulfillmentClaim>> {
        Box::pin(async move {
            if worker_id.is_empty()
                || worker_id != worker_id.trim()
                || limit == 0
                || lease_until <= now
            {
                return Err(repository_error("结算履约领取参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim_postgres(store.pool(), &worker_id, now, lease_until, limit)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => {
                    Err(ApplicationError::CapabilityUnavailable("结算履约仅支持 PostgreSQL 完整档"))
                }
            }
        })
    }

    fn prepare_delivery(
        &self,
        claim: SettlementFulfillmentClaim,
        preparation: SettlementDeliveryPreparation,
        _prepared_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, SettlementFulfillmentResult> {
        Box::pin(async move {
            if preparation.recipient != claim.recipient || preparation.payload.is_null() {
                return Err(repository_error("结算履约 prepared payload 与 claim 不一致"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    prepare_postgres(store.pool(), &claim, &preparation, _prepared_at)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => {
                    Err(ApplicationError::CapabilityUnavailable("结算履约仅支持 PostgreSQL 完整档"))
                }
            }
        })
    }

    fn activate_delivery(
        &self,
        _claim: SettlementFulfillmentClaim,
        _activate_command_id: CommandId,
        _activate_request_hash: auction_application::RequestHash,
        _activated_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<
        '_,
        auction_application::SettlementFulfillmentActivationResult,
    > {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => activate_postgres(
                    store.pool(),
                    &_claim,
                    _activate_command_id,
                    _activate_request_hash,
                    _activated_at,
                )
                .await
                .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => {
                    Err(ApplicationError::CapabilityUnavailable("结算履约仅支持 PostgreSQL 完整档"))
                }
            }
        })
    }

    fn satisfy(
        &self,
        _claim: SettlementFulfillmentClaim,
        _proof: SettlementFulfillmentProof,
    ) -> auction_application::ApplicationFuture<'_, SettlementFulfillmentReconcileResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => satisfy_postgres(store.pool(), &_claim, &_proof)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => {
                    Err(ApplicationError::CapabilityUnavailable("结算履约仅支持 PostgreSQL 完整档"))
                }
            }
        })
    }

    fn manual_reconcile(
        &self,
        _claim: SettlementFulfillmentClaim,
        failure_code: String,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 64);
            if !valid_failure_code(&failure_code) {
                return Err(repository_error("结算履约人工复核代码无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    manual_reconcile_postgres(store.pool(), &_claim, &failure_code)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => {
                    Err(ApplicationError::CapabilityUnavailable("结算履约仅支持 PostgreSQL 完整档"))
                }
            }
        })
    }

    fn fail(
        &self,
        _claim: SettlementFulfillmentClaim,
        _retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 64);
            let safe_detail = bounded_text(safe_detail, 512);
            if !valid_failure_code(&failure_code) || safe_detail.is_empty() {
                return Err(repository_error("结算履约失败记录无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    fail_postgres(store.pool(), &_claim, _retry_at, &failure_code, &safe_detail)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => {
                    Err(ApplicationError::CapabilityUnavailable("结算履约仅支持 PostgreSQL 完整档"))
                }
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn claim_postgres(
    pool: &super::TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<SettlementFulfillmentClaim>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let rows = sqlx::query(
        r#"SELECT f.obligation_id, f.execution_state, f.state_version,
                  f.claim_fencing, f.retry_count, f.dispatch_world,
                  f.delivery_operation,
                  f.delivery_id, f.prepare_command_id, f.prepare_request_digest,
                  delivery.external_reference AS delivery_external_reference,
                  delivery.result_metadata ->> 'external_reference_proven'
                      AS delivery_external_reference_proven,
                  o.auction_id, o.lot_id, o.origin_command_id, o.obligation_kind,
                  o.recipient_user_id, recipient_user.game_player_id AS recipient_user_game_identity,
                  o.recipient_game_identity, o.amount, o.carrier_credit, o.equipment_id,
                  equipment.external_equipment_id, equipment.name AS equipment_name,
                  o.submission_id, submission.source_kind AS submission_source_kind,
                  submission.external_mail_id AS submission_external_mail_id,
                  o.prerequisite_obligation_id
           FROM public.settlement_obligation_fulfillments AS f
           INNER JOIN public.settlement_obligations AS o
               ON o.id = f.obligation_id
           INNER JOIN public.equipment AS equipment
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
             AND (f.execution_state <> 'enqueued' OR delivery.status = 'succeeded')
             AND (f.claim_token IS NULL OR f.claim_expires_at <= $1)
             AND (f.next_attempt_at IS NULL OR f.next_attempt_at <= $1)
             AND o.business_state = 'ready'
             AND (
                 o.prerequisite_obligation_id IS NULL
                 OR prerequisite.execution_state = 'satisfied'
             )
           ORDER BY COALESCE(f.next_attempt_at, f.created_at), f.obligation_id
           LIMIT $2
           FOR UPDATE OF f, o SKIP LOCKED"#,
    )
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let obligation_id = uuid_column(&row, "obligation_id")?;
        let claim_token = Uuid::new_v4();
        let old_fencing: i64 = row.try_get("claim_fencing").map_err(map_storage_error)?;
        let claim_fencing = old_fencing
            .checked_add(1)
            .ok_or_else(|| repository_error_owned("结算履约 fencing 版本溢出"))?;
        if claim_fencing <= 0 {
            return Err(repository_error_owned("结算履约 fencing 版本无效"));
        }
        let old_attempt_count: i32 = row.try_get("retry_count").map_err(map_storage_error)?;
        let attempt_count = old_attempt_count
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| repository_error_owned("结算履约尝试次数溢出"))?;
        let state_version: i64 = row.try_get("state_version").map_err(map_storage_error)?;
        let next_state_version = state_version
            .checked_add(1)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| repository_error_owned("结算履约状态版本溢出"))?;
        let attempt_count_db = i32::try_from(attempt_count)
            .map_err(|_| repository_error_owned("结算履约尝试次数超出数据库范围"))?;
        let updated = sqlx::query(
            r#"UPDATE public.settlement_obligation_fulfillments
               SET claim_token = $2,
                   claimed_at = $3,
                   claim_expires_at = $4,
                   claim_fencing = $5,
                   retry_count = $6,
                   state_version = state_version + 1,
                   next_attempt_at = NULL,
                   last_error_code = NULL,
                   updated_at = CURRENT_TIMESTAMP
               WHERE obligation_id = $1
                 AND execution_state IN ('pending', 'prepared', 'enqueued')
                 AND (claim_token IS NULL OR claim_expires_at <= $3)"#,
        )
        .bind(obligation_id)
        .bind(claim_token)
        .bind(now)
        .bind(lease_until)
        .bind(claim_fencing)
        .bind(attempt_count_db)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(repository_error_owned("结算履约领取时租约已变化"));
        }

        claims.push(map_claim(
            &row,
            worker_id,
            claim_token,
            claim_fencing,
            next_state_version,
            now,
            lease_until,
            attempt_count,
        )?);
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(claims)
}

#[cfg(feature = "postgres")]
// SQL 行映射的参数分别对应 claim 的租约、版本和尝试次数；拆成共享大结构会越过存储 owner 边界。
#[allow(clippy::too_many_arguments)]
fn map_claim(
    row: &PgRow,
    worker_id: &str,
    claim_token: Uuid,
    claim_fencing: i64,
    state_version: u64,
    claimed_at: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    attempt_count: u32,
) -> Result<SettlementFulfillmentClaim, auction_application::RepositoryError> {
    let world = settlement_world(string_column(row, "dispatch_world")?)?;
    let operation = fulfillment_operation(string_column(row, "delivery_operation")?)?;
    let kind = obligation_kind(string_column(row, "obligation_kind")?)?;
    let recipient_user_id: Option<Uuid> =
        row.try_get("recipient_user_id").map_err(map_storage_error)?;
    let recipient_user_game_identity: Option<String> =
        row.try_get("recipient_user_game_identity").map_err(map_storage_error)?;
    let recipient_game_identity: Option<String> =
        row.try_get("recipient_game_identity").map_err(map_storage_error)?;
    let recipient = match (recipient_user_id, recipient_user_game_identity, recipient_game_identity)
    {
        (Some(_), Some(identity), None) if !identity.is_empty() && identity == identity.trim() => {
            identity
        }
        (None, None, Some(identity)) if !identity.is_empty() && identity == identity.trim() => {
            identity
        }
        _ => return Err(repository_error_owned("结算履约收件人身份不完整")),
    };
    let submission_id: Option<Uuid> = row.try_get("submission_id").map_err(map_storage_error)?;
    let prerequisite_id: Option<Uuid> =
        row.try_get("prerequisite_obligation_id").map_err(map_storage_error)?;
    let external_reference: Option<String> =
        row.try_get("delivery_external_reference").map_err(map_storage_error)?;
    let external_reference_proven = match row
        .try_get::<Option<String>, _>("delivery_external_reference_proven")
        .map_err(map_storage_error)?
        .as_deref()
    {
        None => false,
        Some("true") => true,
        Some("false") => false,
        Some(_) => return Err(repository_error_owned("结算履约外部引用证明标记无效")),
    };
    Ok(SettlementFulfillmentClaim {
        obligation_id: SettlementId::from_uuid(uuid_column(row, "obligation_id")?),
        auction_id: AuctionId::from_uuid(uuid_column(row, "auction_id")?),
        lot_id: LotId::from_uuid(uuid_column(row, "lot_id")?),
        origin_command_id: CommandId::from_uuid(uuid_column(row, "origin_command_id")?),
        world,
        kind,
        state: match string_column(row, "execution_state")?.as_str() {
            "pending" => SettlementFulfillmentState::Pending,
            "prepared" => SettlementFulfillmentState::Prepared,
            "enqueued" => SettlementFulfillmentState::Enqueued,
            _ => return Err(repository_error_owned("结算履约阶段无效")),
        },
        operation,
        recipient,
        amount: row.try_get("amount").map_err(map_storage_error)?,
        carrier_credit: u32::try_from(
            row.try_get::<i64, _>("carrier_credit").map_err(map_storage_error)?,
        )
        .map_err(|_| repository_error_owned("结算履约 carrier credit 超出范围"))?,
        equipment_id: EquipmentId::from_uuid(uuid_column(row, "equipment_id")?),
        external_equipment_id: string_column(row, "external_equipment_id")?,
        equipment_name: string_column(row, "equipment_name")?,
        submission_id: submission_id.map(SubmissionId::from_uuid),
        submission_source_kind: row.try_get("submission_source_kind").map_err(map_storage_error)?,
        submission_external_mail_id: row
            .try_get("submission_external_mail_id")
            .map_err(map_storage_error)?,
        prerequisite_id: prerequisite_id.map(SettlementId::from_uuid),
        delivery_id: row.try_get("delivery_id").map_err(map_storage_error)?,
        external_reference,
        external_reference_proven,
        prepare_command_id: row
            .try_get::<Option<Uuid>, _>("prepare_command_id")
            .map_err(map_storage_error)?
            .map(CommandId::from_uuid),
        prepare_request_hash: request_hash_from_optional(
            row.try_get("prepare_request_digest").map_err(map_storage_error)?,
        )?,
        claim_token,
        claim_fencing,
        state_version,
        worker_id: worker_id.to_owned(),
        claimed_at,
        lease_until,
        attempt_count,
    })
}

#[cfg(feature = "postgres")]
async fn prepare_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    preparation: &SettlementDeliveryPreparation,
    _prepared_at: DateTime<Utc>,
) -> Result<SettlementFulfillmentResult, auction_application::RepositoryError> {
    validate_claim_identity(claim)?;
    if preparation.delivery_id.is_nil()
        || preparation.prepare_command_id.is_nil()
        || preparation.request_hash.as_bytes() == [0; 32]
        || !preparation.payload.is_object()
        || preparation.recipient != claim.recipient
    {
        return Err(repository_error_owned("结算履约 prepared delivery 身份无效"));
    }

    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let inserted = sqlx::query(
        r#"INSERT INTO public.deliveries (
               id, command_id, request_digest, origin_kind, origin_id,
               recipient, payload, status, created_at, updated_at
           ) VALUES ($1, $2, $3, 'settlement', $4, $5, $6, 'prepared', $7, $7)
           ON CONFLICT (command_id) DO NOTHING"#,
    )
    .bind(preparation.delivery_id)
    .bind(preparation.prepare_command_id.as_uuid())
    .bind(preparation.request_hash.as_bytes().to_vec())
    .bind(claim.obligation_id.to_string())
    .bind(&preparation.recipient)
    .bind(&preparation.payload)
    .bind(_prepared_at)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let delivery = sqlx::query(
        r#"SELECT id, request_digest, origin_kind, origin_id, recipient, payload, status
           FROM public.deliveries
           WHERE command_id = $1
           FOR UPDATE"#,
    )
    .bind(preparation.prepare_command_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error_owned("prepared delivery 写入后无法读取"))?;
    let stored_id: Uuid = delivery.try_get("id").map_err(map_storage_error)?;
    let stored_digest: Vec<u8> = delivery.try_get("request_digest").map_err(map_storage_error)?;
    let stored_origin_kind: String = delivery.try_get("origin_kind").map_err(map_storage_error)?;
    let stored_origin_id: String = delivery.try_get("origin_id").map_err(map_storage_error)?;
    let stored_recipient: String = delivery.try_get("recipient").map_err(map_storage_error)?;
    let stored_payload: serde_json::Value =
        delivery.try_get("payload").map_err(map_storage_error)?;
    let stored_status: String = delivery.try_get("status").map_err(map_storage_error)?;
    if stored_id != preparation.delivery_id
        || stored_digest != preparation.request_hash.as_bytes()
        || stored_origin_kind != "settlement"
        || stored_origin_id != claim.obligation_id.to_string()
        || stored_recipient != preparation.recipient
        || stored_payload != preparation.payload
        || !matches!(stored_status.as_str(), "prepared" | "pending")
    {
        return Err(repository_error_owned("prepared delivery 命令账本冲突"));
    }

    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'prepared',
               delivery_id = $2,
               prepare_command_id = $3,
               prepare_request_digest = $4,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               state_version = state_version + 1,
               next_attempt_at = NULL,
               last_error_code = NULL,
               updated_at = CURRENT_TIMESTAMP
           WHERE obligation_id = $1
             AND execution_state = 'pending'
             AND claim_token = $5
             AND claim_fencing = $6
             AND delivery_id IS NULL"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(preparation.delivery_id)
    .bind(preparation.prepare_command_id.as_uuid())
    .bind(preparation.request_hash.as_bytes().to_vec())
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    if updated.rows_affected() == 0 {
        let current = sqlx::query(
            r#"SELECT execution_state, delivery_id, prepare_command_id, prepare_request_digest
               FROM public.settlement_obligation_fulfillments
               WHERE obligation_id = $1
               FOR UPDATE"#,
        )
        .bind(claim.obligation_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_storage_error)?
        .ok_or_else(|| repository_error_owned("结算履约记录不存在"))?;
        let state: String = current.try_get("execution_state").map_err(map_storage_error)?;
        let current_delivery_id: Option<Uuid> =
            current.try_get("delivery_id").map_err(map_storage_error)?;
        let current_command_id: Option<Uuid> =
            current.try_get("prepare_command_id").map_err(map_storage_error)?;
        let current_digest: Option<Vec<u8>> =
            current.try_get("prepare_request_digest").map_err(map_storage_error)?;
        if state != "prepared"
            || current_delivery_id != Some(preparation.delivery_id)
            || current_command_id != Some(preparation.prepare_command_id.as_uuid())
            || current_digest.as_deref() != Some(preparation.request_hash.as_bytes().as_slice())
        {
            return Err(repository_error_owned("结算履约 claim 已失效或结果冲突"));
        }
    }
    if inserted.rows_affected() == 1 {
        let request_id = preparation.prepare_command_id.to_string();
        append_delivery_audit(
            &mut transaction,
            DeliveryAudit {
                occurred_at: _prepared_at,
                actor_type: "system",
                actor_id: "delivery-service",
                action: PREPARE,
                delivery_id: preparation.delivery_id,
                request_id: Some(&request_id),
                source: "system",
                metadata: serde_json::json!({
                    "origin_kind": "settlement",
                    "status": "prepared",
                }),
            },
        )
        .await
        .map_err(map_storage_error)?;
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(SettlementFulfillmentResult {
        obligation_id: claim.obligation_id,
        delivery_id: preparation.delivery_id,
        prepare_command_id: preparation.prepare_command_id,
    })
}

#[cfg(feature = "postgres")]
async fn activate_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    activate_command_id: CommandId,
    activate_request_hash: auction_application::RequestHash,
    activated_at: DateTime<Utc>,
) -> Result<
    auction_application::SettlementFulfillmentActivationResult,
    auction_application::RepositoryError,
> {
    validate_claim_identity(claim)?;
    let (Some(delivery_id), Some(prepare_command_id), Some(prepare_request_hash)) =
        (claim.delivery_id, claim.prepare_command_id, claim.prepare_request_hash)
    else {
        return Err(repository_error_owned("prepared 履约缺少 delivery prepare identity"));
    };
    if claim.state != SettlementFulfillmentState::Prepared
        || activate_command_id.is_nil()
        || activate_request_hash.as_bytes() == [0; 32]
    {
        return Err(repository_error_owned("结算履约 activation identity 无效"));
    }

    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let delivery = sqlx::query(
        r#"SELECT status, activation_command_id, activation_request_digest
           FROM public.deliveries
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(delivery_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error_owned("prepared delivery 不存在"))?;
    let status: String = delivery.try_get("status").map_err(map_storage_error)?;
    let existing_command: Option<Uuid> =
        delivery.try_get("activation_command_id").map_err(map_storage_error)?;
    let existing_digest: Option<Vec<u8>> =
        delivery.try_get("activation_request_digest").map_err(map_storage_error)?;
    let activated = match status.as_str() {
        "prepared" => {
            if existing_command.is_some() || existing_digest.is_some() {
                return Err(repository_error_owned("delivery activation identity 已被占用"));
            }
            let updated = sqlx::query(
                r#"UPDATE public.deliveries
                   SET status = 'pending',
                       activation_command_id = $2,
                       activation_request_digest = $3,
                       activated_at = $4,
                       updated_at = CURRENT_TIMESTAMP
                   WHERE id = $1 AND status = 'prepared'"#,
            )
            .bind(delivery_id)
            .bind(activate_command_id.as_uuid())
            .bind(activate_request_hash.as_bytes().to_vec())
            .bind(activated_at)
            .execute(&mut *transaction)
            .await
            .map_err(map_storage_error)?;
            if updated.rows_affected() != 1 {
                return Err(repository_error_owned("delivery 激活状态已变化"));
            }
            true
        }
        "pending" | "claimed" | "sending" | "succeeded" | "failed" | "manual_reconcile"
            if existing_command == Some(activate_command_id.as_uuid())
                && existing_digest.as_deref()
                    == Some(activate_request_hash.as_bytes().as_slice()) =>
        {
            false
        }
        _ => return Err(repository_error_owned("delivery 当前状态不允许激活")),
    };

    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'enqueued',
               activate_command_id = $2,
               activate_request_digest = $3,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               state_version = state_version + 1,
               next_attempt_at = NULL,
               last_error_code = NULL,
               updated_at = CURRENT_TIMESTAMP
           WHERE obligation_id = $1
             AND execution_state = 'prepared'
             AND claim_token = $4
             AND claim_fencing = $5
             AND delivery_id = $6
             AND prepare_command_id = $7
             AND prepare_request_digest = $8"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(activate_command_id.as_uuid())
    .bind(activate_request_hash.as_bytes().to_vec())
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .bind(delivery_id)
    .bind(prepare_command_id.as_uuid())
    .bind(prepare_request_hash.as_bytes().to_vec())
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() == 0 {
        let current = sqlx::query(
            r#"SELECT execution_state, delivery_id, activate_command_id,
                      activate_request_digest
               FROM public.settlement_obligation_fulfillments
               WHERE obligation_id = $1
               FOR UPDATE"#,
        )
        .bind(claim.obligation_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_storage_error)?
        .ok_or_else(|| repository_error_owned("结算履约记录不存在"))?;
        let current_state: String =
            current.try_get("execution_state").map_err(map_storage_error)?;
        let current_delivery: Option<Uuid> =
            current.try_get("delivery_id").map_err(map_storage_error)?;
        let current_command: Option<Uuid> =
            current.try_get("activate_command_id").map_err(map_storage_error)?;
        let current_digest: Option<Vec<u8>> =
            current.try_get("activate_request_digest").map_err(map_storage_error)?;
        if current_state != "enqueued"
            || current_delivery != Some(delivery_id)
            || current_command != Some(activate_command_id.as_uuid())
            || current_digest.as_deref() != Some(activate_request_hash.as_bytes().as_slice())
        {
            return Err(repository_error_owned("结算履约 activation claim 已失效或冲突"));
        }
    }
    if activated {
        let request_id = activate_command_id.to_string();
        append_delivery_audit(
            &mut transaction,
            DeliveryAudit {
                occurred_at: activated_at,
                actor_type: "system",
                actor_id: "delivery-service",
                action: ACTIVATE,
                delivery_id,
                request_id: Some(&request_id),
                source: "system",
                metadata: serde_json::json!({"status": "pending"}),
            },
        )
        .await
        .map_err(map_storage_error)?;
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(auction_application::SettlementFulfillmentActivationResult {
        obligation_id: claim.obligation_id,
        delivery_id,
        activate_command_id,
    })
}

#[cfg(feature = "postgres")]
async fn satisfy_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    proof: &SettlementFulfillmentProof,
) -> Result<SettlementFulfillmentReconcileResult, auction_application::RepositoryError> {
    validate_claim_identity(claim)?;
    let Some(delivery_id) = claim.delivery_id else {
        return Err(repository_error_owned("结算履约满足证明缺少 delivery identity"));
    };
    if claim.state != SettlementFulfillmentState::Enqueued
        || proof.kind.is_empty()
        || proof.kind != proof.kind.trim()
        || proof.reference.is_empty()
        || proof.reference != proof.reference.trim()
        || proof.digest == [0; 32]
        || proof.observed_at < claim.claimed_at
        || proof.observed_at > Utc::now()
        || claim.external_reference.as_deref() != Some(proof.reference.as_str())
        || !claim.external_reference_proven
    {
        return Err(repository_error_owned("结算履约满足证明不满足 fencing 或身份约束"));
    }
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'satisfied',
               proof_kind = $2,
               proof_reference = $3,
               proof_digest = $4,
               proof_observed_at = $5,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               completed_at = CURRENT_TIMESTAMP,
               state_version = state_version + 1,
               next_attempt_at = NULL,
               last_error_code = NULL,
               updated_at = CURRENT_TIMESTAMP
           WHERE obligation_id = $1
             AND execution_state = 'enqueued'
             AND claim_token = $6
             AND claim_fencing = $7
             AND delivery_id = $8"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(&proof.kind)
    .bind(&proof.reference)
    .bind(proof.digest.to_vec())
    .bind(proof.observed_at)
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .bind(delivery_id)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error_owned("结算履约满足证明的 claim 已失效"));
    }
    Ok(SettlementFulfillmentReconcileResult {
        obligation_id: claim.obligation_id,
        delivery_id,
        status: SettlementFulfillmentReconcileStatus::Satisfied,
    })
}

#[cfg(feature = "postgres")]
async fn manual_reconcile_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    validate_claim_identity(claim)?;
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'manual_reconcile',
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               completed_at = CURRENT_TIMESTAMP,
               state_version = state_version + 1,
               next_attempt_at = NULL,
               last_error_code = $2,
               updated_at = CURRENT_TIMESTAMP
           WHERE obligation_id = $1
             AND execution_state IN ('pending', 'prepared', 'enqueued')
             AND claim_token = $3
             AND claim_fencing = $4"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(failure_code)
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error_owned("结算履约人工复核的 claim 已失效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn fail_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
    _safe_detail: &str,
) -> Result<(), auction_application::RepositoryError> {
    validate_claim_identity(claim)?;
    let result = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               state_version = state_version + 1,
               next_attempt_at = $2,
               last_error_code = $3,
               updated_at = CURRENT_TIMESTAMP
           WHERE obligation_id = $1
             AND execution_state IN ('pending', 'prepared', 'enqueued')
             AND claim_token = $4
             AND claim_fencing = $5"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(retry_at)
    .bind(failure_code)
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("结算履约失败记录的 claim 已失效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn validate_claim_identity(
    claim: &SettlementFulfillmentClaim,
) -> Result<(), auction_application::RepositoryError> {
    if claim.obligation_id.is_nil()
        || claim.claim_token.is_nil()
        || claim.claim_fencing <= 0
        || claim.state_version == 0
        || claim.worker_id.is_empty()
        || claim.worker_id != claim.worker_id.trim()
    {
        return Err(repository_error_owned("结算履约 claim 身份无效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn uuid_column(row: &PgRow, column: &str) -> Result<Uuid, auction_application::RepositoryError> {
    row.try_get(column).map_err(map_storage_error)
}

#[cfg(feature = "postgres")]
fn string_column(
    row: &PgRow,
    column: &str,
) -> Result<String, auction_application::RepositoryError> {
    row.try_get(column).map_err(map_storage_error)
}

#[cfg(feature = "postgres")]
fn request_hash_from_optional(
    value: Option<Vec<u8>>,
) -> Result<Option<auction_application::RequestHash>, auction_application::RepositoryError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes: [u8; 32] =
        value.try_into().map_err(|_| repository_error_owned("结算履约请求摘要长度无效"))?;
    if bytes == [0; 32] {
        return Err(repository_error_owned("结算履约请求摘要不能为零"));
    }
    Ok(Some(auction_application::RequestHash::new(bytes)))
}

#[cfg(feature = "postgres")]
fn settlement_world(
    value: String,
) -> Result<SettlementWorld, auction_application::RepositoryError> {
    match value.as_str() {
        "main" => Ok(SettlementWorld::Main),
        "isekai" => Ok(SettlementWorld::Isekai),
        _ => Err(repository_error_owned("结算履约世界标识无效")),
    }
}

#[cfg(feature = "postgres")]
fn fulfillment_operation(
    value: String,
) -> Result<SettlementFulfillmentOperation, auction_application::RepositoryError> {
    match value.as_str() {
        "send" => Ok(SettlementFulfillmentOperation::Send),
        "return" => Ok(SettlementFulfillmentOperation::Return),
        _ => Err(repository_error_owned("结算履约操作无效")),
    }
}

#[cfg(feature = "postgres")]
fn obligation_kind(
    value: String,
) -> Result<SettlementObligationKind, auction_application::RepositoryError> {
    match value.as_str() {
        "winner_equipment_cod" => Ok(SettlementObligationKind::WinnerEquipmentCod),
        "winner_payment_carrier_cod" => Ok(SettlementObligationKind::WinnerPaymentCarrierCod),
        "winner_equipment_handoff" => Ok(SettlementObligationKind::WinnerEquipmentHandoff),
        "seller_payout" => Ok(SettlementObligationKind::SellerPayout),
        "buyer_refund" => Ok(SettlementObligationKind::BuyerRefund),
        "unsold_return" => Ok(SettlementObligationKind::UnsoldReturn),
        "withdrawn_return" => Ok(SettlementObligationKind::WithdrawnReturn),
        _ => Err(repository_error_owned("结算履约义务种类无效")),
    }
}

fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn valid_failure_code(value: &str) -> bool {
    matches!(
        value,
        "invalid_fulfillment_fact"
            | "recipient_unavailable"
            | "balance_unavailable"
            | "balance_insufficient"
            | "balance_stale"
            | "delivery_conflict"
            | "delivery_unavailable"
            | "delivery_failed"
            | "stale_fulfillment_claim"
            | "evidence_not_observed"
            | "evidence_unavailable"
            | "evidence_mismatch"
            | "fulfillment_outcome_unknown"
    )
}

fn repository_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "postgres")]
fn repository_error_owned(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}

#[cfg(feature = "postgres")]
fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(match error {
        sqlx::Error::RowNotFound => "结算履约记录不存在".to_owned(),
        _ => "结算履约数据库操作失败".to_owned(),
    })
}
