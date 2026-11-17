//! 装备来源请求的节流与单次无重定向读取；有界结果和敏感缓冲的生命周期在这里收口。
//!
//! 注入共享的 MoogleMail 客户端后，每次请求先取得同账号许可：与邮件请求不重叠、账号忙窗口内
//! 不发网络，战斗页打开同一账号忙窗口。战斗页与限流只表示暂时不可用，绝不归为来源无效。
//! 许可上的排队随调用方放弃撤销登记；请求发出后被放弃时，剩余部分连同许可交给独立任务，借用的
//! 在途锁持有到远端回应或本次读取预算结束。

use std::{
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
    time::Instant as StdInstant,
};

use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::external_metrics::MailRequestCategory;
use auction_application::mail::MailEquipmentDiagnosticCategory;
use reqwest::RequestBuilder;
use tokio::time::{Instant, sleep_until};
use url::Url;

use super::super::config::DEFAULT_USER_AGENT;
use super::{EquipmentFetchError, EquipmentSourceAdapter};
use crate::clients::owner_probe::probe_owner;
use crate::clients::sent_request::SentRequest;
use crate::{
    mooglemail::{
        EquipmentReadPermit, MoogleMailCookie, MoogleMailWorld, equipment_read_hold,
        is_hentaiverse_login_response,
    },
    safe_diagnostics,
};

impl EquipmentSourceAdapter {
    /// 同一适配器的相邻请求至少间隔 `minimum_request_interval`；持锁到发出时刻之后，
    /// 让并发读取也遵守同一节奏。
    pub(super) async fn await_request_slot(&self) {
        let mut next_request_at = self.next_request_at.lock().await;
        let now = Instant::now();
        if *next_request_at > now {
            sleep_until(*next_request_at).await;
        }
        *next_request_at = Instant::now() + self.policy.minimum_request_interval;
    }
}

pub(super) struct EquipmentFetchFailure {
    pub(super) category: MailEquipmentDiagnosticCategory,
    pub(super) status: Option<u16>,
    pub(super) elapsed_millis: Option<u64>,
    pub(super) body: Option<Vec<u8>>,
}

impl Drop for EquipmentFetchFailure {
    fn drop(&mut self) {
        if let Some(body) = self.body.as_mut() {
            body.fill(0);
        }
    }
}

pub(super) struct EquipmentFetchSuccess {
    pub(super) body: Vec<u8>,
    pub(super) status: u16,
    pub(super) elapsed_millis: Option<u64>,
}

impl Drop for EquipmentFetchSuccess {
    fn drop(&mut self) {
        self.body.fill(0);
    }
}

pub(super) struct SensitiveResponseBody(pub(super) Vec<u8>);

