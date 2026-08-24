//! PostgreSQL 结算 workflow 的 due claim 与租约写入。

use auction_application::{RequestHash, SettlementWorkClaim};
use auction_domain::{AuctionId, CommandId, deterministic_uuid};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::{map_storage_error, repository_error_owned};
use crate::database::TrackedPool;
use crate::database::audit_health::AuditWriteResult as _;

#[derive(Debug, Serialize)]
struct SettlementClaimRequest {
    workflow_id: Uuid,
    due_at: DateTime<Utc>,
}

fn workflow_id(auction_id: Uuid) -> Uuid {
    deterministic_uuid(CommandId::from_uuid(auction_id), "settlement-workflow", 1)
}

fn claim_command_id(auction_id: Uuid) -> CommandId {
    CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(auction_id),
        "settlement-claim",
        1,
    ))
}

fn settlement_command_id(claim_command_id: CommandId) -> CommandId {
    CommandId::from_uuid(deterministic_uuid(claim_command_id, "settlement-execute", 1))
}

pub(super) async fn claim_postgres(
    pool: &TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<SettlementWorkClaim>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let rows = sqlx::query(
        r#"SELECT a.id, a.status, a.state_version,
                  COALESCE(a.settlement_due_at, lot_schedule.due_at) AS due_at,
                  a.settlement_workflow_id, a.settlement_claim_command_id,
                  a.settlement_attempt_count, a.settlement_next_attempt_at
           FROM public.auctions AS a
           LEFT JOIN LATERAL (
               SELECT
                   MAX(l.effective_end_at) AS due_at,
                   COUNT(*)::bigint AS lot_count,
                   COUNT(*) FILTER (
                       WHERE l.status = 'live' AND l.effective_end_at IS NOT NULL
                   )::bigint AS live_lot_count
               FROM public.lots AS l
               WHERE l.auction_id = a.id
           ) AS lot_schedule ON TRUE
           WHERE (
               a.status = 'final'
               AND lot_schedule.lot_count > 0
               AND lot_schedule.live_lot_count = lot_schedule.lot_count
               AND lot_schedule.due_at <= $1
               AND a.settlement_workflow_id IS NULL
           )
           OR (
               a.status = 'settling'
               AND a.settlement_workflow_id IS NOT NULL
               AND a.settlement_claim_command_id IS NOT NULL
               AND a.settlement_complete_command_id IS NULL
               AND a.settlement_next_attempt_at IS NOT NULL
               AND a.settlement_next_attempt_at <= $1
           )
           OR (
               a.status = 'settled'
               AND a.settlement_workflow_id IS NOT NULL
               AND a.settlement_claim_command_id IS NOT NULL
               AND a.settlement_complete_command_id IS NULL
               AND a.settlement_next_attempt_at IS NOT NULL
               AND a.settlement_next_attempt_at <= $1
           )
           ORDER BY COALESCE(
               a.settlement_next_attempt_at,
               a.settlement_due_at,
               lot_schedule.due_at
           ), a.id
           LIMIT $2
           FOR UPDATE OF a SKIP LOCKED"#,
    )
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let auction_uuid: Uuid = row.try_get("id").map_err(map_storage_error)?;
        let status: String = row.try_get("status").map_err(map_storage_error)?;
        let due_at: DateTime<Utc> = row.try_get("due_at").map_err(map_storage_error)?;
        let previous_state_version: i64 =
            row.try_get("state_version").map_err(map_storage_error)?;
        if previous_state_version <= 0 {
            return Err(repository_error_owned("结算拍卖状态版本无效"));
        }
        if status == "final"
            && lock_initial_lots(&mut transaction, auction_uuid, now, due_at)
                .await
                .map_err(map_storage_error)?
                .is_none()
        {
            // 等待 lot 锁期间可能刚好提交了软截止延期；本轮跳过并让下一次扫描读取新世代。
            continue;
        }
        let (workflow, claim_command, attempt_count, is_initial, completion_only) =
            if status == "final" {
                let workflow = workflow_id(auction_uuid);
                let claim_command = claim_command_id(auction_uuid);
                let request_hash = RequestHash::from_request(
                    "claim_settlement",
                    auction_uuid,
                    &SettlementClaimRequest { workflow_id: workflow, due_at },
                )
                .map_err(|_| repository_error_owned("无法生成结算领取命令摘要"))?;
                let expected_hash = request_hash.as_bytes().to_vec();
                let command_row = sqlx::query(
                    r#"INSERT INTO public.auction_commands
                       (command_id, command_kind, aggregate_id, request_hash,
                        result_code, result_payload)
                   VALUES ($1, 'claim_settlement', $2, $3, 'accepted', $4)
                   ON CONFLICT (command_id) DO UPDATE
                       SET command_id = EXCLUDED.command_id
                   RETURNING command_kind, aggregate_id, request_hash, result_code"#,
                )
                .bind(claim_command.as_uuid())
                .bind(auction_uuid)
                .bind(&expected_hash)
                .bind(json!({
                    "auction_id": auction_uuid,
                    "workflow_id": workflow,
                    "due_at": due_at,
                }))
                .fetch_one(&mut *transaction)
                .await
                .map_err(map_storage_error)?;
                let existing_kind: String =
                    command_row.try_get("command_kind").map_err(map_storage_error)?;
                let existing_aggregate: Uuid =
                    command_row.try_get("aggregate_id").map_err(map_storage_error)?;
                let existing_hash: Vec<u8> =
                    command_row.try_get("request_hash").map_err(map_storage_error)?;
                let existing_result: String =
                    command_row.try_get("result_code").map_err(map_storage_error)?;
                if existing_kind != "claim_settlement"
                    || existing_aggregate != auction_uuid
                    || existing_hash != expected_hash
                    || existing_result != "accepted"
                {
                    return Err(repository_error_owned("结算领取命令账本发生冲突"));
                }
                (workflow, claim_command, 1_u32, true, false)
            } else {
                let workflow: Option<Uuid> =
                    row.try_get("settlement_workflow_id").map_err(map_storage_error)?;
                let workflow =
                    workflow.ok_or_else(|| repository_error_owned("结算 workflow 标识缺失"))?;
                let claim_command_uuid: Option<Uuid> =
                    row.try_get("settlement_claim_command_id").map_err(map_storage_error)?;
                let claim_command_uuid = claim_command_uuid
                    .ok_or_else(|| repository_error_owned("结算领取命令标识缺失"))?;
                let old_attempt_count: i32 =
                    row.try_get("settlement_attempt_count").map_err(map_storage_error)?;
                if old_attempt_count <= 0 {
                    return Err(repository_error_owned("结算尝试次数无效"));
                }
                let completion_only = status == "settled";
                let attempt_count = if completion_only {
                    u32::try_from(old_attempt_count)
                        .map_err(|_| repository_error_owned("结算尝试次数超出范围"))?
                } else {
                    old_attempt_count
                        .checked_add(1)
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or_else(|| repository_error_owned("结算尝试次数溢出"))?
                };
                (
                    workflow,
                    CommandId::from_uuid(claim_command_uuid),
                    attempt_count,
                    false,
                    completion_only,
                )
            };
        let attempt_count_i32 = i32::try_from(attempt_count)
            .map_err(|_| repository_error_owned("结算尝试次数超出数据库范围"))?;
        let updated = if is_initial {
            sqlx::query(
                r#"UPDATE public.auctions
                   SET status = 'settling', settlement_workflow_id = $2,
                       settlement_claim_command_id = $3,
                       settlement_due_at = $4,
                       settlement_started_at = $5, settlement_attempt_count = $6,
                       settlement_next_attempt_at = $7, settlement_last_error_code = NULL,
                       state_version = state_version + 1, updated_at = $5
                   WHERE id = $1 AND status = 'final' AND state_version = $8"#,
            )
            .bind(auction_uuid)
            .bind(workflow)
            .bind(claim_command.as_uuid())
            .bind(due_at)
            .bind(now)
            .bind(attempt_count_i32)
            .bind(lease_until)
            .bind(previous_state_version)
            .execute(&mut *transaction)
            .await
        } else if completion_only {
            sqlx::query(
                r#"UPDATE public.auctions
                   SET settlement_next_attempt_at = $2, updated_at = $3
                   WHERE id = $1 AND status = 'settled'
                     AND settlement_workflow_id = $4
                     AND settlement_next_attempt_at <= $3
                     AND settlement_complete_command_id IS NULL"#,
            )
            .bind(auction_uuid)
            .bind(lease_until)
            .bind(now)
            .bind(workflow)
            .execute(&mut *transaction)
            .await
        } else {
            sqlx::query(
                r#"UPDATE public.auctions
                   SET settlement_attempt_count = $2, settlement_next_attempt_at = $3,
                       settlement_last_error_code = NULL,
                       state_version = state_version + 1, updated_at = $4
                   WHERE id = $1 AND status = 'settling'
                     AND settlement_workflow_id = $5
                     AND settlement_next_attempt_at <= $4
                     AND settlement_complete_command_id IS NULL"#,
            )
            .bind(auction_uuid)
            .bind(attempt_count_i32)
            .bind(lease_until)
            .bind(now)
            .bind(workflow)
            .execute(&mut *transaction)
            .await
        }
        .map_err(map_storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(repository_error_owned("结算 workflow 领取时状态已变化"));
        }
        let settlement_command_id = settlement_command_id(claim_command);
        if is_initial {
            let next_state_version = previous_state_version
                .checked_add(1)
                .ok_or_else(|| repository_error_owned("结算拍卖状态版本溢出"))?;
            insert_settlement_claim_audit(
                &mut transaction,
                auction_uuid,
                claim_command,
                worker_id,
                next_state_version,
            )
            .await
            .map_err(map_storage_error)?;
            insert_settlement_claim_event(
                &mut transaction,
                auction_uuid,
                claim_command,
                next_state_version,
            )
            .await
            .map_err(map_storage_error)?;
        }
        claims.push(SettlementWorkClaim {
            auction_id: AuctionId::from_uuid(auction_uuid),
            workflow_id: workflow,
            settlement_command_id,
            due_at,
            completion_only,
            worker_id: worker_id.to_owned(),
            lease_until,
            attempt_count,
        });
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(claims)
}

