//! 匿名目录、本人历史及原命令回执的只读处理器。

use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingRegistrationAvailability, ListingWorlds,
};
use auction_contracts::{ConsignmentStatus, ListingFeeBps, ListingMode, ListingWorld};
use auction_domain::consignment::ListingPrice;
use auction_domain::{CommandId, UserId};
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Uri};
use tokio::time::timeout;

use crate::public::consignment::input;
use crate::public::consignment::mapping;
use crate::public::consignment::response::{RouteResult, loaded};
use crate::public::consignment::{ConsignmentCapabilities, ConsignmentState};

const LISTING_NOT_FOUND: ConsignmentError =
    ConsignmentError::Business(ListingBusinessRejection::ListingNotFound);
const REGISTRATION_AVAILABILITY_BUDGET: Duration = Duration::from_secs(1);

/// 加载有效开关与当前登记就绪的两世界能力。
pub(super) async fn status<S: ConsignmentState>(State(state): State<S>, uri: Uri) -> RouteResult {
    input::no_query(&uri)?;
    let capabilities = state.consignment();
    let worlds = registration_worlds(capabilities).await;
    let registration_worlds: Vec<_> =
        [worlds.main.then_some(ListingWorld::Main), worlds.isekai.then_some(ListingWorld::Isekai)]
            .into_iter()
            .flatten()
            .collect();
    let registration_ready = !registration_worlds.is_empty();
    let registration_modes = if registration_ready {
        vec![ListingMode::Direct, ListingMode::Consigned]
    } else {
        Vec::new()
    };
    loaded(ConsignmentStatus {
        enabled: capabilities.enabled,
        registration_ready,
        registration_modes,
        registration_worlds,
        accepts_mail: false,
        purchasable: false,
        fee_bps: ListingFeeBps::new(i64::from(capabilities.fee_bps.value()))
            .ok_or(ConsignmentError::Unavailable)?,
        min_price: ListingPrice::MIN,
        max_price: ListingPrice::MAX,
    })
}

async fn registration_worlds(capabilities: &ConsignmentCapabilities) -> ListingWorlds {
    if capabilities.enabled
        && let (Some(reader), Some(crypto)) =
            (&capabilities.public_read, &capabilities.replay_crypto)
        && let Ok(Ok(ListingRegistrationAvailability::Ready(worlds))) = timeout(
            REGISTRATION_AVAILABILITY_BUDGET,
            reader.registration_availability(crypto.as_ref()),
        )
        .await
        && capabilities.registration_sealer_available
        && capabilities.register.is_some()
    {
        return worlds;
    }
    // 就绪查询超时或无可用依赖时只公告配置，外层请求期限仍由 HTTP 边界负责。
    ListingWorlds::default()
}

/// 匿名读取一页经端口确认仍可公开的摘要。
pub(super) async fn list_public<S: ConsignmentState>(
    State(state): State<S>,
    uri: Uri,
) -> RouteResult {
    let query = input::public_query(&uri)?;
    let capabilities = state.consignment();
    if !capabilities.enabled {
        return Err(ConsignmentError::Disabled.into());
    }
    let reader = capabilities.public_read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let window = query.window;
    let page = reader.list_public(query).await.map_err(read_failure)?;
    loaded(mapping::page(page, window, mapping::public_summary)?)
}

/// 匿名读取经端口确认可公开的独立快照。
pub(super) async fn public_detail<S: ConsignmentState>(
    State(state): State<S>,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> RouteResult {
    input::no_query(&uri)?;
    let listing_id = input::listing_id(path)?;
    let capabilities = state.consignment();
    if !capabilities.enabled {
        return Err(ConsignmentError::Disabled.into());
    }
    let reader = capabilities.public_read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let detail =
        reader.public_detail(listing_id).await.map_err(read_failure)?.ok_or(LISTING_NOT_FOUND)?;
    if detail.summary.listing_id != listing_id {
        return Err(ConsignmentError::Unavailable.into());
    }
    loaded(mapping::public_detail(detail)?)
}

/// 按当前会话与有效态读取本人历史。
pub(super) async fn list_owned<S: ConsignmentState>(
    State(state): State<S>,
    headers: HeaderMap,
    uri: Uri,
) -> RouteResult {
    let identity = input::private_identity(state.public_state(), &headers).await?;
    let query = input::private_query(&uri)?;
    let capabilities = state.consignment();
    let reader = capabilities.owned_read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let page = reader
        .list_owned(UserId::from_uuid(identity.user_id), query)
        .await
        .map_err(read_failure)?;
    loaded(mapping::page(page, query.window, |entry| {
        let effective_state = entry.listing().view.effective_state().0;
        // 有效状态由读端口判定。关闭时尚返回活动态，或不符合有效态筛选时，拒绝整页。
        if (!capabilities.enabled && effective_state.is_active())
            || query.state.is_some_and(|state| state != effective_state)
        {
            return Err(ConsignmentError::Unavailable);
        }
        mapping::owned(entry)
    })?)
}

/// 按当前会话读取单条挂单，关闭后仍可查。
pub(super) async fn owned_detail<S: ConsignmentState>(
    State(state): State<S>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> RouteResult {
    let identity = input::private_identity(state.public_state(), &headers).await?;
    input::no_query(&uri)?;
    let listing_id = input::listing_id(path)?;
    let capabilities = state.consignment();
    let reader = capabilities.owned_read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let entry = reader
        .owned_detail(UserId::from_uuid(identity.user_id), listing_id)
        .await
        .map_err(read_failure)?
        .ok_or(LISTING_NOT_FOUND)?;
    if entry.listing().listing_id != listing_id
        || (!capabilities.enabled && entry.listing().view.effective_state().0.is_active())
    {
        return Err(ConsignmentError::Unavailable.into());
    }
    loaded(mapping::owned(entry)?)
}

/// 按当前会话查已提交命令，保留首次业务状态和回执。
pub(super) async fn owned_command<S: ConsignmentState>(
    State(state): State<S>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    uri: Uri,
) -> RouteResult {
    let identity = input::private_identity(state.public_state(), &headers).await?;
    input::no_query(&uri)?;
    let command_id = CommandId::from_uuid(input::command_id(path)?);
    let reader = state.consignment().owned_read.as_ref().ok_or(ConsignmentError::Unavailable)?;
    let entry = reader
        .owned_command(UserId::from_uuid(identity.user_id), command_id)
        .await
        .map_err(read_failure)?
        .ok_or(ConsignmentError::CommandNotFound)?;
    if entry.command_id != command_id {
        return Err(ConsignmentError::Unavailable.into());
    }
    loaded(mapping::stored_command(entry)?)
}

// 读端口失败只表示没有可信快照。资源不可见由端口的 Ok(None) 单独表达。
fn read_failure(_failure: ConsignmentError) -> ConsignmentError {
    ConsignmentError::Unavailable
}
