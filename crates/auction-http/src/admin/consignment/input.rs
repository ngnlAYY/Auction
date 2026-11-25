//! 守卫通过后的 Admin 挂单筛选、路径身份及下架正文解码。

use std::error::Error as _;
use std::str::FromStr;

use auction_application::consignment::{
    AdminDepositQuery, AdminListingQuery, ConsignmentError, ListingDelistReason, ListingPageWindow,
    PrivateListingQuery,
};
use auction_contracts::{AdminDelistListing, ListingVersion};
use axum::body::{Body, to_bytes};
use axum::extract::Path;
use axum::extract::rejection::PathRejection;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use http_body_util::LengthLimitError;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use super::boundary::{error, json_error};
use crate::middleware::strict_json::application_json;
use crate::query::{parse_canonical_uuid, parse_query};

const LIST_KEYS: [&str; 6] = ["world", "mode", "state", "seller_user_id", "page", "page_size"];
const DELIST_BODY_LIMIT: usize = 4 << 10;
const DEPOSIT_BODY_LIMIT: usize = 4 << 10;
const MAX_OBSERVATIONS: u16 = 200;

/// 仅把经过完整校验的闭集条件与有效态筛选交给查询端口。
pub(super) fn list_query(uri: &Uri) -> Result<AdminListingQuery, ConsignmentError> {
    let query = parse_query(uri, &LIST_KEYS).map_err(|()| ConsignmentError::InvalidQuery)?;
    let optional = |key| query.get(key).map(String::as_str);
    let page = optional("page").map(decimal).transpose()?;
    let page_size = optional("page_size").map(decimal).transpose()?;
    let seller_user_id = optional("seller_user_id")
        .map(|value| {
            parse_canonical_uuid(value).map(Into::into).ok_or(ConsignmentError::InvalidQuery)
        })
        .transpose()?;
    Ok(AdminListingQuery {
        listings: PrivateListingQuery {
            window: ListingPageWindow::new(page, page_size)?,
            world: filter(optional("world"))?,
            mode: filter(optional("mode"))?,
            state: filter(optional("state"))?,
        },
        seller_user_id,
    })
}

pub(super) fn decimal(raw: &str) -> Result<u32, ConsignmentError> {
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ConsignmentError::InvalidQuery);
    }
    parse(raw)
}

pub(super) fn filter<T: FromStr>(raw: Option<&str>) -> Result<Option<T>, ConsignmentError> {
    raw.map(parse).transpose()
}

fn parse<T: FromStr>(raw: &str) -> Result<T, ConsignmentError> {
    let Ok(value) = raw.parse() else {
        return Err(ConsignmentError::InvalidQuery);
    };
    Ok(value)
}

/// 详情和写入拒绝查询参数；路径解码失败只返回固定的挂单编号错误。
pub(super) fn listing_target(
    uri: &Uri,
    path: Result<Path<String>, PathRejection>,
) -> Result<Uuid, ConsignmentError> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(ConsignmentError::InvalidQuery);
    }
    if uri.path().contains('%') {
        return Err(ConsignmentError::InvalidListingId);
    }
    let Ok(Path(value)) = path else {
        return Err(ConsignmentError::InvalidListingId);
    };
    parse_canonical_uuid(&value).ok_or(ConsignmentError::InvalidListingId)
}

/// 隔离详情允许观察游标与页长；其他键或非法编码一律 400。
pub(super) struct DepositDetailQuery {
    pub(super) cursor: Option<Uuid>,
    pub(super) limit: u16,
}

impl DepositDetailQuery {
    pub(super) fn into_port(self, quarantine_id: Uuid) -> AdminDepositQuery {
        AdminDepositQuery { quarantine_id, cursor: self.cursor, limit: self.limit }
    }
}

pub(super) fn deposit_detail_query(uri: &Uri) -> Result<DepositDetailQuery, ConsignmentError> {
    let query = parse_query(uri, &["cursor", "limit"])
        .map_err(|()| ConsignmentError::InvalidDepositRequest)?;
    let cursor = query
        .get("cursor")
        .map(|value| parse_canonical_uuid(value).ok_or(ConsignmentError::InvalidDepositRequest))
        .transpose()?;
    let limit = match query.get("limit") {
        Some(value) => {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(ConsignmentError::InvalidDepositRequest);
            }
            value
                .parse::<u16>()
                .ok()
                .filter(|limit| (1..=MAX_OBSERVATIONS).contains(limit))
                .ok_or(ConsignmentError::InvalidDepositRequest)?
        }
        None => MAX_OBSERVATIONS,
    };
    Ok(DepositDetailQuery { cursor, limit })
}

/// 隔离路径只接受规范小写 UUID；写入前拒绝查询参数。
pub(super) fn deposit_target(
    uri: &Uri,
    path: Result<Path<String>, PathRejection>,
) -> Result<Uuid, ConsignmentError> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(ConsignmentError::InvalidDepositRequest);
    }
    deposit_path(path)
}

