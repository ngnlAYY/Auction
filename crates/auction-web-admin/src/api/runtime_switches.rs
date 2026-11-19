//! 运行时开关客户端：读取全部开关；写入只开放外部访问暂停。

use auction_contracts::{
    AdminExternalAccessPauseRequest, AdminExternalAccessPauseResponse, ApiResponse,
    RuntimeSwitchIntentDto, RuntimeSwitchListResponse,
};
use auction_time::Timestamp;
use gloo_net::http::Request;
use uuid::Uuid;

use super::{
    AdminApiFailure, fetch_csrf_token, invalid_request_error, invalid_response_error,
    response_error, response_failure, send_get, send_request, valid_safe_text,
};

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
type PauseSubmitStub = std::rc::Rc<
    dyn Fn(&AdminExternalAccessPauseRequest) -> Result<RuntimeSwitchIntentDto, AdminApiFailure>,
>;

#[cfg(test)]
impl ExternalAccessPauseAttempt {
    pub(crate) fn stub_submit(
        submit: impl Fn(
            &AdminExternalAccessPauseRequest,
        ) -> Result<RuntimeSwitchIntentDto, AdminApiFailure>
        + 'static,
    ) {
        dioxus::prelude::provide_context(std::rc::Rc::new(submit) as PauseSubmitStub);
    }
}

/// 提交外部访问暂停或恢复；只接受与命令一致的已接受事实。
pub(crate) async fn submit_external_access_pause(
    attempt: &ExternalAccessPauseAttempt,
) -> Result<RuntimeSwitchIntentDto, AdminApiFailure> {
    let reason = attempt.reason.trim();
    if attempt.command_id.is_nil()
        || attempt.expected_revision < 0
        || attempt.paused != attempt.expires_at.is_some()
        || !ExternalAccessPauseAttempt::valid_reason(reason)
    {
        return Err(AdminApiFailure::client("invalid_request"));
    }
    let command = AdminExternalAccessPauseRequest {
        command_id: attempt.command_id.to_string(),
        expected_revision: attempt.expected_revision,
        paused: attempt.paused,
        expires_at: attempt.expires_at,
        reason: reason.to_owned(),
    };
    #[cfg(test)]
    if let Some(submit) = dioxus::prelude::try_consume_context::<PauseSubmitStub>() {
        return submit(&command);
    }
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let response = send_request(|abort_signal| {
        Request::post("/api/v1/admin/runtime-switches/external-access-paused")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let body = response
        .text()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_pause_receipt(&body, attempt.paused)
}

fn decode_pause_receipt(
    body: &str,
    paused: bool,
) -> Result<RuntimeSwitchIntentDto, AdminApiFailure> {
    let payload: ApiResponse<AdminExternalAccessPauseResponse> = serde_json::from_str(body)
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    let accepted = payload.data.accepted;
    let expected_operation = if paused { "set" } else { "clear" };
    if accepted.name != "external_access_paused"
        || accepted.operation.as_deref() != Some(expected_operation)
        || accepted.revision < 1
    {
        return Err(AdminApiFailure::client(invalid_response_error("external access pause")));
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
    fn runtime_switch_read_and_command_contract_fixtures() {
        use serde_json::{Value, json};

        use super::{decode_pause_receipt, decode_runtime_switches};
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
        assert!(decode_pause_receipt(body, true).is_err());
        let mut invalid: Value = serde_json::from_str(body).unwrap();
        invalid["data"]["accepted"]["name"] = json!("metrics_sli_snapshot_enabled");
        assert!(decode_pause_receipt(&invalid.to_string(), false).is_err());
    }
}
