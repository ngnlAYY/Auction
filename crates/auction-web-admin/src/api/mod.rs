//! Admin 浏览器端的同源 HTTP 数据访问。
//!
//! 浏览器只接收后端安全投影。人工动作携带由页面按输入复用的 `command_id` 和对应版本 fence；
//! 服务端仍负责最终的状态校验、幂等和审计。所有 wire DTO 统一由 `auction-contracts` 拥有，
//! 本 crate 只保留浏览器表单、筛选和交互状态。

#[path = "auction/auctions.rs"]
mod auctions;
#[path = "operations/audit.rs"]
mod audit;
#[path = "identity/credentials.rs"]
mod credentials;
#[path = "operations/dashboard.rs"]
mod dashboard;
#[path = "auction/intake.rs"]
mod intake;
#[path = "auction/lifecycle.rs"]
mod lifecycle;
#[path = "auction/lots.rs"]
mod lots;
#[path = "fulfillment/mail.rs"]
mod mail;
#[path = "auction/proxy_events.rs"]
mod proxy_events;
#[path = "operations/realtime.rs"]
mod realtime;
#[path = "auction/recovery.rs"]
mod recovery;
#[path = "identity/renames.rs"]
mod renames;
#[path = "content/rules.rs"]
mod rules;
#[path = "operations/runtime_config.rs"]
mod runtime_config;
#[path = "identity/session.rs"]
mod session;
#[path = "auction/settlement.rs"]
mod settlement;
#[path = "operations/status.rs"]
mod status;
#[path = "identity/users.rs"]
mod users;

pub(crate) use auctions::{
    AdminAuction, AdminAuctionCapabilities, AdminAuctionPage, create_admin_auction,
    fetch_admin_auctions, update_admin_auction_schedule,
};
pub(crate) use audit::{AdminAuditFilter, AdminAuditPage, AdminAuditRecord, fetch_admin_audit};
pub(crate) use credentials::{
    AdminCredential, AdminCredentialPage, create_admin_credential, delete_admin_credential,
    fetch_admin_credentials, mutate_admin_credential,
};
pub(crate) use dashboard::{AdminDashboardSnapshot, fetch_dashboard};
pub(crate) use intake::{AdminIntakeResult, add_admin_intake};
pub(crate) use lifecycle::{AdminAuctionLifecycleAction, execute_admin_auction_lifecycle};
pub(crate) use lots::{
    AdminLot, AdminLotPage, AdminLotUpdateAction, fetch_admin_lots, update_admin_lot,
};
pub(crate) use mail::{
    AdminMailIntake, AdminMailIntakeDetail, AdminMailIntakePage, AdminMailIntakeQuery,
    AdminMailPolling, fetch_mail_intake_detail, fetch_mail_intakes, fetch_mail_state,
    set_mail_paused, trigger_mail_poll,
};
pub(crate) use proxy_events::{AdminProxyEvent, AdminProxyEventPage, fetch_admin_proxy_events};
pub(crate) use realtime::{
    RealtimeEventPage, RealtimeFailure, RealtimeFailurePage, RealtimeLagPage,
    fetch_realtime_event_page, fetch_realtime_event_total, fetch_realtime_failure_page,
    fetch_realtime_lag_page, replay_realtime_failure,
};
pub(crate) use recovery::{
    AdminRecovery, AdminRecoveryAction, AdminRecoveryReport, confirm_admin_recovery,
    fetch_admin_recovery,
};
pub(crate) use renames::{AdminRename, AdminRenamePage, fetch_admin_renames, revert_admin_rename};
pub(crate) use rules::{
    AdminRules, AdminRulesCandidate, fetch_admin_rules, publish_admin_rules, validate_admin_rules,
};
pub(crate) use runtime_config::{
    fetch_admin_runtime_config, update_admin_runtime_config, validate_admin_runtime_config,
};
pub(crate) use session::{AdminSession, fetch_admin_session, login_admin, logout_admin};
pub(crate) use settlement::{
    AdminSettlementLot, AdminSettlementReport, AdminSettlementSummary, fetch_admin_settlement,
};
pub(crate) use status::{AdminReadinessState, fetch_admin_readiness};
pub(crate) use users::{
    AdminUser, AdminUserPage, AdminUserStatusFilter, blacklist_admin_user, create_admin_user,
    fetch_admin_users, mutate_admin_user, unblacklist_admin_user,
};

