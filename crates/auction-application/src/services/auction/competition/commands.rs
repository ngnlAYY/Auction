//! 竞价命令载荷与纯领域触发器构造：稳定命令种类、幂等摘要和事实 ID 派生。

use auction_domain::{
    BidAmount, BidId, CommandId, CompetitionTrigger, DomainError, LotId, ProxyAuthorizationId,
    ProxyMode, UserId,
};
use serde::Serialize;

use crate::ports::core::{
    CompetitionCommandAuditContext, CompetitionCommandAuditKind, ProxyUsageContext, RequestHash,
};

use super::legacy_ledger;

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
            mode: mode.as_str(),
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
pub(super) enum CompetitionCommand {
    PlaceBid(PlaceBidCommand),
    CreateProxy(CreateProxyCommand),
    LowerProxy(LowerProxyCommand),
}

impl CompetitionCommand {
    pub(super) fn command_id(&self) -> CommandId {
        match self {
            Self::PlaceBid(command) => command.command_id,
            Self::CreateProxy(command) => command.command_id,
            Self::LowerProxy(command) => command.command_id,
        }
    }

    pub(super) fn lot_id(&self) -> LotId {
        match self {
            Self::PlaceBid(command) => command.lot_id,
            Self::CreateProxy(command) => command.lot_id,
            Self::LowerProxy(command) => command.lot_id,
        }
    }

    pub(super) fn request_hash(&self) -> RequestHash {
        match self {
            Self::PlaceBid(command) => command.request_hash,
            Self::CreateProxy(command) => command.request_hash,
            Self::LowerProxy(command) => command.request_hash,
        }
    }

    pub(super) fn bidder_id(&self) -> UserId {
        match self {
            Self::PlaceBid(command) => command.bidder_id,
            Self::CreateProxy(command) => command.bidder_id,
            Self::LowerProxy(command) => command.bidder_id,
        }
    }

    pub(super) const fn kind(&self) -> &'static str {
        match self {
            Self::PlaceBid(_) => PLACE_BID_COMMAND_KIND,
            Self::CreateProxy(_) => CREATE_PROXY_COMMAND_KIND,
            Self::LowerProxy(_) => LOWER_PROXY_COMMAND_KIND,
        }
    }

    pub(super) fn audit_context(
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

    pub(super) fn trigger(
        &self,
        snapshot: &crate::ports::core::CompetitionSnapshot,
        now: chrono::DateTime<chrono::Utc>,
        proxy_usage_context: Option<ProxyUsageContext>,
    ) -> Result<CompetitionTrigger, DomainError> {
        match self {
            Self::PlaceBid(command) => {
                let fact_count = if let Some(active) = snapshot.active_proxy {
                    let maximum = active.maximum_amount.bid_ceiling();
                    let can_respond = if snapshot.rules.bidding_version == 1 {
                        command.amount <= maximum
                    } else {
                        command.amount < maximum
                    };
                    let immediate = active.mode == ProxyMode::Fast
                        || (snapshot.rules.bidding_version == 1 && command.amount == maximum);
                    1 + u32::from(can_respond && immediate)
                } else {
                    1
                };
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

pub(super) fn competition_bid_ids(command_id: CommandId, count: u32) -> Vec<BidId> {
    (1..=count)
        .map(|index| {
            BidId::from_uuid(legacy_ledger::deterministic_id(command_id, "auction-bid-fact", index))
        })
        .collect()
}
