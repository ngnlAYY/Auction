//! Admin 浏览器端的同源 HTTP 数据访问。
//! 浏览器只接收后端安全投影；改变状态的人工动作携带按输入复用的 `command_id` 与版本
//! fence，最终校验、幂等与审计归服务端。wire DTO 统一由 `auction-contracts` 拥有。

mod alerts;
mod auctions;
mod audit;
mod credentials;
mod dashboard;
mod delivery;
mod forum;
mod intake;
mod lifecycle;
mod lots;
mod mail;
mod pagination;
mod proxy_events;
mod realtime;
mod recovery;
mod renames;
mod rules;
mod runtime_config;
mod session;
mod settlement;
mod settlement_catalog;
mod setup;
mod status;
mod users;
mod worlds;

pub(crate) use alerts::{
    AdminAlert, AdminAlertPage, AdminPendingCounts, acknowledge_admin_alert, fetch_admin_alerts,
    fetch_admin_pending_counts, silence_admin_alert,
};
pub(crate) use auctions::{
    AdminAuction, AdminAuctionCapabilities, AdminAuctionPage, create_admin_auction,
    fetch_admin_auctions, update_admin_auction_schedule,
};
pub(crate) use audit::{AdminAuditFilter, AdminAuditPage, AdminAuditRecord, fetch_admin_audit};
pub(crate) use credentials::{
    AdminCredential, AdminCredentialCheck, AdminCredentialPage, check_admin_credential,
    create_admin_credential, delete_admin_credential, fetch_admin_credentials,
    mutate_admin_credential,
};
pub(crate) use dashboard::{AdminDashboardSnapshot, fetch_dashboard};
pub(crate) use delivery::{
    SentProofCommand, confirm_delivery_sent_proof, fetch_delivery_detail, fetch_player_support,
    preview_delivery_sent_proof,
};
pub(crate) use forum::{AdminForumStatus, fetch_admin_forum_status};
pub(crate) use intake::{AdminIntakeResult, add_admin_intake};
pub(crate) use lifecycle::{AdminAuctionLifecycleAction, execute_admin_auction_lifecycle};
pub(crate) use lots::{
    AdminLot, AdminLotPage, AdminLotUpdateAction, fetch_admin_lots, update_admin_lot,
};
pub(crate) use mail::{
    AdminMailIntake, AdminMailIntakeDelivery, AdminMailIntakeDetail, AdminMailIntakePage,
    AdminMailIntakeQuery, AdminMailPolling, fetch_mail_intake_detail, fetch_mail_intakes,
    fetch_mail_state, set_mail_paused, trigger_mail_poll,
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
pub(crate) use renames::{
    AdminRename, AdminRenamePage, AdminRenameRevertCommand, fetch_admin_renames,
    prepare_admin_rename_revert, revert_admin_rename,
};
pub(crate) use rules::{
    AdminRules, AdminRulesCandidate, fetch_admin_rules, publish_admin_rules, validate_admin_rules,
};
pub(crate) use runtime_config::{
    fetch_admin_runtime_config, update_admin_runtime_config, validate_admin_runtime_config,
};
pub(crate) use session::{
    AdminSession, AdminSessionCheckTrigger, AdminSessionWritePermission,
    admin_session_writes_allowed, fetch_admin_session, login_admin, logout_admin,
};
pub(crate) use settlement::{
    AdminSettlementLot, AdminSettlementReport, AdminSettlementSummary, fetch_admin_settlement,
};
pub(crate) use settlement_catalog::{
    AdminSettlementCatalogEntry, AdminSettlementCatalogPage, fetch_admin_settlement_catalog,
};
pub(crate) use setup::{submit_first_run, verify_first_run};
pub(crate) use status::{AdminReadinessState, fetch_admin_readiness};
pub(crate) use users::{
    AdminUser, AdminUserPage, AdminUserStatusFilter, blacklist_admin_user, create_admin_user,
    fetch_admin_users, mutate_admin_user, unblacklist_admin_user,
};
pub(crate) use worlds::{
    AdminBusinessWorld, fetch_admin_business_worlds, set_admin_business_world_enabled,
};

pub(crate) use auction_contracts::{
    AdminDeliveryAttemptResponse as DeliveryAttempt,
    AdminDeliveryCustodyResponse as DeliveryCustody, AdminDeliveryDetailResponse as DeliveryDetail,
    AdminDeliveryDispositionResponse as DeliveryDisposition,
    AdminDeliveryEvidenceResponse as DeliveryEvidence, AdminDeliveryListResponse as DeliveryPage,
    AdminDeliveryObligationResponse as DeliveryObligation,
    AdminDeliveryRecoveryResponse as DeliveryRecovery, AdminDeliveryResponse as Delivery,
    AdminDeliverySentEvidenceRequest as DeliverySentEvidence,
    AdminDeliverySentProofPreviewResponse as DeliverySentProofPreview,
    AdminDeliverySentProofResponse as DeliverySentProofResult,
    AdminPlayerSupportBidResponse as PlayerSupportBid,
    AdminPlayerSupportBundleResponse as PlayerSupportBundle,
    AdminPlayerSupportDealResponse as PlayerSupportDeal,
    AdminPlayerSupportNotificationResponse as PlayerSupportNotification,
    AdminPlayerSupportObligationResponse as PlayerSupportObligation,
};
use auction_contracts::{
    AdminDeliveryCommandRequest, AdminDeliveryCommandResponse, AdminDeliveryListResponse,
    AdminDeliveryResponse, ApiResponse,
};
use auction_web_shared::{BrowserErrorClass, TransportError, report_browser_error};
use chrono::{DateTime, Utc};
use gloo_net::http::{Request, Response};
use serde::Deserialize;
use uuid::Uuid;

/// 改名命令准备阶段读取历史的固定数量；可见列表数量由页面显式传入。
pub(crate) const TABLE_PAGE_SIZE: i64 = 10;

/// Admin 浏览器请求的稳定失败投影。
/// 服务端正文的自然语言不进入界面；页面只消费稳定 `code`，再经 Fluent 映射。HTTP 状态
/// 单独保留，供 404、409 等需改变交互流程的页面判断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminApiFailure {
    pub status: Option<u16>,
    pub code: String,
}

