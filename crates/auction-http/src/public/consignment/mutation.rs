//! Public 登记、改价和取消的认证入口及首次业务回执。

use std::net::SocketAddr;

use auction_application::consignment::{
    AnonymousRegisterListingInput, ConsignmentError, RegisterListingInput,
};
use auction_contracts::{RegisterDirectListing, RegisterListing, RepriceListing, WithdrawListing};
use auction_domain::CommandId;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, Request, State};

use crate::middleware::boundary::request_id;
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

    // 不变量 1、6、7、10：把原参数和认证身份交给用例，关联号仅供审计。
    // 新开关、费率和密码能力不得挡住旧命令仲裁，原装备链接不进入记录。
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
            request_id(&parts.headers),
        )
        .await?;
    command(mapping::registered(command_id, result))
}

/// 免登录登记非寄售挂单：来源与 mutation 限流守卫先于解码，新命令才消耗每来源登记额度；
/// 卖家身份只取正文的自填游戏角色名。
pub(super) async fn register_direct<S: ConsignmentState>(
    State(state): State<S>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> RouteResult {
    let (parts, body) = request.into_parts();
    input::anonymous_mutation_guard(state.public_state(), &parts.headers, address, &parts.uri)?;
    let input: RegisterDirectListing = input::decode_write(&parts.headers, body).await?;
    let service = state.consignment().register.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let command_id = CommandId::from_uuid(input.command_id);
    // 不变量 1：每来源额度只限制新命令的创建，重放从不创建新事实，因此已入账的命令 ID 不再扣额度，
    // 交由用例仲裁（同请求回原回执，异请求 409 command_id_conflict）。这里只用“是否存在”分流，
    // 查到的内容不进入响应；读端口未装配时按新命令扣额度。账本读取失败时失败关闭：传播不可用错误，
    // 不扣额度、不进入用例（此时登记本来也会失败）。并发的同 ID 新请求可能各扣一次，只会多算、不会少算。
    let replay = match state.consignment().public_read.as_ref() {
        Some(reader) => reader.anonymous_command(command_id).await?.is_some(),
        None => false,
    };
    if !replay {
        input::anonymous_listing_quota(state.public_state(), address)?;
    }

    // 不变量 1、6、7：模式与零费率由用例固定；同一命令 ID 的原样重试按自填名仲裁回原回执，
    // 不读会话 Cookie，原装备链接不进入记录。
    let result = service
        .register_anonymous(
            AnonymousRegisterListingInput {
                command_id,
                seller_game_name: &input.seller_game_name,
                equipment_url: &input.equipment_url,
                price: input.price.get(),
            },
            request_id(&parts.headers),
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
        .reprice(
            &identity,
            command_id,
            listing_id,
            expected_version,
            input.price.get(),
            request_id(&parts.headers),
        )
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
    let result = service
        .withdraw(&identity, command_id, listing_id, expected_version, request_id(&parts.headers))
        .await?;
    if result.listing_id() != listing_id {
        return Err(ConsignmentError::OutcomeUnknown.into());
    }
    command(mapping::closed(command_id, result))
}