use auction_contracts::{AdminDeliveryCommandRequest, AdminSessionResponse, ApiResponse};
pub(crate) use auction_contracts::{
    AdminDeliveryListResponse as DeliveryPage, AdminDeliveryResponse as Delivery,
};
use chrono::{DateTime, Utc};
#[cfg(target_arch = "wasm32")]
use futures_util::{
    future::{Either, select},
    pin_mut,
};
use gloo_net::http::{Request, Response};
#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;
use serde::Deserialize;
use uuid::Uuid;

/// 旧版 Admin 所有表格列表请求共用的页大小。
pub(crate) const TABLE_PAGE_SIZE: i64 = 10;

/// Admin 浏览器请求的稳定失败投影。
///
/// 服务端正文中的自然语言不能直接进入界面；页面只消费稳定 `code`，再通过 Fluent 映射为
/// 当前语言。HTTP 状态仍单独保留，供 404、409 等需要改变交互流程的页面判断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminApiFailure {
    pub status: Option<u16>,
    pub code: String,
}

/// Admin 错误 envelope 的浏览器侧兼容模型。
///
/// 旧版要求 `code`、`message`、`data` 三个字段存在，但允许服务端增加扩展字段，也允许任意
/// 字符串错误码。错误数据不进入页面，仍以不透明 JSON 保留，仅为兼容既有 envelope 形状；
/// 成功响应继续使用各自严格的路由专用 decoder。
#[derive(Debug, Deserialize)]
struct AdminApiErrorResponse {
    code: String,
    message: String,
    data: serde_json::Value,
}

impl AdminApiErrorResponse {
    fn into_code(self) -> String {
        let Self { code, message, data } = self;
        // 错误正文和不透明 data 只用于完成旧 wire 解码，不能进入浏览器状态或界面。
        let _ = (message, data);
        code
    }
}

impl AdminApiFailure {
    pub(crate) fn client(code: impl ToString) -> Self {
        Self { status: None, code: code.to_string() }
    }

    pub(crate) const fn is_conflict(&self) -> bool {
        matches!(self.status, Some(409))
    }
}

/// 将同源错误 envelope 缩减为稳定错误码；正文无效时只保留 HTTP 状态兜底。
pub(crate) async fn response_failure(response: Response) -> AdminApiFailure {
    let status = response.status();
    let code = response
        .json::<AdminApiErrorResponse>()
        .await
        .ok()
        .map(AdminApiErrorResponse::into_code)
        .unwrap_or_else(|| format!("http_{status}"));
    AdminApiFailure { status: Some(status), code }
}

/// `Result<_, String>` API 只返回稳定错误码，兼容现有页面状态而不泄漏底层错误正文。
pub(crate) async fn response_error(response: Response) -> String {
    response_failure(response).await.code
}

pub(crate) fn network_error(_error: impl ToString) -> String {
    "network_error".to_owned()
}

/// 发送不改写 URL 的 Admin GET 请求。
///
/// gloo-net 0.6.0 会把自身的空 QueryParams 拼到已有 query 后，产生 `...?a=b&`。旧版直接使用
/// 浏览器 fetch，因此在共用 GET owner 建立原生请求，再交回 gloo-net 读取响应。
pub(crate) async fn send_get(url: &str) -> Result<Response, String> {
    send_request(|abort_signal| {
        let options = web_sys::RequestInit::new();
        options.set_method("GET");
        options.set_signal(abort_signal);
        web_sys::Request::new_with_str_and_init(url, &options)
            .map(Request::from)
            .map_err(|_| "network_error".to_owned())
    })
    .await
}

