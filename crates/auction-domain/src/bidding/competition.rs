use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    AcceptedBid, AuctionRules, BidAmount, BidMechanism, CommandId, DomainError, LotBidState,
    ProxyAuthorization, ProxyAuthorizationId, ProxyMode, ProxyStatus, ProxyTerminalReason,
    ProxyTransition, ProxyTransitionKind, UserId,
};

/// 纯竞价规划器接受的命令种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompetitionTriggerKind {
    OrdinaryBid,
    CreateProxy,
    LowerProxy,
    SlowDue,
    FinalProxy,
    FreezeProxy,
    CloseProxy,
}

/// 竞价规划器的输入命令。
///
/// 每个变体只携带自己需要的字段，避免用一组可互相矛盾的可选字段表达命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompetitionTrigger {
    OrdinaryBid {
        command_id: CommandId,
        actor_id: UserId,
        amount: BidAmount,
        bid_ids: Vec<crate::BidId>,
        now: DateTime<Utc>,
    },
    CreateProxy {
        command_id: CommandId,
        actor_id: UserId,
        authorization_id: ProxyAuthorizationId,
        maximum_amount: BidAmount,
        mode: ProxyMode,
        authorization_sequence: u64,
        usage_number: u64,
        bid_ids: Vec<crate::BidId>,
        now: DateTime<Utc>,
    },
    LowerProxy {
        command_id: CommandId,
        actor_id: UserId,
        authorization_id: ProxyAuthorizationId,
        new_maximum: BidAmount,
        now: DateTime<Utc>,
    },
    SlowDue {
        command_id: CommandId,
        authorization_id: ProxyAuthorizationId,
        generation: u64,
        due_at: DateTime<Utc>,
        bid_ids: Vec<crate::BidId>,
        now: DateTime<Utc>,
    },
    FinalProxy {
        command_id: CommandId,
        authorization_id: ProxyAuthorizationId,
        bid_ids: Vec<crate::BidId>,
        now: DateTime<Utc>,
    },
    FreezeProxy {
        command_id: CommandId,
        authorization_id: ProxyAuthorizationId,
        now: DateTime<Utc>,
    },
    CloseProxy {
        command_id: CommandId,
        authorization_id: ProxyAuthorizationId,
        now: DateTime<Utc>,
    },
}

impl CompetitionTrigger {
    /// 返回命令种类，供应用层选择正确的事务和审计编码。
    pub const fn kind(&self) -> CompetitionTriggerKind {
        match self {
            Self::OrdinaryBid { .. } => CompetitionTriggerKind::OrdinaryBid,
            Self::CreateProxy { .. } => CompetitionTriggerKind::CreateProxy,
            Self::LowerProxy { .. } => CompetitionTriggerKind::LowerProxy,
            Self::SlowDue { .. } => CompetitionTriggerKind::SlowDue,
            Self::FinalProxy { .. } => CompetitionTriggerKind::FinalProxy,
            Self::FreezeProxy { .. } => CompetitionTriggerKind::FreezeProxy,
            Self::CloseProxy { .. } => CompetitionTriggerKind::CloseProxy,
        }
    }

    fn command_id(&self) -> CommandId {
        match self {
            Self::OrdinaryBid { command_id, .. }
            | Self::CreateProxy { command_id, .. }
            | Self::LowerProxy { command_id, .. }
            | Self::SlowDue { command_id, .. }
            | Self::FinalProxy { command_id, .. }
            | Self::FreezeProxy { command_id, .. }
            | Self::CloseProxy { command_id, .. } => *command_id,
        }
    }

    fn now(&self) -> DateTime<Utc> {
        match self {
            Self::OrdinaryBid { now, .. }
            | Self::CreateProxy { now, .. }
            | Self::LowerProxy { now, .. }
            | Self::SlowDue { now, .. }
            | Self::FinalProxy { now, .. }
            | Self::FreezeProxy { now, .. }
            | Self::CloseProxy { now, .. } => *now,
        }
    }

