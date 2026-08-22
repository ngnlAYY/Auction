use std::sync::Arc;

use auction_domain::{
    BidAmount, BidId, CommandId, CompetitionPlan, CompetitionTrigger, DomainError, LotId,
    ProxyAuthorizationId, ProxyMode, UserId, deterministic_uuid,
};

use crate::{
    ApplicationError, Clock, CompetitionCommandPort, CompetitionTransaction, RequestHash,
    StoredCommand, TransactionPolicy, TransactionRunner,
};

/// 普通出价命令。事实 ID 由命令 ID 在应用层稳定派生，调用方不能注入第二个随机 ID。
#[derive(Debug, Clone, Copy)]
pub struct PlaceBidCommand {
    pub command_id: CommandId,
    pub lot_id: LotId,
    pub bidder_id: UserId,
    pub amount: BidAmount,
    pub request_hash: RequestHash,
}

/// 创建代理授权命令；授权标识由命令 ID 在应用层稳定派生。
#[derive(Debug, Clone, Copy)]
pub struct CreateProxyCommand {
    pub command_id: CommandId,
    pub lot_id: LotId,
    pub bidder_id: UserId,
    pub maximum_amount: BidAmount,
    pub mode: ProxyMode,
    pub request_hash: RequestHash,
}

/// 降低代理授权上限命令。
#[derive(Debug, Clone, Copy)]
pub struct LowerProxyCommand {
    pub command_id: CommandId,
    pub lot_id: LotId,
    pub bidder_id: UserId,
    pub authorization_id: ProxyAuthorizationId,
    pub new_maximum: BidAmount,
    pub request_hash: RequestHash,
}

#[derive(Debug, Clone, Copy)]
enum CompetitionCommand {
    PlaceBid(PlaceBidCommand),
    CreateProxy(CreateProxyCommand),
    LowerProxy(LowerProxyCommand),
}

impl CompetitionCommand {
    fn command_id(self) -> CommandId {
        match self {
            Self::PlaceBid(command) => command.command_id,
            Self::CreateProxy(command) => command.command_id,
            Self::LowerProxy(command) => command.command_id,
        }
    }

    fn lot_id(self) -> LotId {
        match self {
            Self::PlaceBid(command) => command.lot_id,
            Self::CreateProxy(command) => command.lot_id,
            Self::LowerProxy(command) => command.lot_id,
        }
    }

    fn request_hash(self) -> RequestHash {
        match self {
            Self::PlaceBid(command) => command.request_hash,
            Self::CreateProxy(command) => command.request_hash,
            Self::LowerProxy(command) => command.request_hash,
        }
    }

    const fn kind(self) -> &'static str {
        match self {
            Self::PlaceBid(_) => "place_bid",
            Self::CreateProxy(_) => "create_proxy",
            Self::LowerProxy(_) => "lower_proxy",
        }
    }

    fn trigger(
        self,
        snapshot: crate::CompetitionSnapshot,
        now: chrono::DateTime<chrono::Utc>,
        next_proxy_usage_number: Option<u64>,
    ) -> Result<CompetitionTrigger, DomainError> {
        match self {
            Self::PlaceBid(command) => Ok(CompetitionTrigger::OrdinaryBid {
                command_id: command.command_id,
                actor_id: command.bidder_id,
                amount: command.amount,
                bid_ids: vec![BidId::from_uuid(deterministic_uuid(
                    command.command_id,
                    "auction-bid-fact",
                    1,
                ))],
                now,
            }),
            Self::CreateProxy(command) => {
                let usage_number = next_proxy_usage_number
                    .ok_or(DomainError::InvalidBidState("创建代理缺少使用次数"))?;
                let authorization_sequence = snapshot
                    .state
                    .last_proxy_sequence
                    .checked_add(1)
                    .ok_or(DomainError::StateVersionOverflow)?;
                Ok(CompetitionTrigger::CreateProxy {
                    command_id: command.command_id,
                    actor_id: command.bidder_id,
                    authorization_id: ProxyAuthorizationId::from_uuid(deterministic_uuid(
                        command.command_id,
                        "proxy-authorization",
                        1,
                    )),
                    maximum_amount: command.maximum_amount,
                    mode: command.mode,
                    authorization_sequence,
                    usage_number,
                    bid_ids: vec![BidId::from_uuid(deterministic_uuid(
                        command.command_id,
                        "auction-bid-fact",
                        1,
                    ))],
                    now,
                })
            }
            Self::LowerProxy(command) => Ok(CompetitionTrigger::LowerProxy {
                command_id: command.command_id,
                actor_id: command.bidder_id,
                authorization_id: command.authorization_id,
                new_maximum: command.new_maximum,
                now,
            }),
        }
    }
}

