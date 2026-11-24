//! Public v2 收件能力、本人寄件指引、预约签发与代管件撤回意愿处理器。
//!
//! 处理器只做认证、严格输入解码、端口调用与 DTO 映射；命令仲裁、开关顺序、版本与
//! 业务拒绝全部由应用用例和存储事务决定。路由挂载由集成车道按本模块导出的 `mount` 接入。

use std::net::SocketAddr;
use std::sync::Arc;

use auction_application::consignment::{
    ActiveDepositInstructions, ConsignmentError, DepositCommandService, DepositRead, DepositStatus,
    DepositWorldStatus, DepositWorlds, ListingBusinessRejection, OwnedDepositInstructions,
    WithdrawDepositReceipt,
};
use auction_contracts::{
    ConsignmentStatusV2, ConsignmentWorldsV2, DepositInstructions, DepositReservationReceipt,
    IssueDepositReservation, ListingFeeBps, ListingStateV2, ListingWorld, SeasonStateV2,
    WithdrawListingV2, WithdrawReceiptV2, WorldCapabilityV2,
};
use auction_domain::consignment::ListingMode;
use auction_domain::consignment::ListingState;
use auction_domain::{CommandId, SettlementWorld, UserId};
use axum::Router;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, Uri};
use axum::middleware;
use serde::de::DeserializeOwned;

use crate::middleware::method_strict;
use crate::middleware::{boundary, response_cache};
use crate::public::consignment::ConsignmentState;
use crate::public::consignment::input;
use crate::public::consignment::response::{self, RouteError, RouteResult};
use crate::query::parse_public_uuid;

#[cfg(test)]
mod tests;

/// 进程内已装配的 v2 收件能力。端口缺失时保留无数据库骨架语义。
#[derive(Clone, Default)]
pub struct DepositCapabilities {
    /// 有效开关；新承诺在关闭时返回 404，旧命令仲裁仍由端口决定。
    pub enabled: bool,
    /// v2 状态、本人指引与 Admin 隔离读取。
    pub read: Option<Arc<dyn DepositRead>>,
    /// 预约、撤回、止售与隔离用例。
    pub commands: Option<Arc<DepositCommandService>>,
}

/// 路由只借用既有 Public 会话与 v2 收件能力。
pub(crate) trait DepositState: ConsignmentState {
    /// 当前装配为此进程提供的 v2 收件能力。
    fn deposit(&self) -> &DepositCapabilities;
}

/// 将四条 Public v2 路由放入给定路由器，并沿用严格方法与无缓存边界。
pub(crate) fn mount<S: DepositState>(router: Router<S>) -> Router<S> {
    let routes = Router::new()
        .route("/api/v2/consignment/status", method_strict::get(status::<S>))
        .route(
            "/api/v2/me/consignment/listings/{listing_id}/deposit-instructions",
            method_strict::get(instructions::<S>),
        )
        .route(
            "/api/v2/me/consignment/listings/{listing_id}/deposit-reservations",
            method_strict::post(issue_reservation::<S>),
        )
        .route(
            "/api/v2/consignment/listings/{listing_id}/withdraw",
            method_strict::post(withdraw::<S>),
        )
        .route_layer(middleware::from_fn(boundary::normalize_api_rejection))
        .route_layer(middleware::from_fn(response_cache::no_store));
    router.merge(routes)
}

async fn status<S: DepositState>(State(state): State<S>, uri: Uri) -> RouteResult {
    no_query(&uri)?;
    let capabilities = state.deposit();
    let reader = capabilities.read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let status = reader.status().await.map_err(read_failure)?;
    response::loaded(status_dto(status)?)
}