    fn bid_ids(&self) -> &[crate::BidId] {
        match self {
            Self::OrdinaryBid { bid_ids, .. }
            | Self::CreateProxy { bid_ids, .. }
            | Self::SlowDue { bid_ids, .. }
            | Self::FinalProxy { bid_ids, .. } => bid_ids,
            Self::LowerProxy { .. } | Self::FreezeProxy { .. } | Self::CloseProxy { .. } => &[],
        }
    }
}

/// 一个命令在锁内计算出的完整竞价结果。
///
/// 应用层必须把这里的公开事实、代理转换、快照和最终 lot 投影放进同一个本地事务；
/// 规划器本身不修改输入，也不执行数据库或网络 I/O。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompetitionPlan {
    pub public_bids: Vec<AcceptedBid>,
    pub proxy_transitions: Vec<ProxyTransition>,
    pub proxy_states: Vec<ProxyAuthorization>,
    pub active_proxy_id: Option<ProxyAuthorizationId>,
    pub final_lot: LotBidState,
    pub soft_close_applied: bool,
}

#[derive(Debug, Clone, Copy)]
struct BidDraft {
    bid_id: crate::BidId,
    bidder_id: UserId,
    amount: BidAmount,
    mechanism: BidMechanism,
}

struct ProxyPlanOutput {
    transitions: Vec<ProxyTransition>,
    states: Vec<ProxyAuthorization>,
    active_proxy_id: Option<ProxyAuthorizationId>,
}

impl CompetitionPlan {
    /// 计算一次竞价、代理或代理生命周期命令的原子结果。
    pub fn plan(
        rules: AuctionRules,
        state: LotBidState,
        active: Option<ProxyAuthorization>,
        trigger: CompetitionTrigger,
    ) -> Result<Self, DomainError> {
        rules.validate()?;
        state.validate()?;
        validate_active_authorization(state, active.as_ref())?;
        validate_trigger_basics(&trigger)?;

        let plan = match trigger.kind() {
            CompetitionTriggerKind::OrdinaryBid => plan_ordinary_bid(rules, state, active, trigger),
            CompetitionTriggerKind::CreateProxy => plan_create_proxy(rules, state, active, trigger),
            CompetitionTriggerKind::LowerProxy => plan_lower_proxy(rules, state, active, trigger),
            CompetitionTriggerKind::SlowDue => plan_slow_due(rules, state, active, trigger),
            CompetitionTriggerKind::FinalProxy => plan_final_proxy(rules, state, active, trigger),
            CompetitionTriggerKind::FreezeProxy => {
                plan_terminal_proxy(rules, state, active, trigger, ProxyStatus::Frozen)
            }
            CompetitionTriggerKind::CloseProxy => {
                plan_terminal_proxy(rules, state, active, trigger, ProxyStatus::Closed)
            }
        }?;

        plan.validate()?;
        Ok(plan)
    }

