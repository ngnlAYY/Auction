//! Public 竞价与代理命令的稳定出口。
//! 普通竞价和代理创建/降价分别归属相邻 owner；本文件负责模块装配、路由导出与三者共用的命令入口和指标收尾。

use auction_application::auction::{ApplicationFuture, CompetitionCommandOutcome};

use crate::http::routes::state::PublicApiState;
use crate::http::routes::{
    api_error, authenticated_public_competition_identity, business_outcome_for_competition_error,
    map_competition_error, observe_public_business, parse_public_uuid, session_guard,
};
use auction_application::public::PublicSessionIdentity;
use auction_observability::{BusinessEvent, BusinessOutcome};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use std::time::Instant;
use uuid::Uuid;

mod bid;
mod proxy;

// 编译期约束浏览器公开常量与领域权威边界，避免只改前端或只改后端造成上限漂移。
const _: () = assert!(auction_contracts::MAX_PUBLIC_BID_AMOUNT == auction_domain::MAX_BID_AMOUNT);

pub(crate) use bid::place_bid;
pub(crate) use proxy::{create_proxy_authorization, lower_proxy_authorization};

/// 竞价与代理命令的共同入口：会话边界、已认证竞价身份、拒绝查询参数并解析 lot 路径，顺序与旧 v1 一致。
async fn command_entry(
    state: &PublicApiState,
    headers: &HeaderMap,
    address: SocketAddr,
    uri: &Uri,
    lot_id: &str,
) -> Result<(PublicSessionIdentity, Uuid), Box<Response>> {
    if let Some(response) = session_guard(state, headers, address, true, false) {
        return Err(Box::new(response));
    }
    let identity = authenticated_public_competition_identity(state, headers).await?;
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(Box::new(
            api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response(),
        ));
    }
    let Ok(lot_uuid) = parse_public_uuid(lot_id) else {
        return Err(Box::new(
            api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
                .into_response(),
        ));
    };
    Ok((identity, lot_uuid))
}

/// 等待竞价命令并按同一起点记录业务指标；失败映射为稳定的竞价错误响应。
async fn observed_command(
    state: &PublicApiState,
    event: BusinessEvent,
    command: ApplicationFuture<'_, CompetitionCommandOutcome>,
) -> Result<CompetitionCommandOutcome, Box<Response>> {
    let started = Instant::now();
    let result = command.await;
    let outcome = match &result {
        Ok(_) => BusinessOutcome::Success,
        Err(error) => business_outcome_for_competition_error(error),
    };
    observe_public_business(state, event, outcome, started);
    result.map_err(|error| Box::new(map_competition_error(error)))
}
