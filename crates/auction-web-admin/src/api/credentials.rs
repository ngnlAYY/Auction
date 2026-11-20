//! Admin 凭据管理 API 客户端。
//! 导入的密码和 Cookie 随会话命令驻留内存，未知结果只允许原样手动重试。
//! 列表和命令响应只包含凭据安全投影。

use std::cell::{Cell, OnceCell};

use auction_contracts::{
    AdminCredentialCheckRequest, AdminCredentialCreateRequest, AdminCredentialDeleteRequest,
    AdminCredentialDeleteResponse, AdminCredentialListResponse, AdminCredentialMutationRequest,
    AdminCredentialMutationResponse, ApiResponse,
};
pub(crate) use auction_contracts::{
    AdminCredentialCheckResponse as AdminCredentialCheck,
    AdminCredentialDeleteResponse as AdminCredentialDeleteResult,
    AdminCredentialResponse as AdminCredential,
};
use gloo_net::http::{Method, Request};
use uuid::Uuid;

use super::AdminWriteFailure;
use super::transport::WriteRequest;
use super::{
    MAX_JSON_SAFE_INTEGER, fetch_csrf_token, invalid_request_error, invalid_response_error,
    response_error, send_get, send_request_with_timeout, valid_admin_time, valid_trimmed_text,
    valid_uuid,
};

// 导入须容纳服务端 10 秒论坛获取及原子写入；不延长其他 API 请求，也不自动重试。
const CREDENTIAL_IMPORT_TIMEOUT_MILLIS: u32 = 20_000;

// shared 邮件检查串行检查两个世界，须覆盖 WORKER_MAIL_CREDENTIAL_CHECK_TIMEOUT_SECONDS（12 秒）
// 和 Admin HTTP 默认的 15 秒预算，另留 3 秒给响应传输。
const SHARED_MAIL_CHECK_TIMEOUT_MILLIS: u32 = 18_000;

// 结构闭集由 contracts 的 `AdminCredentialResponse`（`rotated_at` 保持必填 nullable 语义）、
// `AdminCredentialListResponse`/`AdminCredentialMutationResponse`/`AdminCredentialDeleteResponse`
//（均为 `deny_unknown_fields`）负责；本文件只保留筛选回显与版本递增等跨字段谓词。

/// 凭据列表页的浏览器展示投影；分页字段在这里展平，wire 形状仍由 contracts 拥有。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminCredentialPage {
    pub credentials: Vec<AdminCredential>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub restart_required: bool,
}

impl From<AdminCredentialListResponse> for AdminCredentialPage {
    fn from(data: AdminCredentialListResponse) -> Self {
        Self {
            credentials: data.credentials,
            page: data.pagination.page,
            page_size: data.pagination.page_size,
            total: data.pagination.total,
            restart_required: data.restart_required,
        }
    }
}

/// 只读检查指定版本；响应的世界、状态与时间必须通过闭集校验。
pub(crate) async fn check_admin_credential(
    credential: &AdminCredential,
) -> Result<AdminCredentialCheck, String> {
    decode_credential(credential.clone()).map_err(invalid_request_error)?;
    let credential_id = Uuid::parse_str(&credential.id).map_err(invalid_request_error)?;
    let csrf_token = fetch_csrf_token().await?;
    let request = AdminCredentialCheckRequest { expected_state_version: credential.state_version };
    let response = send_request_with_timeout(
        |abort_signal| {
            Request::post(&format!("/api/v1/admin/credentials/{credential_id}/check"))
                .abort_signal(abort_signal)
                .header("x-csrf-token", &csrf_token)
                .json(&request)
                .map_err(invalid_request_error)
        },
        credential_check_timeout(credential),
    )
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminCredentialCheck> =
        response.json().await.map_err(invalid_response_error)?;
    decode_credential_check(payload.data, credential)
}

fn credential_check_timeout(credential: &AdminCredential) -> u32 {
    if credential.kind == "mail_cookie" && credential.world == "shared" {
        SHARED_MAIL_CHECK_TIMEOUT_MILLIS
    } else {
        auction_web_shared::DEFAULT_REQUEST_TIMEOUT_MILLIS
    }
}