    /// 校验规划结果的事实顺序、版本推进和代理转换链。
    pub fn validate(&self) -> Result<(), DomainError> {
        self.final_lot.validate()?;

        let mut previous_version = self
            .final_lot
            .state_version
            .checked_sub(self.public_bids.len() as u64)
            .ok_or(DomainError::InvalidBidState("公开竞价事实超过最终状态版本"))?;
        let mut command_id = None;
        let mut seen_bid_ids = HashSet::with_capacity(self.public_bids.len());
        for (index, bid) in self.public_bids.iter().enumerate() {
            bid.validate()?;
            if bid.fact_index != (index as u32 + 1)
                || bid.lot_id != self.final_lot.lot_id
                || bid.state_version != previous_version + 1
                || bid.effective_end_at != self.final_lot.effective_end_at
                || !seen_bid_ids.insert(bid.id)
            {
                return Err(DomainError::InvalidBidState("公开竞价事实顺序或版本无效"));
            }
            if let Some(expected) = command_id {
                if expected != bid.command_id {
                    return Err(DomainError::InvalidBidState("同一规划结果包含多个命令"));
                }
            } else {
                command_id = Some(bid.command_id);
            }
            previous_version = bid.state_version;
        }

        let mut states = HashMap::with_capacity(self.proxy_states.len());
        for state in &self.proxy_states {
            state.validate()?;
            if state.lot_id != self.final_lot.lot_id || states.insert(state.id, *state).is_some() {
                return Err(DomainError::InvalidBidState("代理快照重复或 lot 不一致"));
            }
        }

        let mut transition_versions: HashMap<ProxyAuthorizationId, u64> = HashMap::new();
        for transition in &self.proxy_transitions {
            transition.validate()?;
            if let Some(expected) = command_id {
                if expected != transition.command_id {
                    return Err(DomainError::InvalidBidState("代理转换与竞价事实命令不一致"));
                }
            } else {
                command_id = Some(transition.command_id);
            }

            let expected_version = match transition.kind {
                ProxyTransitionKind::Created => 1,
                _ => {
                    transition_versions
                        .get(&transition.authorization_id)
                        .copied()
                        .unwrap_or(transition.state_version - 1)
                        + 1
                }
            };
            if transition.state_version != expected_version {
                return Err(DomainError::InvalidBidState("代理转换版本没有连续推进"));
            }
            transition_versions.insert(transition.authorization_id, transition.state_version);
        }

        for (authorization_id, state) in &states {
            if let Some(version) = transition_versions.get(authorization_id)
                && *version != state.state_version
            {
                return Err(DomainError::InvalidBidState("代理快照与转换链版本不一致"));
            }
        }

        if let Some(active_id) = self.active_proxy_id
            && states.get(&active_id).is_some_and(|state| !is_active_proxy_status(state.status))
        {
            return Err(DomainError::InvalidBidState("活动代理标识指向终态快照"));
        }
        if let Some(slow_id) = self.final_lot.slow_authorization_id
            && self.active_proxy_id != Some(slow_id)
        {
            return Err(DomainError::InvalidBidState("慢速代理调度与活动授权不一致"));
        }
        if self.final_lot.slow_authorization_id.is_none() && self.final_lot.slow_due_at.is_some() {
            return Err(DomainError::InvalidBidState("慢速代理到期时间缺少授权"));
        }

        Ok(())
    }
}

fn validate_trigger_basics(trigger: &CompetitionTrigger) -> Result<(), DomainError> {
    if trigger.command_id().is_nil() || trigger.now().timestamp_nanos_opt().is_none() {
        return Err(DomainError::InvalidBidState("竞价命令缺少有效标识或时间"));
    }

    let mut seen = HashSet::with_capacity(trigger.bid_ids().len());
    for bid_id in trigger.bid_ids() {
        if bid_id.is_nil() || !seen.insert(*bid_id) {
            return Err(DomainError::InvalidBidState("公开竞价事实标识重复或无效"));
        }
    }
    Ok(())
}

fn validate_active_authorization(
    state: LotBidState,
    active: Option<&ProxyAuthorization>,
) -> Result<(), DomainError> {
    let Some(active) = active else {
        if state.leader_proxy_last || state.slow_authorization_id.is_some() {
            return Err(DomainError::InvalidBidState("竞价状态引用了不存在的活动代理"));
        }
        return Ok(());
    };

    active.validate()?;
    if !is_active_proxy_status(active.status)
        || active.lot_id != state.lot_id
        || active.authorization_sequence == 0
        || active.authorization_sequence > state.last_proxy_sequence
        || active.maximum_amount < state.current_price.unwrap_or(state.start_price)
    {
        return Err(DomainError::InvalidBidState("活动代理快照与 lot 状态不一致"));
    }

    if state.slow_authorization_id == Some(active.id) {
        if active.mode != ProxyMode::Slow
            || state.leader_id == Some(active.user_id)
            || state.slow_due_at.is_none()
        {
            return Err(DomainError::InvalidBidState("慢速代理调度状态无效"));
        }
    } else if state.slow_authorization_id.is_some()
        || (state.leader_id.is_some() && state.leader_id != Some(active.user_id))
    {
        return Err(DomainError::InvalidBidState("活动代理与领先者状态不一致"));
    }
    Ok(())
}

