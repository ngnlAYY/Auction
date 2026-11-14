//! 投递队列的检索与人工命令。
//! 列表只提交显式筛选与分页；释放/重试/reconcile 都携带按输入复用的 `command_id` 与版本
//! fence，浏览器不推导状态机，幂等与审计归服务端所有。

use auction_contracts::{
    AdminDeliveryCommandRequest, AdminDeliveryCommandResponse, AdminDeliveryListResponse,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::decode::{
    StrictDeliveryEnvelope, decode_delivery_command, decode_delivery_list, parse_delivery_time,
};
use crate::api::{
    AdminApiFailure, Delivery, DeliveryPage, fetch_csrf_token, invalid_request_error,
    invalid_response_error, pagination, response_error, response_failure, send_get, send_request,
    valid_uuid,
};

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

/// 投递检索条件；空串表示该维度未设置，由页面从地址栏恢复。
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

/// 读取指定状态、筛选与页码的 delivery 安全投影。
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
