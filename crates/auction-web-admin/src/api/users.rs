//! Admin 用户管理 API 客户端。
//! 浏览器只保存安全用户投影和当前表单内容；登录 key 的生成结果只在创建/重签响应里短暂
//! 展示，不参与前端状态推导。

use auction_contracts::AdminUserBlacklistHistoryResponse as AdminUserBlacklistHistory;
use auction_contracts::{
    AdminUserBlacklistRequest, AdminUserCreateRequest, AdminUserMutationRequest,
    AdminUserPaginationResponse, AdminUserResponse, AdminUserUnblacklistRequest, ApiResponse,
};
use gloo_net::http::Method;
use serde::Deserialize;
use uuid::Uuid;

use super::AdminWriteFailure;
use super::failure::AdminWriteEndpoint;
use super::transport::AdminResponse;
use super::transport::WriteRequest;
use super::{
    MAX_JSON_SAFE_INTEGER, invalid_request_error, invalid_response_error, parse_admin_time,
    response_error, send_get, valid_admin_time, valid_safe_text, valid_uuid,
};

// 保留镜像的理由：contracts 的 `AdminUserResponse` 被 `AdminUserListItemResponse` 以
// `#[serde(flatten)]` 内嵌，serde 不支持 flatten 与 `deny_unknown_fields` 组合，因此用户
// 列表与写命令 wire 无法由 contracts DTO 承担闭集；下方 Strict 镜像继续保持 fail-closed。
// 同步义务：`AdminUserResponse`/`AdminUserListItemResponse`/`AdminUserBlacklistHistoryResponse`/
// `AdminUserPaginationResponse`/`AdminUserMutationResponse` 字段集变化时必须同步本文件；
// decode 函数中构造 DTO 的穷举字段列表会在编译期捕捉漂移。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUser {
    id: String,
    username: String,
    forum_account_id: Option<String>,
    status: String,
    key_status: String,
    state_version: i64,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictBlacklistHistory {
    public_reason: String,
    blacklisted_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictListedUser {
    id: String,
    username: String,
    forum_account_id: Option<String>,
    status: String,
    key_status: String,
    state_version: i64,
    created_at: String,
    updated_at: String,
    blacklist_count: i64,
    blacklist_history: Vec<StrictBlacklistHistory>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUserPagination {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUserList {
    users: Vec<StrictListedUser>,
    pagination: StrictUserPagination,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUserData {
    user: StrictUser,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictUserDataWithKey {
    user: StrictUser,
    generated_key: String,
}

// 严格 wire 镜像同样遮蔽一次性 Key，不能绕过 contracts 的调试脱敏边界。
impl std::fmt::Debug for StrictUserDataWithKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StrictUserDataWithKey")
            .field("user", &self.user)
            .field("generated_key", &"[REDACTED]")
            .finish()
    }
}

/// 用户列表行的浏览器投影；它把基础用户响应与列表专属黑名单摘要合并，避免 UI 传播 wire 嵌套。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminUser {
    pub id: String,
    pub username: String,
    pub forum_account_id: Option<String>,
    pub status: String,
    pub key_status: String,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub blacklist_count: i64,
    pub blacklist_history: Vec<AdminUserBlacklistHistory>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminUserPage {
    pub users: Vec<AdminUser>,
    pub pagination: auction_contracts::AdminUserPaginationResponse,
}

impl From<AdminUserResponse> for AdminUser {
    fn from(user: AdminUserResponse) -> Self {
        Self {
            id: user.id,
            username: user.username,
            forum_account_id: user.forum_account_id,
            status: user.status,
            key_status: user.key_status,
            state_version: user.state_version,
            created_at: user.created_at,
            updated_at: user.updated_at,
            blacklist_count: 0,
            blacklist_history: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminUserStatusFilter {
    All,
    Active,
    Blacklisted,
}

impl AdminUserStatusFilter {
    const fn as_query(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Active => Some("active"),
            Self::Blacklisted => Some("blacklisted"),
        }
    }
}

/// 用户写命令的浏览器结果；基础用户 wire 响应在边界转换为列表行投影。
/// 调试格式只保留是否生成 Key，不输出认证材料。
#[derive(Clone, PartialEq)]
pub(crate) struct AdminUserMutationResult {
    pub user: AdminUser,
    pub generated_key: Option<String>,
}

impl std::fmt::Debug for AdminUserMutationResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminUserMutationResult")
            .field("user", &self.user)
            .field("generated_key", &self.generated_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// 获取 Admin 用户列表。
pub(crate) async fn fetch_admin_users(
    name: String,
    status: AdminUserStatusFilter,
    page: i64,
    page_size: i64,
) -> Result<AdminUserPage, String> {
    super::pagination::validate(page, page_size, 200, i64::from(i32::MAX))?;
    let mut url = format!("/api/v1/admin/users?page={page}&page_size={page_size}");
    if !name.is_empty() {
        append_query(&mut url, "name", &name);
    }
    if let Some(status) = status.as_query() {
        append_query(&mut url, "status", status);
    }
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictUserList> =
        response.json().await.map_err(invalid_response_error)?;
    let result = decode_user_list(payload.data).map_err(invalid_response_error)?;
    let pagination = &result.pagination;
    if !super::pagination::valid_page_echo(
        page,
        page_size,
        pagination.page,
        pagination.page_size,
        pagination.total,
        result.users.len(),
    ) {
        return Err(invalid_response_error("user_pagination"));
    }
    Ok(result)
}

/// 创建用户；`key_mode = generated` 时返回一次性 key。
pub(crate) async fn create_admin_user(
    command_id: Uuid,
    username: String,
    forum_account_id: Option<String>,
    key_mode: &'static str,
    key: Option<String>,
) -> Result<AdminUserMutationResult, AdminWriteFailure> {
    let command = AdminUserCreateRequest {
        command_id: command_id.to_string(),
        username,
        forum_account_id,
        key_mode: key_mode.to_owned(),
        key,
    };
    let response =
        WriteRequest::general("/api/v1/admin/users", Method::POST).send_json(&command).await?;
    let result = decode_user_mutation(response, key_mode == "generated").await?;
    validate_user_create_receipt(&command, result)
}

fn validate_user_create_receipt(
    command: &AdminUserCreateRequest,
    result: AdminUserMutationResult,
) -> Result<AdminUserMutationResult, AdminWriteFailure> {
    // 创建没有预知的用户 ID；核对冻结表单和初始投影后才可交付 Key。
    if result.user.username != command.username
        || result.user.forum_account_id != command.forum_account_id
        || result.user.status != "active"
        || result.user.key_status != "active"
        || result.user.state_version != 1
    {
        return Err(AdminWriteFailure::invalid_response("user_create_receipt"));
    }
    Ok(result)
}

/// 修改用户名、论坛账号 ID 或登录 key 状态。
pub(crate) async fn mutate_admin_user(
    command_id: Uuid,
    user: &AdminUser,
    action: &'static str,
    username: Option<String>,
    forum_account_id: Option<String>,
) -> Result<AdminUserMutationResult, AdminWriteFailure> {
    let user_id = Uuid::parse_str(&user.id)
        .map_err(|error| AdminWriteFailure::not_sent(invalid_request_error(error)))?;
    let command = AdminUserMutationRequest {
        command_id: command_id.to_string(),
        user_id: user_id.to_string(),
        expected_state_version: user.state_version,
        action: action.to_owned(),
        username,
        forum_account_id,
    };
    #[cfg(test)]
    if let Some(respond) = dioxus::prelude::try_consume_context::<tests::UserMutationResponseStub>()
    {
        return decode_user_mutation_body(respond(&command), action == "reissue_key");
    }
    let response =
        WriteRequest::new("/api/v1/admin/users", Method::PATCH, AdminWriteEndpoint::UserMutation)
            .send_json(&command)
            .await?;
    let result = decode_user_mutation(response, action == "reissue_key").await?;
    let user = &result.user;
    let matches_action = match action {
        "rename" => command.username.as_deref() == Some(user.username.as_str()),
        "set_forum_account_id" => {
            command.forum_account_id.is_some() && user.forum_account_id == command.forum_account_id
        }
        "revoke_key" => user.key_status == "revoked",
        "reissue_key" => user.key_status == "active",
        _ => false,
    };
    if !matches_action
        || !matches_user_mutation_target(user, &command.user_id, command.expected_state_version)
    {
        return Err(AdminWriteFailure::invalid_response("user_mutation_receipt"));
    }
    Ok(result)
}

/// 将用户加入黑名单。
pub(crate) async fn blacklist_admin_user(
    command_id: Uuid,
    user: &AdminUser,
    public_reason: String,
) -> Result<AdminUser, AdminWriteFailure> {
    let user_id = Uuid::parse_str(&user.id)
        .map_err(|error| AdminWriteFailure::not_sent(invalid_request_error(error)))?
        .to_string();
    let url = format!("/api/v1/admin/users/{user_id}/blacklist");
    let command = AdminUserBlacklistRequest {
        command_id: command_id.to_string(),
        expected_state_version: user.state_version,
        public_reason,
    };
    let response = WriteRequest::new(&url, Method::PUT, AdminWriteEndpoint::UserMutation)
        .send_json(&command)
        .await?;
    let user = decode_user_mutation(response, false).await?.user;
    if user.status != "blacklisted"
        || !matches_user_mutation_target(&user, &user_id, command.expected_state_version)
    {
        return Err(AdminWriteFailure::invalid_response("user_blacklist_receipt"));
    }
    Ok(user)
}

/// 将用户移出黑名单。
pub(crate) async fn unblacklist_admin_user(
    command_id: Uuid,
    user: &AdminUser,
) -> Result<AdminUser, AdminWriteFailure> {
    let user_id = Uuid::parse_str(&user.id)
        .map_err(|error| AdminWriteFailure::not_sent(invalid_request_error(error)))?
        .to_string();
    let url = format!("/api/v1/admin/users/{user_id}/blacklist");
    let command = AdminUserUnblacklistRequest {
        command_id: command_id.to_string(),
        expected_state_version: user.state_version,
    };
    let response = WriteRequest::new(&url, Method::DELETE, AdminWriteEndpoint::UserMutation)
        .send_json(&command)
        .await?;
    let user = decode_user_mutation(response, false).await?.user;
    if user.status != "active"
        || !matches_user_mutation_target(&user, &user_id, command.expected_state_version)
    {
        return Err(AdminWriteFailure::invalid_response("user_unblacklist_receipt"));
    }
    Ok(user)
}

/// 写回执只可确认提交时冻结的用户与紧邻版本；错配时保留命令供原样重试。
pub(super) fn matches_user_mutation_target(
    user: &AdminUser,
    user_id: &str,
    expected_state_version: i64,
) -> bool {
    user.id == user_id
        && expected_state_version > 0
        && expected_state_version.checked_add(1) == Some(user.state_version)
}

fn decode_user_list(data: StrictUserList) -> Result<AdminUserPage, &'static str> {
    let users = data.users.into_iter().map(decode_listed_user).collect::<Result<Vec<_>, _>>()?;
    let pagination = decode_pagination(data.pagination)?;
    Ok(AdminUserPage { users, pagination })
}

/// 按命令类型解码含 Key 或无 Key 的闭集回执，调用方须再核对冻结字段。
pub(super) async fn decode_user_mutation(
    response: AdminResponse,
    want_key: bool,
) -> Result<AdminUserMutationResult, AdminWriteFailure> {
    let body = response.text().await.map_err(AdminWriteFailure::invalid_response)?;
    decode_user_mutation_body(&body, want_key)
}

fn decode_user_mutation_body(
    body: &str,
    want_key: bool,
) -> Result<AdminUserMutationResult, AdminWriteFailure> {
    // 直接从响应正文解码，已定义字段重复时保持结果未知，不能交付有歧义的一次性 Key。
    let (user, generated_key) = if want_key {
        let payload: ApiResponse<StrictUserDataWithKey> =
            serde_json::from_str(body).map_err(AdminWriteFailure::invalid_response)?;
        let data = payload.data;
        if !valid_generated_key(&data.generated_key) {
            return Err(AdminWriteFailure::outcome_unknown("invalid_response"));
        }
        (data.user, Some(data.generated_key))
    } else {
        let payload: ApiResponse<StrictUserData> =
            serde_json::from_str(body).map_err(AdminWriteFailure::invalid_response)?;
        (payload.data.user, None)
    };
    Ok(AdminUserMutationResult {
        user: decode_user(user).map_err(AdminWriteFailure::invalid_response)?.into(),
        generated_key,
    })
}

fn decode_listed_user(value: StrictListedUser) -> Result<AdminUser, &'static str> {
    let history_length = usize::try_from(value.blacklist_count.min(10)).map_err(|_| "user")?;
    if !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.blacklist_count)
        || value.blacklist_history.len() != history_length
    {
        return Err("user");
    }
    let history = value
        .blacklist_history
        .into_iter()
        .map(decode_blacklist_history)
        .collect::<Result<Vec<_>, _>>()?;
    if history.windows(2).any(|pair| {
        parse_admin_time(&pair[1].blacklisted_at) > parse_admin_time(&pair[0].blacklisted_at)
    }) {
        return Err("user_history_order");
    }
    let user = decode_user(StrictUser {
        id: value.id,
        username: value.username,
        forum_account_id: value.forum_account_id,
        status: value.status,
        key_status: value.key_status,
        state_version: value.state_version,
        created_at: value.created_at,
        updated_at: value.updated_at,
    })?;
    let mut user: AdminUser = user.into();
    user.blacklist_count = value.blacklist_count;
    user.blacklist_history = history;
    Ok(user)
}

fn decode_user(value: StrictUser) -> Result<AdminUserResponse, &'static str> {
    if !valid_uuid(&value.id)
        || !valid_safe_text(&value.username, 256)
        || value
            .forum_account_id
            .as_deref()
            .is_some_and(|account_id| !valid_forum_account_id(account_id))
        || !matches!(value.status.as_str(), "active" | "blacklisted")
        || !matches!(value.key_status.as_str(), "active" | "revoked")
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.state_version)
        || !valid_admin_time(&value.created_at)
        || !valid_admin_time(&value.updated_at)
    {
        return Err("user");
    }
    Ok(AdminUserResponse {
        id: value.id,
        username: value.username,
        forum_account_id: value.forum_account_id,
        status: value.status,
        key_status: value.key_status,
        state_version: value.state_version,
        created_at: value.created_at,
        updated_at: value.updated_at,
    })
}

fn valid_forum_account_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0] != b'0'
        && bytes.iter().all(u8::is_ascii_digit)
}

