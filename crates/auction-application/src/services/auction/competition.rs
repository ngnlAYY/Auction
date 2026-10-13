use std::sync::Arc;

use auction_domain::{
    AuctionStatus, BidAmount, BidId, CommandId, CompetitionPlan, CompetitionTrigger, DomainError,
    LotId, ProxyAuthorizationId, ProxyMode, ProxyStatus, UserId,
};
use serde::Serialize;

use crate::{
    ApplicationError, Clock, CompetitionCommandAuditContext, CompetitionCommandAuditKind,
    CompetitionCommandOutcome, CompetitionCommandPort, CompetitionTransaction,
    CompetitionWireTimestamp, ExecuteSlowProxyCommand, ProxyUsageContext, RequestHash,
    SLOW_PROXY_COMMAND_KIND, SlowProxyCommandPort, SlowProxyExecutionContext, StoredCommand,
    TransactionPolicy, TransactionRunner,
};

#[path = "competition/legacy_ledger.rs"]
mod legacy_ledger;
#[path = "competition/slow_replay.rs"]
mod slow_replay;

/// 旧版竞价命令账本使用的稳定命令种类。
pub const PLACE_BID_COMMAND_KIND: &str = "place_bid";
/// 旧版创建代理授权命令账本使用的稳定命令种类。
pub const CREATE_PROXY_COMMAND_KIND: &str = "create_proxy_authorization";
/// 旧版降低代理授权命令账本使用的稳定命令种类。
pub const LOWER_PROXY_COMMAND_KIND: &str = "lower_proxy_authorization";

/// 普通出价命令。事实 ID 由命令 ID 在应用层稳定派生，调用方不能注入第二个随机 ID。
#[derive(Debug, Clone)]
pub struct PlaceBidCommand {
    pub command_id: CommandId,
    pub lot_id: LotId,
    pub bidder_id: UserId,
    pub amount: BidAmount,
    pub request_hash: RequestHash,
    /// HTTP 发起时使用请求边界生成/校验的关联 ID；Worker 内部命令为 `None`。
    pub request_id: Option<String>,
}

impl PlaceBidCommand {
    /// 按旧版 Go `{LotID,BidderID,Amount}` 字段顺序计算幂等摘要。
    pub fn legacy_request_hash(
        lot_id: LotId,
        bidder_id: UserId,
        amount: BidAmount,
    ) -> Result<RequestHash, serde_json::Error> {
        #[derive(Serialize)]
        struct LegacyPlaceBidRequest {
            #[serde(rename = "LotID")]
            lot_id: [u8; 16],
            #[serde(rename = "BidderID")]
            bidder_id: [u8; 16],
            #[serde(rename = "Amount")]
            amount: i64,
        }

        legacy_competition_request_hash(&LegacyPlaceBidRequest {
            lot_id: *lot_id.as_uuid().as_bytes(),
            bidder_id: *bidder_id.as_uuid().as_bytes(),
            amount: amount.value(),
        })
    }
}

/// 创建代理授权命令；授权标识由命令 ID 在应用层稳定派生。
#[derive(Debug, Clone)]
pub struct CreateProxyCommand {
    pub command_id: CommandId,
    pub lot_id: LotId,
    pub bidder_id: UserId,
    pub maximum_amount: BidAmount,
    /// 代理模式；未提供时使用拍卖冻结的默认模式。
    pub mode: Option<ProxyMode>,
    pub request_hash: RequestHash,
    /// HTTP 发起时使用请求边界生成/校验的关联 ID；Worker 内部命令为 `None`。
    pub request_id: Option<String>,
}

impl CreateProxyCommand {
    /// 按旧版 Go `{LotID,UserID,Mode,MaximumAmount}` 字段顺序计算幂等摘要。
    pub fn legacy_request_hash(
        lot_id: LotId,
        bidder_id: UserId,
        mode: ProxyMode,
        maximum_amount: BidAmount,
    ) -> Result<RequestHash, serde_json::Error> {
        #[derive(Serialize)]
        struct LegacyCreateProxyRequest {
            #[serde(rename = "LotID")]
            lot_id: [u8; 16],
            #[serde(rename = "UserID")]
            user_id: [u8; 16],
            #[serde(rename = "Mode")]
            mode: &'static str,
            #[serde(rename = "MaximumAmount")]
            maximum_amount: i64,
        }

        legacy_competition_request_hash(&LegacyCreateProxyRequest {
            lot_id: *lot_id.as_uuid().as_bytes(),
            user_id: *bidder_id.as_uuid().as_bytes(),
            mode: proxy_mode_name(mode),
            maximum_amount: maximum_amount.value(),
        })
    }
}

