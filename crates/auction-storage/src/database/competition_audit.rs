//! Auction-BF 竞价命令审计映射；只追加接受命令产生的事实。

use auction_application::{
    CompetitionCommandAuditContext, CompetitionCommandAuditKind, CompetitionTransactionError,
};
use auction_domain::{BidMechanism, CompetitionPlan, ProxyMode, ProxyTransitionKind};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

/// 两种数据库适配器写入同一冻结审计闭集所需的后端无关记录。
pub(crate) struct CompetitionCommandAuditRecord {
    pub occurred_at: DateTime<Utc>,
    pub actor_type: &'static str,
    pub actor_id: String,
    pub action: &'static str,
    pub entity_type: &'static str,
    pub entity_id: String,
    pub source: &'static str,
    pub metadata: Value,
}

/// 把一次外部 Public 命令映射为 Auction-BF 的唯一命令级审计。
pub(crate) fn competition_command_audit_record(
    plan: &CompetitionPlan,
    context: CompetitionCommandAuditContext,
) -> Result<CompetitionCommandAuditRecord, CompetitionTransactionError> {
    plan.validate().map_err(|_| invalid_audit("竞价审计计划无效"))?;
    if context.command_id.is_nil()
        || context.actor_id.is_nil()
        || context.occurred_at.timestamp_nanos_opt().is_none()
        || plan.public_bids.iter().any(|bid| bid.command_id != context.command_id)
        || plan
            .proxy_transitions
            .iter()
            .any(|transition| transition.command_id != context.command_id)
    {
        return Err(invalid_audit("竞价审计上下文与计划不一致"));
    }

    match context.kind {
        CompetitionCommandAuditKind::PlaceBid => place_bid_record(plan, context),
        CompetitionCommandAuditKind::CreateProxy => create_proxy_record(plan, context),
        CompetitionCommandAuditKind::LowerProxy { authorization_id, new_maximum } => {
            lower_proxy_record(plan, context, authorization_id, new_maximum)
        }
    }
}

fn place_bid_record(
    plan: &CompetitionPlan,
    context: CompetitionCommandAuditContext,
) -> Result<CompetitionCommandAuditRecord, CompetitionTransactionError> {
    let Some(first_bid) = plan.public_bids.first() else {
        return Err(invalid_audit("普通竞价审计缺少公开事实"));
    };
    let Some(current_price) = plan.final_lot.current_price else {
        return Err(invalid_audit("普通竞价审计缺少最终价格"));
    };
    let Some(leader_id) = plan.final_lot.leader_id else {
        return Err(invalid_audit("普通竞价审计缺少最终领先者"));
    };
    if first_bid.bidder_id != context.actor_id || first_bid.mechanism != BidMechanism::Ordinary {
        return Err(invalid_audit("普通竞价审计发起者不一致"));
    }
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
    Ok(CompetitionCommandAuditRecord {
        occurred_at: context.occurred_at,
        actor_type: "public_user",
        actor_id: context.actor_id.as_uuid().to_string(),
        action: "lot.bid_accepted",
        entity_type: "lot",
        entity_id: plan.final_lot.lot_id.as_uuid().to_string(),
        source: "public",
        metadata: json!({
            "lot_id": uuid_bytes(plan.final_lot.lot_id.as_uuid()),
            "amount": current_price.value(),
            "leader_id": uuid_bytes(leader_id.as_uuid()),
            "effective_end": plan.final_lot.effective_end_at,
            "state_version": plan.final_lot.state_version,
            "facts": facts,
        }),
    })
}

fn create_proxy_record(
    plan: &CompetitionPlan,
    context: CompetitionCommandAuditContext,
) -> Result<CompetitionCommandAuditRecord, CompetitionTransactionError> {
    let created = plan
        .proxy_transitions
        .iter()
        .find(|transition| transition.kind == ProxyTransitionKind::Created)
        .ok_or_else(|| invalid_audit("代理创建审计缺少 created 转换"))?;
    let state = plan
        .proxy_states
        .iter()
        .find(|state| state.id == created.authorization_id)
        .ok_or_else(|| invalid_audit("代理创建审计缺少授权快照"))?;
    if state.user_id != context.actor_id
        || state.creation_command_id != context.command_id
        || state.lot_id != plan.final_lot.lot_id
        || plan.public_bids.len() != 1
    {
        return Err(invalid_audit("代理创建审计上下文与授权不一致"));
    }
    Ok(CompetitionCommandAuditRecord {
        occurred_at: context.occurred_at,
        // 旧 helper 未把新 action 加入 Public 分类；该历史分类属于冻结兼容行为。
        actor_type: "admin",
        actor_id: context.actor_id.as_uuid().to_string(),
        action: "lot.proxy_authorization_created",
        entity_type: "auction",
        entity_id: plan.final_lot.lot_id.as_uuid().to_string(),
        source: "admin",
        metadata: json!({
            "authorization_id": state.id.as_uuid().to_string(),
            "lot_id": state.lot_id.as_uuid().to_string(),
            "mode": proxy_mode(state.mode),
            "usage_number": state.usage_number,
            "public_fact_count": plan.public_bids.len(),
        }),
    })
}

fn lower_proxy_record(
    plan: &CompetitionPlan,
    context: CompetitionCommandAuditContext,
    authorization_id: auction_domain::ProxyAuthorizationId,
    new_maximum: auction_domain::BidAmount,
) -> Result<CompetitionCommandAuditRecord, CompetitionTransactionError> {
    let matching = plan
        .proxy_transitions
        .iter()
        .filter(|transition| {
            transition.authorization_id == authorization_id
                && transition.kind == ProxyTransitionKind::Lowered
        })
        .collect::<Vec<_>>();
    let state = plan.proxy_states.iter().find(|state| state.id == authorization_id);
    if !plan.public_bids.is_empty()
        || matching.len() != 1
        || matching[0].new_maximum != new_maximum
        || state.is_none_or(|state| {
            state.user_id != context.actor_id
                || state.maximum_amount != new_maximum
                || state.lot_id != plan.final_lot.lot_id
        })
    {
        return Err(invalid_audit("代理降低审计上下文与授权不一致"));
    }
    Ok(CompetitionCommandAuditRecord {
        occurred_at: context.occurred_at,
        actor_type: "admin",
        actor_id: context.actor_id.as_uuid().to_string(),
        action: "lot.proxy_authorization_lowered",
        entity_type: "auction",
        entity_id: plan.final_lot.lot_id.as_uuid().to_string(),
        source: "admin",
        metadata: json!({
            "authorization_id": authorization_id.as_uuid().to_string(),
            "lot_id": plan.final_lot.lot_id.as_uuid().to_string(),
            "maximum_amount": new_maximum.value(),
        }),
    })
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