fn decode_blacklist_history(
    value: StrictBlacklistHistory,
) -> Result<AdminUserBlacklistHistory, &'static str> {
    if !valid_safe_text(&value.public_reason, 1024) || !valid_admin_time(&value.blacklisted_at) {
        return Err("blacklist_history");
    }
    Ok(AdminUserBlacklistHistory {
        public_reason: value.public_reason,
        blacklisted_at: value.blacklisted_at,
    })
}

fn decode_pagination(
    value: StrictUserPagination,
) -> Result<AdminUserPaginationResponse, &'static str> {
    if !crate::api::pagination::valid_response(value.page, value.page_size, value.total) {
        return Err("pagination");
    }
    Ok(AdminUserPaginationResponse {
        page: value.page,
        page_size: value.page_size,
        total: value.total,
    })
}

fn valid_generated_key(value: &str) -> bool {
    (32..=256).contains(&value.len()) && !value.chars().any(char::is_control)
}

fn append_query(url: &mut String, name: &str, value: &str) {
    url.push('&');
    url.push_str(name);
    url.push('=');
    url.push_str(&url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>());
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    use auction_contracts::{AdminUserCreateRequest, AdminUserMutationRequest, ApiResponse};
    use dioxus::prelude::{ScopeId, VNode, VirtualDom, provide_context};
    use gloo_net::http::Method;
    use uuid::Uuid;

    use super::{
        AdminUser, StrictUserList, blacklist_admin_user, create_admin_user, decode_user_list,
        decode_user_mutation_body, matches_user_mutation_target, mutate_admin_user,
        unblacklist_admin_user, validate_user_create_receipt,
    };
    use crate::api::contract_fixtures::admin_fixture;
    use crate::api::{AdminApiFailure, AdminWriteFailure, Rejection};

    pub(super) type UserMutationResponseStub =
        Rc<dyn Fn(&AdminUserMutationRequest) -> &'static str>;

    impl AdminApiFailure {
        /// 为宿主用户行提供已经核对目标的成功正文，沿用正常的解码。
        pub(crate) fn stub_user_mutation_success(
            respond: impl Fn(&AdminUserMutationRequest) -> &'static str + 'static,
        ) {
            provide_context(Rc::new(respond) as UserMutationResponseStub);
        }
    }

    const USER_ID: &str = "00000000-0000-4000-8000-000000000002";
    const OTHER_ID: &str = "00000000-0000-4000-8000-000000000004";
    const CONFLICT: &str = r#"{"code":"state_conflict","message":"用户版本已变化","data":{"current_state_version":4},"request_id":null}"#;

    fn target() -> AdminUser {
        AdminUser {
            id: USER_ID.into(),
            username: "before".into(),
            forum_account_id: None,
            status: "active".into(),
            key_status: "active".into(),
            state_version: 3,
            created_at: "2026-11-14T00:00:00Z".into(),
            updated_at: "2026-11-14T00:00:00Z".into(),
            blacklist_count: 0,
            blacklist_history: Vec::new(),
        }
    }

    #[test]
    fn user_write_target_requires_the_frozen_id_and_next_version() {
        let mut echoed = target();
        echoed.state_version = 4;
        assert!(matches_user_mutation_target(&echoed, USER_ID, 3));
        assert!(!matches_user_mutation_target(&echoed, OTHER_ID, 3));
        assert!(!matches_user_mutation_target(&echoed, USER_ID, 4));
        assert!(!matches_user_mutation_target(&echoed, USER_ID, 2));
    }

    #[test]
    fn generated_user_key_requires_exactly_one_id_version_and_key() {
        let text = admin_fixture!("user-create-command");
        let result = decode_user_mutation_body(text, true).unwrap();
        assert_eq!(result.user.username, "ExamplePlayer");
        assert_eq!(result.user.state_version, 1);
        assert_eq!(result.generated_key, Some("A".repeat(43)));

        for (field, extra) in [
            ("id", format!("\"id\":\"{OTHER_ID}\",")),
            ("state_version", String::from("\"state_version\":7,")),
            ("generated_key", format!("\"generated_key\":\"{}\",", "B".repeat(43))),
        ] {
            let key = format!("\"{field}\"");
            let duplicate = text.replacen(&key, &format!("{extra}{key}"), 1);
            assert_ne!(duplicate, text);
            assert_invalid_user_receipt(decode_user_mutation_body(&duplicate, true).unwrap_err());
        }

        let mut keyless: serde_json::Value = serde_json::from_str(text).unwrap();
        keyless["data"].as_object_mut().unwrap().remove("generated_key");
        let keyless = keyless.to_string();
        assert_invalid_user_receipt(decode_user_mutation_body(&keyless, true).unwrap_err());
        let manual = decode_user_mutation_body(&keyless, false).unwrap();
        assert_eq!(manual.user.username, "ExamplePlayer");
        assert_eq!(manual.generated_key, None);
        assert_invalid_user_receipt(decode_user_mutation_body(text, false).unwrap_err());
    }

    fn assert_invalid_user_receipt(failure: AdminWriteFailure) {
        assert!(matches!(failure.classify(), Rejection::Unknown), "{failure:?}");
        assert_eq!(failure.display_code(), "invalid_response");
    }

    #[test]
    fn user_create_receipt_delivers_key_only_for_the_frozen_username() {
        let text = admin_fixture!("user-create-command");
        let command = AdminUserCreateRequest {
            command_id: "00000000-0000-4000-8000-000000000003".into(),
            username: "ExamplePlayer".into(),
            forum_account_id: None,
            key_mode: "generated".into(),
            key: None,
        };
        let checked = decode_user_mutation_body(text, true)
            .and_then(|result| validate_user_create_receipt(&command, result))
            .unwrap();
        assert_eq!(checked.generated_key, Some("A".repeat(43)));

        let other_user = text.replace("\"ExamplePlayer\"", "\"OtherPlayer\"");
        let decoded = decode_user_mutation_body(&other_user, true).unwrap();
        assert_eq!(decoded.user.username, "OtherPlayer");
        assert_invalid_user_receipt(validate_user_create_receipt(&command, decoded).unwrap_err());
    }

    fn assert_write_rejection<T>(
        method: Method,
        path: &'static str,
        body: &'static str,
        status: u16,
        expected: (Rejection, &str),
        request: impl Future<Output = Result<T, AdminWriteFailure>>,
    ) {
        let mut dom = VirtualDom::new(VNode::empty);
        dom.rebuild_in_place();
        dom.in_scope(ScopeId::APP, || {
            AdminApiFailure::stub_write_response(path, method, status, body);
            let mut request = pin!(request);
            let Poll::Ready(Err(failure)) =
                request.as_mut().poll(&mut Context::from_waker(Waker::noop()))
            else {
                panic!("用户写入应经传输替身返回拒绝回执");
            };
            assert!(failure.classify() == expected.0, "{failure:?}");
            assert_eq!(failure.display_code(), expected.1);
        });
    }

    #[test]
    fn user_patch_and_blacklist_replay_only_verified_409_state_conflict() {
        let target = target();
        let command_id = Uuid::parse_str("00000000-0000-4000-8000-000000000003").unwrap();
        let path = "/api/v1/admin/users/00000000-0000-4000-8000-000000000002/blacklist";
        for (status, rejection, code) in [
            (409, Rejection::ReplayChecked, "state_conflict"),
            (422, Rejection::Unknown, "http_422"),
        ] {
            for method in [Method::PATCH, Method::PUT, Method::DELETE] {
                match method {
                    Method::PATCH => assert_write_rejection(
                        method,
                        "/api/v1/admin/users",
                        CONFLICT,
                        status,
                        (rejection, code),
                        mutate_admin_user(
                            command_id,
                            &target,
                            "rename",
                            Some("after".into()),
                            None,
                        ),
                    ),
                    Method::PUT => assert_write_rejection(
                        method,
                        path,
                        CONFLICT,
                        status,
                        (rejection, code),
                        blacklist_admin_user(command_id, &target, "reason".into()),
                    ),
                    Method::DELETE => assert_write_rejection(
                        method,
                        path,
                        CONFLICT,
                        status,
                        (rejection, code),
                        unblacklist_admin_user(command_id, &target),
                    ),
                    _ => unreachable!("只测试用户修改及黑名单"),
                }
            }
        }
        assert_write_rejection(
            Method::POST,
            "/api/v1/admin/users",
            CONFLICT,
            409,
            (Rejection::Unknown, "state_conflict"),
            create_admin_user(command_id, "before".into(), Some("42".into()), "generated", None),
        );
        assert_write_rejection(
            Method::POST,
            "/api/v1/admin/users",
            r#"{"code":"user_conflict","message":"用户已存在","data":null,"request_id":null}"#,
            409,
            (Rejection::ThisAttempt, "user_conflict"),
            create_admin_user(command_id, "before".into(), Some("42".into()), "generated", None),
        );
    }

    #[test]
    fn users_decode_backend_contract_fixtures() {
        for (body, count) in [
            (include_str!("../../../auction-contracts/tests/fixtures/admin/users.json"), 0),
            (
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/users-blacklisted.json"
                ),
                1,
            ),
        ] {
            let payload: ApiResponse<StrictUserList> =
                serde_json::from_str(body).expect("用户样本");
            let page = decode_user_list(payload.data).expect("用户列表");
            assert_eq!(page.users[0].blacklist_history.len(), count);
        }
    }
    #[test]
    fn rename_revert_contract_fixture() {
        use super::{ApiResponse, StrictUserData, decode_user};
        use crate::api::contract_fixtures::{admin_fixture, decode};

        let payload: ApiResponse<StrictUserData> =
            decode(admin_fixture!("rename-revert-command"), "/data/user", "username");
        let user = decode_user(payload.data.user).expect("改名回滚回执");
        assert_eq!(user.username, "Previous");
        assert_eq!(user.state_version, 4);
    }

    #[test]
    fn generated_user_key_receipt_contract_fixture() {
        use super::{ApiResponse, StrictUserDataWithKey, decode_user, valid_generated_key};
        use crate::api::contract_fixtures::{admin_fixture, decode};

        let payload: ApiResponse<StrictUserDataWithKey> =
            decode(admin_fixture!("user-create-command"), "/data", "generated_key");
        let user = decode_user(payload.data.user).expect("创建用户回执");
        assert_eq!(payload.code, "created");
        assert_eq!(user.username, "ExamplePlayer");
        assert_eq!(user.state_version, 1);
        assert!(valid_generated_key(&payload.data.generated_key));
        assert_eq!(payload.data.generated_key, "A".repeat(43));
    }
}
