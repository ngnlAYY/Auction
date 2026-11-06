use super::*;

pub(super) async fn lifecycle_plan<T>(
    transaction: &mut T,
    command: &AuctionLifecycleCommand,
    context: &AuctionLifecycleContext,
    policy: AuctionLifecyclePolicy,
    successor_policy: &AuctionPolicyTemplate,
    calendar: Option<&AuctionScheduleCalendar>,
) -> Result<AuctionLifecyclePlan, ApplicationError>
where
    T: CompetitionTransaction + AuctionLifecycleTransaction,
{
    let final_inputs = match command.action {
        crate::ports::auction::AuctionLifecycleAction::FinalPhase => {
            Some(load_final_proxy_inputs(transaction, command).await?)
        }
        crate::ports::auction::AuctionLifecycleAction::EarlySettlement
            if context.auction.status == auction_domain::AuctionStatus::Live =>
        {
            Some(load_final_proxy_inputs(transaction, command).await?)
        }
        crate::ports::auction::AuctionLifecycleAction::EarlySettlement => {
            if transaction.list_final_lot_ids_for_update(command.auction_id).await?.is_empty() {
                return Err(CompetitionTransactionError::InvalidState.into());
            }
            None
        }
        _ => None,
    };
    // 生命周期、代理竞价事实和提前截止使用锁齐备后的同一个数据库时间；重试会重新读取。
    let now = transaction.current_time().await?;
    let mut auction = context.auction.clone();
    let admin_action =
        command.actor.actor_type == crate::ports::auction::AuctionLifecycleActorType::Admin;
    let effect = match command.action {
        crate::ports::auction::AuctionLifecycleAction::Preview => {
            if !context.world_enabled {
                return Err(CompetitionTransactionError::InvalidState.into());
            }
            if !admin_action && now < context.preview_scheduled_at {
                return Err(CompetitionTransactionError::InvalidState.into());
            }
            auction.preview(now)?;
            AuctionLifecycleEffect::Preview { manual: admin_action }
        }
        crate::ports::auction::AuctionLifecycleAction::Open => {
            if !context.world_enabled {
                return Err(CompetitionTransactionError::InvalidState.into());
            }
            let manual_override = admin_action && now < context.scheduled_open_at;
            if manual_override && !policy.allows_manual_early_open() {
                return Err(CompetitionTransactionError::InvalidState.into());
            }
            if !manual_override
                && (now < context.preview_scheduled_at || now < context.scheduled_open_at)
            {
                return Err(CompetitionTransactionError::InvalidState.into());
            }
            auction.open(context.policy_scheduled_end_at, now)?;
            AuctionLifecycleEffect::Open {
                manual_override,
                successor: successor_plan(context, successor_policy, calendar, now)?,
            }
        }
        crate::ports::auction::AuctionLifecycleAction::Cancel => {
            let reason = command
                .reason
                .clone()
                .unwrap_or_else(|| "Worker lifecycle cancellation".to_owned());
            let successor = (auction.status == auction_domain::AuctionStatus::Preview)
                .then(|| successor_plan(context, successor_policy, calendar, now))
                .transpose()?;
            auction.cancel_before_open_with_metadata(
                now,
                reason,
                command.actor.actor_id.clone(),
            )?;
            AuctionLifecycleEffect::Cancel { successor }
        }
        crate::ports::auction::AuctionLifecycleAction::FinalPhase => {
            let (proxy_plans, earliest_end_at) =
                final_proxy_plans(command, final_inputs.unwrap_or_default(), now)?;
            if !admin_action {
                let due_at = earliest_end_at
                    .checked_sub_signed(context.policy_final_phase_lead)
                    .ok_or_else(|| lifecycle_state("最终阶段到期时间超出范围"))?;
                if now < due_at {
                    return Err(CompetitionTransactionError::InvalidState.into());
                }
            }
            let effective_end_at = if admin_action {
                Some(
                    now.checked_add_signed(context.policy_final_phase_lead)
                        .ok_or_else(|| lifecycle_state("最终阶段截止时间超出范围"))?,
                )
            } else {
                None
            };
            auction.enter_final(now)?;
            AuctionLifecycleEffect::FinalPhase { proxy_plans, effective_end_at }
        }
        crate::ports::auction::AuctionLifecycleAction::EarlySettlement => {
            if !admin_action {
                return Err(CompetitionTransactionError::InvalidState.into());
            }
            if !policy.allows_manual_early_settlement() {
                return Err(ApplicationError::CapabilityUnavailable(
                    "manual early settlement disabled",
                ));
            }
            let reason = command
                .reason
                .clone()
                .filter(|reason| {
                    !reason.trim().is_empty()
                        && reason.len() <= 1024
                        && !reason.chars().any(char::is_control)
                })
                .ok_or(CompetitionTransactionError::InvalidState)?;
            let proxy_plans = match auction.status {
                auction_domain::AuctionStatus::Live => {
                    final_proxy_plans(command, final_inputs.unwrap_or_default(), now)?.0
                }
                auction_domain::AuctionStatus::Final => Vec::new(),
                _ => return Err(CompetitionTransactionError::InvalidState.into()),
            };
            auction.request_early_settlement(now)?;
            AuctionLifecycleEffect::EarlySettlement { proxy_plans, effective_end_at: now, reason }
        }
    };
    Ok(AuctionLifecyclePlan { auction, effect })
}

