//! 结算退件列表与详情的只读处理器；响应都是安全投影并带 `no-store`。

use std::net::SocketAddr;

use auction_contracts::ApiResponse;
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::boundary::{feature_unavailable, map_settlement_return_error, parse_return_id};
use super::filters::{ListQuery, list_query};
use super::mapping::{map_detail, map_page};
use crate::errors::query_rejection;
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

/// 读取一页结算退件案件；隔离记录与未知义务也会列出。
pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let Ok(ListQuery { page, request }) = list_query(&uri) else {
        return query_rejection().into_response();
    };
    let Some(port) = state.settlement_returns() else {
        return feature_unavailable();
    };
    let (limit, offset) = (request.limit, request.offset);
    let result = match port.list_settlement_returns(request).await {
        Ok(result) => result,
        Err(error) => return map_settlement_return_error("list", error),
    };
    let Some(response) = map_page(result, page, limit, offset) else {
        return internal_error();
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("结算退件列表已加载", response)))
        .into_response()
}

/// 读取单个案件详情：事件、派发链、Take、代管、资产位置、允许动作与阻断原因。
pub(crate) async fn detail(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(return_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return query_rejection().into_response();
    }
    let return_id = match parse_return_id(&return_id) {
        Ok(return_id) => return_id,
        Err(response) => return *response,
    };
    let Some(port) = state.settlement_returns() else {
        return feature_unavailable();
    };
    let detail = match port.settlement_return_detail(return_id).await {
        Ok(detail) => detail,
        Err(error) => return map_settlement_return_error("detail", error),
    };
    let Some(response) = map_detail(detail, return_id) else {
        return internal_error();
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("结算退件详情已加载", response)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::core::ApplicationFuture;
    use auction_application::settlement::{
        SettlementReturnAdminPort, SettlementReturnAssetLocation, SettlementReturnCaseSummary,
        SettlementReturnCommand, SettlementReturnCommandResult, SettlementReturnDetail,
        SettlementReturnEvent, SettlementReturnListRequest, SettlementReturnMatch,
        SettlementReturnPage, SettlementReturnReason,
    };
    use auction_domain::{
        SettlementReturnAction, SettlementReturnBlocker, SettlementReturnCaseStatus,
    };
    use axum::http::{Method, StatusCode};
    use serde_json::json;
    use uuid::Uuid;

    use crate::server::contract_fixtures::{
        admin_get, admin_request, admin_state, assert_http, time,
    };

    const RETURN_ID: Uuid = Uuid::from_u128(0x4000_8000_0000_0000_0024);

    fn summary() -> SettlementReturnCaseSummary {
        SettlementReturnCaseSummary {
            return_mail_id: RETURN_ID,
            case_version: 1,
            status: SettlementReturnCaseStatus::Quarantined,
            match_state: SettlementReturnMatch::Untrusted,
            reason: SettlementReturnReason::SourceUnproven,
            world_id: "main".to_owned(),
            external_mail_id: "9100".to_owned(),
            received_at: time(),
            obligation_id: None,
            obligation_kind: None,
            auction_id: None,
            lot_id: None,
            delivery_id: None,
            dispatch_no: None,
            recipient: None,
            credits: 1000,
            cod: 0,
            attachment_count: 0,
            plan: None,
            resend_limit: auction_domain::SETTLEMENT_RETURN_RESEND_LIMIT,
            resends_used: None,
            asset_location: SettlementReturnAssetLocation::Unverified,
            created_at: time(),
            updated_at: time(),
            closed_at: None,
        }
    }

    struct Returns;

    impl SettlementReturnAdminPort for Returns {
        fn list_settlement_returns(
            &self,
            request: SettlementReturnListRequest,
        ) -> ApplicationFuture<'_, SettlementReturnPage> {
            Box::pin(async move {
                assert_eq!(
                    request.status,
                    (request.offset == 1).then_some(SettlementReturnCaseStatus::Quarantined),
                );
                Ok(SettlementReturnPage {
                    items: if request.offset == 0 { vec![summary()] } else { Vec::new() },
                    total: 1,
                })
            })
        }

        fn settlement_return_detail(
            &self,
            return_mail_id: Uuid,
        ) -> ApplicationFuture<'_, SettlementReturnDetail> {
            assert_eq!(return_mail_id, RETURN_ID);
            Box::pin(async {
                Ok(SettlementReturnDetail {
                    summary: summary(),
                    events: vec![SettlementReturnEvent {
                        case_version: 1,
                        event_kind: "opened".to_owned(),
                        occurred_at: time(),
                        actor_id: None,
                        command_id: None,
                        details: json!({"match_state":"untrusted","reason_code":"source_unproven","dispatch_no":null}),
                    }],
                    dispatches: Vec::new(),
                    take: None,
                    resend_dispatch_no: None,
                    custody: None,
                    allowed_actions: vec![SettlementReturnAction::Acknowledge],
                    blockers: SettlementReturnAction::ALL
                        .iter()
                        .copied()
                        .filter(|action| *action != SettlementReturnAction::Acknowledge)
                        .map(|action| (action, SettlementReturnBlocker::Untrusted))
                        .collect(),
                    recheck_unavailable: false,
                    take_unavailable: false,
                    take_unknown_reason: None,
                })
            })
        }

        fn handle_settlement_return(
            &self,
            command: SettlementReturnCommand,
        ) -> ApplicationFuture<'_, SettlementReturnCommandResult> {
            Box::pin(async move {
                assert_eq!(command.return_mail_id, RETURN_ID);
                assert_eq!(command.action, SettlementReturnAction::Acknowledge);
                assert_eq!(command.expected_version, 1);
                Ok(SettlementReturnCommandResult {
                    command_id: command.command_id,
                    return_mail_id: command.return_mail_id,
                    action: command.action,
                    case_version: 2,
                    status: SettlementReturnCaseStatus::Reviewed,
                })
            })
        }

        fn open_settlement_return_count(&self) -> ApplicationFuture<'_, i64> {
            Box::pin(async { Ok(1) })
        }
    }

    #[tokio::test]
    async fn settlement_return_read_and_command_contract_fixtures() {
        let state = admin_state().with_settlement_returns(Arc::new(Returns));
        let path = "/api/v1/admin/settlement-returns";
        let detail = format!("{path}/{RETURN_ID}");
        for (name, path) in [
            ("admin/settlement-returns", path.to_owned()),
            (
                "admin/settlement-returns-empty-page",
                format!("{path}?status=quarantined&page=2&page_size=1"),
            ),
            ("admin/settlement-return", detail.clone()),
        ] {
            assert_http(name, StatusCode::OK, admin_get(state.clone(), &path).await).await;
        }
        let body = json!({"command_id": Uuid::from_u128(0x4000_8000_0000_0000_0007), "expected_version": 1, "action": "acknowledge"});
        let response = admin_request(
            state,
            Method::POST,
            &format!("{detail}/handle"),
            body.to_string().into(),
        )
        .await;
        assert_http("admin/settlement-return-command", StatusCode::OK, response).await;
    }
}
