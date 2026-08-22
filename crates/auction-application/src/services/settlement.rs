use std::sync::Arc;

use auction_domain::{
    AuctionId, CommandId, SettlementId, SettlementObligation, SettlementObligationKind,
    SettlementObligationState, SettlementPlan, SettlementWorld, deterministic_uuid,
};

use crate::{
    ApplicationError, ApplicationFuture, RequestHash, SettlementSnapshot, SettlementTransaction,
    SettlementTransactionRunner, StoredCommand, TransactionPolicy,
};

/// 请求一次拍卖结算。结算命令必须拥有稳定 ID，Worker 重试只能重放同一命令。
#[derive(Debug, Clone, Copy)]
pub struct SettleAuctionCommand {
    pub command_id: CommandId,
    pub auction_id: AuctionId,
    pub request_hash: RequestHash,
}

/// 结算命令的窄应用端口，供 Admin 和 Worker 共享同一幂等用例。
pub trait SettlementCommandPort: Send + Sync {
    fn settle(&self, command: SettleAuctionCommand) -> ApplicationFuture<'_, SettlementPlan>;
}

/// 结算用例服务，只编排锁内事实读取、领域计算和持久化事务。
pub struct SettlementService<R> {
    repository: Arc<R>,
}

impl<R> SettlementService<R>
where
    R: SettlementTransactionRunner,
    R::Transaction: SettlementTransaction,
{
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    /// 在 Serializable 事务中生成并保存结算事实、义务和命令账本。
    pub async fn settle(
        &self,
        command: SettleAuctionCommand,
    ) -> Result<SettlementPlan, ApplicationError> {
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                Box::pin(async move {
                    let snapshot = transaction.load_for_update(command.auction_id).await?;
                    if let Some(stored) = transaction.find_command(command.command_id).await? {
                        if !stored.matches_request(
                            "settle_auction",
                            command.auction_id.as_uuid(),
                            command.request_hash,
                        ) {
                            return Err(crate::CommandLedgerError::Conflict.into());
                        }
                        if stored.result_code != "accepted" {
                            return Err(crate::SettlementTransactionError::Failed(
                                "命令账本结果分类不支持结算重放".to_owned(),
                            )
                            .into());
                        }
                        let replayed: SettlementPlan =
                            serde_json::from_slice(&stored.result_payload)?;
                        replayed.validate(&snapshot.policy)?;
                        return Ok(replayed);
                    }

                    let mut plan = SettlementPlan::calculate(
                        &snapshot.policy,
                        snapshot.scheduled_open_at,
                        snapshot.scheduled_end_at,
                        snapshot.opening_pool,
                        &snapshot.facts,
                    )?;
                    append_obligations(command.command_id, &snapshot, &mut plan)?;
                    plan.validate(&snapshot.policy)?;

                    transaction
                        .insert_command(StoredCommand {
                            command_id: command.command_id,
                            command_kind: "settle_auction".to_owned(),
                            aggregate_id: command.auction_id.as_uuid(),
                            request_hash: command.request_hash,
                            result_code: "accepted".to_owned(),
                            result_payload: serde_json::to_vec(&plan)?,
                        })
                        .await?;
                    transaction
                        .apply_settlement(command.auction_id, command.command_id, plan.clone())
                        .await?;
                    Ok(plan)
                })
            })
            .await
    }
}

impl<R> SettlementCommandPort for SettlementService<R>
where
    R: SettlementTransactionRunner + 'static,
    R::Transaction: SettlementTransaction,
{
    fn settle(&self, command: SettleAuctionCommand) -> ApplicationFuture<'_, SettlementPlan> {
        Box::pin(async move { SettlementService::settle(self, command).await })
    }
}