/// Admin 错误 envelope 的浏览器侧兼容模型。
/// 旧版要求 `code`、`message`、`data` 三字段存在但允许扩展字段与任意错误码；错误数据不进入
/// 页面，仍以不透明 JSON 保留，仅为兼容既有 envelope 形状。
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
    let failure = read_response_failure(response).await;
    if failure.status == Some(401) && failure.code == "unauthenticated" {
        session::request_admin_session_check();
    }
    failure
}

/// 读取错误 envelope 而不触发认证副作用，供会话检查自身消费确定的未认证结果。
async fn read_response_failure(response: Response) -> AdminApiFailure {
    let status = response.status();
    let code = match response.json::<AdminApiErrorResponse>().await {
        Ok(payload) => payload.into_code(),
        Err(_) => {
            report_browser_error(BrowserErrorClass::DtoDecode);
            format!("http_{status}")
        }
    };
    AdminApiFailure { status: Some(status), code }
}

/// `Result<_, String>` API 只返回稳定错误码，兼容现有页面状态而不泄漏底层错误正文。
pub(crate) async fn response_error(response: Response) -> String {
    response_failure(response).await.code
}

fn transport_error_code(error: TransportError) -> String {
    error.code().to_owned()
}

/// 发送不改写 URL 的 Admin GET 请求；传输边界复用共享实现，本层只把结构化错误折回稳定 code。
pub(crate) async fn send_get(url: &str) -> Result<Response, String> {
    auction_web_shared::send_get(url).await.map_err(transport_error_code)
}

/// 在共享传输层的统一超时上限内发送一次 Admin 请求。
/// `x-csrf-token` 与 Basic Auth header 由调用方在 `build` 闭包叠加（共享层不感知凭据）。
/// `build` 失败携带稳定业务 code，[`TransportError`] 无法承载，先暂存再原样返回。
pub(crate) async fn send_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
) -> Result<Response, String> {
    send_request_with_timeout(build, auction_web_shared::DEFAULT_REQUEST_TIMEOUT_MILLIS).await
}

/// 为单次 Admin 请求指定等待预算；仍复用共享 fetch，不改变其他请求预算或添加重试。
pub(crate) async fn send_request_with_timeout(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
    timeout_millis: u32,
) -> Result<Response, String> {
    let build_failure = std::cell::RefCell::new(None::<String>);
    let result = auction_web_shared::send_request_with_timeout(
        |abort_signal| {
            build(abort_signal).map_err(|code| {
                // 占位错误不会被外层使用：build 失败时以暂存的业务 code 为准。
                *build_failure.borrow_mut() = Some(code);
                TransportError::Network(None)
            })
        },
        timeout_millis,
    )
    .await;
    match result {
        Ok(response) => Ok(response),
        Err(error) => Err(build_failure.into_inner().unwrap_or_else(|| error.code().to_owned())),
    }
}

