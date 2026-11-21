//! Public 寄售查询、路径与写入正文的有界解码，不回显请求原文。

use std::collections::BTreeMap;
use std::error::Error as _;
use std::net::SocketAddr;
use std::str::FromStr;

use auction_application::consignment::{
    ConsignmentError, ListingCategoryFilter, ListingNameQuery, ListingPageWindow,
    PrivateListingQuery, PublicListingQuery,
};
use auction_application::public::PublicSessionIdentity;
use axum::body::{Body, to_bytes};
use axum::extract::Path;
use axum::extract::rejection::PathRejection;
use axum::http::{HeaderMap, Uri};
use http_body_util::LengthLimitError;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::middleware::strict_json::application_json;
use crate::public::consignment::response::RouteError;
use crate::query::{parse_positive_i64, parse_public_page_size, parse_public_uuid, parse_query};
use crate::session::{authenticated_public_mutation_identity, session_guard};
use crate::state::PublicApiState;

const MAX_WRITE_BYTES: usize = 4 << 10;
type PathInput = Result<Path<String>, PathRejection>;

pub(super) fn no_query(uri: &Uri) -> Result<(), ConsignmentError> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(ConsignmentError::InvalidQuery);
    }
    Ok(())
}

pub(super) fn listing_id(path: PathInput) -> Result<Uuid, ConsignmentError> {
    path_uuid(path, ConsignmentError::InvalidListingId)
}

pub(super) fn command_id(path: PathInput) -> Result<Uuid, ConsignmentError> {
    path_uuid(path, ConsignmentError::InvalidCommandId)
}

fn path_uuid(path: PathInput, error: ConsignmentError) -> Result<Uuid, ConsignmentError> {
    let Ok(Path(raw)) = path else { return Err(error) };
    parse_public_uuid(&raw).map_err(|()| error)
}

pub(super) fn public_query(uri: &Uri) -> Result<PublicListingQuery, ConsignmentError> {
    let mut query = listing_query(uri, &["world", "mode", "category", "q", "page", "page_size"])?;
    Ok(PublicListingQuery {
        window: page_window(&query)?,
        world: enum_filter(&query, "world")?,
        mode: enum_filter(&query, "mode")?,
        category: query.remove("category").map(ListingCategoryFilter::new).transpose()?,
        name: query.remove("q").map(ListingNameQuery::new).transpose()?,
    })
}

pub(super) fn private_query(uri: &Uri) -> Result<PrivateListingQuery, ConsignmentError> {
    let query = listing_query(uri, &["world", "mode", "state", "page", "page_size"])?;
    Ok(PrivateListingQuery {
        window: page_window(&query)?,
        world: enum_filter(&query, "world")?,
        mode: enum_filter(&query, "mode")?,
        state: enum_filter(&query, "state")?,
    })
}

fn enum_filter<T: FromStr>(
    query: &BTreeMap<&str, String>,
    key: &str,
) -> Result<Option<T>, ConsignmentError> {
    let Ok(value) = query.get(key).map(|raw| raw.parse()).transpose() else {
        return Err(ConsignmentError::InvalidQuery);
    };
    Ok(value)
}

fn listing_query<'a>(
    uri: &Uri,
    allowed: &[&'a str],
) -> Result<BTreeMap<&'a str, String>, ConsignmentError> {
    // 共用查询解析器按表单协议做有损 UTF-8 解码；严格寄售合同要求非法编码失败关闭。
    if uri.query().is_some_and(|query| !valid_query_encoding(query)) {
        return Err(ConsignmentError::InvalidQuery);
    }
    parse_query(uri, allowed).map_err(|()| ConsignmentError::InvalidQuery)
}

fn valid_query_encoding(query: &str) -> bool {
    if !query.contains('%') {
        return true;
    }
    let mut decoded = Vec::with_capacity(query.len());
    let mut bytes = query.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let Some(high) = bytes.next().and_then(|byte| char::from(byte).to_digit(16)) else {
                return false;
            };
            let Some(low) = bytes.next().and_then(|byte| char::from(byte).to_digit(16)) else {
                return false;
            };
            decoded.push((high * 16 + low) as u8);
        } else {
            decoded.push(byte);
        }
    }
    std::str::from_utf8(&decoded).is_ok()
}

fn page_window(query: &BTreeMap<&str, String>) -> Result<ListingPageWindow, ConsignmentError> {
    let invalid = ConsignmentError::InvalidQuery;
    let page = parse_positive_i64(query.get("page"), 1, None).map_err(|()| invalid)?;
    let size = parse_public_page_size(query.get("page_size"), 20).map_err(|()| invalid)?;
    let (Ok(page), Ok(size)) = (u32::try_from(page), u32::try_from(size)) else {
        return Err(invalid);
    };
    ListingPageWindow::new(Some(page), Some(size))
}

pub(super) async fn private_identity(
    state: &PublicApiState,
    headers: &HeaderMap,
) -> Result<PublicSessionIdentity, RouteError> {
    Ok(authenticated_public_mutation_identity(state, headers).await?)
}

pub(super) async fn mutation_identity(
    state: &PublicApiState,
    headers: &HeaderMap,
    address: SocketAddr,
    uri: &Uri,
) -> Result<PublicSessionIdentity, RouteError> {
    // 不变量 1、6、7：守卫先于命令、查询、目标及正文解码，actor 只取经认证的 Cookie。
    if let Some(response) = session_guard(state, headers, address, true, false) {
        return Err(Box::new(response).into());
    }
    let identity = private_identity(state, headers).await?;
    no_query(uri)?;
    Ok(identity)
}

pub(super) async fn decode_write<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: Body,
) -> Result<T, RouteError> {
    if !application_json(headers) {
        return Err(RouteError::UnsupportedMediaType);
    }
    // 旧版 decode_after_guard 把字段错误折叠成 invalid_json。共享媒体类型和有界读体，
    // 此处保留严格 DTO 的固定字段分类；不记录正文及 serde 错误。
    let bytes = to_bytes(body, MAX_WRITE_BYTES).await.map_err(|error| {
        if error.source().is_some_and(<dyn std::error::Error>::is::<LengthLimitError>) {
            RouteError::BodyTooLarge
        } else {
            ConsignmentError::InvalidListingRequest.into()
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|error| write_error(&error).into())
}

fn write_error(error: &serde_json::Error) -> ConsignmentError {
    if error.is_data() {
        // 这些前缀由契约解码器生成。只识别固定分类，不转发任何错误文本。
        let diagnostic = error.to_string();
        if diagnostic.starts_with("command_id 字段") {
            return ConsignmentError::InvalidCommandId;
        }
        if diagnostic.starts_with("equipment_url 字段") {
            return ConsignmentError::InvalidEquipmentUrl;
        }
        if diagnostic.starts_with("price 字段") {
            return ConsignmentError::InvalidPrice;
        }
    }
    ConsignmentError::InvalidListingRequest
}
