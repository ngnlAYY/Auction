//! Public 登记、改价和取消的认证入口及首次业务回执。

use std::net::SocketAddr;

use auction_application::consignment::{ConsignmentError, RegisterListingInput};
use auction_contracts::{RegisterListing, RepriceListing, WithdrawListing};
use auction_domain::CommandId;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, Request, State};

use crate::public::consignment::ConsignmentState;
use crate::public::consignment::input;
use crate::public::consignment::mapping;
use crate::public::consignment::response::{RouteResult, command};

/// 严格解码登记输入，并向命令用例请求首次决定或原回执。
pub(super) async fn register<S: ConsignmentState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> RouteResult {
    let (parts, body) = request.into_parts();
    let identity =
        input::mutation_identity(state.public_state(), &parts.headers, address, &parts.uri).await?;
    let input: RegisterListing = input::decode_write(&parts.headers, body).await?;
    let service = state.consignment().register.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let command_id = CommandId::from_uuid(input.command_id);

    // 不变量 1、6、7、10：只把原参数和认证身份交给用例；新开关、费率和密码能力
    // 都不得挡住旧命令仲裁。原装备链接仅借用于本地校验，绝不记录。
    let result = service
        .register(
            &identity,
            RegisterListingInput {
                command_id,
                mode: mapping::domain_mode(input.mode),
                equipment_url: &input.equipment_url,
                price: input.price.get(),
                expected_fee_bps: input.expected_fee_bps.get(),
            },
        )
        .await?;
    command(mapping::registered(command_id, result))
}

/// 提交本人改价，绑定路径目标与预期版本。
pub(super) async fn reprice<S: ConsignmentState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> RouteResult {
    let (parts, body) = request.into_parts();
    let identity =
        input::mutation_identity(state.public_state(), &parts.headers, address, &parts.uri).await?;
    let listing_id = input::listing_id(path)?;
    let input: RepriceListing = input::decode_write(&parts.headers, body).await?;
    let service = state.consignment().manage.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let command_id = CommandId::from_uuid(input.command_id);
    let Ok(expected_version) = u64::try_from(input.expected_version.get()) else {
        return Err(ConsignmentError::InvalidListingRequest.into());
    };

    // 不变量 1、2、6：即使关闭也进入用例，先仲裁并由仓储核对归属、状态和版本。
    let result = service
        .reprice(&identity, command_id, listing_id, expected_version, input.price.get())
        .await?;
    if result.listing_id() != listing_id {
        return Err(ConsignmentError::OutcomeUnknown.into());
    }
    command(mapping::repriced(command_id, result))
}

/// 提交本人取消，关闭时仍由用例仲裁及清退。
pub(super) async fn withdraw<S: ConsignmentState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> RouteResult {
    let (parts, body) = request.into_parts();
    let identity =
        input::mutation_identity(state.public_state(), &parts.headers, address, &parts.uri).await?;
    let listing_id = input::listing_id(path)?;
    let input: WithdrawListing = input::decode_write(&parts.headers, body).await?;
    let service = state.consignment().manage.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let command_id = CommandId::from_uuid(input.command_id);
    let Ok(expected_version) = u64::try_from(input.expected_version.get()) else {
        return Err(ConsignmentError::InvalidListingRequest.into());
    };

    // 不变量 1、3、5、8：在同一仓储决定里回放或清退，不在 HTTP 推导终态。
    let result = service.withdraw(&identity, command_id, listing_id, expected_version).await?;
    if result.listing_id() != listing_id {
        return Err(ConsignmentError::OutcomeUnknown.into());
    }
    command(mapping::closed(command_id, result))
}