fn plan_ordinary_bid(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::OrdinaryBid { command_id, actor_id, amount, bid_ids, now } = trigger
    else {
        unreachable!("竞价触发器种类已在外层匹配");
    };
    if actor_id.is_nil() || state.leader_id == Some(actor_id) {
        return Err(DomainError::InvalidBidState("领先者不能重复提交普通出价"));
    }
    if active.as_ref().is_some_and(|proxy| proxy.user_id == actor_id) {
        return Err(DomainError::InvalidBidState("活动代理持有者不能提交普通出价"));
    }

    let minimum =
        rules.next_minimum_bid(Some(state.start_price), state.current_price, state.material, false);
    if amount < minimum {
        return Err(DomainError::BidTooLow { minimum });
    }

    let mut final_state = state;
    let mut transitions = Vec::new();
    let mut proxy_states = Vec::new();
    let mut active_proxy_id = active.as_ref().map(|proxy| proxy.id);
    let mut drafts = vec![BidDraft {
        bid_id: *bid_ids.first().ok_or(DomainError::InvalidBidState("普通出价缺少事实标识"))?,
        bidder_id: actor_id,
        amount,
        mechanism: BidMechanism::Ordinary,
    }];

    if let Some(active) = active {
        if amount > active.maximum_amount {
            let (after, transition) =
                exhaust_proxy(active, command_id, ProxyTerminalReason::StrictlyDefeated)?;
            final_state = clear_slow_schedule(final_state)?;
            transitions.push(transition);
            proxy_states.push(after);
            active_proxy_id = None;
        } else if amount == active.maximum_amount {
            final_state = clear_slow_schedule(final_state)?;
            let bid_id = *bid_ids
                .get(1)
                .ok_or(DomainError::InvalidBidState("代理反超缺少第二条事实标识"))?;
            drafts.push(BidDraft {
                bid_id,
                bidder_id: active.user_id,
                amount,
                mechanism: BidMechanism::Proxy,
            });
        } else if active.mode == ProxyMode::Fast {
            let response = rules.next_minimum_bid(
                Some(final_state.start_price),
                Some(amount),
                final_state.material,
                false,
            );
            let response = response.min(active.maximum_amount);
            if response <= amount {
                return Err(DomainError::InvalidBidState("快速代理无法产生更高出价"));
            }
            let bid_id = *bid_ids
                .get(1)
                .ok_or(DomainError::InvalidBidState("快速代理缺少第二条事实标识"))?;
            drafts.push(BidDraft {
                bid_id,
                bidder_id: active.user_id,
                amount: response,
                mechanism: BidMechanism::Proxy,
            });
        } else {
            if final_state.slow_authorization_id.is_none() {
                final_state = schedule_slow_proxy(rules, final_state, active.id, now)?;
            }
        }
    } else if bid_ids.len() != 1 {
        return Err(DomainError::InvalidBidState("普通出价事实数量不正确"));
    }
    if bid_ids.len() != drafts.len() {
        return Err(DomainError::InvalidBidState("普通出价事实数量不正确"));
    }

    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        drafts,
        ProxyPlanOutput { transitions, states: proxy_states, active_proxy_id },
    )
}

