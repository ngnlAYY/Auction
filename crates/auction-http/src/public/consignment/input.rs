//! Public 寄售查询、路径与写入正文的有界解码，不回显请求原文。

use std::cell::Cell;
use std::collections::BTreeMap;
use std::error::Error as _;
use std::fmt;
use std::marker::PhantomData;
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
use serde::Deserializer;
use serde::de::value::MapAccessDeserializer;
use serde::de::{
    DeserializeOwned, DeserializeSeed, IgnoredAny, IntoDeserializer, MapAccess, Visitor,
};
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
    for key in ["category", "q"] {
        if let Some(value) = query.get(key) {
            let folded = value.to_ascii_lowercase();
            if ["://", "/equip/", "/equip?", "key="].iter().any(|part| folded.contains(part)) {
                return Err(ConsignmentError::InvalidQuery);
            }
        }
    }
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
    let bytes = to_bytes(body, MAX_WRITE_BYTES).await.map_err(|error| {
        if error.source().is_some_and(<dyn std::error::Error>::is::<LengthLimitError>) {
            RouteError::BodyTooLarge
        } else {
            ConsignmentError::InvalidListingRequest.into()
        }
    })?;
    Ok(decode_write_json(&bytes)?)
}

fn decode_write_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ConsignmentError> {
    let invalid = ConsignmentError::InvalidListingRequest;
    let field_error = Cell::new(invalid);
    let mut json = serde_json::Deserializer::from_slice(bytes);
    if let Ok(input) = json.deserialize_map(WriteObject::<T>(&field_error, PhantomData))
        && json.end().is_ok()
    {
        return Ok(input);
    }
    let field_error = field_error.get();
    // 不变量 6：DTO 会脱敏并包装字段值的解析错误。先用 IgnoredAny 核对结构，
    // 再解码所有键和值中的字符串以拒绝孤立代理项。数字只检查语法，保留溢出数字的
    // 字段分类。严格 DTO 始终解码原始对象，继续拒绝重复字段。
    if field_error != invalid
        && let Ok(json) = std::str::from_utf8(bytes)
        && serde_json::from_str::<IgnoredAny>(json).is_ok()
        && valid_json_strings(bytes)
    {
        return Err(field_error);
    }
    Err(invalid)
}

fn valid_json_strings(mut bytes: &[u8]) -> bool {
    while let Some(start) = bytes.iter().position(|&byte| byte == b'"') {
        let mut string =
            serde_json::Deserializer::from_slice(&bytes[start..]).into_iter::<String>();
        if !matches!(string.next(), Some(Ok(_))) {
            return false;
        }
        // 跳过已解码的整个词元，避免把字符串里的转义引号当成下一段的起点。
        bytes = &bytes[start + string.byte_offset()..];
    }
    true
}

struct WriteObject<'a, T>(&'a Cell<ConsignmentError>, PhantomData<T>);

impl<'de, T: DeserializeOwned> Visitor<'de> for WriteObject<'_, T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("寄售写入 JSON 对象")
    }

    fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
        T::deserialize(MapAccessDeserializer::new(WriteFields {
            map,
            current: ConsignmentError::InvalidListingRequest,
            failure: self.0,
        }))
    }
}

struct WriteFields<'a, M> {
    map: M,
    current: ConsignmentError,
    failure: &'a Cell<ConsignmentError>,
}

impl<'de, M: MapAccess<'de>> MapAccess<'de> for WriteFields<'_, M> {
    type Error = M::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, M::Error> {
        let Some(key) = self.map.next_key::<String>()? else { return Ok(None) };
        self.current = match key.as_str() {
            "command_id" => ConsignmentError::InvalidCommandId,
            "equipment_url" => ConsignmentError::InvalidEquipmentUrl,
            "price" => ConsignmentError::InvalidPrice,
            _ => ConsignmentError::InvalidListingRequest,
        };
        seed.deserialize(key.into_deserializer()).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, M::Error> {
        self.map.next_value_seed(seed).inspect_err(|_error| self.failure.set(self.current))
    }
}
