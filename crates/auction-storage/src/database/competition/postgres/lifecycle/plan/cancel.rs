//! PostgreSQL 取消阶段计划写入与返还义务建立。

use super::super::record::previous_lifecycle_version;
use super::super::*;

pub(super) async fn apply_cancel_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
) -> Result<u64, CompetitionTransactionError> {
    let previous_status: String = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar("SELECT status FROM public.auctions WHERE id = $1 FOR UPDATE")
            .bind(auction.id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
            .ok_or(CompetitionTransactionError::NotFound)?
    };
    if !matches!(previous_status.as_str(), "draft" | "preview") {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let lot_rows = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT l.id, l.status, l.state_version, l.equipment_id, l.submission_id,
                    s.seller,
                    COALESCE((a.policy_snapshot->'settlement'->>'version')::integer, 0)
                        AS policy_version,
                    a.world_id
             FROM public.lots AS l
             INNER JOIN public.auctions AS a ON a.id = l.auction_id
             LEFT JOIN public.auction_submissions AS s ON s.id = l.submission_id
             WHERE l.auction_id = $1
             ORDER BY l.lot_number
             FOR UPDATE OF l",
        )
        .bind(auction.id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let mut return_obligation_count = 0_u64;
    for lot in lot_rows {
        let status: String = lot.try_get("status").map_err(map_sql_error)?;
        if status == "withdrawn" {
            // 已由单 lot 撤回命令生成过返还义务，取消命令不能重复建立第二份责任。
            continue;
        }
        if status != "pending" {
            return Err(invalid_state("取消拍卖包含非 pending/withdrawn 标的"));
        }
        let lot_id: Uuid = lot.try_get("id").map_err(map_sql_error)?;
        let equipment_id: Uuid = lot.try_get("equipment_id").map_err(map_sql_error)?;
        let submission_id: Option<Uuid> = lot.try_get("submission_id").map_err(map_sql_error)?;
        let seller: Option<String> = lot.try_get("seller").map_err(map_sql_error)?;
        let seller = seller
            .filter(|value| !value.trim().is_empty() && value == value.trim())
            .ok_or_else(|| invalid_state("取消返还义务缺少卖方游戏身份"))?;
        let world_id: String = lot.try_get("world_id").map_err(map_sql_error)?;
        if !matches!(world_id.as_str(), "main" | "isekai") {
            return Err(invalid_state("取消返还义务的世界不在允许集合内"));
        }
        let policy_version: i32 = lot.try_get("policy_version").map_err(map_sql_error)?;
        if policy_version < 0 {
            return Err(invalid_state("取消返还义务的策略版本无效"));
        }
        let lot_state_version: i64 = lot.try_get("state_version").map_err(map_sql_error)?;
        if lot_state_version <= 0 {
            return Err(invalid_state("取消标的状态版本无效"));
        }

        let withdrawn = {
            let transaction = transaction.transaction_mut()?;
            sqlx::query(
                "UPDATE public.lots
                 SET status = 'withdrawn', slow_authorization_id = NULL, slow_due_at = NULL,
                     state_version = state_version + 1, updated_at = $1
                 WHERE id = $2 AND auction_id = $3 AND status = 'pending' AND state_version = $4",
            )
            .bind(auction.updated_at)
            .bind(lot_id)
            .bind(auction.id.as_uuid())
            .bind(lot_state_version)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };
        if withdrawn.rows_affected() != 1 {
            return Err(CompetitionTransactionError::StaleState);
        }
        insert_cancellation_return_obligation_postgres(
            transaction,
            command,
            auction,
            lot_id,
            equipment_id,
            submission_id,
            &world_id,
            &seller,
            policy_version,
        )
        .await?;
        return_obligation_count = return_obligation_count
            .checked_add(1)
            .ok_or_else(|| invalid_state("取消返还义务数量溢出"))?;
    }

    {
        let transaction = transaction.transaction_mut()?;
        if previous_status == "draft" {
            sqlx::query(
                "DELETE FROM public.auction_intake_targets
                 WHERE auction_id = $1",
            )
            .bind(auction.id.as_uuid())
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
        }
        sqlx::query(
            "UPDATE public.scheduled_rounds
             SET status = 'missed', auction_id = NULL,
                 lease_owner = NULL, lease_until = NULL,
                 state_version = state_version + 1, updated_at = $1
             WHERE auction_id = $2 AND status = 'active' AND open_completed_at IS NULL",
        )
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        let updated = sqlx::query(
            "UPDATE public.auctions
             SET status = 'cancelled', state_version = $1, updated_at = $2,
                 cancelled_at = $3, cancelled_reason = $4, cancelled_by = $5
             WHERE id = $6 AND status = $7 AND state_version = $8",
        )
        .bind(i64_version(auction.state_version, "取消状态版本")?)
        .bind(auction.updated_at)
        .bind(auction.cancelled_at)
        .bind(&auction.cancelled_reason)
        .bind(&auction.cancelled_by)
        .bind(auction.id.as_uuid())
        .bind(&previous_status)
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if updated.rows_affected() != 1 {
            return Err(CompetitionTransactionError::InvalidState);
        }
    }
    Ok(return_obligation_count)
}

#[allow(clippy::too_many_arguments)]
async fn insert_cancellation_return_obligation_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    lot_id: Uuid,
    equipment_id: Uuid,
    submission_id: Option<Uuid>,
    world_id: &str,
    seller: &str,
    policy_version: i32,
) -> Result<(), CompetitionTransactionError> {
    let obligation_id = deterministic_uuid(
        command.command_id,
        &format!("settlement-obligation:{lot_id}:withdrawn_return"),
        1,
    );
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, state_version, recipient_user_id,
             recipient_game_identity, amount, carrier_credit, equipment_id, submission_id,
             prerequisite_obligation_id, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, 'withdrawn_return', 'ready', 1, NULL,
                 NULLIF($7, ''), 0, 0, $8, $9, NULL, $10, $10)",
    )
    .bind(obligation_id)
    .bind(auction.id.as_uuid())
    .bind(lot_id)
    .bind(world_id)
    .bind(command.command_id.as_uuid())
    .bind(policy_version)
    .bind(seller)
    .bind(equipment_id)
    .bind(submission_id)
    .bind(auction.updated_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, dispatch_world, delivery_operation)
         VALUES ($1, 'pending', $2, 'return')",
    )
    .bind(obligation_id)
    .bind(world_id)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
