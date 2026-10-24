//! 查询参数、错误响应和请求游标解析辅助函数。

use super::super::*;

pub(crate) fn api_error(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> (StatusCode, Json<ApiErrorResponse>) {
    (
        status,
        Json(ApiErrorResponse {
            code: code.to_owned(),
            message: message.to_owned(),
            data: None,
            request_id: None,
        }),
    )
}

pub(crate) fn request_id(headers: &HeaderMap) -> Option<String> {
    boundary::request_id(headers)
}

pub(crate) fn parse_query(uri: &Uri, allowed: &[&str]) -> Result<BTreeMap<String, String>, ()> {
    let mut values = BTreeMap::new();
    let Some(query) = uri.query() else {
        return Ok(values);
    };
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if !allowed.iter().any(|allowed_key| *allowed_key == key)
            || values.insert(key.into_owned(), value.into_owned()).is_some()
        {
            return Err(());
        }
    }
    Ok(values)
}

pub(crate) fn parse_positive_i64(
    value: Option<&String>,
    default: i64,
    maximum: Option<i64>,
) -> Result<i64, ()> {
    let Some(value) = value else {
        return Ok(default);
    };
    if value.is_empty() {
        return Err(());
    }
    let parsed = value.parse::<i64>().map_err(|_| ())?;
    if parsed < 1 || maximum.is_some_and(|maximum| parsed > maximum) {
        return Err(());
    }
    Ok(parsed)
}

pub(crate) fn page_offset(page: i64, page_size: i64) -> Option<i64> {
    page.checked_sub(1)
        .and_then(|page| page.checked_mul(page_size))
        .filter(|offset| *offset <= MAX_PUBLIC_ACCOUNT_OFFSET)
}

pub(crate) fn valid_account_page(
    page: &PublicMyBidsPage,
    user_id: Uuid,
    request: &PublicMyBidsRequest,
) -> bool {
    let Ok(bid_count) = i64::try_from(page.bids.len()) else {
        return false;
    };
    let Ok(proxy_count) = i64::try_from(page.proxies.len()) else {
        return false;
    };
    page.bid_total >= 0
        && page.proxy_total >= 0
        && bid_count <= request.bids.limit
        && proxy_count <= request.proxies.limit
        && bid_count <= page.bid_total
        && proxy_count <= page.proxy_total
        && page.proxies.iter().all(|proxy| proxy.user_id == user_id)
}

pub(crate) fn parse_history_name_terms(value: Option<&String>) -> Result<Vec<String>, ()> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_empty() {
        return Ok(Vec::new());
    }
    if value.len() > 1024 {
        return Err(());
    }
    // 空白缩写与旧逗号参数共同拆成 AND 词条；包含匹配及 SQL 通配符转义仍由存储负责。
    let terms = value
        .split(|character: char| character == ',' || character.is_whitespace())
        .map(str::trim)
        .filter(|term| !term.is_empty())
        // 第九个非空词已足够判定超限，不再为剩余无效输入分配字符串。
        .take(MAX_PUBLIC_HISTORY_NAME_TERMS + 1)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if terms.len() > MAX_PUBLIC_HISTORY_NAME_TERMS {
        return Err(());
    }
    Ok(terms)
}

pub(crate) fn parse_optional_history_text(
    value: Option<&String>,
    maximum: usize,
) -> Result<Option<String>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > maximum || value.chars().any(char::is_control) {
        return Err(());
    }
    Ok(Some(value.clone()))
}

pub(crate) fn parse_history_date(
    value: Option<&String>,
    exclusive_next_day: bool,
) -> Result<Option<DateTime<Utc>>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value != value.trim() {
        return Err(());
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| ())?;
    if date.format("%Y-%m-%d").to_string() != *value {
        return Err(());
    }
    let midnight = date.and_hms_opt(0, 0, 0).ok_or(())?;
    let timestamp = Utc.from_utc_datetime(&midnight);
    if exclusive_next_day {
        timestamp.checked_add_signed(Duration::days(1)).map(Some).ok_or(())
    } else {
        Ok(Some(timestamp))
    }
}

pub(crate) fn parse_history_price(value: Option<&String>) -> Result<Option<i64>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return if value.is_empty() { Ok(None) } else { Err(()) };
    }
    let parsed = value.parse::<u64>().map_err(|_| ())?;
    if parsed > i64::MAX as u64 || parsed.to_string() != *value {
        return Err(());
    }
    Ok(Some(parsed as i64))
}

pub(crate) fn not_found_error() -> (StatusCode, Json<ApiErrorResponse>) {
    api_error(StatusCode::NOT_FOUND, "not_found", "请求的资源不存在")
}
