//! 受限权利的响应外壳、DTO 映射与只含安全分类的错误映射。
//!
//! 找回与挑战的 202 只有一种形状；任何分支都不得依据目标是否真实改变状态码、头或正文（不变量 7）。

use auction_application::consignment::{
    ChallengeIssued, ClaimDisposition, ClaimOutcome, RightReceipt, RightVerified, RightsStoreError,
};
use auction_contracts::{
    ApiResponse, ListingWorld, RightActionV2, RightChallengeAccepted, RightClaimDispositionV2,
    RightClaimReceipt, RightKindV2, RightTargetV2, RightVerifiedGrant, RightViewV2,
};
use auction_domain::consignment::rights::{RightAction, RightKind, RightTarget, RightView};
use auction_domain::{CommandId, SettlementWorld};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use super::cookie::{BrowserCookie, session_set_cookie};
use crate::errors::api_error;
use crate::public::consignment::response::RouteError;

/// 权利处理器的路由错误。
pub(super) enum RightsRouteError {
    /// 用例或存储给出的安全分类。
    Rights(RightsStoreError),
    /// 命令回执不存在。
    ReceiptNotFound,
    /// 正文大小、媒体类型与来源守卫等共用边界的拒绝。
    Route(RouteError),
}

impl From<RightsStoreError> for RightsRouteError {
    fn from(error: RightsStoreError) -> Self {
        Self::Rights(error)
    }
}

impl From<RouteError> for RightsRouteError {
    fn from(error: RouteError) -> Self {
        Self::Route(error)
    }
}

impl From<Box<Response>> for RightsRouteError {
    fn from(response: Box<Response>) -> Self {
        Self::Route(response.into())
    }
}

impl IntoResponse for RightsRouteError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Route(error) => return error.into_response(),
            Self::ReceiptNotFound => {
                (StatusCode::NOT_FOUND, "command_not_found", "找不到该命令回执")
            }
            Self::Rights(error) => {
                let (status, code) = error.api_error();
                let status =
                    StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
                (status, code, message(error))
            }
        };
        api_error(status, code, message).into_response()
    }
}

/// 固定文案：不回显输入，也不暗示目标是否存在。
const fn message(error: RightsStoreError) -> &'static str {
    match error {
        RightsStoreError::InvalidRequest => "请求无效",
        RightsStoreError::ChallengeInvalid => "挑战无效或已过期",
        RightsStoreError::RightExpired => "权利会话已过期",
        RightsStoreError::ActionNotAllowed => "当前不允许该动作",
        RightsStoreError::StaleVersion => "权利版本已变化，请刷新后重试",
        RightsStoreError::BudgetExceeded => "请求过于频繁",
        RightsStoreError::Unauthenticated => "需要先验证权利",
        RightsStoreError::CommandIdConflict => "命令编号已用于不同请求",
        RightsStoreError::Unavailable => "权利服务暂不可用",
    }
}

/// 在浏览器绑定已确定后收口：无论成功或失败都带上同一枚绑定 Cookie，使各分支形状一致。
pub(super) fn with_browser_cookie(
    result: Result<Response, RightsRouteError>,
    browser: &BrowserCookie,
) -> Response {
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    match browser.set_cookie() {
        Some(cookie) => {
            response.headers_mut().append(header::SET_COOKIE, cookie);
            response
        }
        None => RightsRouteError::Rights(RightsStoreError::Unavailable).into_response(),
    }
}