pub(crate) fn invalid_request_error(_error: impl ToString) -> String {
    "invalid_request".to_owned()
}

pub(crate) fn invalid_response_error(_error: impl ToString) -> String {
    report_browser_error(BrowserErrorClass::DtoDecode);
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
        timestamp.is_finite().then_some(timestamp)
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
/// 共享 `auction_contracts::ApiResponse` 同时服务 Public surface，刻意保持开放；delivery
/// 路由要求信封闭集，因此本地保留该泛型外壳。同步义务：`ApiResponse` 的 code/message/data
/// 形状变化时必须同步此处。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictDeliveryEnvelope<T> {
    code: String,
    message: String,
    data: T,
}

pub(crate) const MAX_JSON_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const MAX_DELIVERY_PAGE_SIZE: i64 = 100;

fn valid_success_code(code: &str) -> bool {
    matches!(code, "ok" | "created" | "accepted")
}

fn decode_delivery_list(
    payload: StrictDeliveryEnvelope<AdminDeliveryListResponse>,
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

    Ok(DeliveryPage { deliveries, pagination })
}

fn decode_delivery_command(
    payload: StrictDeliveryEnvelope<AdminDeliveryCommandResponse>,
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

fn decode_delivery(value: AdminDeliveryResponse) -> Result<Delivery, ()> {
    // W4.5 新增字段同样 fail-closed：模板/类别/世界/原因只接受服务端闭集。
    if !valid_optional_text(value.template.as_deref())
        || value.template.as_deref().is_some_and(|template| template.len() > 64)
        || value.category.as_deref().is_some_and(|category| {
            !matches!(
                category,
                "outbid"
                    | "account"
                    | "intake"
                    | "settlement"
                    | "equipment"
                    | "operations"
                    | "manual"
            )
        })
        || value.world.as_deref().is_some_and(|world| !matches!(world, "main" | "isekai"))
        || !valid_optional_text(value.retirement_reason.as_deref())
        || !valid_optional_text(value.suppression_reason.as_deref())
    {
        return Err(());
    }
    if !valid_uuid(&value.id) {
        return Err(());
    }
    let created_at = parse_delivery_time(&value.created_at).ok_or(())?;
    let updated_at = parse_delivery_time(&value.updated_at).ok_or(())?;
    let lease_expires_at = parse_optional_delivery_time(value.lease_expires_at.as_ref())?;
    let claimed_at = parse_optional_delivery_time(value.claimed_at.as_ref())?;
    let sending_at = parse_optional_delivery_time(value.sending_at.as_ref())?;
    let completed_at = parse_optional_delivery_time(value.completed_at.as_ref())?;
    // 服务端只为从未激活的告警投递使用该终态，时间链从创建直接到完成。
    let before_activation = value.status == "failed"
        && value.origin_kind == "operations_alert"
        && value.reason_code.as_deref() == Some("operations_alert_batch_terminal");
    let disposition_conflict = value.reason_code.as_deref() == Some("mail_disposition_conflict");
    let watch_invalidated = value.origin_kind == "lot_watch"
        && matches!(
            value.reason_code.as_deref(),
            Some("lot_watch_invalidated" | "recipient_renamed")
        );
    let preparation_rejected = value.status == "failed"
        && value.failure_code.as_deref() == Some("invalid_payload")
        && sending_at.is_none();
    let timeline_valid = if preparation_rejected {
        claimed_at.is_some_and(|claimed| {
            !is_zero_delivery_time(claimed)
                && claimed >= created_at
                && completed_at.is_some_and(|completed| {
                    !is_zero_delivery_time(completed)
                        && completed >= claimed
                        && completed <= updated_at
                })
        })
    } else if disposition_conflict || watch_invalidated {
        sending_at.is_none()
            && claimed_at.is_none_or(|claimed| {
                !is_zero_delivery_time(claimed) && claimed >= created_at && value.attempt_count > 0
            })
            && completed_at.is_some_and(|completed| {
                !is_zero_delivery_time(completed)
                    && completed >= created_at
                    && claimed_at.is_none_or(|claimed| completed >= claimed)
                    && completed <= updated_at
            })
    } else if before_activation {
        claimed_at.is_none()
            && sending_at.is_none()
            && completed_at.is_some_and(|completed| {
                !is_zero_delivery_time(completed)
                    && completed >= created_at
                    && completed <= updated_at
            })
    } else {
        valid_timeline(created_at, claimed_at, sending_at, completed_at)
    };

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
        || !timeline_valid
        || !valid_optional_text(value.external_reference.as_deref())
        || !valid_optional_text(value.failure_code.as_deref())
        || !valid_optional_text(value.reason_code.as_deref())
        || !valid_returned_mail_id(value.returned_mail_id.as_deref())
        || !valid_state_fields(&value, lease_expires_at, claimed_at, sending_at, completed_at)
    {
        return Err(());
    }

    Ok(Delivery {
        id: value.id,
        origin_kind: value.origin_kind,
        origin_id: value.origin_id,
        recipient: value.recipient,
        cod: value.cod,
        status: value.status,
        template: value.template,
        category: value.category,
        world: value.world,
        retirement_reason: value.retirement_reason,
        suppression_reason: value.suppression_reason,
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

pub(crate) use auction_web_shared::is_legacy_uuid as valid_uuid;

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
            | "lot_watch"
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
    value: &AdminDeliveryResponse,
    lease_expires_at: Option<DateTime<Utc>>,
    claimed_at: Option<DateTime<Utc>>,
    sending_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
) -> bool {
    // 服务端已验证未写入 metadata；浏览器只接受明确非法载荷的完整领取/完成形状。
    if value.status == "failed"
        && value.failure_code.as_deref() == Some("invalid_payload")
        && sending_at.is_none()
    {
        return value.reason_code.is_none()
            && lease_expires_at.is_none()
            && claimed_at.is_some()
            && completed_at.is_some()
            && value.external_reference.is_none()
            && !value.take_required
            && value.returned_mail_id.is_none()
            && value.fencing_version > 0
            && value.attempt_count > 0;
    }
    // watch 失效或收件人改名只能作为未发送的明确失败；不能放宽普通失败或未知发送的时间链。
    if matches!(value.reason_code.as_deref(), Some("lot_watch_invalidated" | "recipient_renamed")) {
        return value.origin_kind == "lot_watch"
            && value.status == "failed"
            && value.failure_code.as_deref() == value.reason_code.as_deref()
            && value.cod == Some(0)
            && lease_expires_at.is_none()
            && sending_at.is_none()
            && completed_at.is_some()
            && value.external_reference.is_none()
            && !value.take_required
            && value.returned_mail_id.is_none()
            && value.fencing_version > 0;
    }
    // 资产处置冲突只能在发送前隔离；人工失败后仍保留原因，不能混入普通可重试失败。
    if value.reason_code.as_deref() == Some("mail_disposition_conflict") {
        return value.origin_kind == "mail_intake"
            && match value.status.as_str() {
                "manual_reconcile" => {
                    value.failure_code.as_deref() == Some("mail_disposition_conflict")
                }
                "failed" => value.failure_code.as_deref().is_some_and(valid_stored_text),
                _ => false,
            }
            && lease_expires_at.is_none()
            && sending_at.is_none()
            && completed_at.is_some()
            && value.external_reference.is_none()
            && !value.take_required
            && value.returned_mail_id.is_none()
            && value.fencing_version > 0;
    }
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
            let before_activation = value.status == "failed"
                && value.origin_kind == "operations_alert"
                && value.reason_code.as_deref() == Some("operations_alert_batch_terminal")
                && lease_expires_at.is_none()
                && claimed_at.is_none()
                && sending_at.is_none()
                && completed_at.is_some()
                && value.external_reference.is_none()
                && !value.take_required
                && value.returned_mail_id.is_none()
                && matches!(
                    value.failure_code.as_deref(),
                    Some("dependency_mismatch" | "retry_exhausted")
                )
                && value.fencing_version == 0
                && value.attempt_count == 0;
            // 已证明收件人不存在的退信 MID 是人工取回线索，不是发送成功引用。
            let returned_to_sender = value.status == "manual_reconcile"
                && value.failure_code.as_deref() == Some("recipient_not_found")
                && value.reason_code.as_deref() == Some("recipient_not_found")
                && value.cod.is_some()
                && value.take_required
                && value.returned_mail_id.as_deref().is_some_and(|mid| {
                    value.external_reference.as_deref() == Some(mid)
                        && valid_returned_mail_id(Some(mid))
                });
            before_activation
                || (lease_expires_at.is_none()
                    && claimed_at.is_some()
                    && sending_at.is_some()
                    && completed_at.is_some()
                    && (value.external_reference.is_none() || returned_to_sender)
                    && value.failure_code.as_deref().is_some_and(valid_stored_text)
                    && value.fencing_version > 0
                    && value.attempt_count > 0)
        }
        _ => false,
    }
}

/// 将浏览器 `datetime-local` 按本地时区转换为 API 使用的 UTC 文本。
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
/// W4.5 投递检索条件；空串表示该维度未设置，由页面从地址栏恢复。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeliverySearchFilters {
    pub recipient: String,
    pub reference: String,
    pub template: String,
    pub category: String,
    pub world: String,
    pub created_from: String,
    pub created_to: String,
}

