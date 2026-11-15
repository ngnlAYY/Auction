//! PostgreSQL 拍卖生命周期审计与实时事件记录。

use auction_application::auction::{
    AuctionLifecycleCommand, AuctionLifecycleContext, AuctionLifecycleEffect, AuctionLifecyclePlan,
    AuctionLifecycleResult, AuctionLifecycleWriteSummary,
};
use auction_application::core::CompetitionTransactionError;
use auction_domain::{Auction, AuctionStatus};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::database::auction::competition::PostgresCompetitionTransaction;
use crate::database::auction::competition::map_sql_error;
use crate::database::auction::competition::mapping::i64_version;
use crate::database::auction::competition::mapping::invalid_state;
use crate::database::auction::lifecycle_audit::{
    LifecycleAuditRecord, lifecycle_parent_audit_record,
};
use crate::database::operations::audit_health::AuditWriteResult;
use crate::database::realtime::SequencedEvent;
use crate::database::realtime::append_sequenced_event;

pub(super) fn previous_lifecycle_version(
    auction: &Auction,
) -> Result<i64, CompetitionTransactionError> {
    auction
        .state_version
        .checked_sub(1)
        .and_then(|version| i64::try_from(version).ok())
        .ok_or_else(|| invalid_state("拍卖状态版本无法回退校验"))
}

#[allow(clippy::manual_async_fn)]
pub(super) async fn append_lifecycle_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    context: &AuctionLifecycleContext,
    plan: &AuctionLifecyclePlan,
    result: &AuctionLifecycleResult,
    write_summary: AuctionLifecycleWriteSummary,
) -> Result<(), CompetitionTransactionError> {
    let preview_item_count = preview_item_count(transaction, command).await?;
    let record = lifecycle_parent_audit_record(
        command,
        context,
        plan,
        result,
        write_summary,
        preview_item_count,
    )?;
    insert_lifecycle_audit(transaction, record).await?;

    let Some((event_kind, payload)) =
        lifecycle_event_payload(command, context, plan, result, preview_item_count)?
    else {
        return Ok(());
    };
    // 不变量 3：事件与生命周期事实同事务写入本拍卖流；公开实时没有跨拍卖的派生流。
    let stream_key = format!("auction:{}", result.auction_id.as_uuid());
    let transaction = transaction.transaction_mut()?;
    let state_version = i64_version(result.state_version, "生命周期事件状态版本")?;
    append_sequenced_event(
        transaction,
        SequencedEvent {
            event_id: Uuid::new_v4(),
            stream_key: &stream_key,
            kind: event_kind,
            schema_version: 1,
            payload,
            state_version,
            origin_command_id: command.command_id.as_uuid(),
            created_at: None,
        },
    )
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

/// 在生命周期父记录与 Final 代理子记录之间复用同一旧版写入边界。
pub(super) async fn insert_lifecycle_audit(
    transaction: &mut PostgresCompetitionTransaction,
    record: LifecycleAuditRecord,
) -> Result<(), CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'success', NULL, $8, $9)"#,
    )
    .bind(Uuid::new_v4())
    .bind(record.occurred_at)
    .bind(record.actor_type)
    .bind(record.actor_id)
    .bind(record.action)
    .bind(record.entity_type)
    .bind(record.entity_id)
    .bind(record.source)
    .bind(record.metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

async fn preview_item_count(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
) -> Result<Option<u64>, CompetitionTransactionError> {
    if command.action != auction_application::auction::AuctionLifecycleAction::Preview {
        return Ok(None);
    }
    let transaction = transaction.transaction_mut()?;
    let item_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint
         FROM public.auction_preview_items
         WHERE auction_id = $1",
    )
    .bind(command.auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    u64::try_from(item_count).map(Some).map_err(|_| invalid_state("预览物化条目数量无效"))
}

fn lifecycle_event_payload(
    command: &AuctionLifecycleCommand,
    context: &AuctionLifecycleContext,
    plan: &AuctionLifecyclePlan,
    result: &AuctionLifecycleResult,
    preview_item_count: Option<u64>,
) -> Result<Option<(&'static str, Value)>, CompetitionTransactionError> {
    let event = match &plan.effect {
        AuctionLifecycleEffect::Preview { manual, .. } => Some((
            "auction.preview_updated",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "preview_revision": context.preview_revision,
                "item_count": preview_item_count
                    .ok_or_else(|| invalid_state("预览实时事件缺少物化条目数量"))?,
                "manual_open": context.preview_manual_open || *manual,
                "state_version": result.state_version,
            }),
        )),
        AuctionLifecycleEffect::Open { .. } => Some((
            "auction.opened",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "world_id": &context.world_id,
                "state_version": result.state_version,
                "preview_revision": context.preview_revision,
            }),
        )),
        AuctionLifecycleEffect::Cancel { .. } if context.auction.status == AuctionStatus::Draft => {
            None
        }
        AuctionLifecycleEffect::Cancel { .. } => Some((
            "auction.cancelled",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "state_version": result.state_version,
            }),
        )),
        AuctionLifecycleEffect::FinalPhase { .. } => Some((
            "auction.final_phase_entered",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "state_version": result.state_version,
            }),
        )),
        AuctionLifecycleEffect::EarlySettlement { .. } => Some((
            "auction.early_settlement_requested",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "state_version": result.state_version,
            }),
        )),
    };
    if plan.effect.action() != command.action {
        return Err(invalid_state("生命周期实时事件与命令动作不一致"));
    }
    Ok(event)
}