/// 隔离详情同样拒绝百分号编码，避免绕过路径规范化。
pub(super) fn deposit_path(
    path: Result<Path<String>, PathRejection>,
) -> Result<Uuid, ConsignmentError> {
    let Ok(Path(value)) = path else {
        return Err(ConsignmentError::InvalidDepositRequest);
    };
    if value.contains('%') {
        return Err(ConsignmentError::InvalidDepositRequest);
    }
    parse_canonical_uuid(&value).ok_or(ConsignmentError::InvalidDepositRequest)
}

/// v2 回执中的安全整数版本。
pub(super) fn listing_version(
    value: i64,
) -> Result<auction_contracts::ListingVersion, ConsignmentError> {
    auction_contracts::ListingVersion::new(value).ok_or(ConsignmentError::InvalidDepositRequest)
}

/// 送进应用命令的领域版本。
pub(super) fn domain_listing_version(
    value: i64,
) -> Result<auction_domain::consignment::ListingVersion, ConsignmentError> {
    let value = u64::try_from(value).map_err(invalid_deposit)?;
    auction_domain::consignment::ListingVersion::new(value).map_err(invalid_deposit)
}

fn invalid_deposit<T>(_error: T) -> ConsignmentError {
    ConsignmentError::InvalidDepositRequest
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DelistFields {
    command_id: serde_json::Value,
    expected_version: ListingVersion,
    reason: String,
}

/// 不变量 1、6、7：先严格验证规范身份、版本和私有说明，错误外壳不携带输入原文。
/// 复用 DTO 版本标量。字段级业务码需区分 UUID 和说明的失败，先取字段再调用各自的校验器。
pub(super) async fn delist_request(
    headers: &HeaderMap,
    body: Body,
) -> Result<AdminDelistListing, Box<Response>> {
    if !application_json(headers) {
        return Err(Box::new(json_error(
            headers,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "请求必须使用 JSON",
        )));
    }
    let bytes = match to_bytes(body, DELIST_BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(cause) => {
            let response = if cause
                .source()
                .is_some_and(<dyn std::error::Error + 'static>::is::<LengthLimitError>)
            {
                json_error(headers, StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", "请求体过大")
            } else {
                error(headers, ConsignmentError::InvalidListingRequest)
            };
            return Err(Box::new(response));
        }
    };
    parse_delist(&bytes).map_err(|failure| Box::new(error(headers, failure)))
}

/// 严格解码 v2 收件、隔离与止售写入；DTO 自行拒绝未知键、重复键与尾随值。
pub(super) async fn deposit_write<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: Body,
) -> Result<T, Box<Response>> {
    if !application_json(headers) {
        return Err(Box::new(json_error(
            headers,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "请求必须使用 JSON",
        )));
    }
    let bytes = match to_bytes(body, DEPOSIT_BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(cause) => {
            let response = if cause
                .source()
                .is_some_and(<dyn std::error::Error + 'static>::is::<LengthLimitError>)
            {
                json_error(headers, StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", "请求体过大")
            } else {
                error(headers, ConsignmentError::InvalidDepositRequest)
            };
            return Err(Box::new(response));
        }
    };
    if bytes.iter().copied().find(|byte| !byte.is_ascii_whitespace()) != Some(b'{') {
        return Err(Box::new(error(headers, ConsignmentError::InvalidDepositRequest)));
    }
    serde_json::from_slice(&bytes).map_err(|parse_error| {
        // 解码器细节可能含输入片段，只保留固定安全错误。
        drop(parse_error);
        Box::new(error(headers, ConsignmentError::InvalidDepositRequest))
    })
}

fn parse_delist(bytes: &[u8]) -> Result<AdminDelistListing, ConsignmentError> {
    // Serde 派生结构也接受位置数组，先限定根形状；解码器继续核重复键、未知键及尾随值。
    if bytes.iter().copied().find(|byte| !byte.is_ascii_whitespace()) != Some(b'{') {
        return Err(ConsignmentError::InvalidListingRequest);
    }
    // 解码错误可能包含输入原文，只映射固定错误。
    let Ok(input) = serde_json::from_slice::<DelistFields>(bytes) else {
        return Err(ConsignmentError::InvalidListingRequest);
    };
    let command_id = input
        .command_id
        .as_str()
        .and_then(parse_canonical_uuid)
        .ok_or(ConsignmentError::InvalidCommandId)?;
    // 用例仍会核验说明并把它纳入摘要。在此先校验以保留依赖缺失时的输入错误优先级。
    let reason = ListingDelistReason::new(input.reason)?;
    Ok(AdminDelistListing {
        command_id,
        expected_version: input.expected_version,
        reason: reason.as_str().to_owned(),
    })
}
