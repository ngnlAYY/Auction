//! 结算退件处理命令：只读重新核验与闭集处理动作。
//!
//! 每条命令携带浏览器生成的 `command_id` 与读取时的案件版本；操作者取自服务端会话，请求时刻取
//! 服务端时钟。同一 `command_id` 原样重试由存储层重放原结果，参数不同返回 409 `command_conflict`
//! （不变量 1）；命令只做本地写入，外部 Take 由 Worker 在本地提交后执行（不变量 4）。

use std::net::SocketAddr;

use auction_application::settlement::SettlementReturnCommand;
use auction_contracts::{
    AdminSettlementReturnHandleRequest, AdminSettlementReturnRecheckRequest, ApiResponse,
};
use auction_domain::{CommandId, SettlementReturnAction};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use uuid::Uuid;

use super::boundary::{
    feature_unavailable, invalid_command, map_settlement_return_error, parse_return_id,
};
use super::mapping::map_command_result;
use crate::errors::api_error;
use crate::session::{authenticated_admin_mutation, internal_error, no_store_headers};
use crate::state::AdminApiState;

/// 未绑定案件提交只读重新核验；Admin 进程不直连游戏，由 Worker 读取原退件后写入核验事实。
pub(crate) async fn recheck(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(return_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if let Some(response) = reject_query(&uri) {
        return response;
    }
    let Ok(input) = serde_json::from_slice::<AdminSettlementReturnRecheckRequest>(&body) else {
        return invalid_json();
    };
    let request = CommandInput {
        command_id: input.command_id,
        expected_version: input.expected_version,
        action: SettlementReturnAction::Recheck,
    };
    execute(&state, &return_id, request, &identity.actor).await
}

/// 执行闭集处理动作：领回并补寄、领回后代管、从代管补寄、留箱结案、标记已核对。
pub(crate) async fn handle(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(return_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if let Some(response) = reject_query(&uri) {
        return response;
    }
    let Ok(input) = serde_json::from_slice::<AdminSettlementReturnHandleRequest>(&body) else {
        return invalid_json();
    };
    let Some(action) = handle_action(&input.action) else {
        return invalid_command();
    };
    let request = CommandInput {
        command_id: input.command_id,
        expected_version: input.expected_version,
        action,
    };
    execute(&state, &return_id, request, &identity.actor).await
}

/// 已解析请求体中的命令字段。
struct CommandInput {
    command_id: Uuid,
    expected_version: i64,
    action: SettlementReturnAction,
}

/// handle 路由接受的动作闭集；重新核验只走独立路由，未知名称返回 `None`。
fn handle_action(value: &str) -> Option<SettlementReturnAction> {
    SettlementReturnAction::from_name(value)
        .filter(|action| *action != SettlementReturnAction::Recheck)
}

/// 命令字段的结构校验；资格、版本与来源由存储层在命令事务内锁内复核。
fn valid_input(input: &CommandInput) -> bool {
    !input.command_id.is_nil() && input.expected_version > 0
}

async fn execute(
    state: &AdminApiState,
    return_id: &str,
    input: CommandInput,
    actor: &str,
) -> Response {
    let return_id = match parse_return_id(return_id) {
        Ok(return_id) => return_id,
        Err(response) => return *response,
    };
    if !valid_input(&input) {
        return invalid_command();
    }
    let Some(port) = state.settlement_returns() else {
        return feature_unavailable();
    };
    let command = match SettlementReturnCommand::new(
        CommandId::from_uuid(input.command_id),
        return_id,
        input.expected_version,
        input.action,
        actor,
        Utc::now(),
    ) {
        Ok(command) => command,
        Err(error) => return map_settlement_return_error("build_command", error),
    };
    let result = match port.handle_settlement_return(command).await {
        Ok(result) => result,
        Err(error) => return map_settlement_return_error("handle", error),
    };
    // 重放结果也必须对应本次请求：同一命令身份、同一案件与动作，版本只会前进。
    if result.command_id.as_uuid() != input.command_id
        || result.return_mail_id != return_id
        || result.action != input.action
        || result.case_version <= input.expected_version
    {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(action_message(input.action), map_command_result(result))),
    )
        .into_response()
}

fn reject_query(uri: &Uri) -> Option<Response> {
    uri.query().is_some_and(|query| !query.is_empty()).then(|| {
        api_error(StatusCode::BAD_REQUEST, "invalid_query", "处理命令不接受查询参数")
            .into_response()
    })
}

fn invalid_json() -> Response {
    api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效").into_response()
}

fn action_message(action: SettlementReturnAction) -> &'static str {
    match action {
        SettlementReturnAction::ReclaimAndResend => "已提交领回并补寄",
        SettlementReturnAction::ReclaimAndHold => "已提交领回后代管",
        SettlementReturnAction::ResendFromCustody => "已授权从代管补寄",
        SettlementReturnAction::CloseInbox => "已留箱结案",
        SettlementReturnAction::Acknowledge => "已标记已核对",
        SettlementReturnAction::Recheck => "已提交只读重新核验",
    }
}

#[cfg(test)]
mod tests {
    use auction_contracts::ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS;
    use auction_domain::SettlementReturnAction;
    use uuid::Uuid;

    use super::{CommandInput, handle_action, valid_input};

    /// handle 只接受契约列出的五个动作；重新核验、未知名称与大小写变体都拒绝。
    #[test]
    fn handle_accepts_only_the_contract_action_set() {
        for name in ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS {
            let action = handle_action(name).expect(name);
            assert_eq!(action.as_str(), name);
        }
        for invalid in
            ["recheck", "reconcile", "force_bind", "CLOSE_INBOX", "acknowledge_quarantine", ""]
        {
            assert_eq!(handle_action(invalid), None, "{invalid}");
        }
    }

    /// 命令身份必须非零，案件版本必须为正。
    #[test]
    fn command_fields_require_identity_and_positive_version() {
        let input = |command_id, expected_version| CommandInput {
            command_id,
            expected_version,
            action: SettlementReturnAction::CloseInbox,
        };
        assert!(valid_input(&input(Uuid::from_u128(1), 1)));
        assert!(!valid_input(&input(Uuid::nil(), 1)));
        assert!(!valid_input(&input(Uuid::from_u128(1), 0)));
        assert!(!valid_input(&input(Uuid::from_u128(1), -3)));
    }
}
