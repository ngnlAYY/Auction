//! 运行时开关客户端：读取全部开关；写入只开放外部访问暂停。

use auction_contracts::{
    AdminExternalAccessPauseRequest, AdminExternalAccessPauseResponse, ApiResponse,
    RuntimeSwitchIntentDto, RuntimeSwitchListResponse,
};
use auction_time::Timestamp;
use gloo_net::http::Method;
use uuid::Uuid;

use super::AdminWriteFailure;
use super::transport::WriteRequest;
use super::{invalid_response_error, response_error, send_get, valid_safe_text};

const KNOWN_SWITCHES: [&str; 7] = [
    "readiness_audit_failure_mode",
    "readiness_cache_enabled",
    "metrics_sli_snapshot_enabled",
    "mooglemail_poll_isolation_enabled",
    "metrics_reject_forwarded_requests",
    "external_access_paused",
    "disposition_read_mode",
];

pub(crate) async fn fetch_runtime_switches() -> Result<RuntimeSwitchListResponse, String> {
    let response = send_get("/api/v1/admin/runtime-switches").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let body = response.text().await.map_err(invalid_response_error)?;
    decode_runtime_switches(&body)
}

fn decode_runtime_switches(body: &str) -> Result<RuntimeSwitchListResponse, String> {
    let payload: ApiResponse<RuntimeSwitchListResponse> =
        serde_json::from_str(body).map_err(invalid_response_error)?;
    let database_names =
        payload.data.database_intent.iter().map(|item| item.name.as_str()).collect::<Vec<_>>();
    let applied_names =
        payload.data.admin_applied.iter().map(|item| item.name.as_str()).collect::<Vec<_>>();
    if !closed_subset(&database_names)
        || !closed_subset(&applied_names)
        || !applied_names.iter().all(|name| database_names.contains(name))
    {
        return Err("invalid_response".to_owned());
    }
    Ok(payload.data)
}

fn closed_subset(names: &[&str]) -> bool {
    names.iter().all(|name| KNOWN_SWITCHES.contains(name))
        && names.iter().enumerate().all(|(index, name)| !names[..index].contains(name))
}

/// 一次暂停或恢复尝试。结果未知时必须整份原样重试：命令 ID、期望版本、绝对期限与原因
/// 都在首次提交时确定，不能按重试时刻重新计算。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExternalAccessPauseAttempt {
    pub(crate) command_id: Uuid,
    pub(crate) expected_revision: i64,
    pub(crate) paused: bool,
    pub(crate) expires_at: Option<Timestamp>,
    pub(crate) reason: String,
}

impl ExternalAccessPauseAttempt {
    /// 与服务端 RuntimeSwitchCommand 共用的 UTF-8 字节边界。
    pub(crate) const MAX_REASON_BYTES: usize = 1024;

    /// 校验将提交的单行原因。表单与发送前检查使用同一口径。
    pub(crate) fn valid_reason(value: &str) -> bool {
        valid_safe_text(value, Self::MAX_REASON_BYTES)
    }
}

#[cfg(test)]
type PauseSubmitStub = std::rc::Rc<dyn Fn(&AdminExternalAccessPauseRequest) -> (u16, String)>;

#[cfg(test)]
impl ExternalAccessPauseAttempt {
    pub(crate) fn stub_submit(
        submit: impl Fn(&AdminExternalAccessPauseRequest) -> (u16, String) + 'static,
    ) {
        dioxus::prelude::provide_context(std::rc::Rc::new(submit) as PauseSubmitStub);
    }
}

/// 提交外部访问暂停或恢复；只接受与命令一致的已接受事实。
pub(crate) async fn submit_external_access_pause(
    attempt: &ExternalAccessPauseAttempt,
) -> Result<RuntimeSwitchIntentDto, AdminWriteFailure> {
    let reason = attempt.reason.trim();
    if attempt.command_id.is_nil()
        || attempt.expected_revision < 0
        || attempt.paused != attempt.expires_at.is_some()
        || !ExternalAccessPauseAttempt::valid_reason(reason)
    {
        return Err(AdminWriteFailure::not_sent("invalid_request"));
    }
    let command = AdminExternalAccessPauseRequest {
        command_id: attempt.command_id.to_string(),
        expected_revision: attempt.expected_revision,
        paused: attempt.paused,
        expires_at: attempt.expires_at,
        reason: reason.to_owned(),
    };
    let request = WriteRequest::general(
        "/api/v1/admin/runtime-switches/external-access-paused",
        Method::POST,
    );
    let body = send_pause_command(request, &command).await?;
    validate_pause_intent(decode_pause_receipt(&body, command.paused)?, &command)
}

