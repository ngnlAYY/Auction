//! PostgreSQL 与 SQLite 共用的 Auction-BF 生命周期审计映射。

use auction_application::{
    AuctionFinalProxyPlan, AuctionLifecycleAction, AuctionLifecycleActorType,
    AuctionLifecycleCommand, AuctionLifecycleContext, AuctionLifecycleEffect, AuctionLifecyclePlan,
    AuctionLifecycleResult, AuctionLifecycleWriteSummary, CompetitionTransactionError,
};
use auction_domain::{
    AuctionStatus, BidMechanism, ProxyMode, ProxyStatus, ProxyTerminalReason, ProxyTransitionKind,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

/// 两种数据库适配器写入同一冻结审计闭集所需的后端无关记录。
pub(crate) struct LifecycleAuditRecord {
    pub occurred_at: DateTime<Utc>,
    pub actor_type: &'static str,
    pub actor_id: String,
    pub action: &'static str,
    pub entity_type: &'static str,
    pub entity_id: String,
    pub source: &'static str,
    pub metadata: Value,
}

/// 把生命周期父命令映射为 Auction-BF 的动作名、固定分类和精确 metadata。
pub(crate) fn lifecycle_parent_audit_record(
    command: &AuctionLifecycleCommand,
    context: &AuctionLifecycleContext,
    plan: &AuctionLifecyclePlan,
    result: &AuctionLifecycleResult,
    write_summary: AuctionLifecycleWriteSummary,
    preview_item_count: Option<u64>,
) -> Result<LifecycleAuditRecord, CompetitionTransactionError> {
    validate_parent_context(command, context, plan, result)?;

    let (action, metadata) = match &plan.effect {
        AuctionLifecycleEffect::Preview { manual, .. } => {
            let item_count = preview_item_count
                .ok_or_else(|| invalid_audit("预览生命周期审计缺少物化条目数量"))?;
            if context.auction.status != AuctionStatus::Draft
                || plan.auction.status != AuctionStatus::Preview
                || write_summary != AuctionLifecycleWriteSummary::default()
            {
                return Err(invalid_audit("预览生命周期审计状态不一致"));
            }
            (
                "auction.preview_open",
                json!({
                    "auction_id": command.auction_id.as_uuid().to_string(),
                    "preview_revision": context.preview_revision,
                    "item_count": item_count,
                    "manual_open": context.preview_manual_open || *manual,
                    "state_version": result.state_version,
                }),
            )
        }
        AuctionLifecycleEffect::Open { manual_override } => {
            if context.auction.status != AuctionStatus::Preview
                || plan.auction.status != AuctionStatus::Live
                || preview_item_count.is_some()
                || write_summary != AuctionLifecycleWriteSummary::default()
            {
                return Err(invalid_audit("开拍生命周期审计状态不一致"));
            }
            (
                "auction.open",
                json!({
                    "auction_id": command.auction_id.as_uuid().to_string(),
                    "world_id": context.world_id,
                    "state_version": result.state_version,
                    "preview_revision": context.preview_revision,
                    "original_opens_at": context.scheduled_open_at,
                    "actual_opens_at": plan.auction.updated_at,
                    "manual_override": manual_override,
                    "trigger": if command.actor.actor_type == AuctionLifecycleActorType::Admin {
                        "manual_early"
                    } else {
                        "scheduled"
                    },
                }),
            )
        }
        AuctionLifecycleEffect::Cancel => {
            if !matches!(context.auction.status, AuctionStatus::Draft | AuctionStatus::Preview)
                || plan.auction.status != AuctionStatus::Cancelled
                || preview_item_count.is_some()
            {
                return Err(invalid_audit("取消生命周期审计状态不一致"));
            }
            let reason = plan
                .auction
                .cancelled_reason
                .as_deref()
                .filter(|reason| !reason.is_empty())
                .ok_or_else(|| invalid_audit("取消生命周期审计缺少原因"))?;
            (
                "auction.cancel",
                json!({
                    "auction_id": command.auction_id.as_uuid().to_string(),
                    "previous_status": auction_status_name(context.auction.status),
                    "state_version": result.state_version,
                    "reason": reason,
                    "return_obligation_count": write_summary.return_obligation_count,
                }),
            )
        }
        AuctionLifecycleEffect::FinalPhase { .. } => {
            if context.auction.status != AuctionStatus::Live
                || plan.auction.status != AuctionStatus::Final
                || preview_item_count.is_some()
                || write_summary != AuctionLifecycleWriteSummary::default()
            {
                return Err(invalid_audit("最终阶段生命周期审计状态不一致"));
            }
            (
                "auction.enter_final_phase",
                json!({
                    "auction_id": command.auction_id.as_uuid().to_string(),
                    "state_version": result.state_version,
                }),
            )
        }
    };

    Ok(LifecycleAuditRecord {
        occurred_at: plan.auction.updated_at,
        // 旧版 helper 只识别竞价 action；生命周期 action 历史上统一落入 Admin 分类。
        actor_type: "admin",
        actor_id: command.actor.actor_id.clone(),
        action,
        entity_type: "auction",
        entity_id: command.auction_id.as_uuid().to_string(),
        source: "admin",
        metadata: append_request_id(metadata, command.request_id.as_deref())?,
    })
}

/// 把 Final phase 的单个代理子命令压缩为旧版唯一命令级审计。
pub(crate) fn final_proxy_audit_record(
    parent: &AuctionLifecycleCommand,
    occurred_at: DateTime<Utc>,
    proxy_plan: &AuctionFinalProxyPlan,
) -> Result<LifecycleAuditRecord, CompetitionTransactionError> {
    let plan = &proxy_plan.competition;
    plan.validate().map_err(|_| invalid_audit("最终阶段代理审计计划无效"))?;
    if parent.action != AuctionLifecycleAction::FinalPhase
        || parent.command_id.is_nil()
        || proxy_plan.command.command_id.is_nil()
        || proxy_plan.command.command_kind != "final_proxy_transition"
        || proxy_plan.command.result_code != "accepted"
        || proxy_plan.command.aggregate_id != plan.final_lot.lot_id.as_uuid()
        || occurred_at.timestamp_nanos_opt().is_none()
        || plan.public_bids.iter().any(|bid| {
            bid.command_id != proxy_plan.command.command_id || bid.lot_id != plan.final_lot.lot_id
        })
        || plan.proxy_transitions.len() != 1
        || plan.proxy_states.len() != 1
    {
        return Err(invalid_audit("最终阶段代理审计上下文与计划不一致"));
    }

    let transition = plan.proxy_transitions[0];
    let state = plan.proxy_states[0];
    if transition.command_id != proxy_plan.command.command_id
        || transition.authorization_id != state.id
        || transition.state_version != state.state_version
        || state.lot_id != plan.final_lot.lot_id
    {
        return Err(invalid_audit("最终阶段代理审计转换与授权不一致"));
    }

    let (status, terminal_reason) = match transition.kind {
        ProxyTransitionKind::FinalFast
            if state.mode == ProxyMode::Fast
                && state.status == ProxyStatus::FinalFast
                && transition.reason.is_none()
                && plan.active_proxy_id == Some(state.id) =>
        {
            // Rust 内部用 FinalFast 标记最终阶段；旧版外部审计仍把它投影为 active。
            ("active", None)
        }
        ProxyTransitionKind::Frozen
            if state.status == ProxyStatus::Frozen
                && transition.reason == Some(ProxyTerminalReason::EligibilityLost)
                && plan.active_proxy_id.is_none() =>
        {
            ("frozen", Some("eligibility_lost"))
        }
        _ => return Err(invalid_audit("最终阶段代理审计转换不在兼容闭集内")),
    };

    let facts = plan
        .public_bids
        .iter()
        .map(|bid| {
            json!({
                "bid_id": uuid_bytes(bid.id.as_uuid()),
                "origin_command_id": uuid_bytes(bid.command_id.as_uuid()),
                "fact_index": bid.fact_index,
                "lot_id": uuid_bytes(bid.lot_id.as_uuid()),
                "amount": bid.amount.value(),
                "leader_id": uuid_bytes(bid.bidder_id.as_uuid()),
                "effective_end": bid.effective_end_at,
                "accepted_state_version": bid.state_version,
                "mechanism": bid_mechanism(bid.mechanism),
            })
        })
        .collect::<Vec<_>>();
    // 旧版 Go 结构体中的 nil slice 会编码成 null；无最终竞价事实时必须保留这一线协议。
    let facts = if facts.is_empty() { Value::Null } else { Value::Array(facts) };
    let lot = plan.final_lot;
    let has_leader = lot.leader_id.is_some();
    let has_slow_schedule = lot.slow_authorization_id.is_some();
    let slow_due_at = lot
        .slow_due_at
        .map_or_else(|| Value::String("0001-01-01T00:00:00Z".to_owned()), |value| json!(value));
    let mut metadata = json!({
        "authorization_id": uuid_bytes(state.id.as_uuid()),
        "mode": proxy_mode(state.mode),
        "status": status,
        "state_version": state.state_version,
        "final_lot": {
            "lot_id": uuid_bytes(lot.lot_id.as_uuid()),
            "start_price": lot.start_price.value(),
            "current_price": lot.current_price.unwrap_or(lot.start_price).value(),
            "leader_id": lot.leader_id.map_or([0_u8; 16], |id| uuid_bytes(id.as_uuid())),
            "has_leader": has_leader,
            "effective_end": lot.effective_end_at,
            "state_version": lot.state_version,
            "last_proxy_sequence": lot.last_proxy_sequence,
            "slow_generation": lot.slow_generation,
            "slow_authorization_id": lot
                .slow_authorization_id
                .map_or([0_u8; 16], |id| uuid_bytes(id.as_uuid())),
            "slow_due_at": slow_due_at,
            "has_slow_schedule": has_slow_schedule,
        },
        "facts": facts,
    });
    if let Some(reason) = terminal_reason {
        metadata
            .as_object_mut()
            .ok_or_else(|| invalid_audit("最终阶段代理审计 metadata 无效"))?
            .insert("terminal_reason".to_owned(), Value::String(reason.to_owned()));
    }

    Ok(LifecycleAuditRecord {
        occurred_at,
        actor_type: "admin",
        actor_id: parent.actor.actor_id.clone(),
        action: "lot.proxy_final_transition",
        entity_type: "auction",
        entity_id: plan.final_lot.lot_id.as_uuid().to_string(),
        source: "admin",
        metadata,
    })
}

fn validate_parent_context(
    command: &AuctionLifecycleCommand,
    context: &AuctionLifecycleContext,
    plan: &AuctionLifecyclePlan,
    result: &AuctionLifecycleResult,
) -> Result<(), CompetitionTransactionError> {
    if command.command_id.is_nil()
        || command.auction_id.is_nil()
        || context.auction.id != command.auction_id
        || plan.auction.id != command.auction_id
        || plan.effect.action() != command.action
        || result.command_id != command.command_id
        || result.auction_id != command.auction_id
        || result.status != plan.auction.status
        || result.state_version != plan.auction.state_version
        || result.updated_at != plan.auction.updated_at
        || plan.auction.updated_at.timestamp_nanos_opt().is_none()
    {
        return Err(invalid_audit("生命周期父审计上下文与结果不一致"));
    }
    Ok(())
}

fn append_request_id(
    mut metadata: Value,
    request_id: Option<&str>,
) -> Result<Value, CompetitionTransactionError> {
    if let Some(request_id) = request_id {
        metadata
            .as_object_mut()
            .ok_or_else(|| invalid_audit("生命周期父审计 metadata 无效"))?
            .insert("request_id".to_owned(), Value::String(request_id.to_owned()));
    }
    Ok(metadata)
}

const fn auction_status_name(status: AuctionStatus) -> &'static str {
    match status {
        AuctionStatus::Draft => "draft",
        AuctionStatus::Preview => "preview",
        AuctionStatus::Live => "live",
        AuctionStatus::Final => "final",
        AuctionStatus::Settling => "settling",
        AuctionStatus::Settled => "settled",
        AuctionStatus::Cancelled => "cancelled",
    }
}

fn uuid_bytes(id: Uuid) -> [u8; 16] {
    *id.as_bytes()
}

const fn bid_mechanism(mechanism: BidMechanism) -> &'static str {
    match mechanism {
        BidMechanism::Ordinary => "ordinary",
        BidMechanism::Proxy => "proxy",
    }
}

const fn proxy_mode(mode: ProxyMode) -> &'static str {
    match mode {
        ProxyMode::Fast => "fast",
        ProxyMode::Slow => "slow",
    }
}

fn invalid_audit(message: &'static str) -> CompetitionTransactionError {
    CompetitionTransactionError::Failed(message.to_owned())
}
