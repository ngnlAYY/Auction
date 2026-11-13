//! Admin 出站通知模板接口：读取每个模板的当前正文与内置默认正文，逐个保存或恢复默认。
//! 正文保存在数据库的只追加修订事实里；校验、摘要、重放与版本规则由应用命令定义，这里只做
//! 请求与 DTO 映射。

use super::AdminApiState;
use crate::diagnostics;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::mail_templates::{
    MailTemplateCommand, MailTemplateEvent, MailTemplateOperation, MailTemplateSet,
    MailTemplateStoreError,
};
use auction_contracts::{
    AdminMailTemplateDto, AdminMailTemplateListResponse, AdminMailTemplateUpdateRequest,
    AdminMailTemplateUpdateResponse, ApiResponse,
};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use std::{net::SocketAddr, time::Duration};
use uuid::Uuid;

/// 读取等待上限。
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// 数据库写入等待上限；超时后提交结果未知，只允许原样重试。
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// 全部模板的当前状态。
pub(crate) async fn list(
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
    let Some(store) = state.mail_templates() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "feature_unavailable", "邮件模板不可用");
    };
    let events = match tokio::time::timeout(READ_TIMEOUT, store.latest_mail_templates()).await {
        Ok(Ok(events)) => events,
        other => {
            let (phase, safe_kind) = match &other {
                Ok(Err(cause)) => ("read_templates", cause.diagnostic_kind()),
                Err(cause) => ("read_templates_timeout", diagnostics::timeout_kind(cause)),
                Ok(Ok(_)) => return super::internal_error(),
            };
            diagnostics::warn(
                "admin_mail_templates",
                phase,
                safe_kind,
                DiagnosticContext::default(),
            );
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "邮件模板读取失败",
            );
        }
    };
    let defaults = MailTemplateSet::default();
    let templates = MailTemplateSet::KEYS
        .into_iter()
        .map(|key| template_dto(key, &defaults, latest(&events, key)))
        .collect();
    (
        StatusCode::OK,
        super::no_store_headers(),
        axum::Json(ApiResponse::ok("邮件模板已读取", AdminMailTemplateListResponse { templates })),
    )
        .into_response()
}

/// 保存一个模板的正文，或恢复内置默认正文。命令经模板修订账本写入：同 ID 同参重放返回原
/// 事实，版本已变化或同 ID 异参为冲突，结果未知只能原样重试。
pub(crate) async fn update(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(template_key): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let identity = match super::authenticated_admin_mutation(&state, &headers, address).await {
        Ok((_, identity)) => identity,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return invalid("查询参数无效");
    }
    let Ok(request) = serde_json::from_slice::<AdminMailTemplateUpdateRequest>(&body) else {
        return invalid("请求 JSON 无效");
    };
    let command =
        match update_command(template_key, request, identity.actor, super::request_id(&headers)) {
            Ok(command) => command,
            Err(failure) => return store_error(failure),
        };
    let Some(store) = state.mail_templates() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "feature_unavailable", "邮件模板不可用");
    };
    match tokio::time::timeout(WRITE_TIMEOUT, store.write_mail_template(command)).await {
        Ok(Ok(event)) => {
            let defaults = MailTemplateSet::default();
            // 已接受的事实来自本版闭集内的命令，键一定能映射；映射不到说明账本与程序不一致。
            let Some(key) =
                MailTemplateSet::KEYS.into_iter().find(|key| *key == event.template_key)
            else {
                return super::internal_error();
            };
            (
                StatusCode::OK,
                super::no_store_headers(),
                axum::Json(ApiResponse::ok(
                    "数据库已接受；各角色约 5 秒内刷新",
                    AdminMailTemplateUpdateResponse {
                        template: template_dto(key, &defaults, Some(&event)),
                    },
                )),
            )
                .into_response()
        }
        Ok(Err(failure)) => {
            diagnostics::warn(
                "admin_mail_templates",
                "write_template",
                failure.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            store_error(failure)
        }
        Err(cause) => {
            diagnostics::warn(
                "admin_mail_templates",
                "write_template_timeout",
                diagnostics::timeout_kind(&cause),
                DiagnosticContext::default(),
            );
            store_error(MailTemplateStoreError::OutcomeUnknown)
        }
    }
}