async fn send_pause_command(
    request: WriteRequest<'_>,
    command: &AdminExternalAccessPauseRequest,
) -> Result<String, AdminWriteFailure> {
    #[cfg(test)]
    if let Some(submit) = dioxus::prelude::try_consume_context::<PauseSubmitStub>() {
        let (status, body) = submit(command);
        if !(200..300).contains(&status) {
            return Err(request.decode_stub_failure(status, &body));
        }
        return Ok(body);
    }
    let response = request.send_json(command).await?;
    response.text().await.map_err(AdminWriteFailure::invalid_response)
}

fn decode_pause_receipt(
    body: &str,
    paused: bool,
) -> Result<RuntimeSwitchIntentDto, AdminWriteFailure> {
    let payload: ApiResponse<AdminExternalAccessPauseResponse> =
        serde_json::from_str(body).map_err(AdminWriteFailure::invalid_response)?;
    validate_pause_operation(payload.data.accepted, paused)
}

fn validate_pause_operation(
    accepted: RuntimeSwitchIntentDto,
    paused: bool,
) -> Result<RuntimeSwitchIntentDto, AdminWriteFailure> {
    let expected_operation = if paused { "set" } else { "clear" };
    if accepted.name != "external_access_paused"
        || accepted.operation.as_deref() != Some(expected_operation)
        || accepted.revision < 1
    {
        return Err(AdminWriteFailure::invalid_response("external access pause"));
    }
    Ok(accepted)
}

fn validate_pause_intent(
    accepted: RuntimeSwitchIntentDto,
    command: &AdminExternalAccessPauseRequest,
) -> Result<RuntimeSwitchIntentDto, AdminWriteFailure> {
    let value_matches = if command.paused {
        accepted.value.as_ref().and_then(serde_json::Value::as_bool) == Some(true)
    } else {
        accepted.value.is_none()
    };
    // 账本返回本次命令接受时的事实，重放也沿用原修订与期限。错配保留未决命令供原样重试。
    if Some(accepted.revision) != command.expected_revision.checked_add(1)
        || accepted.expires_at != command.expires_at
        || !value_matches
        || accepted.reason.as_deref() != Some(command.reason.as_str())
        || accepted.entrypoint.as_deref() != Some("admin")
    {
        return Err(AdminWriteFailure::invalid_response("external access pause"));
    }
    Ok(accepted)
}

#[cfg(test)]
mod tests {
    use super::ExternalAccessPauseAttempt;

    #[test]
    fn pause_reason_uses_the_server_utf8_byte_limit() {
        let at_limit = format!("{}x", "停".repeat(341));
        assert_eq!(at_limit.len(), 1024);
        assert!(ExternalAccessPauseAttempt::valid_reason(&at_limit));
        assert!(ExternalAccessPauseAttempt::valid_reason(&"x".repeat(1024)));
        assert!(!ExternalAccessPauseAttempt::valid_reason(&"停".repeat(342)));
        assert!(!ExternalAccessPauseAttempt::valid_reason(&"x".repeat(1025)));
    }