/// 先锁定全部 lot、代理与参与者，再统一计时和生成计划，避免等待期间使用过期截止时间。
async fn load_final_proxy_inputs<T>(
    transaction: &mut T,
    parent: &AuctionLifecycleCommand,
) -> Result<Vec<(crate::ports::core::CompetitionSnapshot, bool)>, ApplicationError>
where
    T: CompetitionTransaction + AuctionLifecycleTransaction,
{
    let lot_ids = transaction.list_final_lot_ids_for_update(parent.auction_id).await?;
    if lot_ids.is_empty() {
        return Err(CompetitionTransactionError::InvalidState.into());
    }
    let mut snapshots = Vec::with_capacity(lot_ids.len());
    for lot_id in lot_ids {
        snapshots.push(transaction.load_final_lot_for_update(lot_id).await?);
    }
    let mut participants: Vec<_> = snapshots
        .iter()
        .filter_map(|snapshot| snapshot.active_proxy.map(|proxy| proxy.user_id))
        .collect();
    participants.sort_unstable();
    participants.dedup();
    let mut eligibility = std::collections::BTreeMap::new();
    for user_id in participants {
        eligibility.insert(user_id, transaction.participant_is_eligible_for_update(user_id).await?);
    }
    Ok(snapshots
        .into_iter()
        .map(|snapshot| {
            let eligible = snapshot
                .active_proxy
                .is_none_or(|proxy| eligibility.get(&proxy.user_id).copied().unwrap_or(false));
            (snapshot, eligible)
        })
        .collect())
}

fn final_proxy_plans(
    parent: &AuctionLifecycleCommand,
    snapshots: Vec<(crate::ports::core::CompetitionSnapshot, bool)>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(Vec<AuctionFinalProxyPlan>, chrono::DateTime<chrono::Utc>), ApplicationError> {
    let mut plans = Vec::new();
    let mut earliest_end_at: Option<chrono::DateTime<chrono::Utc>> = None;
    for (snapshot, eligible) in snapshots {
        let lot_id = snapshot.state.lot_id;
        earliest_end_at =
            Some(earliest_end_at.map_or(snapshot.state.effective_end_at, |current| {
                current.min(snapshot.state.effective_end_at)
            }));
        // 已自然到期的 lot 不再接受 Final 代理转换；结算事务会关闭其剩余授权。其它 lot
        // 仍按同一锁内快照完成 Final 转换，避免一件已到期 lot 阻断整场拍卖收口。
        if now >= snapshot.state.effective_end_at {
            continue;
        }
        let Some(active) = snapshot.active_proxy else {
            continue;
        };
        if eligible && active.mode == ProxyMode::Fast {
            continue;
        }

        let child_command_id = CommandId::from_uuid(deterministic_uuid(
            parent.command_id,
            &format!("auction-final-proxy:{lot_id}"),
            1,
        ));
        let trigger = if eligible {
            let bid_ids = if snapshot.state.slow_authorization_id == Some(active.id) {
                vec![BidId::from_uuid(deterministic_uuid(child_command_id, "auction-bid-fact", 1))]
            } else {
                Vec::new()
            };
            CompetitionTrigger::FinalProxy {
                command_id: child_command_id,
                authorization_id: active.id,
                bid_ids,
                now,
            }
        } else {
            CompetitionTrigger::FreezeProxy {
                command_id: child_command_id,
                authorization_id: active.id,
                now,
            }
        };
        let request_hash =
            RequestHash::from_request("final_proxy_transition", lot_id.as_uuid(), &trigger)?;
        let competition =
            CompetitionPlan::plan(snapshot.rules, snapshot.state, Some(active), trigger)?;
        plans.push(AuctionFinalProxyPlan {
            command: StoredCommand {
                command_id: child_command_id,
                command_kind: "final_proxy_transition".to_owned(),
                aggregate_id: lot_id.as_uuid(),
                request_hash,
                result_code: "accepted".to_owned(),
                result_payload: serde_json::to_vec(&competition)?,
            },
            competition,
        });
    }
    let earliest_end_at =
        earliest_end_at.ok_or_else(|| lifecycle_state("最终阶段缺少活动标的结束时间"))?;
    Ok((plans, earliest_end_at))
}