/// 降低代理授权上限命令。
#[derive(Debug, Clone)]
pub struct LowerProxyCommand {
    pub command_id: CommandId,
    pub lot_id: LotId,
    pub bidder_id: UserId,
    pub authorization_id: ProxyAuthorizationId,
    pub new_maximum: BidAmount,
    pub request_hash: RequestHash,
    /// HTTP 发起时使用请求边界生成/校验的关联 ID；Worker 内部命令为 `None`。
    pub request_id: Option<String>,
}

impl LowerProxyCommand {
    /// 按旧版 Go `{LotID,UserID,AuthorizationID,NewMaximum}` 字段顺序计算幂等摘要。
    pub fn legacy_request_hash(
        lot_id: LotId,
        bidder_id: UserId,
        authorization_id: ProxyAuthorizationId,
        new_maximum: BidAmount,
    ) -> Result<RequestHash, serde_json::Error> {
        #[derive(Serialize)]
        struct LegacyLowerProxyRequest {
            #[serde(rename = "LotID")]
            lot_id: [u8; 16],
            #[serde(rename = "UserID")]
            user_id: [u8; 16],
            #[serde(rename = "AuthorizationID")]
            authorization_id: [u8; 16],
            #[serde(rename = "NewMaximum")]
            new_maximum: i64,
        }

        legacy_competition_request_hash(&LegacyLowerProxyRequest {
            lot_id: *lot_id.as_uuid().as_bytes(),
            user_id: *bidder_id.as_uuid().as_bytes(),
            authorization_id: *authorization_id.as_uuid().as_bytes(),
            new_maximum: new_maximum.value(),
        })
    }
}

#[derive(Debug, Clone)]
enum CompetitionCommand {
    PlaceBid(PlaceBidCommand),
    CreateProxy(CreateProxyCommand),
    LowerProxy(LowerProxyCommand),
}

impl CompetitionCommand {
    fn command_id(&self) -> CommandId {
        match self {
            Self::PlaceBid(command) => command.command_id,
            Self::CreateProxy(command) => command.command_id,
            Self::LowerProxy(command) => command.command_id,
        }
    }

    fn lot_id(&self) -> LotId {
        match self {
            Self::PlaceBid(command) => command.lot_id,
            Self::CreateProxy(command) => command.lot_id,
            Self::LowerProxy(command) => command.lot_id,
        }
    }

    fn request_hash(&self) -> RequestHash {
        match self {
            Self::PlaceBid(command) => command.request_hash,
            Self::CreateProxy(command) => command.request_hash,
            Self::LowerProxy(command) => command.request_hash,
        }
    }

    fn bidder_id(&self) -> UserId {
        match self {
            Self::PlaceBid(command) => command.bidder_id,
            Self::CreateProxy(command) => command.bidder_id,
            Self::LowerProxy(command) => command.bidder_id,
        }
    }