fn plan_create_proxy(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::CreateProxy {
        command_id,
        actor_id,
        authorization_id,
        maximum_amount,
        mode,
        authorization_sequence,
        usage_number,
        bid_ids,
        now,
    } = trigger
    else {
        unreachable!("竞价触发器种类已在外层匹配");
    };
    if !rules.proxy_enabled {
        return Err(DomainError::ProxyDisabled);
    }
    if rules.proxy_max_outbid_uses > 0 && usage_number > u64::from(rules.proxy_max_outbid_uses) {
        return Err(DomainError::ProxyOutbidLimit);
    }
    if mode == ProxyMode::Slow {
        let final_phase_start = state
            .effective_end_at
            .checked_sub_signed(rules.final_phase_lead)
            .ok_or(DomainError::InvalidSchedule("最终阶段窗口超出时间范围"))?;
        if now >= final_phase_start {
            return Err(DomainError::ProxySlowFinal);
        }
    }
    let expected_sequence =
        state.last_proxy_sequence.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
    if actor_id.is_nil()
        || authorization_id.is_nil()
        || authorization_sequence != expected_sequence
        || state.leader_id == Some(actor_id)
        || active.as_ref().is_some_and(|proxy| proxy.user_id == actor_id)
    {
        return Err(DomainError::InvalidBidState("代理授权输入与 lot 状态不一致"));
    }

    let minimum =
        rules.next_minimum_bid(Some(state.start_price), state.current_price, state.material, false);
    if maximum_amount < minimum {
        return Err(DomainError::BidTooLow { minimum });
    }

    let created = ProxyAuthorization::new(
        authorization_id,
        state.lot_id,
        actor_id,
        command_id,
        authorization_sequence,
        usage_number,
        maximum_amount,
        mode,
    )?;
    let creation_transition = created.creation_transition()?;
    let mut final_state = state;
    final_state.last_proxy_sequence = authorization_sequence;

    let (transitions, proxy_states, active_proxy_id, amount, winner_user_id) =
        if let Some(active) = active {
            let candidate_wins = maximum_amount > active.maximum_amount
                || (maximum_amount == active.maximum_amount
                    && authorization_sequence < active.authorization_sequence);
            let (winner_user_id, winner_proxy_id, winner_maximum, loser) = if candidate_wins {
                (created.user_id, created.id, created.maximum_amount, active)
            } else {
                (active.user_id, active.id, active.maximum_amount, created)
            };
            let mut price = rules.next_minimum_bid(
                Some(final_state.start_price),
                Some(loser.maximum_amount),
                final_state.material,
                false,
            );
            price = price.min(winner_maximum);
            if let Some(current) = final_state.current_price {
                price = price.max(current);
            }
            if price <= final_state.current_price.unwrap_or(final_state.start_price) {
                return Err(DomainError::InvalidBidState("新代理无法产生有效竞价"));
            }
            final_state = clear_slow_schedule(final_state)?;

            let (loser, loser_transition) =
                exhaust_proxy(loser, command_id, ProxyTerminalReason::RankedBelow)?;
            let transitions = vec![creation_transition, loser_transition];
            let mut proxy_states = vec![loser];
            if candidate_wins {
                proxy_states.push(created);
            }
            (transitions, proxy_states, Some(winner_proxy_id), price, winner_user_id)
        } else {
            final_state = clear_slow_schedule(final_state)?;
            (vec![creation_transition], vec![created], Some(created.id), minimum, created.user_id)
        };

    if bid_ids.len() != 1 {
        return Err(DomainError::InvalidBidState("代理创建事实数量不正确"));
    }
    let drafts = vec![BidDraft {
        bid_id: bid_ids[0],
        bidder_id: winner_user_id,
        amount,
        mechanism: BidMechanism::Proxy,
    }];
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        drafts,
        ProxyPlanOutput { transitions, states: proxy_states, active_proxy_id },
    )
}

fn plan_lower_proxy(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::LowerProxy { command_id, actor_id, authorization_id, new_maximum, now } =
        trigger
    else {
        unreachable!("竞价触发器种类已在外层匹配");
    };
    let Some(active) = active else {
        return Err(DomainError::InvalidProxyMaximum);
    };
    if actor_id != active.user_id || authorization_id != active.id {
        return Err(DomainError::InvalidProxyMaximum);
    }
    let binding = if state.slow_authorization_id.is_some() {
        rules.next_minimum_bid(Some(state.start_price), state.current_price, state.material, false)
    } else {
        state.current_price.unwrap_or(state.start_price)
    };
    if new_maximum < binding {
        return Err(DomainError::InvalidProxyMaximum);
    }
    let mut after = active;
    let transition = after.lower(command_id, new_maximum)?;
    finalize_plan(
        rules,
        state,
        command_id,
        now,
        Vec::new(),
        ProxyPlanOutput {
            transitions: vec![transition],
            states: vec![after],
            active_proxy_id: Some(after.id),
        },
    )
}

