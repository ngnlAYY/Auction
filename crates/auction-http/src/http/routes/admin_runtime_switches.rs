//! Admin 认证只读开关接口；数据库意图与本进程实际快照分别映射。

use super::AdminApiState;
use auction_application::RuntimeSwitchName;
use auction_contracts::{
    ApiResponse, RuntimeSwitchAppliedDto, RuntimeSwitchIntentDto, RuntimeSwitchListResponse,
};
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use std::{net::SocketAddr, time::Duration};

pub(crate) async fn get(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let Some((store, switches)) = state.runtime_switches() else {
        return (
            super::no_store_headers(),
            super::api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "feature_unavailable",
                "运行时开关不可用",
            ),
        )
            .into_response();
    };
    let read = match tokio::time::timeout(Duration::from_secs(1), store.read()).await {
        Ok(Ok(read)) => read,
        other => {
            drop(other);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                super::no_store_headers(),
                super::api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "运行时开关读取失败",
                ),
            )
                .into_response();
        }
    };
    let status = switches.status();
    let database_intent = RuntimeSwitchName::ALL
        .into_iter()
        .map(|name| {
            let event = read.events.iter().find(|event| event.name == name);
            RuntimeSwitchIntentDto {
                name: name.label().to_owned(),
                revision: event.map_or(0, |event| event.revision),
                operation: event.map(|event| event.operation.label().to_owned()),
                value: event.and_then(|event| event.value.clone()),
                expires_at: event.and_then(|event| event.expires_at),
                actor_id: event.map(|event| event.actor_id.clone()),
                entrypoint: event.map(|event| event.entrypoint.clone()),
                reason: event.map(|event| event.reason.clone()),
            }
        })
        .collect();
    let admin_applied = status
        .switches
        .into_iter()
        .filter(|view| view.applies)
        .map(|view| RuntimeSwitchAppliedDto {
            name: view.name.label().to_owned(),
            revision: view.applied_revision,
            toml_baseline: view.toml_baseline,
            effective_value: view.effective_value,
            overridden: view.overridden,
        })
        .collect();
    (
        StatusCode::OK,
        super::no_store_headers(),
        axum::Json(ApiResponse::ok(
            "运行时开关已读取",
            RuntimeSwitchListResponse {
                database_time: read.database_time,
                database_intent,
                admin_last_success_age_seconds: status.last_success_age_seconds,
                admin_applied,
            },
        )),
    )
        .into_response()
}