async fn instructions<S: DepositState>(
    State(state): State<S>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> RouteResult {
    let identity = input::private_identity(state.public_state(), &headers).await?;
    no_query(&uri)?;
    let listing_id = listing_id(path)?;
    // 4W wire 明确：首次禁用时本人指引返回 404；旧预约的到期与撤销仍由读端口投影。
    if !state.deposit().enabled {
        return Err(ConsignmentError::Disabled.into());
    }
    let reader = state.deposit().read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let instructions = reader
        .instructions(UserId::from_uuid(identity.user_id), listing_id)
        .await
        .map_err(read_failure)?
        .ok_or_else(|| ConsignmentError::from(ListingBusinessRejection::ListingNotFound))?;
    response::loaded(instructions_dto(instructions))
}

async fn issue_reservation<S: DepositState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> RouteResult {
    let (parts, body) = request.into_parts();
    let identity =
        input::mutation_identity(state.public_state(), &parts.headers, address, &parts.uri).await?;
    let listing_id = listing_id(path)?;
    let input: IssueDepositReservation = decode_write(&parts.headers, body).await?;
    let capabilities = state.deposit();
    if !capabilities.enabled {
        // 不变量 1、D9 §6.3：关闭只截止新承诺；关闭前已接受的同 ID 同请求仍返回原回执。
        let Some(service) = capabilities.commands.as_ref() else {
            return Err(ConsignmentError::Disabled.into());
        };
        let expected_version = positive_version(input.expected_version.get())?;
        let replay = service
            .replay_issue_reservation(
                UserId::from_uuid(identity.user_id),
                CommandId::from_uuid(input.command_id),
                listing_id,
                expected_version,
                crate::middleware::boundary::request_id(&parts.headers),
            )
            .await?;
        let Some(receipt) = replay else {
            return Err(ConsignmentError::Disabled.into());
        };
        if receipt.listing_id != listing_id {
            return Err(ConsignmentError::OutcomeUnknown.into());
        }
        return response::created("寄件预约已签发", reservation_dto(receipt)?);
    }
    let service = capabilities.commands.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let expected_version = positive_version(input.expected_version.get())?;
    let receipt = service
        .issue_reservation(
            UserId::from_uuid(identity.user_id),
            CommandId::from_uuid(input.command_id),
            listing_id,
            expected_version,
            crate::middleware::boundary::request_id(&parts.headers),
        )
        .await?;
    if receipt.listing_id != listing_id {
        return Err(ConsignmentError::OutcomeUnknown.into());
    }
    response::created("寄件预约已签发", reservation_dto(receipt)?)
}

async fn withdraw<S: DepositState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> RouteResult {
    let (parts, body) = request.into_parts();
    let identity =
        input::mutation_identity(state.public_state(), &parts.headers, address, &parts.uri).await?;
    let listing_id = listing_id(path)?;
    let input: WithdrawListingV2 = decode_write(&parts.headers, body).await?;
    let service = state.deposit().commands.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let expected_version = positive_version(input.expected_version.get())?;
    let receipt = service
        .request_withdrawal(
            UserId::from_uuid(identity.user_id),
            CommandId::from_uuid(input.command_id),
            listing_id,
            expected_version,
            crate::middleware::boundary::request_id(&parts.headers),
        )
        .await?;
    if receipt.listing_id != listing_id {
        return Err(ConsignmentError::OutcomeUnknown.into());
    }
    match withdrawal_dto(receipt)? {
        receipt @ WithdrawReceiptV2::WithdrawalRequested { .. } => {
            response::accepted("撤回意愿已登记", receipt)
        }
        receipt @ WithdrawReceiptV2::Closed { .. } => response::loaded(receipt),
    }
}

fn status_dto(status: DepositStatus) -> Result<ConsignmentStatusV2, ConsignmentError> {
    Ok(ConsignmentStatusV2 {
        enabled: status.enabled,
        registration_ready: status.registration_ready,
        registration_modes: status
            .registration_modes
            .into_iter()
            .map(wire_mode)
            .collect::<Result<_, _>>()?,
        registration_worlds: status.registration_worlds.into_iter().map(wire_world).collect(),
        fee_bps: ListingFeeBps::new(i64::from(status.fee_bps.value()))
            .ok_or(ConsignmentError::Unavailable)?,
        min_price: status.min_price.value(),
        max_price: status.max_price.value(),
        accepts_mail: status.accepts_mail,
        purchasable: status.purchasable,
        worlds: worlds_dto(status.worlds),
    })
}

fn worlds_dto(worlds: DepositWorlds) -> ConsignmentWorldsV2 {
    ConsignmentWorldsV2 { main: world_dto(worlds.main), isekai: world_dto(worlds.isekai) }
}

fn world_dto(world: DepositWorldStatus) -> WorldCapabilityV2 {
    WorldCapabilityV2 {
        mail_ready: world.mail_ready,
        purchase_ready: world.purchase_ready,
        season_state: wire_season(world.season_state),
        cutoff_at: world.cutoff_at,
        season_end_at: world.season_end_at,
    }
}

fn instructions_dto(instructions: OwnedDepositInstructions) -> DepositInstructions {
    match instructions {
        OwnedDepositInstructions::None => DepositInstructions::None {},
        OwnedDepositInstructions::Active(active) => active_dto(active),
    }
}

fn active_dto(active: ActiveDepositInstructions) -> DepositInstructions {
    DepositInstructions::Active {
        world: wire_world(active.world),
        bot: active.bot,
        subject: active.subject,
        // 不变量 7：令牌正文只在这里解封，且只进入本人指引响应体。
        body: active.body.as_str().to_owned(),
        reservation_id: active.reservation_id,
        expires_at: active.expires_at,
    }
}

fn reservation_dto(
    receipt: auction_application::consignment::ReservationReceipt,
) -> Result<DepositReservationReceipt, ConsignmentError> {
    if receipt.state != ListingState::AwaitingDeposit || receipt.schema_version != 2 {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    Ok(DepositReservationReceipt {
        schema_version: 2,
        operation: auction_contracts::ConsignmentOperationV2::IssueReservation,
        disposition: auction_contracts::ConsignmentDispositionV2::ReservationIssued,
        command_id: receipt.command_id.as_uuid(),
        listing_id: receipt.listing_id,
        listing_version: wire_version(receipt.listing_version.value())?,
        state: ListingStateV2::AwaitingDeposit,
        reservation_id: receipt.reservation_id,
        expires_at: receipt.expires_at,
    })
}

fn withdrawal_dto(receipt: WithdrawDepositReceipt) -> Result<WithdrawReceiptV2, ConsignmentError> {
    if receipt.schema_version != 2
        || receipt.operation != auction_application::consignment::DepositOperation::Withdraw
        || receipt.contract_id.is_some()
        || receipt.equipment_id.is_some()
    {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    match receipt.disposition {
        auction_application::consignment::ReceiptDisposition::WithdrawalRequested => {
            let Some(intent_id) = receipt.intent_id else {
                return Err(ConsignmentError::OutcomeUnknown);
            };
            if !matches!(
                receipt.state,
                ListingState::DepositAccepted
                    | ListingState::WithdrawalPending
                    | ListingState::HeldOffMarket
            ) {
                return Err(ConsignmentError::OutcomeUnknown);
            }
            Ok(WithdrawReceiptV2::WithdrawalRequested {
                schema_version: 2,
                operation: auction_contracts::ConsignmentOperationV2::Withdraw,
                command_id: receipt.command_id.as_uuid(),
                listing_id: receipt.listing_id,
                listing_version: wire_version(receipt.listing_version.value())?,
                state: wire_listing_state(receipt.state)?,
                intent_id,
            })
        }
        auction_application::consignment::ReceiptDisposition::Closed => {
            if receipt.intent_id.is_some() || receipt.state != ListingState::Closed {
                return Err(ConsignmentError::OutcomeUnknown);
            }
            Ok(WithdrawReceiptV2::Closed {
                schema_version: 2,
                operation: auction_contracts::ConsignmentOperationV2::Withdraw,
                command_id: receipt.command_id.as_uuid(),
                listing_id: receipt.listing_id,
                listing_version: wire_version(receipt.listing_version.value())?,
                state: ListingStateV2::Closed,
            })
        }
        _ => Err(ConsignmentError::OutcomeUnknown),
    }
}

fn wire_mode(mode: ListingMode) -> Result<auction_contracts::ListingMode, ConsignmentError> {
    Ok(match mode {
        ListingMode::Direct => auction_contracts::ListingMode::Direct,
        ListingMode::Consigned => auction_contracts::ListingMode::Consigned,
    })
}

fn wire_world(world: SettlementWorld) -> ListingWorld {
    match world {
        SettlementWorld::Main => ListingWorld::Main,
        SettlementWorld::Isekai => ListingWorld::Isekai,
    }
}

fn wire_season(state: auction_domain::consignment::SeasonState) -> SeasonStateV2 {
    match state {
        auction_domain::consignment::SeasonState::NotSeasonal => SeasonStateV2::NotSeasonal,
        auction_domain::consignment::SeasonState::Open => SeasonStateV2::Open,
        auction_domain::consignment::SeasonState::Preclose => SeasonStateV2::Preclose,
        auction_domain::consignment::SeasonState::Closed => SeasonStateV2::Closed,
        auction_domain::consignment::SeasonState::Unknown => SeasonStateV2::Unknown,
    }
}

fn wire_listing_state(state: ListingState) -> Result<ListingStateV2, ConsignmentError> {
    Ok(match state {
        ListingState::Resolving => ListingStateV2::Resolving,
        ListingState::Displayed => ListingStateV2::Displayed,
        ListingState::AwaitingDeposit => ListingStateV2::AwaitingDeposit,
        ListingState::DepositAccepted => ListingStateV2::DepositAccepted,
        ListingState::Available => ListingStateV2::Available,
        ListingState::SalePending => ListingStateV2::SalePending,
        ListingState::HeldOffMarket => ListingStateV2::HeldOffMarket,
        ListingState::WithdrawalPending => ListingStateV2::WithdrawalPending,
        ListingState::Rejected => ListingStateV2::Rejected,
        ListingState::Expired => ListingStateV2::Expired,
        ListingState::Closed => ListingStateV2::Closed,
    })
}

fn wire_version(value: u64) -> Result<auction_contracts::ListingVersion, ConsignmentError> {
    i64::try_from(value)
        .ok()
        .and_then(auction_contracts::ListingVersion::new)
        .ok_or(ConsignmentError::OutcomeUnknown)
}

fn positive_version(
    value: i64,
) -> Result<auction_domain::consignment::ListingVersion, ConsignmentError> {
    let value = u64::try_from(value).map_err(invalid_deposit)?;
    auction_domain::consignment::ListingVersion::new(value).map_err(invalid_deposit)
}

fn invalid_deposit<T>(_error: T) -> ConsignmentError {
    ConsignmentError::InvalidDepositRequest
}

fn no_query(uri: &Uri) -> Result<(), ConsignmentError> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(ConsignmentError::InvalidDepositRequest);
    }
    Ok(())
}

fn listing_id(path: Result<Path<String>, PathRejection>) -> Result<uuid::Uuid, ConsignmentError> {
    let Ok(Path(raw)) = path else {
        return Err(ConsignmentError::InvalidDepositRequest);
    };
    parse_public_uuid(&raw).map_err(|()| ConsignmentError::InvalidDepositRequest)
}

async fn decode_write<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: Body,
) -> Result<T, RouteError> {
    input::decode_write(headers, body).await.map_err(|error| match error {
        RouteError::Consignment(
            ConsignmentError::InvalidCommandId
            | ConsignmentError::InvalidListingRequest
            | ConsignmentError::InvalidPrice,
        ) => RouteError::Consignment(ConsignmentError::InvalidDepositRequest),
        other => other,
    })
}

fn read_failure(_error: ConsignmentError) -> ConsignmentError {
    ConsignmentError::Unavailable
}