fn decode_credential_check(
    result: AdminCredentialCheck,
    credential: &AdminCredential,
) -> Result<AdminCredentialCheck, String> {
    let worlds: &[&str] = match (credential.kind.as_str(), credential.world.as_str()) {
        ("forum_cookie", _) => &["shared"],
        ("mail_cookie", "main") => &["main"],
        ("mail_cookie", "isekai") => &["isekai"],
        ("mail_cookie", "shared") => &["main", "isekai"],
        _ => return Err("invalid_request".to_owned()),
    };
    if result.credential_id != credential.id
        || result.state_version != credential.state_version
        || !valid_admin_time(&result.checked_at)
        || result.checks.len() != worlds.len()
        || result.checks.iter().zip(worlds).any(|(check, world)| {
            check.world != *world
                || !matches!(check.status.as_str(), "valid" | "invalid" | "inconclusive")
        })
    {
        return Err("invalid_response".to_owned());
    }
    Ok(result)
}

/// 获取带筛选和页码的 Admin 凭据安全列表。
pub(crate) async fn fetch_admin_credentials(
    kind: String,
    world: String,
    status: String,
    page: i64,
    page_size: i64,
) -> Result<AdminCredentialPage, String> {
    super::pagination::validate(page, page_size, 200, i64::from(i32::MAX))?;
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.max(1).to_string())
        .append_pair("page_size", &page_size.to_string());
    if !kind.is_empty() {
        query.append_pair("kind", kind.trim());
    }
    if !world.is_empty() {
        query.append_pair("world", world.trim());
    }
    if !status.is_empty() {
        query.append_pair("status", status.trim());
    }
    let url = format!("/api/v1/admin/credentials?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminCredentialListResponse> =
        response.json().await.map_err(invalid_response_error)?;
    decode_credential_list(payload.data, kind.trim(), world.trim(), status.trim(), page, page_size)
        .map(AdminCredentialPage::from)
        .map_err(invalid_response_error)
}

struct PreparedCredentialCreate {
    body: String,
    kind: String,
    world: String,
    label: String,
    player_name: Option<String>,
}

struct CredentialCreatePreparation<F> {
    request: Cell<Option<F>>,
    prepared: OnceCell<Result<PreparedCredentialCreate, AdminWriteFailure>>,
}

impl<F: FnOnce(Uuid) -> AdminCredentialCreateRequest> CredentialCreatePreparation<F> {
    fn new(request: F) -> Self {
        Self { request: Cell::new(Some(request)), prepared: OnceCell::new() }
    }