const ADMIN_REQUEST_TIMEOUT_MILLIS: u32 = 10_000;

/// 在旧版默认的 10 秒上限内发送一次 Admin 请求；超时后取消浏览器 fetch，避免迟到响应改写页面。
pub(crate) async fn send_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
) -> Result<Response, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let controller = web_sys::AbortController::new().map_err(|_| "network_error".to_owned())?;
        let signal = controller.signal();
        let request = build(Some(&signal))?;
        let send = request.send();
        let timeout = TimeoutFuture::new(ADMIN_REQUEST_TIMEOUT_MILLIS);
        pin_mut!(send, timeout);
        return match select(send, timeout).await {
            Either::Left((response, _)) => response.map_err(network_error),
            Either::Right((_, _)) => {
                controller.abort();
                Err("request_timeout".to_owned())
            }
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = ADMIN_REQUEST_TIMEOUT_MILLIS;
        build(None)?.send().await.map_err(network_error)
    }
}

pub(crate) fn invalid_request_error(_error: impl ToString) -> String {
    "invalid_request".to_owned()
}

pub(crate) fn invalid_response_error(_error: impl ToString) -> String {
    "invalid_response".to_owned()
}

/// 让 JSON `null` 可映射为 `None`，同时仍要求响应中显式存在该字段。
#[derive(Debug)]
pub(crate) struct RequiredOption<T>(pub(crate) Option<T>);

impl<'de, T> Deserialize<'de> for RequiredOption<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Self)
    }
}

/// 表示可以缺失、但一旦出现就不能是 JSON `null` 的旧 wire 字段。
#[derive(Debug)]
pub(crate) struct OptionalField<T>(pub(crate) Option<T>);

impl<T> Default for OptionalField<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<'de, T> Deserialize<'de> for OptionalField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        T::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}

/// 对齐旧浏览器 `Date.parse` 的时间接受边界；非 WASM 分支只用于本地静态编译。
pub(crate) fn parse_admin_time(value: &str) -> Option<f64> {
    if value.is_empty() {
        return None;
    }

    #[cfg(target_arch = "wasm32")]
    {
        let timestamp = js_sys::Date::parse(value);
        return timestamp.is_finite().then_some(timestamp);
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        DateTime::parse_from_rfc3339(value).ok().map(|value| value.timestamp_millis() as f64)
    }
}

pub(crate) fn valid_admin_time(value: &str) -> bool {
    parse_admin_time(value).is_some()
}

pub(crate) fn valid_trimmed_text(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

pub(crate) fn valid_safe_text(value: &str, maximum_bytes: usize) -> bool {
    valid_trimmed_text(value)
        && value.len() <= maximum_bytes
        && !value.chars().any(char::is_control)
}

/// Delivery 路由专用的成功响应外壳；不能用宽松的共享 DTO 接受未知字段。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictDeliveryEnvelope<T> {
    code: String,
    message: String,
    data: T,
}

