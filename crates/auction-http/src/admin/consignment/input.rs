//! 守卫通过后的 Admin 挂单筛选、路径身份及下架正文解码。

use std::error::Error as _;
use std::str::FromStr;

use auction_application::consignment::{
    AdminListingQuery, ConsignmentError, ListingDelistReason, ListingPageWindow,
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
use uuid::Uuid;

use super::boundary::{error, json_error};
use crate::middleware::strict_json::application_json;
use crate::query::{parse_canonical_uuid, parse_query};

const LIST_KEYS: [&str; 6] = ["world", "mode", "state", "seller_user_id", "page", "page_size"];
const DELIST_BODY_LIMIT: usize = 4 << 10;

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

fn decimal(raw: &str) -> Result<u32, ConsignmentError> {
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ConsignmentError::InvalidQuery);
    }
    parse(raw)
}

fn filter<T: FromStr>(raw: Option<&str>) -> Result<Option<T>, ConsignmentError> {
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