fn json<T: Serialize>(
    status: StatusCode,
    body: ApiResponse<T>,
    cookie: Option<HeaderValue>,
) -> Result<Response, RightsRouteError> {
    // DTO 在序列化时复验自洽性；失败时写结果可能已提交，客户端按原命令 ID 重放。
    let Ok(bytes) = serde_json::to_vec(&body) else {
        return Err(RightsStoreError::Unavailable.into());
    };
    let mut response =
        (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response();
    if let Some(cookie) = cookie {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    Ok(response)
}

/// 找回与挑战共用的 202：真实、虚拟与未匹配目标的唯一成功形状。
pub(super) fn challenge_accepted(issued: ChallengeIssued) -> Result<Response, RightsRouteError> {
    json(
        StatusCode::ACCEPTED,
        ApiResponse::accepted(
            "请求已受理",
            RightChallengeAccepted {
                challenge_id: issued.challenge_id,
                expires_at: issued.expires_at,
            },
        ),
        None,
    )
}

/// 核验成功：令牌只进 `hv_right` Cookie，正文不含它。
pub(super) fn verified(grant: RightVerified) -> Result<Response, RightsRouteError> {
    let cookie = session_set_cookie(&grant.session_token).ok_or(RightsStoreError::Unavailable)?;
    json(
        StatusCode::OK,
        ApiResponse::ok(
            "权利已核验",
            RightVerifiedGrant {
                right_id: grant.right_id,
                right_version: wire_version(grant.right_version)?,
                expires_at: grant.expires_at,
            },
        ),
        Some(cookie),
    )
}

/// 当前会话对应的权利视图。
pub(super) fn current(view: RightView) -> Result<Response, RightsRouteError> {
    json(
        StatusCode::OK,
        ApiResponse::ok(
            "已加载",
            RightViewV2 {
                right_id: view.right_id,
                right_version: wire_version(view.right_version)?,
                kind: wire_kind(view.kind),
                world: wire_world(view.world),
                target: wire_target(view.contract_or_position),
                allowed_actions: view.allowed_actions.into_iter().map(wire_action).collect(),
            },
        ),
        None,
    )
}

/// 续办结果：受理为 202，另一命令已在履约或已完成为 200。
pub(super) fn claim(
    command_id: CommandId,
    outcome: ClaimOutcome,
) -> Result<Response, RightsRouteError> {
    let receipt = RightClaimReceipt {
        command_id: command_id.as_uuid(),
        right_id: outcome.right_id,
        disposition: wire_disposition(outcome.disposition),
    };
    match outcome.disposition {
        ClaimDisposition::ClaimAccepted => {
            json(StatusCode::ACCEPTED, ApiResponse::accepted("续办已受理", receipt), None)
        }
        ClaimDisposition::AlreadyFulfilling | ClaimDisposition::AlreadyCompleted => {
            json(StatusCode::OK, ApiResponse::ok("续办已处理", receipt), None)
        }
    }
}

/// 命令回执查询。
pub(super) fn receipt(receipt: RightReceipt) -> Result<Response, RightsRouteError> {
    json(
        StatusCode::OK,
        ApiResponse::ok(
            "已加载",
            RightClaimReceipt {
                command_id: receipt.command_id.as_uuid(),
                right_id: receipt.right_id,
                disposition: wire_disposition(receipt.disposition),
            },
        ),
        None,
    )
}

fn wire_version(version: u64) -> Result<i64, RightsRouteError> {
    let Ok(version) = i64::try_from(version) else {
        return Err(RightsStoreError::Unavailable.into());
    };
    Ok(version)
}

fn wire_world(world: SettlementWorld) -> ListingWorld {
    match world {
        SettlementWorld::Main => ListingWorld::Main,
        SettlementWorld::Isekai => ListingWorld::Isekai,
    }
}

/// 请求里的世界到领域世界。
pub(super) fn domain_world(world: ListingWorld) -> SettlementWorld {
    match world {
        ListingWorld::Main => SettlementWorld::Main,
        ListingWorld::Isekai => SettlementWorld::Isekai,
    }
}

/// 请求里的动作到领域动作。
pub(super) fn domain_action(action: RightActionV2) -> RightAction {
    match action {
        RightActionV2::ContinueOrder => RightAction::ContinueOrder,
        RightActionV2::ClaimEquipment => RightAction::ClaimEquipment,
        RightActionV2::ClaimPayment => RightAction::ClaimPayment,
        RightActionV2::WithdrawHeld => RightAction::WithdrawHeld,
    }
}

fn wire_action(action: RightAction) -> RightActionV2 {
    match action {
        RightAction::ContinueOrder => RightActionV2::ContinueOrder,
        RightAction::ClaimEquipment => RightActionV2::ClaimEquipment,
        RightAction::ClaimPayment => RightActionV2::ClaimPayment,
        RightAction::WithdrawHeld => RightActionV2::WithdrawHeld,
    }
}

fn wire_kind(kind: RightKind) -> RightKindV2 {
    match kind {
        RightKind::ContinueOrder => RightKindV2::ContinueOrder,
        RightKind::ClaimEquipment => RightKindV2::ClaimEquipment,
        RightKind::ClaimPayment => RightKindV2::ClaimPayment,
        RightKind::WithdrawHeld => RightKindV2::WithdrawHeld,
        RightKind::LossCase => RightKindV2::LossCase,
    }
}

fn wire_target(target: RightTarget) -> RightTargetV2 {
    match target {
        RightTarget::Contract(contract_id) => RightTargetV2::Contract { contract_id },
        RightTarget::Position(position_id) => RightTargetV2::Position { position_id },
    }
}

fn wire_disposition(disposition: ClaimDisposition) -> RightClaimDispositionV2 {
    match disposition {
        ClaimDisposition::ClaimAccepted => RightClaimDispositionV2::ClaimAccepted,
        ClaimDisposition::AlreadyFulfilling => RightClaimDispositionV2::AlreadyFulfilling,
        ClaimDisposition::AlreadyCompleted => RightClaimDispositionV2::AlreadyCompleted,
    }
}