/// 把请求映射为修订命令并做不依赖数据库的校验：`body` 为空表示恢复默认。
fn update_command(
    template_key: String,
    request: AdminMailTemplateUpdateRequest,
    actor_id: String,
    request_id: Option<String>,
) -> Result<MailTemplateCommand, MailTemplateStoreError> {
    let command_id = Uuid::parse_str(&request.command_id)
        .ok()
        .filter(|id| !id.is_nil())
        .ok_or(MailTemplateStoreError::Invalid)?;
    let operation = if request.body.is_some() {
        MailTemplateOperation::Set
    } else {
        MailTemplateOperation::Reset
    };
    let command = MailTemplateCommand {
        command_id,
        template_key,
        expected_revision: request.expected_revision,
        operation,
        body: request.body,
        actor_id,
        request_id,
    };
    command.validate()?;
    Ok(command)
}

fn latest<'a>(events: &'a [MailTemplateEvent], key: &str) -> Option<&'a MailTemplateEvent> {
    events.iter().filter(|event| event.template_key == key).max_by_key(|event| event.revision)
}

/// 一个模板键的展示状态。运营保存过的正文原样返回，即使它已不满足现行规则：编辑框要能
/// 看到并改正它；`usable` 说明实际发信是否在用它。
fn template_dto(
    key: &'static str,
    defaults: &MailTemplateSet,
    event: Option<&MailTemplateEvent>,
) -> AdminMailTemplateDto {
    let default_body = defaults.body(key).unwrap_or_default().to_owned();
    let custom = event
        .filter(|event| event.operation == MailTemplateOperation::Set)
        .and_then(|event| event.body.clone());
    let usable =
        custom.as_deref().is_none_or(|body| MailTemplateSet::validate_body(key, body).is_ok());
    let (required, allowed) = MailTemplateSet::placeholders(key).unwrap_or((&[], &[]));
    AdminMailTemplateDto {
        key: key.to_owned(),
        customized: custom.is_some(),
        body: custom.unwrap_or_else(|| default_body.clone()),
        default_body,
        usable,
        revision: event.map_or(0, |event| event.revision),
        updated_by: event.map(|event| event.actor_id.clone()),
        updated_at: event.map(|event| event.recorded_at),
        required_placeholders: required.iter().map(|name| (*name).to_owned()).collect(),
        allowed_placeholders: allowed.iter().map(|name| (*name).to_owned()).collect(),
    }
}

fn store_error(failure: MailTemplateStoreError) -> Response {
    match failure {
        MailTemplateStoreError::Invalid => invalid("模板命令无效"),
        MailTemplateStoreError::UnknownTemplate => {
            error(StatusCode::NOT_FOUND, "mail_template_not_found", "不存在该邮件模板")
        }
        // 原因取自模板 owner 的固定消息，不回显正文。
        MailTemplateStoreError::InvalidBody(reason) => invalid(reason),
        MailTemplateStoreError::Conflict => error(
            StatusCode::CONFLICT,
            "mail_template_conflict",
            "版本已变化或命令 ID 参数不一致；刷新后用新命令重试",
        ),
        MailTemplateStoreError::OutcomeUnknown => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "outcome_unknown",
            "提交结果未知；请用原命令 ID 与原参数重试",
        ),
        MailTemplateStoreError::Corrupt | MailTemplateStoreError::Unavailable => {
            error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "邮件模板存储不可用")
        }
    }
}

fn invalid(message: &'static str) -> Response {
    error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_mail_template", message)
}

fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (status, super::no_store_headers(), super::api_error(status, code, message)).into_response()
}

#[cfg(test)]
mod tests {
    use super::{
        AdminMailTemplateUpdateRequest, MailTemplateEvent, MailTemplateOperation, MailTemplateSet,
        MailTemplateStoreError, template_dto, update_command,
    };
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    const PAYOUT: &str = "Sold {item_name}.\nPayout: {amount} Credits\nReference: {reference}";