/// Delivery 路由专用的安全投影；可选元数据字段允许服务端按旧契约省略。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictDeliveryResponse {
    id: String,
    origin_kind: String,
    origin_id: String,
    recipient: String,
    cod: Option<i64>,
    status: String,
    lease_expires_at: Option<String>,
    fencing_version: i64,
    attempt_count: u32,
    claimed_at: Option<String>,
    sending_at: Option<String>,
    completed_at: Option<String>,
    external_reference: Option<String>,
    failure_code: Option<String>,
    #[serde(default)]
    reason_code: Option<String>,
    #[serde(default)]
    take_required: bool,
    #[serde(default)]
    returned_mail_id: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictDeliveryListResponse {
    deliveries: Vec<StrictDeliveryResponse>,
    pagination: StrictDeliveryPaginationResponse,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictDeliveryPaginationResponse {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictDeliveryCommandResponse {
    delivery: StrictDeliveryResponse,
}

pub(crate) const MAX_JSON_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const MAX_DELIVERY_PAGE_SIZE: i64 = 100;

fn valid_success_code(code: &str) -> bool {
    matches!(code, "ok" | "created" | "accepted")
}

fn decode_delivery_list(
    payload: StrictDeliveryEnvelope<StrictDeliveryListResponse>,
) -> Result<DeliveryPage, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }

    let pagination = payload.data.pagination;
    if pagination.page <= 0
        || pagination.page > MAX_JSON_SAFE_INTEGER
        || pagination.page_size <= 0
        || pagination.page_size > MAX_DELIVERY_PAGE_SIZE
        || pagination.total < 0
        || pagination.total > MAX_JSON_SAFE_INTEGER
    {
        return Err(());
    }

    let deliveries =
        payload.data.deliveries.into_iter().map(decode_delivery).collect::<Result<Vec<_>, _>>()?;
    if deliveries.len() as i64 > pagination.page_size {
        return Err(());
    }

    Ok(DeliveryPage {
        deliveries,
        pagination: auction_contracts::AdminDeliveryPaginationResponse {
            page: pagination.page,
            page_size: pagination.page_size,
            total: pagination.total,
        },
    })
}

fn decode_delivery_command(
    payload: StrictDeliveryEnvelope<StrictDeliveryCommandResponse>,
    requested_delivery_id: &str,
    action: DeliveryAction,
    expected_fencing_version: i64,
) -> Result<Delivery, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }

    let delivery = decode_delivery(payload.data.delivery)?;
    if delivery.id != requested_delivery_id
        || delivery.fencing_version <= expected_fencing_version
        || delivery.status
            != match action {
                DeliveryAction::Release
                | DeliveryAction::Retry
                | DeliveryAction::ReconcileRetry => "pending",
                DeliveryAction::ReconcileSucceeded => "succeeded",
                DeliveryAction::ReconcileFailed => "failed",
            }
    {
        return Err(());
    }
    Ok(delivery)
}

fn decode_delivery(value: StrictDeliveryResponse) -> Result<Delivery, ()> {
    let id = valid_uuid(&value.id).then_some(value.id.clone()).ok_or(())?;
    let created_at = parse_delivery_time(&value.created_at).ok_or(())?;
    let updated_at = parse_delivery_time(&value.updated_at).ok_or(())?;
    let lease_expires_at = parse_optional_delivery_time(value.lease_expires_at.as_ref())?;
    let claimed_at = parse_optional_delivery_time(value.claimed_at.as_ref())?;
    let sending_at = parse_optional_delivery_time(value.sending_at.as_ref())?;
    let completed_at = parse_optional_delivery_time(value.completed_at.as_ref())?;

    if !valid_origin_kind(&value.origin_kind)
        || !valid_stored_text(&value.origin_id)
        || !valid_stored_text(&value.recipient)
        || !valid_amount(value.cod)
        || !valid_status(&value.status)
        || value.fencing_version < 0
        || value.fencing_version > MAX_JSON_SAFE_INTEGER
        || i64::from(value.attempt_count) > MAX_JSON_SAFE_INTEGER
        || is_zero_delivery_time(created_at)
        || is_zero_delivery_time(updated_at)
        || updated_at < created_at
        || lease_expires_at.is_some_and(is_zero_delivery_time)
        || !valid_timeline(created_at, claimed_at, sending_at, completed_at)
        || !valid_optional_text(value.external_reference.as_deref())
        || !valid_optional_text(value.failure_code.as_deref())
        || !valid_optional_text(value.reason_code.as_deref())
        || !valid_returned_mail_id(value.returned_mail_id.as_deref())
        || !valid_state_fields(&value, lease_expires_at, claimed_at, sending_at, completed_at)
    {
        return Err(());
    }

    Ok(Delivery {
        id,
        origin_kind: value.origin_kind,
        origin_id: value.origin_id,
        recipient: value.recipient,
        cod: value.cod,
        status: value.status,
        lease_expires_at: value.lease_expires_at,
        fencing_version: value.fencing_version,
        attempt_count: value.attempt_count,
        claimed_at: value.claimed_at,
        sending_at: value.sending_at,
        completed_at: value.completed_at,
        external_reference: value.external_reference,
        failure_code: value.failure_code,
        reason_code: value.reason_code,
        take_required: value.take_required,
        returned_mail_id: value.returned_mail_id,
        created_at: value.created_at,
        updated_at: value.updated_at,
    })
}

