//! 装备标记同源传输；身份、集合完整性和版本在边界严格校验。

use auction_contracts::{
    PublicLotWatchResponse, PublicLotWatchesResponse, PublicSetLotWatchRequest,
    PublicSetLotWatchResponse,
};
use gloo_net::http::Request;
use serde_json::Value;
use std::collections::BTreeSet;

use auction_web_shared::is_legacy_uuid as valid_uuid;
use auction_web_shared::wire::json_has_exact_keys as exact_fields;

use super::{CommandError, TransportError, decoder, send_get, send_request};

/// 整场集合有界且不能截断；缺项只有在完整权威读取后才表示 false/version 0。
pub(crate) async fn fetch_lot_watches(
    user_id: &str,
    auction_id: &str,
) -> Result<PublicLotWatchesResponse, CommandError> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("auction_id", auction_id)
        .finish();
    let response = send_get(&format!("/api/v1/me/lot-watches?{query}"))
        .await
        .map_err(|error| failure(error.code(), 0))?;
    let status = response.status();
    if !response.ok() {
        return Err(failure(&decoder::decode_error_code(response).await, status));
    }
    let page: PublicLotWatchesResponse = decoder::decode_success(response, |value| {
        exact_fields(value, &["user_id", "auction_id", "watches"])
            && value.get("watches").and_then(Value::as_array).is_some_and(|watches| {
                watches.len() <= 10_000 && watches.iter().all(valid_watch_value)
            })
    })
    .await
    .map_err(|_| failure("invalid_response", status))?;
    if !valid_watch_collection(&page, user_id, auction_id) {
        return Err(failure("invalid_response", status));
    }
    Ok(page)
}

/// 重试直接复用原 DTO，不重新生成 ID、目标状态或 CAS 版本。
pub(crate) async fn set_lot_watch(
    user_id: &str,
    auction_id: &str,
    lot_id: &str,
    command: &PublicSetLotWatchRequest,
) -> Result<PublicSetLotWatchResponse, CommandError> {
    let response = send_request(|abort_signal| {
        Request::put(&format!("/api/v1/lots/{lot_id}/watch"))
            .abort_signal(abort_signal)
            .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, user_id)
            .json(command)
            .map_err(|_| TransportError::Network(None))
    })
    .await
    .map_err(|error| failure(error.code(), 0))?;
    let status = response.status();
    if !response.ok() {
        return Err(failure(&decoder::decode_error_code(response).await, status));
    }
    let result: PublicSetLotWatchResponse = decoder::decode_success(response, |value| {
        exact_fields(value, &["user_id", "auction_id", "watch"])
            && value.get("watch").is_some_and(valid_watch_value)
    })
    .await
    .map_err(|_| failure("invalid_response", status))?;
    if result.user_id != user_id
        || result.auction_id != auction_id
        || result.watch.lot_id != lot_id
        || !valid_watch(&result.watch)
        || result.watch.watched != command.watched
        || result.watch.channel != command.channel
    {
        return Err(failure("invalid_response", status));
    }
    // 标记成功即产生新的权威状态；公开读取（尤其 lot 列表与详情）不得继续使用 TTL 旧值。
    super::swr_cache::invalidate_all();
    Ok(result)
}

/// 缓存与 HTTP 共用校验；重复 lot、跨身份、超限和无效墓碑都失败关闭。
pub(crate) fn valid_watch_collection(
    page: &PublicLotWatchesResponse,
    user_id: &str,
    auction_id: &str,
) -> bool {
    let mut lots = BTreeSet::new();
    page.user_id == user_id
        && page.auction_id == auction_id
        && valid_uuid(user_id)
        && valid_uuid(auction_id)
        && page.watches.len() <= 10_000
        && page.watches.iter().all(|watch| valid_watch(watch) && lots.insert(&watch.lot_id))
}

fn valid_watch(watch: &PublicLotWatchResponse) -> bool {
    valid_uuid(&watch.lot_id) && watch.channel == "mm" && watch.version > 0
}

fn valid_watch_value(value: &Value) -> bool {
    exact_fields(value, &["lot_id", "watched", "channel", "version"])
        && value.get("watched").is_some_and(Value::is_boolean)
        // watch 的版本契约是完整正 i64；原 JSON 整数直接解析，不经过 JS 浮点。
        && value.get("version").and_then(Value::as_i64).is_some_and(|version| version > 0)
}

fn failure(code: &str, status: u16) -> CommandError {
    super::session::observe_public_session_failure(status, code);
    CommandError { code: code.to_owned(), status }
}