fn append_obligations(
    command_id: CommandId,
    snapshot: &SettlementSnapshot,
    plan: &mut SettlementPlan,
) -> Result<(), auction_domain::DomainError> {
    let mut obligations = Vec::new();

    for fact in &snapshot.facts {
        let line =
            plan.lines.iter().find(|line| line.lot_id == fact.lot_id).ok_or(
                auction_domain::DomainError::InvalidSettlementFacts("结算明细缺少对应 lot"),
            )?;

        let Some(winner_id) = fact.leader_id else {
            obligations.push(SettlementObligation {
                id: obligation_id(command_id, fact.lot_id, "unsold_return"),
                auction_id: snapshot.auction_id,
                lot_id: fact.lot_id,
                origin_command_id: command_id,
                policy_version: plan.policy_version,
                world: fact.world,
                kind: SettlementObligationKind::UnsoldReturn,
                state: SettlementObligationState::Ready,
                state_version: 1,
                recipient_user_id: None,
                recipient_game_identity: fact.seller_identity.clone(),
                amount: auction_domain::SettlementAmount::zero(),
                carrier_credit: 0,
                equipment_id: fact.equipment_id,
                submission_id: fact.submission_id,
                prerequisite_id: None,
            });
            continue;
        };

        let payment_kind = match fact.world {
            SettlementWorld::Main => SettlementObligationKind::WinnerEquipmentCod,
            SettlementWorld::Isekai => SettlementObligationKind::WinnerPaymentCarrierCod,
        };
        let payment_id = obligation_id(command_id, fact.lot_id, kind_name(payment_kind));
        obligations.push(SettlementObligation {
            id: payment_id,
            auction_id: snapshot.auction_id,
            lot_id: fact.lot_id,
            origin_command_id: command_id,
            policy_version: plan.policy_version,
            world: fact.world,
            kind: payment_kind,
            state: SettlementObligationState::Ready,
            state_version: 1,
            recipient_user_id: Some(winner_id),
            recipient_game_identity: String::new(),
            amount: line.buyer_amount,
            carrier_credit: if fact.world == SettlementWorld::Isekai {
                snapshot.policy.isekai_carrier_credit
            } else {
                0
            },
            equipment_id: fact.equipment_id,
            submission_id: fact.submission_id,
            prerequisite_id: None,
        });

        let seller_prerequisite = if fact.world == SettlementWorld::Isekai {
            let handoff_id = obligation_id(command_id, fact.lot_id, "winner_equipment_handoff");
            obligations.push(SettlementObligation {
                id: handoff_id,
                auction_id: snapshot.auction_id,
                lot_id: fact.lot_id,
                origin_command_id: command_id,
                policy_version: plan.policy_version,
                world: fact.world,
                kind: SettlementObligationKind::WinnerEquipmentHandoff,
                state: SettlementObligationState::Blocked,
                state_version: 1,
                recipient_user_id: Some(winner_id),
                recipient_game_identity: String::new(),
                amount: auction_domain::SettlementAmount::zero(),
                carrier_credit: 0,
                equipment_id: fact.equipment_id,
                submission_id: fact.submission_id,
                prerequisite_id: Some(payment_id),
            });
            handoff_id
        } else {
            payment_id
        };

        obligations.push(SettlementObligation {
            id: obligation_id(command_id, fact.lot_id, "seller_payout"),
            auction_id: snapshot.auction_id,
            lot_id: fact.lot_id,
            origin_command_id: command_id,
            policy_version: plan.policy_version,
            world: fact.world,
            kind: SettlementObligationKind::SellerPayout,
            state: SettlementObligationState::Blocked,
            state_version: 1,
            recipient_user_id: None,
            recipient_game_identity: fact.seller_identity.clone(),
            amount: line.seller_amount,
            carrier_credit: 0,
            equipment_id: fact.equipment_id,
            submission_id: fact.submission_id,
            prerequisite_id: Some(seller_prerequisite),
        });

        obligations.push(SettlementObligation {
            id: obligation_id(command_id, fact.lot_id, "buyer_refund"),
            auction_id: snapshot.auction_id,
            lot_id: fact.lot_id,
            origin_command_id: command_id,
            policy_version: plan.policy_version,
            world: fact.world,
            kind: SettlementObligationKind::BuyerRefund,
            state: SettlementObligationState::Contingent,
            state_version: 1,
            recipient_user_id: Some(winner_id),
            recipient_game_identity: String::new(),
            amount: line.buyer_amount,
            carrier_credit: 0,
            equipment_id: fact.equipment_id,
            submission_id: fact.submission_id,
            prerequisite_id: Some(payment_id),
        });
    }

    plan.obligations = obligations;
    Ok(())
}

fn obligation_id(command_id: CommandId, lot_id: auction_domain::LotId, kind: &str) -> SettlementId {
    SettlementId::from_uuid(deterministic_uuid(
        command_id,
        &format!("settlement-obligation:{lot_id}:{kind}"),
        1,
    ))
}

fn kind_name(kind: SettlementObligationKind) -> &'static str {
    match kind {
        SettlementObligationKind::WinnerEquipmentCod => "winner_equipment_cod",
        SettlementObligationKind::WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
        SettlementObligationKind::WinnerEquipmentHandoff => "winner_equipment_handoff",
        SettlementObligationKind::SellerPayout => "seller_payout",
        SettlementObligationKind::BuyerRefund => "buyer_refund",
        SettlementObligationKind::UnsoldReturn => "unsold_return",
        SettlementObligationKind::WithdrawnReturn => "withdrawn_return",
    }
}