/// 竞价用例服务，只编排端口和纯领域规划器。
pub struct CompetitionService<R, C> {
    repository: Arc<R>,
    clock: Arc<C>,
}

impl<R, C> CompetitionService<R, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction,
    C: Clock,
{
    /// 建立不持有具体数据库或网络实现的竞价用例。
    pub fn new(repository: Arc<R>, clock: Arc<C>) -> Self {
        Self { repository, clock }
    }

    /// 锁定竞价快照、计算计划并交给仓储在同一事务中原子应用。
    pub async fn place_bid(
        &self,
        command: PlaceBidCommand,
    ) -> Result<CompetitionPlan, ApplicationError> {
        self.execute(CompetitionCommand::PlaceBid(command)).await
    }

    /// 创建代理授权并在同一事务中应用它产生的初始竞价计划。
    pub async fn create_proxy(
        &self,
        command: CreateProxyCommand,
    ) -> Result<CompetitionPlan, ApplicationError> {
        self.execute(CompetitionCommand::CreateProxy(command)).await
    }

    /// 降低活动代理授权上限并追加转换事实。
    pub async fn lower_proxy(
        &self,
        command: LowerProxyCommand,
    ) -> Result<CompetitionPlan, ApplicationError> {
        self.execute(CompetitionCommand::LowerProxy(command)).await
    }

    async fn execute(
        &self,
        command: CompetitionCommand,
    ) -> Result<CompetitionPlan, ApplicationError> {
        let now = self.clock.now();
        let command_id = command.command_id();
        let lot_id = command.lot_id();
        let request_hash = command.request_hash();
        let command_kind = command.kind();
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command_id).await? {
                        if !stored.matches_request(command_kind, lot_id.as_uuid(), request_hash) {
                            return Err(crate::CommandLedgerError::Conflict.into());
                        }
                        if stored.result_code != "accepted" {
                            return Err(crate::CompetitionTransactionError::Failed(
                                "命令账本结果分类不支持重放".to_owned(),
                            )
                            .into());
                        }
                        let replayed: CompetitionPlan =
                            serde_json::from_slice(&stored.result_payload)?;
                        replayed.validate()?;
                        return Ok(replayed);
                    }

                    let snapshot = transaction.load_for_update(lot_id).await?;
                    let next_proxy_usage_number = match command {
                        CompetitionCommand::CreateProxy(command) => Some(
                            transaction.next_proxy_usage_number(lot_id, command.bidder_id).await?,
                        ),
                        CompetitionCommand::PlaceBid(_) | CompetitionCommand::LowerProxy(_) => None,
                    };
                    let trigger =
                        command.trigger(snapshot.clone(), now, next_proxy_usage_number)?;
                    let plan = CompetitionPlan::plan(
                        snapshot.rules,
                        snapshot.state,
                        snapshot.active_proxy,
                        trigger,
                    )?;
                    transaction
                        .insert_command(StoredCommand {
                            command_id,
                            command_kind: command_kind.to_owned(),
                            aggregate_id: lot_id.as_uuid(),
                            request_hash,
                            result_code: "accepted".to_owned(),
                            result_payload: serde_json::to_vec(&plan)?,
                        })
                        .await?;
                    transaction.apply_plan(plan.clone()).await?;
                    Ok(plan)
                })
            })
            .await
    }
}

impl<R, C> CompetitionCommandPort for CompetitionService<R, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction,
    C: Clock,
{
    fn place_bid(&self, command: PlaceBidCommand) -> crate::ApplicationFuture<'_, CompetitionPlan> {
        Box::pin(async move { CompetitionService::place_bid(self, command).await })
    }

    fn create_proxy(
        &self,
        command: CreateProxyCommand,
    ) -> crate::ApplicationFuture<'_, CompetitionPlan> {
        Box::pin(async move { CompetitionService::create_proxy(self, command).await })
    }

    fn lower_proxy(
        &self,
        command: LowerProxyCommand,
    ) -> crate::ApplicationFuture<'_, CompetitionPlan> {
        Box::pin(async move { CompetitionService::lower_proxy(self, command).await })
    }
}