    const fn kind(&self) -> &'static str {
        match self {
            Self::PlaceBid(_) => PLACE_BID_COMMAND_KIND,
            Self::CreateProxy(_) => CREATE_PROXY_COMMAND_KIND,
            Self::LowerProxy(_) => LOWER_PROXY_COMMAND_KIND,
        }
    }

    fn audit_context(
        &self,
        occurred_at: chrono::DateTime<chrono::Utc>,
    ) -> CompetitionCommandAuditContext {
        let kind = match self {
            Self::PlaceBid(_) => CompetitionCommandAuditKind::PlaceBid,
            Self::CreateProxy(_) => CompetitionCommandAuditKind::CreateProxy,
            Self::LowerProxy(command) => CompetitionCommandAuditKind::LowerProxy {
                authorization_id: command.authorization_id,
                new_maximum: command.new_maximum,
            },
        };
        CompetitionCommandAuditContext {
            command_id: self.command_id(),
            actor_id: self.bidder_id(),
            occurred_at,
            kind,
        }
    }

    fn trigger(
        self,
        snapshot: crate::CompetitionSnapshot,
        now: chrono::DateTime<chrono::Utc>,
        proxy_usage_context: Option<ProxyUsageContext>,
    ) -> Result<CompetitionTrigger, DomainError> {
        match self {
            Self::PlaceBid(command) => {
                let fact_count = snapshot.active_proxy.map_or(1, |active| {
                    if command.amount <= active.maximum_amount
                        && (command.amount == active.maximum_amount
                            || active.mode == ProxyMode::Fast)
                    {
                        2
                    } else {
                        1
                    }
                });
                Ok(CompetitionTrigger::OrdinaryBid {
                    command_id: command.command_id,
                    actor_id: command.bidder_id,
                    amount: command.amount,
                    bid_ids: competition_bid_ids(command.command_id, fact_count),
                    now,
                })
            }
            Self::CreateProxy(command) => {
                let usage_context = proxy_usage_context
                    .ok_or(DomainError::InvalidBidState("创建代理缺少使用次数"))?;
                let authorization_sequence = snapshot
                    .state
                    .last_proxy_sequence
                    .checked_add(1)
                    .ok_or(DomainError::StateVersionOverflow)?;
                Ok(CompetitionTrigger::CreateProxy {
                    command_id: command.command_id,
                    actor_id: command.bidder_id,
                    authorization_id: ProxyAuthorizationId::from_uuid(
                        legacy_ledger::deterministic_id(
                            command.command_id,
                            "auction-proxy-authorization",
                            1,
                        ),
                    ),
                    maximum_amount: command.maximum_amount,
                    mode: command.mode.unwrap_or(snapshot.rules.proxy_default_mode),
                    auction_status: snapshot.auction_status,
                    authorization_sequence,
                    usage_number: usage_context.usage_number,
                    previous_maximum: usage_context.previous_maximum,
                    previous_status: usage_context.previous_status,
                    bid_ids: vec![BidId::from_uuid(legacy_ledger::deterministic_id(
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

fn legacy_competition_request_hash<T>(value: &T) -> Result<RequestHash, serde_json::Error>
where
    T: Serialize,
{
    RequestHash::from_legacy_go_json(value)
}

const fn proxy_mode_name(mode: ProxyMode) -> &'static str {
    match mode {
        ProxyMode::Fast => "fast",
        ProxyMode::Slow => "slow",
    }
}

/// 竞价用例服务，只编排端口和纯领域规划器。
pub struct CompetitionService<R, C> {
    repository: Arc<R>,
    // 保留既有装配签名；命令时刻由锁后的事务端口提供。
    _clock: Arc<C>,
}

impl<R, C> CompetitionService<R, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction,
    C: Clock,
{
    /// 建立不持有具体数据库或网络实现的竞价用例。
    pub fn new(repository: Arc<R>, clock: Arc<C>) -> Self {
        Self { repository, _clock: clock }
    }

    /// 锁定竞价快照、计算计划并交给仓储在同一事务中原子应用。
    pub async fn place_bid(
        &self,
        command: PlaceBidCommand,
    ) -> Result<CompetitionCommandOutcome, ApplicationError> {
        self.execute(CompetitionCommand::PlaceBid(command)).await
    }

    /// 创建代理授权并在同一事务中应用它产生的初始竞价计划。
    pub async fn create_proxy(
        &self,
        command: CreateProxyCommand,
    ) -> Result<CompetitionCommandOutcome, ApplicationError> {
        self.execute(CompetitionCommand::CreateProxy(command)).await
    }

    /// 降低活动代理授权上限并追加转换事实。
    pub async fn lower_proxy(
        &self,
        command: LowerProxyCommand,
    ) -> Result<CompetitionCommandOutcome, ApplicationError> {
        self.execute(CompetitionCommand::LowerProxy(command)).await
    }

    /// 使用数据库时间推进一个持久化慢速代理 generation；过期候选返回稳定 stale 分类。
    pub async fn execute_slow_proxy(
        &self,
        command: ExecuteSlowProxyCommand,
    ) -> Result<CompetitionPlan, ApplicationError> {
        command.validate()?;
        let command_id = command.command_id;
        let lot_id = command.lot_id;
        let request_hash = command.request_hash;
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command_id).await? {
                        if !stored.matches_request(
                            SLOW_PROXY_COMMAND_KIND,
                            lot_id.as_uuid(),
                            request_hash,
                        ) {
                            return Err(crate::CommandLedgerError::Conflict.into());
                        }
                        if stored.result_code != "accepted" {
                            return Err(crate::CompetitionTransactionError::Failed(
                                "慢速代理命令账本结果分类不支持重放".to_owned(),
                            )
                            .into());
                        }
                        let replayed: CompetitionPlan =
                            serde_json::from_slice(&stored.result_payload)?;
                        slow_replay::validate(&command, &replayed)?;
                        return Ok(replayed);
                    }

                    let snapshot = match transaction.load_for_update(lot_id).await {
                        Ok(snapshot) => snapshot,
                        Err(crate::CompetitionTransactionError::NotFound) => {
                            return Err(crate::CompetitionTransactionError::StaleSlowProxy.into());
                        }
                        Err(error) => return Err(error.into()),
                    };
                    let Some(active) = snapshot.active_proxy else {
                        return Err(crate::CompetitionTransactionError::StaleSlowProxy.into());
                    };
                    let eligible =
                        transaction.participant_is_eligible_for_update(active.user_id).await?;
                    let now = transaction.current_time().await?;
                    if snapshot.auction_status != AuctionStatus::Live
                        || active.id != command.authorization_id
                        || active.mode != ProxyMode::Slow
                        || active.status != ProxyStatus::Active
                        || snapshot.state.slow_authorization_id != Some(command.authorization_id)
                        || snapshot.state.slow_generation != command.generation
                        || snapshot.state.slow_due_at != Some(command.due_at)
                        || now < command.due_at
                        || now >= snapshot.state.effective_end_at
                    {
                        return Err(crate::CompetitionTransactionError::StaleSlowProxy.into());
                    }

                    let trigger = if eligible {
                        CompetitionTrigger::SlowDue {
                            command_id,
                            authorization_id: command.authorization_id,
                            generation: command.generation,
                            due_at: command.due_at,
                            bid_ids: competition_bid_ids(command_id, 1),
                            now,
                        }
                    } else {
                        CompetitionTrigger::FreezeProxy {
                            command_id,
                            authorization_id: command.authorization_id,
                            now,
                        }
                    };
                    let plan = CompetitionPlan::plan(
                        snapshot.rules,
                        snapshot.state,
                        Some(active),
                        trigger,
                    )
                    .map_err(map_slow_proxy_planning_error)?;
                    transaction
                        .insert_command(StoredCommand {
                            command_id,
                            command_kind: SLOW_PROXY_COMMAND_KIND.to_owned(),
                            aggregate_id: lot_id.as_uuid(),
                            request_hash,
                            result_code: "accepted".to_owned(),
                            result_payload: serde_json::to_vec(&plan)?,
                        })
                        .await?;
                    transaction
                        .apply_slow_proxy_plan(
                            plan.clone(),
                            SlowProxyExecutionContext {
                                authorization_id: command.authorization_id,
                                generation: command.generation,
                                due_at: command.due_at,
                                executed_at: now,
                            },
                        )
                        .await?;
                    Ok(plan)
                })
            })
            .await
    }

    async fn execute(
        &self,
        command: CompetitionCommand,
    ) -> Result<CompetitionCommandOutcome, ApplicationError> {
        let command_id = command.command_id();
        let lot_id = command.lot_id();
        let request_hash = command.request_hash();
        let command_kind = command.kind();
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command_id).await? {
                        if !stored.matches_request(command_kind, lot_id.as_uuid(), request_hash) {
                            return Err(crate::CommandLedgerError::Conflict.into());
                        }
                        return match stored.result_code.as_str() {
                            "accepted" => {
                                let replayed: CompetitionPlan =
                                    serde_json::from_slice(&stored.result_payload)?;
                                match &command {
                                    CompetitionCommand::PlaceBid(_) => {
                                        legacy_ledger::replay_accepted(&command, replayed, &[], 0)
                                    }
                                    CompetitionCommand::CreateProxy(create) => {
                                        let authorization_id = ProxyAuthorizationId::from_uuid(
                                            legacy_ledger::deterministic_id(
                                                create.command_id,
                                                "auction-proxy-authorization",
                                                1,
                                            ),
                                        );
                                        let transitions = transaction
                                            .proxy_transition_history(authorization_id)
                                            .await?;
                                        let quota_limit =
                                            transaction.proxy_quota_limit(lot_id).await?;
                                        legacy_ledger::replay_accepted(
                                            &command,
                                            replayed,
                                            &transitions,
                                            quota_limit,
                                        )
                                    }
                                    CompetitionCommand::LowerProxy(lower) => {
                                        let transitions = transaction
                                            .proxy_transition_history(lower.authorization_id)
                                            .await?;
                                        let quota_limit =
                                            transaction.proxy_quota_limit(lot_id).await?;
                                        legacy_ledger::replay_accepted(
                                            &command,
                                            replayed,
                                            &transitions,
                                            quota_limit,
                                        )
                                    }
                                }
                            }
                            legacy_ledger::LEGACY_RESULT_CODE => {
                                legacy_ledger::replay(&command, &stored.result_payload)
                            }
                            // 旧版把同一请求命中的非成功结果分类为账本冲突；上方仍先核对
                            // command kind、aggregate 与摘要，保留新版更强的防篡改边界。
                            _ => Err(crate::CommandLedgerError::Conflict.into()),
                        };
                    }

                    let mut snapshot = transaction.load_for_update(lot_id).await?;
                    let quota_limit = snapshot.rules.proxy_max_outbid_uses;
                    if let CompetitionCommand::CreateProxy(create) = &command {
                        CompetitionPlan::validate_proxy_creation_policy(
                            snapshot.rules,
                            snapshot.auction_status,
                            create.mode.unwrap_or(snapshot.rules.proxy_default_mode),
                        )?;
                    }
                    if let CompetitionCommand::LowerProxy(lower) = &command {
                        CompetitionPlan::validate_lower_proxy_target(
                            snapshot.active_proxy,
                            lower.bidder_id,
                            lower.authorization_id,
                        )?;
                    }
                    let proxy_usage_context = match &command {
                        CompetitionCommand::CreateProxy(create) => {
                            let context =
                                transaction.proxy_usage_context(lot_id, create.bidder_id).await?;
                            CompetitionPlan::validate_proxy_creation_history(
                                snapshot.rules,
                                context.usage_number,
                                context.previous_maximum,
                                context.previous_status,
                                create.maximum_amount,
                            )?;
                            Some(context)
                        }
                        CompetitionCommand::PlaceBid(_) | CompetitionCommand::LowerProxy(_) => None,
                    };
                    let previous_proxy_transitions = match &command {
                        CompetitionCommand::LowerProxy(lower) => {
                            transaction.proxy_transition_history(lower.authorization_id).await?
                        }
                        CompetitionCommand::PlaceBid(_) | CompetitionCommand::CreateProxy(_) => {
                            Vec::new()
                        }
                    };
                    let bidder_id = command.bidder_id();
                    let mut participants = vec![bidder_id];
                    if let Some(active) = snapshot.active_proxy {
                        participants.push(active.user_id);
                    }
                    participants.sort_unstable();
                    participants.dedup();

                    let mut bidder_eligible = false;
                    let mut active_proxy_eligible = true;
                    for participant_id in participants {
                        let eligible =
                            transaction.participant_is_eligible_for_update(participant_id).await?;
                        if participant_id == bidder_id {
                            bidder_eligible = eligible;
                        }
                        if snapshot
                            .active_proxy
                            .is_some_and(|active| active.user_id == participant_id)
                        {
                            active_proxy_eligible = eligible;
                        }
                    }
                    if !bidder_eligible {
                        return Err(crate::CompetitionTransactionError::BidderIneligible.into());
                    }

                    // 用户资格锁同样可能等待；每次序列化重试都在全部锁齐备后重新计时。
                    let now = transaction.current_time().await?;
                    if now >= snapshot.state.effective_end_at {
                        return Err(crate::CompetitionTransactionError::InvalidState.into());
                    }
                    let audit = command.audit_context(now);
                    let mut prerequisite_plans = Vec::new();
                    if let Some(active) = snapshot.active_proxy
                        && active.user_id != bidder_id
                        && !active_proxy_eligible
                    {
                        let freeze = CompetitionPlan::plan(
                            snapshot.rules,
                            snapshot.state,
                            Some(active),
                            CompetitionTrigger::FreezeProxy {
                                command_id,
                                authorization_id: active.id,
                                now,
                            },
                        )?;
                        snapshot.state = freeze.final_lot;
                        snapshot.active_proxy = None;
                        prerequisite_plans.push(freeze);
                    }
                    let trigger =
                        command.clone().trigger(snapshot.clone(), now, proxy_usage_context)?;
                    let plan = CompetitionPlan::plan(
                        snapshot.rules,
                        snapshot.state,
                        snapshot.active_proxy,
                        trigger,
                    )?;
                    let legacy_ledger::EncodedCommandResult { payload, proxy } =
                        legacy_ledger::encode(
                            &command,
                            &plan,
                            &previous_proxy_transitions,
                            quota_limit,
                            now,
                        )?;
                    transaction
                        .insert_command(StoredCommand {
                            command_id,
                            command_kind: command_kind.to_owned(),
                            aggregate_id: lot_id.as_uuid(),
                            request_hash,
                            result_code: legacy_ledger::LEGACY_RESULT_CODE.to_owned(),
                            result_payload: payload,
                        })
                        .await?;
                    for prerequisite in prerequisite_plans {
                        transaction.apply_plan(prerequisite, None, now).await?;
                    }
                    transaction.apply_plan(plan.clone(), Some(audit), now).await?;
                    let effective_end_at =
                        CompetitionWireTimestamp::from_utc(plan.final_lot.effective_end_at);
                    Ok(CompetitionCommandOutcome { plan, proxy, effective_end_at })
                })
            })
            .await
    }
}