impl DeliverySearchFilters {
    /// 客户端只做格式与上限检查；服务端仍按闭集和精确匹配失败关闭。
    fn validate(&self) -> bool {
        valid_filter_text(&self.recipient, 256)
            && (self.reference.is_empty() || valid_uuid(&self.reference))
            && valid_filter_text(&self.template, 64)
            && (self.category.is_empty()
                || matches!(
                    self.category.as_str(),
                    "outbid"
                        | "account"
                        | "intake"
                        | "settlement"
                        | "equipment"
                        | "operations"
                        | "manual"
                ))
            && (self.world.is_empty() || matches!(self.world.as_str(), "main" | "isekai"))
            && valid_filter_time(&self.created_from)
            && valid_filter_time(&self.created_to)
    }
}

fn valid_filter_text(value: &str, maximum: usize) -> bool {
    value.is_empty()
        || (value.len() <= maximum && value == value.trim() && !value.chars().any(char::is_control))
}

fn valid_filter_time(value: &str) -> bool {
    value.is_empty() || parse_delivery_time(value).is_some()
}

pub(crate) async fn fetch_deliveries(
    status: String,
    filters: DeliverySearchFilters,
    page: i64,
    page_size: i64,
) -> Result<DeliveryPage, String> {
    pagination::validate(page, page_size, 100, i64::MAX)?;
    if !filters.validate() {
        return Err("invalid_delivery_filter".to_owned());
    }
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.max(1).to_string())
        .append_pair("page_size", &page_size.to_string());
    if !status.is_empty() {
        query.append_pair("status", status.trim());
    }
    for (key, value) in [
        ("recipient", filters.recipient.as_str()),
        ("reference", filters.reference.as_str()),
        ("template", filters.template.as_str()),
        ("category", filters.category.as_str()),
        ("world", filters.world.as_str()),
        ("created_from", filters.created_from.as_str()),
        ("created_to", filters.created_to.as_str()),
    ] {
        if !value.is_empty() {
            query.append_pair(key, value);
        }
    }
    let url = format!("/api/v1/admin/deliveries?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: StrictDeliveryEnvelope<AdminDeliveryListResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let result =
        decode_delivery_list(payload).map_err(|_| invalid_response_error("delivery_list"))?;
    if result.pagination.page != page || result.pagination.page_size != page_size {
        return Err(invalid_response_error("delivery_pagination"));
    }
    Ok(result)
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
    let payload: StrictDeliveryEnvelope<AdminDeliveryCommandResponse> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_delivery_command(payload, &delivery_id, action, expected_fencing_version)
        .map_err(|_| AdminApiFailure::client(invalid_response_error("delivery_result")))
}

