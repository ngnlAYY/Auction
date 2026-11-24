//! 固定来源请求、状态分类及有界正文读取。

#[cfg(test)]
use std::cell::Cell;
use std::time::{Duration, SystemTime};

use auction_application::consignment::{
    ListingEquipmentLookupError, ListingLookupSnapshot, LookupPermanentReason, LookupRetryAfter,
};
use auction_domain::EquipmentKey;
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_time::Timestamp;
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, CONNECTION, CONTENT_ENCODING, CONTENT_TYPE, HeaderMap, HeaderName,
    RETRY_AFTER,
};
use reqwest::{Response, StatusCode};
use tokio::time::Instant;
use url::Url;

use super::HvDataEquipmentReader;
use super::json::parse_object;
use super::snapshot::{known_not_found, known_rejected_request, snapshot};

type Reason = LookupPermanentReason;
type LookupError = ListingEquipmentLookupError;

#[cfg(test)]
thread_local! {
    pub(super) static DECODE_CALLS: Cell<usize> = const { Cell::new(0) };
}

impl HvDataEquipmentReader {
    pub(super) async fn exchange(
        &self,
        identity: &ListingEquipmentIdentity,
        key: &EquipmentKey,
        end: Instant,
    ) -> Result<ListingLookupSnapshot, LookupError> {
        let target = request_target(&self.endpoint, identity, key)?;
        let response = self
            .client
            .get(target)
            .header(ACCEPT, "application/json")
            .header(ACCEPT_ENCODING, "identity")
            .header(CONNECTION, "close")
            .send()
            .await
            .map_err(transport_error)?;
        if !identity_encoding(response.headers()) {
            return Err(Reason::SourceHttpInvalid.into());
        }
        let status = response.status();
        match status {
            StatusCode::TOO_MANY_REQUESTS => {
                let hint = retry_after(response.headers(), SystemTime::now());
                return Err(LookupError::SourceRateLimited(hint));
            }
            status if status.is_server_error() => return Err(LookupError::SourceUnavailable),
            StatusCode::OK | StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => (),
            _ => return Err(Reason::SourceHttpInvalid.into()),
        }
        if !json_content_type(response.headers()) {
            return Err(Reason::SourceHttpInvalid.into());
        }
        let body = read_body(response, self.maximum_response_bytes).await?;
        // 正文读完后复核截止，避免开始已经逾期的同步解码。
        if Instant::now() >= end {
            return Err(LookupError::SourceTimeout);
        }
        decode_response(status, &body, identity, key, Timestamp::now())
    }
}

pub(super) fn decode_response(
    status: StatusCode,
    body: &[u8],
    identity: &ListingEquipmentIdentity,
    key: &EquipmentKey,
    observed_at: Timestamp,
) -> Result<ListingLookupSnapshot, LookupError> {
    #[cfg(test)]
    DECODE_CALLS.set(DECODE_CALLS.get() + 1);
    let object = parse_object(body)?;
    match status {
        StatusCode::OK => snapshot(&object, identity, key, observed_at),
        StatusCode::NOT_FOUND if known_not_found(&object) => Err(Reason::EquipmentNotFound.into()),
        StatusCode::UNPROCESSABLE_ENTITY if known_rejected_request(&object) => {
            Err(Reason::SourceRequestRejected.into())
        }
        _ => Err(Reason::SourceSchemaInvalid.into()),
    }
}

fn request_target(
    endpoint: &Url,
    identity: &ListingEquipmentIdentity,
    key: &EquipmentKey,
) -> Result<Url, LookupError> {
    let mut target = endpoint.clone();
    let world_flag = if identity.world_id() == "isekai" { "true" } else { "false" };
    let Ok(encoded) = key.with_bytes(|bytes| {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return Err(LookupError::CredentialUnavailable);
        };
        target
            .query_pairs_mut()
            .append_pair("eid", identity.external_equipment_id())
            .append_pair("key", text)
            .append_pair("is_isekai", world_flag);
        Ok(target)
    }) else {
        return Err(LookupError::CredentialUnavailable);
    };
    encoded
}

fn transport_error(error: reqwest::Error) -> LookupError {
    // 不变量 7：不保留或格式化 reqwest 错误，以免原错误携带带凭证的地址。
    if error.is_timeout() { LookupError::SourceTimeout } else { LookupError::SourceUnavailable }
}

fn one_header(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn identity_encoding(headers: &HeaderMap) -> bool {
    !headers.contains_key(CONTENT_ENCODING)
        || one_header(headers, CONTENT_ENCODING)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("identity"))
}

fn json_content_type(headers: &HeaderMap) -> bool {
    let Some(media_type) = one_header(headers, CONTENT_TYPE) else {
        return false;
    };
    let mut fields = media_type.split(';').map(str::trim);
    if !fields.next().is_some_and(|value| value.eq_ignore_ascii_case("application/json")) {
        return false;
    }
    let Some(parameter) = fields.next() else {
        return true;
    };
    parameter.split_once('=').is_some_and(|(name, value)| {
        name.trim().eq_ignore_ascii_case("charset")
            && (value.trim().eq_ignore_ascii_case("utf-8")
                || value.trim().eq_ignore_ascii_case("\"utf-8\""))
    }) && fields.next().is_none()
}

fn retry_after(headers: &HeaderMap, now: SystemTime) -> Option<LookupRetryAfter> {
    let raw = one_header(headers, RETRY_AFTER)?.trim();
    if !raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit()) {
        // 超出整数可表示范围的全数字提示也只取上限。
        let seconds = raw.parse().unwrap_or(u64::MAX);
        return Some(LookupRetryAfter::new(Duration::from_secs(seconds)));
    }
    let then = httpdate::parse_http_date(raw).ok()?;
    Some(LookupRetryAfter::new(then.duration_since(now).unwrap_or(Duration::ZERO)))
}

async fn read_body(mut response: Response, maximum: usize) -> Result<Vec<u8>, LookupError> {
    if response.content_length().is_some_and(|size| size > maximum as u64) {
        return Err(Reason::SourceTooLarge.into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if chunk.len() > maximum - body.len() {
            return Err(Reason::SourceTooLarge.into());
        }
        body.extend_from_slice(&chunk);
        // 缓冲数据可能连续就绪，主动给外层截止计时器轮询机会。
        tokio::task::consume_budget().await;
    }
    Ok(body)
}