    #[test]
    fn pause_reason_must_be_nonempty_normalized_single_line_text() {
        assert!(ExternalAccessPauseAttempt::valid_reason("暂停检查"));
        for value in ["", " ", " 前后有空白 ", "两\n行", "有\t控制字符"] {
            assert!(!ExternalAccessPauseAttempt::valid_reason(value), "{value:?}");
        }
    }
    #[test]
    fn pause_receipt_matches_the_frozen_intent_field_by_field() {
        use auction_contracts::AdminExternalAccessPauseRequest;
        use serde_json::json;

        use super::{decode_pause_receipt, validate_pause_intent};
        use crate::api::AdminWriteFailure;

        let command: AdminExternalAccessPauseRequest = serde_json::from_value(json!({
            "command_id": "00000000-0000-4000-8000-000000000002",
            "expected_revision": 7,
            "paused": true,
            "expires_at": "2026-11-20T06:00:00Z",
            "reason": "演练"
        }))
        .unwrap();
        let intent = json!({
            "name": "external_access_paused", "operation": "set", "revision": 8,
            "expires_at": "2026-11-20T01:00:00-05:00", "value": true, "reason": "演练",
            "entrypoint": "admin", "actor_id": "operator"
        });
        let submit = |accepted| {
            let body = json!({"code":"ok","message":"","data":{"accepted":accepted}});
            validate_pause_intent(
                decode_pause_receipt(&body.to_string(), command.paused)?,
                &command,
            )
        };
        let first = submit(intent.clone()).unwrap();
        assert_eq!(first.revision, 8);
        assert_eq!(first.expires_at, command.expires_at);
        for (field, value) in [
            ("name", json!("metrics_sli_snapshot_enabled")),
            ("operation", json!("clear")),
            ("revision", json!(9)),
            ("expires_at", json!("2026-11-20T06:00:01Z")),
            ("value", json!(false)),
            ("reason", json!("另一原因")),
            ("entrypoint", json!("cli")),
        ] {
            let mut invalid = intent.clone();
            invalid[field] = value;
            assert!(
                matches!(
                    submit(invalid),
                    Err(AdminWriteFailure::OutcomeUnknown { code, response_status: None })
                        if code == "invalid_response"
                ),
                "{field}"
            );
        }
        assert_eq!(submit(intent).unwrap(), first);
    }

    #[test]
    fn resume_receipt_rejects_the_wrong_revision() {
        use auction_contracts::AdminExternalAccessPauseRequest;

        use super::{decode_pause_receipt, validate_pause_intent};
        use crate::api::AdminWriteFailure;
        use crate::api::contract_fixtures::admin_fixture;

        let command = AdminExternalAccessPauseRequest {
            command_id: "00000000-0000-4000-8000-000000000002".to_owned(),
            expected_revision: 1,
            paused: false,
            expires_at: None,
            reason: "resume regular polling".to_owned(),
        };
        let mut accepted =
            decode_pause_receipt(admin_fixture!("runtime-switch-command"), false).unwrap();
        accepted.revision = 3;
        assert!(matches!(
            validate_pause_intent(accepted, &command),
            Err(AdminWriteFailure::OutcomeUnknown { code, response_status: None })
                if code == "invalid_response"
        ));
    }

    #[test]
    fn runtime_switch_read_and_command_contract_fixtures() {
        use auction_contracts::AdminExternalAccessPauseRequest;
        use serde_json::{Value, json};

        use super::{decode_pause_receipt, decode_runtime_switches, validate_pause_intent};
        use crate::api::contract_fixtures::{admin_fixture, checked};

        let body = admin_fixture!("runtime-switches");
        let result = checked(body, "/data", "database_intent", decode_runtime_switches);
        let pause = result
            .database_intent
            .iter()
            .find(|item| item.name == "external_access_paused")
            .expect("外部访问意图");
        assert_eq!(pause.operation.as_deref(), Some("set"));
        assert!(result.admin_last_success_age_seconds.is_none());
        let mut invalid: Value = serde_json::from_str(body).unwrap();
        let entries = invalid["data"]["database_intent"].as_array_mut().unwrap();
        entries.push(entries[0].clone());
        assert!(decode_runtime_switches(&invalid.to_string()).is_err());
        let mut invalid: Value = serde_json::from_str(body).unwrap();
        invalid["data"]["database_intent"]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry["name"] != "external_access_paused");
        assert!(decode_runtime_switches(&invalid.to_string()).is_err());

        let body = admin_fixture!("runtime-switch-command");
        let accepted =
            checked(body, "/data/accepted", "revision", |body| decode_pause_receipt(body, false));
        assert_eq!(accepted.name, "external_access_paused");
        assert_eq!(accepted.operation.as_deref(), Some("clear"));
        assert_eq!(accepted.revision, 2);
        let resume = AdminExternalAccessPauseRequest {
            command_id: "00000000-0000-4000-8000-000000000002".to_owned(),
            expected_revision: 1,
            paused: false,
            expires_at: None,
            reason: "resume regular polling".to_owned(),
        };
        assert!(validate_pause_intent(accepted, &resume).is_ok());
        assert!(decode_pause_receipt(body, true).is_err());
        let mut invalid: Value = serde_json::from_str(body).unwrap();
        invalid["data"]["accepted"]["name"] = json!("metrics_sli_snapshot_enabled");
        assert!(decode_pause_receipt(&invalid.to_string(), false).is_err());
    }
}