    fn request(body: Option<&str>) -> AdminMailTemplateUpdateRequest {
        AdminMailTemplateUpdateRequest {
            command_id: Uuid::from_u128(7).to_string(),
            expected_revision: 0,
            body: body.map(str::to_owned),
        }
    }

    fn event(revision: i64, body: Option<&str>) -> MailTemplateEvent {
        MailTemplateEvent {
            command_id: Uuid::from_u128(revision as u128 + 1),
            template_key: "settlement_seller_payout".to_owned(),
            revision,
            operation: if body.is_some() {
                MailTemplateOperation::Set
            } else {
                MailTemplateOperation::Reset
            },
            body: body.map(str::to_owned),
            actor_id: "admin".to_owned(),
            recorded_at: Utc.with_ymd_and_hms(2026, 11, 13, 12, 0, 0).unwrap(),
        }
    }

    #[test]
    fn body_maps_to_set_and_null_maps_to_reset() {
        let key = "settlement_seller_payout".to_owned();
        let set = update_command(key.clone(), request(Some(PAYOUT)), "admin".to_owned(), None)
            .expect("合法保存命令");
        assert_eq!(set.operation, MailTemplateOperation::Set);
        assert_eq!(set.body.as_deref(), Some(PAYOUT));
        let reset =
            update_command(key, request(None), "admin".to_owned(), None).expect("合法恢复默认命令");
        assert_eq!(reset.operation, MailTemplateOperation::Reset);
        assert_eq!(reset.body, None);
    }

    #[test]
    fn invalid_requests_fail_closed_before_reaching_the_store() {
        let key = "settlement_seller_payout".to_owned();
        let mut nil = request(Some(PAYOUT));
        nil.command_id = Uuid::nil().to_string();
        assert_eq!(
            update_command(key.clone(), nil, "admin".to_owned(), None).unwrap_err(),
            MailTemplateStoreError::Invalid
        );
        let mut negative = request(Some(PAYOUT));
        negative.expected_revision = -1;
        assert_eq!(
            update_command(key.clone(), negative, "admin".to_owned(), None).unwrap_err(),
            MailTemplateStoreError::Invalid
        );
        // 正文缺必需占位符；未知模板键。
        assert!(matches!(
            update_command(key, request(Some("Payout only")), "admin".to_owned(), None),
            Err(MailTemplateStoreError::InvalidBody(_))
        ));
        assert_eq!(
            update_command("no_such".to_owned(), request(Some(PAYOUT)), "admin".to_owned(), None)
                .unwrap_err(),
            MailTemplateStoreError::UnknownTemplate
        );
    }

    #[test]
    fn projection_distinguishes_default_custom_and_unusable_bodies() {
        let defaults = MailTemplateSet::default();
        let key = "settlement_seller_payout";
        let default_body = defaults.body(key).unwrap_or_default();

        let untouched = template_dto(key, &defaults, None);
        assert_eq!((untouched.revision, untouched.customized, untouched.usable), (0, false, true));
        assert_eq!(untouched.body, default_body);
        assert_eq!(untouched.required_placeholders, ["amount", "reference"]);
        assert!(untouched.allowed_placeholders.contains(&"item_name".to_owned()));

        let custom = template_dto(key, &defaults, Some(&event(1, Some(PAYOUT))));
        assert_eq!((custom.revision, custom.customized, custom.usable), (1, true, true));
        assert_eq!((custom.body.as_str(), custom.default_body.as_str()), (PAYOUT, default_body));

        // 恢复默认后仍保留版本，下一次保存必须基于它。
        let reset = template_dto(key, &defaults, Some(&event(2, None)));
        assert_eq!((reset.revision, reset.customized), (2, false));
        assert_eq!(reset.body, default_body);

        // 已不满足现行规则的正文原样给编辑框，并标明实际发信没有在用它。
        let unusable = template_dto(key, &defaults, Some(&event(3, Some("Payout only"))));
        assert_eq!((unusable.customized, unusable.usable), (true, false));
        assert_eq!(unusable.body, "Payout only");
    }
}