async fn fetch_csrf_token() -> Result<String, String> {
    // 保留认证内状态不等于继续授权；会话复核未确认时不发起新的管理写入。
    if !admin_session_writes_allowed() {
        return Err("unauthenticated".to_owned());
    }
    let current = match fetch_admin_session().await {
        Ok(Some(current)) => current,
        result => {
            // CSRF 复核失败也撤回写许可；旧宿主与未决命令交给既有会话重查流程保留。
            session::request_admin_session_check();
            return Err(result.err().unwrap_or_else(|| "unauthenticated".to_owned()));
        }
    };
    // 获取 CSRF 的等待期间也可能进入暂时未确认状态，不能带着旧页面授权继续提交。
    if !admin_session_writes_allowed() {
        return Err("unauthenticated".to_owned());
    }
    // 其他标签页可能已重新登录；新会话的 token 不能授权旧宿主保存的命令或草稿。
    let same_identity =
        dioxus::prelude::try_consume_context::<AdminSession>().is_some_and(|confirmed| {
            confirmed.actor == current.actor && confirmed.csrf_token == current.csrf_token
        });
    if !same_identity {
        session::request_admin_session_check();
        return Err("unauthenticated".to_owned());
    }
    Ok(current.csrf_token)
}

mod runtime_switches;
pub(crate) use runtime_switches::fetch_runtime_switches;