fn competition_bid_ids(command_id: CommandId, count: u32) -> Vec<BidId> {
    (1..=count)
        .map(|index| {
            BidId::from_uuid(legacy_ledger::deterministic_id(command_id, "auction-bid-fact", index))
        })
        .collect()
}

fn map_slow_proxy_planning_error(error: DomainError) -> ApplicationError {
    match error {
        DomainError::AuctionClosed
        | DomainError::InvalidBidState(_)
        | DomainError::InvalidProxyTransition { .. } => {
            crate::CompetitionTransactionError::StaleSlowProxy.into()
        }
        error => error.into(),
    }
}

impl<R, C> CompetitionCommandPort for CompetitionService<R, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction,
    C: Clock,
{
    fn place_bid(
        &self,
        command: PlaceBidCommand,
    ) -> crate::ApplicationFuture<'_, CompetitionCommandOutcome> {
        Box::pin(async move { CompetitionService::place_bid(self, command).await })
    }

    fn create_proxy(
        &self,
        command: CreateProxyCommand,
    ) -> crate::ApplicationFuture<'_, CompetitionCommandOutcome> {
        Box::pin(async move { CompetitionService::create_proxy(self, command).await })
    }

    fn lower_proxy(
        &self,
        command: LowerProxyCommand,
    ) -> crate::ApplicationFuture<'_, CompetitionCommandOutcome> {
        Box::pin(async move { CompetitionService::lower_proxy(self, command).await })
    }
}

impl<R, C> SlowProxyCommandPort for CompetitionService<R, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction,
    C: Clock,
{
    fn execute_slow_proxy(
        &self,
        command: ExecuteSlowProxyCommand,
    ) -> crate::ApplicationFuture<'_, CompetitionPlan> {
        Box::pin(async move { CompetitionService::execute_slow_proxy(self, command).await })
    }
}
