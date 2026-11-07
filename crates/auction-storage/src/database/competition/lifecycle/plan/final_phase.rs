//! PostgreSQL 最终阶段计划写入。

use crate::database::lifecycle_audit::final_proxy_audit_record;

use super::super::record::{insert_lifecycle_audit, previous_lifecycle_version};
use crate::database::competition::PostgresCompetitionTransaction;
use crate::database::competition::lifecycle::{i64_version, invalid_state, map_sql_error};
use auction_application::auction::AuctionLifecycleCommand;
use auction_application::core::CompetitionTransactionError;
use auction_domain::{Auction, AuctionId};
use chrono::{DateTime, Utc};
use std::sync::Arc;

pub(super) async fn apply_final_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    proxy_plans: &[auction_application::auction::AuctionFinalProxyPlan],
    effective_end_at: Option<DateTime<Utc>>,
) -> Result<(), CompetitionTransactionError> {
    // 名义与有效截止必须同步收紧：保留 effective >= scheduled 不变量，原始计划留在不可变
    // policy_snapshot 中供审计追溯。
    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.auctions
             SET status = 'final', final_phase_at = $1,
                 scheduled_end_at = CASE
                     WHEN $2::timestamptz IS NOT NULL AND scheduled_end_at > $2 THEN $2
                     ELSE scheduled_end_at
                 END,
                 effective_end_at = CASE
                     WHEN $2::timestamptz IS NOT NULL AND effective_end_at > $2 THEN $2
                     ELSE effective_end_at
                 END,
                 state_version = $3, updated_at = $1
             WHERE id = $4 AND status = 'live' AND state_version = $5",
        )
        .bind(auction.updated_at)
        .bind(effective_end_at)
        .bind(i64_version(auction.state_version, "最终阶段状态版本")?)
        .bind(auction.id.as_uuid())
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    apply_final_proxy_plans_postgres(transaction, command, auction, proxy_plans).await?;
    if let Some(effective_end_at) = effective_end_at {
        clamp_live_lot_deadlines_postgres(
            transaction,
            auction.id,
            effective_end_at,
            auction.updated_at,
        )
        .await?;
    }
    mark_final_schedule_postgres(transaction, command, auction).await?;
    Ok(())
}

pub(super) async fn apply_final_proxy_plans_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    proxy_plans: &[auction_application::auction::AuctionFinalProxyPlan],
) -> Result<(), CompetitionTransactionError> {
    for proxy_plan in proxy_plans {
        if proxy_plan.command.aggregate_id != proxy_plan.competition.final_lot.lot_id.as_uuid() {
            return Err(invalid_state("最终阶段代理子命令与 lot 计划不一致"));
        }
        let audit = final_proxy_audit_record(command, auction.updated_at, proxy_plan)?;
        // 生命周期子命令在锁内首次写入；重复（并发同 ID）沿旧语义视为已完成。
        let _ = transaction.insert_command_inner(&proxy_plan.command).await?;
        transaction
            .apply_public_command_plan_inner(&proxy_plan.competition, None, auction.updated_at)
            .await?;
        // W14.11：Final 子命令不经过 `apply_slow_proxy_plan_inner`；这里同样要解析
        // pending：反超成功退役，无法反超/耗尽时在同一事务入队提醒。
        let notifications = Arc::clone(&transaction.notifications);
        let public_base_url = transaction.public_base_url.clone();
        let mail_templates = Arc::clone(&transaction.mail_templates);
        let pending_wakeup = crate::database::lot_watch::resolve_pending(
            transaction.transaction_mut()?,
            proxy_plan.competition.final_lot.lot_id.as_uuid(),
            auction.updated_at,
            notifications.as_ref(),
            public_base_url.as_deref(),
            mail_templates.as_ref(),
        )
        .await?;
        transaction.delivery_wakeup |= pending_wakeup;
        insert_lifecycle_audit(transaction, audit).await?;
    }
    Ok(())
}

pub(super) async fn clamp_live_lot_deadlines_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
    effective_end_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        "UPDATE public.lots
         SET effective_end_at = $1, state_version = state_version + 1, updated_at = $2
         WHERE auction_id = $3 AND status = 'live' AND effective_end_at > $1",
    )
    .bind(effective_end_at)
    .bind(updated_at)
    .bind(auction_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

pub(super) async fn mark_final_schedule_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
) -> Result<(), CompetitionTransactionError> {
    super::complete_schedule_phase_postgres(
        transaction,
        command,
        auction,
        auction_application::scheduling::ScheduledRoundPhase::Final,
    )
    .await
}