async fn lock_initial_lots(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
    now: DateTime<Utc>,
    expected_due_at: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT status, effective_end_at
           FROM public.lots
           WHERE auction_id = $1
           ORDER BY lot_number, id
           FOR UPDATE"#,
    )
    .bind(auction_id)
    .fetch_all(&mut **transaction)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }

    let mut latest_end = None;
    for row in rows {
        let status: String = row.try_get("status")?;
        let effective_end_at: Option<DateTime<Utc>> = row.try_get("effective_end_at")?;
        let Some(effective_end_at) = effective_end_at else {
            return Ok(None);
        };
        if status != "live" || effective_end_at > now {
            return Ok(None);
        }
        latest_end = Some(
            latest_end
                .map_or(effective_end_at, |current: DateTime<Utc>| current.max(effective_end_at)),
        );
    }
    Ok(latest_end.filter(|latest| *latest == expected_due_at))
}

async fn insert_settlement_claim_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
    command_id: CommandId,
    worker_id: &str,
    state_version: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, actor_type, actor_id, action, entity_type, entity_id, result,
                request_id, source, metadata)
           VALUES ($1, 'worker', $2, 'settlement.claimed', 'auction', $3, 'success', $4, 'worker', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(worker_id)
    .bind(auction_id.to_string())
    .bind(command_id.to_string())
    .bind(json!({ "command_id": command_id, "state_version": state_version }))
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map(|_| ())
}

async fn insert_settlement_claim_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
    command_id: CommandId,
    state_version: i64,
) -> Result<(), sqlx::Error> {
    let stream_key = format!("auction:{auction_id}");
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key) VALUES ($1)
         ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await?;
    let row = sqlx::query(
        r#"UPDATE public.realtime_streams
           SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
           WHERE stream_key = $1
           RETURNING next_sequence - 1 AS stream_sequence"#,
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await?;
    let sequence: i64 = row.try_get("stream_sequence")?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
               (event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, state_version, origin_command_id)
           VALUES ($1, $2, $3, 'auction.settling', 1, $4, $5, $6)"#,
    )
    .bind(deterministic_uuid(command_id, "settlement-claim-realtime-event", 1))
    .bind(&stream_key)
    .bind(sequence)
    .bind(json!({ "auction_id": auction_id, "state_version": state_version }))
    .bind(state_version)
    .bind(command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map(|_| ())
}