fn plan_slow_due(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::SlowDue {
        command_id,
        authorization_id,
        generation,
        due_at,
        bid_ids,
        now,
    } = trigger
    else {
        unreachable!("竞价触发器种类已在外层匹配");
    };
    let Some(active) = active else {
        return Err(DomainError::InvalidBidState("慢速代理已不存在"));
    };
    if active.mode != ProxyMode::Slow
        || authorization_id != active.id
        || state.slow_authorization_id != Some(active.id)
        || state.slow_generation != generation
        || state.slow_due_at != Some(due_at)
        || now < due_at
    {
        return Err(DomainError::InvalidBidState("慢速代理调度已经过期"));
    }
    let current = state.current_price.ok_or(DomainError::InvalidBidState("慢速代理缺少当前价"))?;
    let amount = rules
        .next_minimum_bid(Some(state.start_price), Some(current), state.material, false)
        .min(active.maximum_amount);
    if amount <= current {
        return Err(DomainError::InvalidBidState("慢速代理无法产生更高出价"));
    }
    if bid_ids.len() != 1 {
        return Err(DomainError::InvalidBidState("慢速代理事实数量不正确"));
    }
    let final_state = clear_slow_schedule(state)?;
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        vec![BidDraft {
            bid_id: bid_ids[0],
            bidder_id: active.user_id,
            amount,
            mechanism: BidMechanism::Proxy,
        }],
        ProxyPlanOutput {
            transitions: Vec::new(),
            states: Vec::new(),
            active_proxy_id: Some(active.id),
        },
    )
}

fn plan_final_proxy(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
) -> Result<CompetitionPlan, DomainError> {
    let CompetitionTrigger::FinalProxy { command_id, authorization_id, bid_ids, now } = trigger
    else {
        unreachable!("竞价触发器种类已在外层匹配");
    };
    let Some(active) = active else {
        return Err(DomainError::InvalidBidState("慢速代理已不存在"));
    };
    if active.mode != ProxyMode::Slow || authorization_id != active.id {
        return Err(DomainError::InvalidBidState("只有慢速代理可以进入 final fast"));
    }
    let mut after = active;
    let transition = after.promote_to_final_fast(command_id)?;
    let mut final_state = state;
    let mut drafts = Vec::new();
    if final_state.slow_authorization_id == Some(active.id) {
        let current = final_state
            .current_price
            .ok_or(DomainError::InvalidBidState("最终阶段代理缺少当前价"))?;
        let amount = rules
            .next_minimum_bid(
                Some(final_state.start_price),
                Some(current),
                final_state.material,
                false,
            )
            .min(active.maximum_amount);
        if amount <= current {
            return Err(DomainError::InvalidBidState("最终阶段代理无法产生更高出价"));
        }
        if bid_ids.len() != 1 {
            return Err(DomainError::InvalidBidState("最终阶段代理事实数量不正确"));
        }
        final_state = clear_slow_schedule(final_state)?;
        drafts.push(BidDraft {
            bid_id: bid_ids[0],
            bidder_id: active.user_id,
            amount,
            mechanism: BidMechanism::Proxy,
        });
    } else if !bid_ids.is_empty() {
        return Err(DomainError::InvalidBidState("无慢速调度时不应产生最终阶段事实"));
    }
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        drafts,
        ProxyPlanOutput {
            transitions: vec![transition],
            states: vec![after],
            active_proxy_id: Some(after.id),
        },
    )
}

fn plan_terminal_proxy(
    rules: AuctionRules,
    state: LotBidState,
    active: Option<ProxyAuthorization>,
    trigger: CompetitionTrigger,
    next_status: ProxyStatus,
) -> Result<CompetitionPlan, DomainError> {
    let (command_id, authorization_id, now) = match trigger {
        CompetitionTrigger::FreezeProxy { command_id, authorization_id, now }
        | CompetitionTrigger::CloseProxy { command_id, authorization_id, now } => {
            (command_id, authorization_id, now)
        }
        _ => unreachable!("代理终态触发器种类已在外层匹配"),
    };
    let Some(active) = active else {
        return Err(DomainError::InvalidProxyTransition { from: next_status, to: next_status });
    };
    if authorization_id != active.id {
        return Err(DomainError::InvalidProxyTransition { from: active.status, to: next_status });
    }
    let mut after = active;
    let transition = match next_status {
        ProxyStatus::Frozen => after.freeze(command_id, ProxyTerminalReason::EligibilityLost)?,
        ProxyStatus::Closed => after.close(command_id)?,
        _ => {
            return Err(DomainError::InvalidProxyTransition {
                from: active.status,
                to: next_status,
            });
        }
    };
    let final_state = clear_slow_schedule(state)?;
    finalize_plan(
        rules,
        final_state,
        command_id,
        now,
        Vec::new(),
        ProxyPlanOutput {
            transitions: vec![transition],
            states: vec![after],
            active_proxy_id: None,
        },
    )
}