pub(crate) fn valid_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].into_iter().all(|index| bytes[index] == b'-')
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23)
                || byte.is_ascii_digit()
                || (b'a'..=b'f').contains(byte)
        })
        && (b'1'..=b'5').contains(&bytes[14])
        && matches!(bytes[19], b'8'..=b'9' | b'a'..=b'b')
}

fn parse_delivery_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value).ok().map(|value| value.with_timezone(&Utc))
}

fn parse_optional_delivery_time(value: Option<&String>) -> Result<Option<DateTime<Utc>>, ()> {
    value.map_or(Ok(None), |value| parse_delivery_time(value).map(Some).ok_or(()))
}

fn valid_origin_kind(value: &str) -> bool {
    matches!(
        value,
        "settlement"
            | "settlement_recovery_action"
            | "user_key"
            | "manual"
            | "mail_intake"
            | "operations_alert"
    )
}

fn valid_status(value: &str) -> bool {
    matches!(
        value,
        "prepared"
            | "pending"
            | "claimed"
            | "sending"
            | "succeeded"
            | "failed"
            | "manual_reconcile"
    )
}

fn valid_stored_text(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

fn valid_optional_text(value: Option<&str>) -> bool {
    value.is_none_or(valid_stored_text)
}

fn valid_returned_mail_id(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        (1..=32).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn valid_amount(value: Option<i64>) -> bool {
    value.is_none_or(|value| (0..=MAX_JSON_SAFE_INTEGER).contains(&value))
}

fn is_zero_delivery_time(value: DateTime<Utc>) -> bool {
    value.timestamp() == -62_135_596_800 && value.timestamp_subsec_nanos() == 0
}

fn valid_timeline(
    created_at: DateTime<Utc>,
    claimed_at: Option<DateTime<Utc>>,
    sending_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
) -> bool {
    claimed_at.is_none_or(|value| value >= created_at)
        && sending_at.is_none_or(|value| claimed_at.is_some_and(|claimed| value >= claimed))
        && completed_at.is_none_or(|value| sending_at.is_some_and(|sending| value >= sending))
}

fn valid_state_fields(
    value: &StrictDeliveryResponse,
    lease_expires_at: Option<DateTime<Utc>>,
    claimed_at: Option<DateTime<Utc>>,
    sending_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
) -> bool {
    match value.status.as_str() {
        "prepared" | "pending" => {
            lease_expires_at.is_none()
                && claimed_at.is_none()
                && sending_at.is_none()
                && completed_at.is_none()
                && value.external_reference.is_none()
                && value.failure_code.is_none()
        }
        "claimed" => {
            lease_expires_at.is_some()
                && claimed_at.is_some()
                && sending_at.is_none()
                && completed_at.is_none()
                && value.external_reference.is_none()
                && value.failure_code.is_none()
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        "sending" => {
            lease_expires_at.is_some()
                && claimed_at.is_some()
                && sending_at.is_some()
                && completed_at.is_none()
                && value.external_reference.is_none()
                && value.failure_code.is_none()
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        "succeeded" => {
            lease_expires_at.is_none()
                && claimed_at.is_some()
                && sending_at.is_some()
                && completed_at.is_some()
                && value.external_reference.as_deref().is_some_and(valid_stored_text)
                && value.failure_code.is_none()
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        "failed" | "manual_reconcile" => {
            lease_expires_at.is_none()
                && claimed_at.is_some()
                && sending_at.is_some()
                && completed_at.is_some()
                && value.external_reference.is_none()
                && value.failure_code.as_deref().is_some_and(valid_stored_text)
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        _ => false,
    }
}

/// 将浏览器 `datetime-local` 按本地时区转换为 API 使用的 UTC 文本。
///
/// Admin 的创建和筛选必须共享这一转换，否则非 UTC 管理员提交的时间范围会产生偏移。
pub(crate) fn normalize_datetime(value: &str) -> String {
    if value.is_empty()
        || value.ends_with('Z')
        || value.contains('+')
        || value.get(10..).is_some_and(|tail| tail.contains('-'))
    {
        return value.to_owned();
    }

    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
        if date.get_time().is_finite() {
            return date.to_iso_string().into();
        }
    }

    // 非 WASM 构建只用于静态检查；保留服务端可识别的稳定兜底格式。
    if value.len() == 16 { format!("{value}:00Z") } else { format!("{value}Z") }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryAction {
    Release,
    Retry,
    ReconcileRetry,
    ReconcileSucceeded,
    ReconcileFailed,
}

impl DeliveryAction {
    pub(crate) const fn path(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Retry => "retry",
            Self::ReconcileRetry | Self::ReconcileSucceeded | Self::ReconcileFailed => "reconcile",
        }
    }

    pub(crate) const fn request_action(self) -> Option<&'static str> {
        match self {
            Self::Release | Self::Retry => None,
            Self::ReconcileRetry => Some("retry"),
            Self::ReconcileSucceeded => Some("succeeded"),
            Self::ReconcileFailed => Some("failed"),
        }
    }
}

/// 读取指定状态和页码的 delivery 安全投影；状态筛选改变时回到第一页。
pub(crate) async fn fetch_deliveries(status: String, page: i64) -> Result<DeliveryPage, String> {
    fetch_deliveries_with_size(status, page, TABLE_PAGE_SIZE).await
}

async fn fetch_deliveries_with_size(
    status: String,
    page: i64,
    page_size: i64,
) -> Result<DeliveryPage, String> {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.max(1).to_string())
        .append_pair("page_size", &page_size.to_string());
    if !status.is_empty() {
        query.append_pair("status", status.trim());
    }
    let url = format!("/api/v1/admin/deliveries?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: StrictDeliveryEnvelope<StrictDeliveryListResponse> =
        response.json().await.map_err(invalid_response_error)?;
    decode_delivery_list(payload).map_err(|_| invalid_response_error("delivery_list"))
}

pub(crate) async fn execute_delivery(
    command_id: Uuid,
    delivery_id: String,
    action: DeliveryAction,
    expected_fencing_version: i64,
    external_reference: Option<String>,
    failure_code: Option<String>,
) -> Result<Delivery, AdminApiFailure> {
    let command = AdminDeliveryCommandRequest {
        command_id,
        expected_fencing_version,
        action: action.request_action().map(str::to_owned),
        external_reference,
        failure_code,
    };
    let url = format!("/api/v1/admin/deliveries/{delivery_id}/{}", action.path());
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let response = send_request(|abort_signal| {
        Request::post(&url)
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
    let payload: StrictDeliveryEnvelope<StrictDeliveryCommandResponse> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_delivery_command(payload, &delivery_id, action, expected_fencing_version)
        .map_err(|_| AdminApiFailure::client(invalid_response_error("delivery_result")))
}

async fn fetch_csrf_token() -> Result<String, String> {
    let response = send_get("/api/v1/admin/session").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminSessionResponse> =
        response.json().await.map_err(invalid_response_error)?;
    session::decode_admin_session(payload.data).map(|session| session.csrf_token)
}