    fn prepare(&self, command_id: Uuid) -> Result<&PreparedCredentialCreate, AdminWriteFailure> {
        self.prepared
            .get_or_init(|| {
                // 初次执行消费原输入，此后宿主只持有序列化正文及安全回执核对字段。
                let request = self
                    .request
                    .take()
                    .ok_or_else(|| AdminWriteFailure::not_sent("invalid_request"))?;
                let command = request(command_id);
                let body = serde_json::to_string(&command)
                    .map_err(|error| AdminWriteFailure::not_sent(invalid_request_error(error)))?;
                Ok(PreparedCredentialCreate {
                    body,
                    kind: command.kind,
                    world: command.world,
                    label: command.label,
                    player_name: command.player_name,
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

/// 冻结导入输入，首次执行时用宿主分配的命令 ID 编码，重试复用原正文。
pub(crate) fn create_admin_credential(
    request: impl FnOnce(Uuid) -> AdminCredentialCreateRequest + 'static,
) -> impl AsyncFn(Uuid) -> Result<AdminCredentialMutationResponse, AdminWriteFailure> + 'static {
    let preparation = CredentialCreatePreparation::new(request);
    async move |command_id| send_prepared_credential_create(preparation.prepare(command_id)?).await
}

async fn send_prepared_credential_create(
    command: &PreparedCredentialCreate,
) -> Result<AdminCredentialMutationResponse, AdminWriteFailure> {
    let response = WriteRequest::general("/api/v1/admin/credentials", Method::POST)
        .send_body_with_timeout(&command.body, CREDENTIAL_IMPORT_TIMEOUT_MILLIS)
        .await?;
    let payload: ApiResponse<AdminCredentialMutationResponse> =
        response.json().await.map_err(AdminWriteFailure::invalid_response)?;
    validate_credential_create(command, payload.data)
}

fn validate_credential_create(
    command: &PreparedCredentialCreate,
    data: AdminCredentialMutationResponse,
) -> Result<AdminCredentialMutationResponse, AdminWriteFailure> {
    let data = decode_credential_data(data).map_err(AdminWriteFailure::invalid_response)?;
    if data.credential.kind != command.kind
        || data.credential.world != command.world
        || data.credential.label != command.label
        || data.credential.player_name != command.player_name
    {
        return Err(AdminWriteFailure::outcome_unknown("invalid_response"));
    }
    Ok(data)
}

fn credential_mutation_request(
    command_id: Uuid,
    credential: &AdminCredential,
    action: &str,
    label: Option<String>,
    status: Option<String>,
) -> Result<AdminCredentialMutationRequest, AdminWriteFailure> {
    let credential_id = Uuid::parse_str(&credential.id)
        .map_err(|error| AdminWriteFailure::not_sent(invalid_request_error(error)))?;
    Ok(AdminCredentialMutationRequest {
        command_id,
        credential_id,
        expected_state_version: credential.state_version,
        action: action.to_owned(),
        label,
        status,
    })
}

/// 修改凭据标签或状态。
pub(crate) async fn mutate_admin_credential(
    command_id: Uuid,
    credential: &AdminCredential,
    action: &'static str,
    label: Option<String>,
    status: Option<String>,
) -> Result<AdminCredentialMutationResponse, AdminWriteFailure> {
    let command = credential_mutation_request(command_id, credential, action, label, status)?;
    let response = WriteRequest::general("/api/v1/admin/credentials", Method::PATCH)
        .send_json(&command)
        .await?;
    let payload: ApiResponse<AdminCredentialMutationResponse> =
        response.json().await.map_err(AdminWriteFailure::invalid_response)?;
    validate_credential_mutation(&command, credential, payload.data)
}

fn validate_credential_mutation(
    command: &AdminCredentialMutationRequest,
    credential: &AdminCredential,
    data: AdminCredentialMutationResponse,
) -> Result<AdminCredentialMutationResponse, AdminWriteFailure> {
    let data = decode_credential_data(data).map_err(AdminWriteFailure::invalid_response)?;
    let updated = &data.credential;
    if updated.id != command.credential_id.to_string()
        || Some(updated.state_version) != command.expected_state_version.checked_add(1)
        || updated.kind != credential.kind
        || updated.world != credential.world
        || updated.player_name != credential.player_name
        || match command.action.as_str() {
            "rename" => {
                command.label.as_deref() != Some(updated.label.as_str())
                    || command.status.is_some()
                    || updated.status != credential.status
            }
            "set_status" => {
                command.status.as_deref() != Some(updated.status.as_str())
                    || command.label.is_some()
                    || updated.label != credential.label
            }
            _ => true,
        }
    {
        return Err(AdminWriteFailure::outcome_unknown("invalid_response"));
    }
    Ok(data)
}

/// 删除凭据。
pub(crate) async fn delete_admin_credential(
    command_id: Uuid,
    credential: &AdminCredential,
) -> Result<AdminCredentialDeleteResult, AdminWriteFailure> {
    let credential_id = Uuid::parse_str(&credential.id)
        .map_err(|error| AdminWriteFailure::not_sent(invalid_request_error(error)))?;
    let command = AdminCredentialDeleteRequest {
        command_id,
        credential_id,
        expected_state_version: credential.state_version,
    };
    let response = WriteRequest::general("/api/v1/admin/credentials", Method::DELETE)
        .send_json(&command)
        .await?;
    let payload: ApiResponse<AdminCredentialDeleteResponse> =
        response.json().await.map_err(AdminWriteFailure::invalid_response)?;
    decode_credential_delete(
        payload.data,
        &command.credential_id.to_string(),
        command.expected_state_version,
    )
    .map_err(AdminWriteFailure::invalid_response)
}

fn decode_credential_list(
    data: AdminCredentialListResponse,
    kind_filter: &str,
    world_filter: &str,
    status_filter: &str,
    requested_page: i64,
    requested_size: i64,
) -> Result<AdminCredentialListResponse, &'static str> {
    if !data.restart_required {
        return Err("credential_restart");
    }
    let pagination = &data.pagination;
    if !super::pagination::valid_page_echo(
        requested_page,
        requested_size,
        pagination.page,
        pagination.page_size,
        pagination.total,
        data.credentials.len(),
    ) {
        return Err("credential_pagination");
    }
    let credentials =
        data.credentials.into_iter().map(decode_credential).collect::<Result<Vec<_>, _>>()?;
    if credentials.iter().any(|credential| {
        (!kind_filter.is_empty() && credential.kind != kind_filter)
            || (!world_filter.is_empty() && credential.world != world_filter)
            || (!status_filter.is_empty() && credential.status != status_filter)
    }) {
        return Err("credential_filter");
    }
    Ok(AdminCredentialListResponse {
        credentials,
        pagination: data.pagination,
        restart_required: true,
    })
}

fn decode_credential_data(
    data: AdminCredentialMutationResponse,
) -> Result<AdminCredentialMutationResponse, &'static str> {
    Ok(AdminCredentialMutationResponse {
        credential: decode_credential(data.credential)?,
        restart_required: data.restart_required,
    })
}

fn decode_credential_delete(
    data: AdminCredentialDeleteResponse,
    requested_credential_id: &str,
    expected_state_version: i64,
) -> Result<AdminCredentialDeleteResult, &'static str> {
    if !valid_uuid(&data.credential_id)
        || data.credential_id != requested_credential_id
        || Some(data.state_version) != expected_state_version.checked_add(1)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&data.state_version)
    {
        return Err("credential_delete");
    }
    Ok(data)
}

/// 跨字段谓词保留为独立校验：类型/世界/状态枚举与值域仍在浏览器边界 fail-closed。
fn decode_credential(value: AdminCredential) -> Result<AdminCredential, &'static str> {
    if !valid_uuid(&value.id)
        || !matches!(value.kind.as_str(), "forum_cookie" | "mail_cookie")
        || !matches!(value.world.as_str(), "main" | "isekai" | "shared")
        || !valid_trimmed_text(&value.label)
        || value.label.len() > 256
        || value.player_name.as_deref().is_some_and(|name| {
            value.kind != "mail_cookie" || !valid_trimmed_text(name) || name.len() > 256
        })
        || !matches!(value.status.as_str(), "active" | "disabled" | "invalid")
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.state_version)
        || !valid_admin_time(&value.created_at)
        || value.rotated_at.as_deref().is_some_and(|time| !valid_admin_time(time))
        || !valid_admin_time(&value.updated_at)
    {
        return Err("credential");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use auction_contracts::{
        AdminCredentialCreateRequest, AdminCredentialDeleteResponse, AdminCredentialLoginRequest,
        AdminCredentialMutationRequest, AdminCredentialMutationResponse,
    };
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{
        AdminCredential, CredentialCreatePreparation, credential_check_timeout,
        credential_mutation_request, decode_credential_check, decode_credential_data,
        decode_credential_delete, validate_credential_create, validate_credential_mutation,
    };
    use crate::api::AdminWriteFailure;

    const CREDENTIAL_ID: &str = "11111111-1111-4111-8111-111111111111";
    const OBSERVED_AT: &str = "2026-11-18T00:00:00Z";

    fn credential_document() -> Value {
        json!({
            "id": CREDENTIAL_ID,
            "kind": "mail_cookie",
            "world": "shared",
            "label": "Worker",
            "player_name": null,
            "status": "active",
            "state_version": 7,
            "created_at": OBSERVED_AT,
            "rotated_at": null,
            "updated_at": OBSERVED_AT,
        })
    }

    #[test]
    fn credential_mutations_preserve_the_server_restart_flag() {
        for restart_required in [true, false] {
            let mutation: AdminCredentialMutationResponse = serde_json::from_value(json!({
                "credential": credential_document(),
                "restart_required": restart_required,
            }))
            .unwrap();
            let deletion: AdminCredentialDeleteResponse = serde_json::from_value(json!({
                "credential_id": CREDENTIAL_ID,
                "state_version": 8,
                "restart_required": restart_required,
            }))
            .unwrap();
            assert_eq!(
                decode_credential_data(mutation.clone()).unwrap().restart_required,
                restart_required
            );
            assert_eq!(
                decode_credential_delete(deletion.clone(), CREDENTIAL_ID, 7)
                    .unwrap()
                    .restart_required,
                restart_required
            );
            assert!(decode_credential_delete(deletion, CREDENTIAL_ID, 8).is_err());
            let mut invalid = mutation;
            invalid.credential.status = "unknown".to_owned();
            assert!(decode_credential_data(invalid).is_err());
        }
    }

    #[test]
    fn credential_rename_request_contains_only_the_expected_fields() {
        let mut document = credential_document();
        document["id"] = json!("0f8fad5b-d9cb-469f-a165-70867728950e");
        document["state_version"] = json!(1);
        let previous: AdminCredential = serde_json::from_value(document).unwrap();
        let command_id = Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
        let request = credential_mutation_request(
            command_id,
            &previous,
            "rename",
            Some("新标签".to_owned()),
            None,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "action": "rename",
                "command_id": command_id,
                "credential_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
                "expected_state_version": 1,
                "label": "新标签",
                "status": null,
            })
        );
    }

    #[test]
    fn credential_mutation_requires_matching_version_target_and_action_projection() {
        let previous: AdminCredential = serde_json::from_value(credential_document()).unwrap();
        for (action, label, status) in
            [("rename", Some("New label"), None), ("set_status", None, Some("disabled"))]
        {
            let command = AdminCredentialMutationRequest {
                command_id: Uuid::nil(),
                credential_id: previous.id.parse().unwrap(),
                expected_state_version: 7,
                action: action.to_owned(),
                label: label.map(str::to_owned),
                status: status.map(str::to_owned),
            };
            let mut valid = credential_document();
            valid["state_version"] = json!(8);
            if let Some(label) = label {
                valid["label"] = json!(label);
            }
            if let Some(status) = status {
                valid["status"] = json!(status);
            }
            for (key, value) in [
                ("state_version", json!(7)),
                ("id", json!("22222222-2222-4222-8222-222222222222")),
                ("label", json!("Different")),
                ("status", json!("invalid")),
            ] {
                let mut mismatch = valid.clone();
                mismatch[key] = value;
                assert_eq!(
                    validate_credential_mutation(&command, &previous, mutation_reply(mismatch)),
                    Err(AdminWriteFailure::outcome_unknown("invalid_response")),
                    "{action}: {key}",
                );
            }
            assert_eq!(
                validate_credential_mutation(&command, &previous, mutation_reply(valid.clone())),
                Ok(mutation_reply(valid)),
            );
        }
    }

    fn mutation_reply(credential: Value) -> AdminCredentialMutationResponse {
        serde_json::from_value(json!({ "credential": credential, "restart_required": true }))
            .unwrap()
    }

    fn import_request(command_id: Uuid, forum: bool) -> AdminCredentialCreateRequest {
        AdminCredentialCreateRequest {
            command_id,
            kind: if forum { "forum_cookie" } else { "mail_cookie" }.to_owned(),
            world: if forum { "shared" } else { "main" }.to_owned(),
            label: "测试凭据".to_owned(),
            player_name: None,
            secret: if forum { String::new() } else { "fake-test-cookie=placeholder".to_owned() },
            login: forum.then(|| AdminCredentialLoginRequest {
                username: "fake-test-forum-account".to_owned(),
                password: "fake-test-forum-password".to_owned(),
            }),
        }
    }

    #[test]
    fn credential_create_requires_matching_kind_world_and_player_name() {
        let preparation = CredentialCreatePreparation::new(|id| import_request(id, false));
        let command = preparation.prepare(Uuid::nil()).unwrap();
        let mut mail = credential_document();
        mail["world"] = json!("main");
        mail["label"] = json!("测试凭据");
        mail["status"] = json!("disabled");
        mail["state_version"] = json!(1);
        for (key, value) in [
            ("kind", json!("forum_cookie")),
            ("world", json!("isekai")),
            ("player_name", json!("Other Player")),
        ] {
            let mut mismatch = mail.clone();
            mismatch[key] = value;
            assert_eq!(
                validate_credential_create(command, mutation_reply(mismatch)),
                Err(AdminWriteFailure::outcome_unknown("invalid_response")),
                "{key}",
            );
        }
        assert_eq!(
            validate_credential_create(command, mutation_reply(mail.clone())),
            Ok(mutation_reply(mail.clone())),
        );

        let forum = CredentialCreatePreparation::new(|id| import_request(id, true));
        mail["kind"] = json!("forum_cookie");
        mail["world"] = json!("shared");
        assert_eq!(
            validate_credential_create(
                forum.prepare(Uuid::nil()).unwrap(),
                mutation_reply(mail.clone())
            ),
            Ok(mutation_reply(mail)),
        );
    }

    #[test]
    fn credential_create_reuses_the_same_prepared_object_for_either_source() {
        for forum in [false, true] {
            let command_id = Uuid::from_u128(1);
            let preparation = CredentialCreatePreparation::new(|id| import_request(id, forum));
            let first = preparation.prepare(command_id).unwrap();
            let body = serde_json::to_string(&import_request(command_id, forum)).unwrap();
            assert!(first.body == body);
            assert!(std::ptr::eq(first, preparation.prepare(command_id).unwrap()));
        }
    }

    #[test]
    fn credential_deletion_requires_matching_target_and_frozen_version() {
        for (credential_id, state_version, matches) in [
            (CREDENTIAL_ID, 8, true),
            (CREDENTIAL_ID, 7, false),
            ("22222222-2222-4222-8222-222222222222", 8, false),
        ] {
            let payload = json!({
                "credential_id": credential_id,
                "state_version": state_version,
                "restart_required": true,
            });
            let reply: AdminCredentialDeleteResponse = serde_json::from_value(payload).unwrap();
            assert_eq!(
                decode_credential_delete(reply.clone(), CREDENTIAL_ID, 7),
                if matches { Ok(reply) } else { Err("credential_delete") },
            );
        }
    }

    #[test]
    fn only_shared_mail_check_extends_the_browser_budget() {
        let mut credential: AdminCredential =
            serde_json::from_value(credential_document()).unwrap();
        assert_eq!(credential_check_timeout(&credential), 18_000);
        credential.world = "main".to_owned();
        assert_eq!(
            credential_check_timeout(&credential),
            auction_web_shared::DEFAULT_REQUEST_TIMEOUT_MILLIS
        );
        credential.world = "shared".to_owned();
        credential.kind = "forum_cookie".to_owned();
        assert_eq!(
            credential_check_timeout(&credential),
            auction_web_shared::DEFAULT_REQUEST_TIMEOUT_MILLIS
        );
    }

    #[test]
    fn shared_mail_check_preserves_per_world_inconclusive_results() {
        let credential: AdminCredential = serde_json::from_value(credential_document()).unwrap();
        let mut payload = json!({
            "credential_id": CREDENTIAL_ID,
            "state_version": 7,
            "checked_at": OBSERVED_AT,
            "checks": [
                {"world": "main", "status": "inconclusive"},
                {"world": "isekai", "status": "inconclusive"},
            ],
        });
        let result =
            decode_credential_check(serde_json::from_value(payload.clone()).unwrap(), &credential)
                .unwrap();
        assert_eq!(result.checks.len(), 2);
        assert!(result.checks.iter().all(|check| check.status == "inconclusive"));

        payload["checks"][1]["world"] = json!("main");
        assert!(
            decode_credential_check(serde_json::from_value(payload).unwrap(), &credential).is_err()
        );
    }

    #[test]
    fn credentials_decode_backend_contract_fixtures() {
        use auction_contracts::{AdminCredentialListResponse, ApiResponse};

        use super::decode_credential_list;

        for (body, kind, world, status) in [
            (
                include_str!("../../../auction-contracts/tests/fixtures/admin/credentials.json"),
                "mail_cookie",
                "main",
                "active",
            ),
            (
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/credentials-forum.json"
                ),
                "forum_cookie",
                "shared",
                "disabled",
            ),
        ] {
            let payload: ApiResponse<AdminCredentialListResponse> =
                serde_json::from_str(body).expect("凭据样本");
            let page =
                decode_credential_list(payload.data, kind, world, status, 1, 50).expect("凭据列表");
            assert_eq!(page.credentials[0].player_name.is_some(), kind == "mail_cookie");
            assert!(page.restart_required);
        }
    }
    #[test]
    fn credential_command_contract_fixtures() {
        use super::{
            AdminCredentialDeleteResponse, AdminCredentialMutationResponse, ApiResponse,
            decode_credential_data, decode_credential_delete,
        };
        use crate::api::contract_fixtures::{admin_fixture, decode};

        let payload: ApiResponse<AdminCredentialMutationResponse> =
            decode(admin_fixture!("credential-command"), "/data/credential", "label");
        let result = decode_credential_data(payload.data).expect("凭据修改回执");
        assert_eq!(result.credential.label, "New label");
        assert!(result.restart_required);

        let payload: ApiResponse<AdminCredentialDeleteResponse> =
            decode(admin_fixture!("credential-delete-command"), "/data", "credential_id");
        let result =
            decode_credential_delete(payload.data, "00000000-0000-4000-8000-000000000025", 5)
                .expect("删除回执");
        assert_eq!(result.state_version, 6);
        assert!(result.restart_required);
    }
}