fn finalize_plan(
    rules: AuctionRules,
    mut state: LotBidState,
    command_id: CommandId,
    now: DateTime<Utc>,
    drafts: Vec<BidDraft>,
    proxy_output: ProxyPlanOutput,
) -> Result<CompetitionPlan, DomainError> {
    if !drafts.is_empty() && now > state.effective_end_at {
        return Err(DomainError::AuctionClosed);
    }
    let expected_end = if drafts.is_empty() {
        state.effective_end_at
    } else {
        let soft_close_start = state
            .effective_end_at
            .checked_sub_signed(rules.soft_close_window)
            .ok_or(DomainError::InvalidSchedule("软截止窗口超出时间范围"))?;
        if now >= soft_close_start {
            let candidate = now
                .checked_add_signed(rules.soft_close_extension)
                .ok_or(DomainError::InvalidSchedule("软截止延长超出时间范围"))?;
            candidate.max(state.effective_end_at)
        } else {
            state.effective_end_at
        }
    };
    let soft_close_applied = expected_end > state.effective_end_at;
    state.effective_end_at = expected_end;

    let mut public_bids = Vec::with_capacity(drafts.len());
    for (index, draft) in drafts.into_iter().enumerate() {
        let previous = state.current_price.unwrap_or(state.start_price);
        if draft.amount < previous
            || (draft.amount == previous
                && !(draft.mechanism == BidMechanism::Proxy && state.current_price.is_some()))
        {
            return Err(DomainError::BidTooLow { minimum: previous });
        }
        state.state_version =
            state.state_version.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
        state.current_price = Some(draft.amount);
        state.leader_id = Some(draft.bidder_id);
        state.leader_proxy_last = draft.mechanism == BidMechanism::Proxy;
        let bid = AcceptedBid {
            id: draft.bid_id,
            command_id,
            lot_id: state.lot_id,
            bidder_id: draft.bidder_id,
            fact_index: index as u32 + 1,
            amount: draft.amount,
            effective_end_at: expected_end,
            state_version: state.state_version,
            mechanism: draft.mechanism,
        };
        bid.validate()?;
        public_bids.push(bid);
    }

    Ok(CompetitionPlan {
        public_bids,
        proxy_transitions: proxy_output.transitions,
        proxy_states: proxy_output.states,
        active_proxy_id: proxy_output.active_proxy_id,
        final_lot: state,
        soft_close_applied,
    })
}

fn schedule_slow_proxy(
    rules: AuctionRules,
    mut state: LotBidState,
    authorization_id: ProxyAuthorizationId,
    now: DateTime<Utc>,
) -> Result<LotBidState, DomainError> {
    let generation =
        state.slow_generation.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
    let due_at = now
        .checked_add_signed(rules.proxy_slow_interval)
        .ok_or(DomainError::InvalidSchedule("慢速代理到期时间超出范围"))?;
    state.slow_generation = generation;
    state.slow_authorization_id = Some(authorization_id);
    state.slow_due_at = Some(due_at);
    Ok(state)
}

fn clear_slow_schedule(mut state: LotBidState) -> Result<LotBidState, DomainError> {
    if state.slow_authorization_id.is_none() {
        return Ok(state);
    }
    state.slow_generation =
        state.slow_generation.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
    state.slow_authorization_id = None;
    state.slow_due_at = None;
    Ok(state)
}

fn exhaust_proxy(
    mut authorization: ProxyAuthorization,
    command_id: CommandId,
    reason: ProxyTerminalReason,
) -> Result<(ProxyAuthorization, ProxyTransition), DomainError> {
    let transition = authorization.exhaust(command_id, reason)?;
    authorization.validate()?;
    Ok((authorization, transition))
}

fn is_active_proxy_status(status: ProxyStatus) -> bool {
    matches!(status, ProxyStatus::Active | ProxyStatus::FinalFast)
}