impl Drop for SensitiveResponseBody {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub(super) fn elapsed_millis(started: StdInstant) -> Option<u64> {
    Some(started.elapsed().as_millis().min(300_000) as u64)
}

pub(super) fn nonzero_status(status: &AtomicU16) -> Option<u16> {
    match status.load(Ordering::Relaxed) {
        0 => None,
        status => Some(status),
    }
}

pub(super) fn observed_failure(
    category: MailEquipmentDiagnosticCategory,
    status: Option<u16>,
    started: StdInstant,
    body: Option<Vec<u8>>,
) -> EquipmentFetchError {
    EquipmentFetchError::Observed(EquipmentFetchFailure {
        category,
        status,
        elapsed_millis: elapsed_millis(started),
        body,
    })
}

/// 执行一次不重定向的来源请求；完整正文才会从此模块交给解析层。
pub(super) async fn document_once(
    adapter: &EquipmentSourceAdapter,
    world: MoogleMailWorld,
    target: Url,
    cookie: Option<&MoogleMailCookie>,
    status_slot: Option<Arc<AtomicU16>>,
) -> Result<EquipmentFetchSuccess, EquipmentFetchError> {
    let started = StdInstant::now();
    // 与调用方的外层预算同为本次读取的 request_timeout、同样从排队开始计时；只约束被放弃后分离的
    // 发出阶段，调用方等待期间仍由外层预算裁决。溢出时退回立即丢弃。
    let detached_deadline =
        Instant::now().checked_add(adapter.policy.request_timeout).unwrap_or_else(Instant::now);
    let mut request =
        adapter.client.get(target).header(reqwest::header::USER_AGENT, DEFAULT_USER_AGENT);
    if let Some(cookie) = cookie {
        let mut header = reqwest::header::HeaderValue::from_str(cookie.expose()).map_err(
            safe_diagnostics::warn_map("equipment_fetch", "encode_cookie", || {
                observed_failure(
                    MailEquipmentDiagnosticCategory::CredentialUnavailable,
                    None,
                    started,
                    None,
                )
            }),
        )?;
        header.set_sensitive(true);
        request = request.header(reqwest::header::COOKIE, header);
    }
    // 同一 Cookie 的物理请求与邮件请求串行，许可一直持有到正文读完并完成账号状态分类；
    // 在 owner 复核之前取得，复核贴近真实发出时刻。
    let mut permit = match adapter.mail_client.as_ref() {
        Some(client) => match client.equipment_read_permit(world).await {
            Some(permit) => Some(permit),
            // 账号忙窗口内不发网络：按限流归类，只能延期重试，不能退件。
            None => {
                return Err(observed_failure(
                    MailEquipmentDiagnosticCategory::RateLimited,
                    None,
                    started,
                    None,
                ));
            }
        },
        None => None,
    };
    // 装备读取使用独立 reqwest client，但共享同一个 MoogleMail 凭据 owner；探测在独立任务中
    // 完成，外层读取预算取消本请求不撤销邮件 owner。
    if let Some(ownership) = adapter
        .credentials
        .as_ref()
        .and_then(crate::clients::mooglemail::MoogleMailCredentialSet::ownership)
        && let Err(diagnostic_cause) = probe_owner(&ownership).await
    {
        safe_diagnostics::warn(
            "equipment_fetch",
            diagnostic_cause.phase("check_owner_timeout", "check_owner"),
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        return Err(observed_failure(
            MailEquipmentDiagnosticCategory::CredentialUnavailable,
            None,
            started,
            None,
        ));
    }
    if cookie.is_some_and(|cookie| !cookie.is_active()) {
        return Err(observed_failure(
            MailEquipmentDiagnosticCategory::CredentialUnavailable,
            None,
            started,
            None,
        ));
    }
    if let Some(permit) = permit.as_mut() {
        permit.start_request();
    }
    let endpoint = adapter.endpoint.clone();
    let maximum_response_bytes = adapter.policy.maximum_response_bytes;
    // 发出阶段：调用方等待时就在本任务内走完；被放弃（Worker item timeout、control 路由预算、连接
    // 断开）时连同许可交给独立任务，在途锁持有到远端回应或分离截止，紧接的同账号请求不与之重叠。
    SentRequest::with_detached_deadline(
        async move {
            let mut permit = permit;
            let result = exchange(
                &endpoint,
                maximum_response_bytes,
                request,
                permit.as_mut(),
                status_slot,
                started,
            )
            .await;
            if let Some(permit) = permit {
                permit.finish(request_category(&result));
            }
            result
        },
        detached_deadline,
    )
    .await
}

/// 发出请求并有界读取完整正文，再按登录、账号临时状态与 HTTP 结果分类。
async fn exchange(
    endpoint: &Url,
    maximum_response_bytes: usize,
    request: RequestBuilder,
    permit: Option<&mut EquipmentReadPermit>,
    status_slot: Option<Arc<AtomicU16>>,
    started: StdInstant,
) -> Result<EquipmentFetchSuccess, EquipmentFetchError> {
    let mut response = request.send().await.map_err(safe_diagnostics::warn_map(
        "equipment_fetch",
        "send_request",
        || {
            EquipmentFetchError::Observed(EquipmentFetchFailure {
                category: MailEquipmentDiagnosticCategory::Network,
                status: None,
                elapsed_millis: elapsed_millis(started),
                body: None,
            })
        },
    ))?;
    if let Some(slot) = status_slot.as_ref() {
        slot.store(response.status().as_u16(), Ordering::Relaxed);
    }
    let status = response.status();
    if response.url().scheme() != endpoint.scheme()
        || response.url().host_str() != endpoint.host_str()
        || response.url().port_or_known_default() != endpoint.port_or_known_default()
    {
        return Err(observed_failure(
            MailEquipmentDiagnosticCategory::Redirect,
            Some(status.as_u16()),
            started,
            None,
        ));
    }
    if response.content_length().is_some_and(|length| length > maximum_response_bytes as u64) {
        return Err(observed_failure(
            MailEquipmentDiagnosticCategory::ResponseTooLarge,
            Some(status.as_u16()),
            started,
            None,
        ));
    }
    let encoding = response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim();
    if !encoding.is_empty() && !encoding.eq_ignore_ascii_case("identity") {
        return Err(observed_failure(
            MailEquipmentDiagnosticCategory::ContentEncoding,
            Some(status.as_u16()),
            started,
            None,
        ));
    }
    let location = response.headers().get(reqwest::header::LOCATION).cloned();
    let is_html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| matches!(value.trim(), "text/html" | "application/xhtml+xml"));
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(maximum_response_bytes);
    let mut body = SensitiveResponseBody(Vec::with_capacity(capacity));
    while let Some(chunk) = response.chunk().await.map_err(safe_diagnostics::warn_map(
        "equipment_fetch",
        "read_response_chunk",
        || {
            observed_failure(
                MailEquipmentDiagnosticCategory::Network,
                Some(status.as_u16()),
                started,
                None,
            )
        },
    ))? {
        let next = body.0.len().checked_add(chunk.len()).ok_or_else(|| {
            observed_failure(
                MailEquipmentDiagnosticCategory::ResponseTooLarge,
                Some(status.as_u16()),
                started,
                None,
            )
        })?;
        if next > maximum_response_bytes {
            return Err(observed_failure(
                MailEquipmentDiagnosticCategory::ResponseTooLarge,
                Some(status.as_u16()),
                started,
                None,
            ));
        }
        body.0.extend_from_slice(&chunk);
    }
    if is_hentaiverse_login_response(status, location.as_ref(), &body.0) {
        return Err(EquipmentFetchError::Observed(EquipmentFetchFailure {
            category: MailEquipmentDiagnosticCategory::SessionExpired,
            status: Some(status.as_u16()),
            elapsed_millis: elapsed_millis(started),
            body: Some(std::mem::take(&mut body.0)),
        }));
    }
    // 战斗页、429 与状态锁限流正文都只说明账号暂时不可用，与邮件读取同口径；交给解析器会被
    // 判成来源无效而退件，把临时状态变成不可逆的外部写入。
    if let Some(hold) = equipment_read_hold(status, &body.0) {
        if let Some(permit) = permit {
            permit.record_hold(hold);
        }
        return Err(EquipmentFetchError::Observed(EquipmentFetchFailure {
            category: MailEquipmentDiagnosticCategory::RateLimited,
            status: Some(status.as_u16()),
            elapsed_millis: elapsed_millis(started),
            body: Some(std::mem::take(&mut body.0)),
        }));
    }
    if status.is_server_error() {
        return Err(EquipmentFetchError::Observed(EquipmentFetchFailure {
            category: MailEquipmentDiagnosticCategory::ServerError,
            status: Some(status.as_u16()),
            elapsed_millis: elapsed_millis(started),
            body: Some(std::mem::take(&mut body.0)),
        }));
    }
    if !status.is_success() {
        return Err(EquipmentFetchError::Observed(EquipmentFetchFailure {
            category: MailEquipmentDiagnosticCategory::HttpStatus,
            status: Some(status.as_u16()),
            elapsed_millis: elapsed_millis(started),
            body: Some(std::mem::take(&mut body.0)),
        }));
    }
    if !is_html {
        return Err(EquipmentFetchError::Observed(EquipmentFetchFailure {
            category: MailEquipmentDiagnosticCategory::ContentType,
            status: Some(status.as_u16()),
            elapsed_millis: elapsed_millis(started),
            body: Some(std::mem::take(&mut body.0)),
        }));
    }
    Ok(EquipmentFetchSuccess {
        body: std::mem::take(&mut body.0),
        status: status.as_u16(),
        elapsed_millis: elapsed_millis(started),
    })
}

/// 外部账号请求指标的结果分类；与邮件请求同一闭集，不新增标签。
fn request_category(
    result: &Result<EquipmentFetchSuccess, EquipmentFetchError>,
) -> MailRequestCategory {
    let category = match result {
        Ok(_) => return MailRequestCategory::Ok,
        Err(EquipmentFetchError::SessionExpired) => return MailRequestCategory::SessionExpired,
        Err(EquipmentFetchError::Intake(_)) => return MailRequestCategory::RemoteTemporary,
        Err(EquipmentFetchError::Observed(failure)) => failure.category,
    };
    match category {
        MailEquipmentDiagnosticCategory::Configuration => MailRequestCategory::Configuration,
        MailEquipmentDiagnosticCategory::CredentialUnavailable => {
            MailRequestCategory::CredentialUnavailable
        }
        MailEquipmentDiagnosticCategory::SessionExpired => MailRequestCategory::SessionExpired,
        MailEquipmentDiagnosticCategory::RateLimited => MailRequestCategory::RateLimited,
        MailEquipmentDiagnosticCategory::Timeout
        | MailEquipmentDiagnosticCategory::Network
        | MailEquipmentDiagnosticCategory::ServerError => MailRequestCategory::RemoteTemporary,
        MailEquipmentDiagnosticCategory::ResponseTooLarge
        | MailEquipmentDiagnosticCategory::ContentEncoding => MailRequestCategory::ResponseTooLarge,
        MailEquipmentDiagnosticCategory::Redirect
        | MailEquipmentDiagnosticCategory::HttpStatus
        | MailEquipmentDiagnosticCategory::ContentType
        | MailEquipmentDiagnosticCategory::ProtocolDrift
        | MailEquipmentDiagnosticCategory::IdentityMismatch
        | MailEquipmentDiagnosticCategory::SnapshotInvalid
        | MailEquipmentDiagnosticCategory::FilterRejected => MailRequestCategory::ProtocolDrift,
    }
}
