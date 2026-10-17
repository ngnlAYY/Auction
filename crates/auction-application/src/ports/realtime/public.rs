//! Public 实时事件回放端口。
//! 这里定义事件信封、游标和页面边界，不暴露数据库行、SQL 类型或连接细节。事件注册表
//! 也在应用边界校验，存储适配器遇到未知 schema 时必须要求客户端回读权威快照。

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use uuid::{Uuid, Variant};

use super::ApplicationFuture;

/// Public 事件回放允许的最大页大小。
pub const MAX_PUBLIC_EVENT_PAGE_SIZE: i64 = 200;

/// 浏览器 JSON number 能无损表达的最大整数；公开事件不能把 i64 精度损失留给客户端处理。
const MAX_PUBLIC_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Go `time.Time{}` 的 UTC Unix 秒；不能把合法 Unix epoch（0 秒）误判成旧版零时间。
const GO_ZERO_TIME_UNIX_SECONDS: i64 = -62_135_596_800;

/// Public 事件回放查询。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicRealtimeRequest {
    pub auction_id: Option<Uuid>,
    pub after: i64,
    pub page_size: i64,
}

impl PublicRealtimeRequest {
    /// 在应用端口边界拒绝负游标、空拍卖标识和无界页大小。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.auction_id.is_some_and(|id| id.is_nil())
            || self.after < 0
            || self.page_size < 1
            || self.page_size > MAX_PUBLIC_EVENT_PAGE_SIZE
        {
            return Err("实时事件回放参数无效");
        }
        Ok(())
    }
}

/// 已通过注册表校验的公开实时事件信封。
#[derive(Debug, Clone, PartialEq)]
pub struct PublicRealtimeEvent {
    pub event_id: Uuid,
    pub stream_key: String,
    pub stream_sequence: i64,
    pub kind: String,
    pub schema_version: i32,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

impl PublicRealtimeEvent {
    /// 校验公开事件的标识、流名和注册表字段集合。
    /// 事件负载必须是精确字段集合。这样旧事件、数据库误写字段或未完成迁移的内部事件
    /// 不会被 HTTP 层当作可安全消费的公共消息直接转发。
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_public_uuid(self.event_id)
            || !positive_integer(self.stream_sequence)
            || self.stream_key.trim().is_empty()
            || self.kind.trim().is_empty()
            || self.schema_version <= 0
            || !valid_created_at(&self.created_at)
        {
            return Err("实时事件信封无效");
        }
        if !valid_stream_key(&self.stream_key) {
            return Err("实时事件流名无效");
        }
        if self.stream_key == "auction:discovery" && self.kind != "auction.created" {
            return Err("实时发现流事件种类无效");
        }

        let Some(fields) = registered_fields(&self.kind, self.schema_version) else {
            return Err("实时事件 schema 未注册");
        };
        let Some(object) = self.payload.as_object() else {
            return Err("实时事件负载必须是对象");
        };
        if object.len() != fields.len()
            || fields.iter().any(|field| {
                object.get(*field).is_none_or(Value::is_null) || !object.contains_key(*field)
            })
        {
            return Err("实时事件负载字段不符合注册表");
        }
        // 旧版 codec 在公开投影前同时校验 JSON 类型、范围与闭集。这里恢复同一防火墙，
        // 避免损坏的持久事件先进入 HTTP、WebSocket 或 relay，再依赖某个浏览器兜底拒绝。
        if !valid_registered_payload(&self.kind, self.schema_version, object) {
            return Err("实时事件负载值不符合注册表");
        }
        if let Some(stream_auction_id) = stream_auction_id(&self.stream_key) {
            let Some(payload_auction_id) = object.get("auction_id").and_then(Value::as_str) else {
                return Err("实时事件负载与拍卖流不匹配");
            };
            if stream_auction_id != payload_auction_id {
                return Err("实时事件负载与拍卖流不匹配");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublicRealtimePage {
    pub events: Vec<PublicRealtimeEvent>,
    pub next_after: i64,
    pub has_more: bool,
    pub refetch_required: bool,
}

pub trait PublicRealtimeReader: Send + Sync {
    fn list_public_events(
        &self,
        request: PublicRealtimeRequest,
    ) -> ApplicationFuture<'_, Option<PublicRealtimePage>>;
}

fn valid_stream_key(stream_key: &str) -> bool {
    if stream_key == "auction:discovery" {
        return true;
    }
    stream_key.strip_prefix("auction:").is_some_and(valid_public_uuid_string)
}

fn stream_auction_id(stream_key: &str) -> Option<&str> {
    stream_key.strip_prefix("auction:").filter(|id| *id != "discovery")
}

fn registered_fields(kind: &str, schema_version: i32) -> Option<&'static [&'static str]> {
    match (kind, schema_version) {
        ("auction.created", 1) => Some(&["auction_id", "world_id", "sequence", "state_version"]),
        ("auction.preview_updated", 1) => {
            Some(&["auction_id", "preview_revision", "item_count", "manual_open", "state_version"])
        }
        ("preview_catalog_changed", 1) => Some(&["world", "auction_id", "preview_revision"]),
        ("auction.opened", 1) => {
            Some(&["auction_id", "world_id", "state_version", "preview_revision"])
        }
        ("auction.schedule_updated", 1) => {
            Some(&["auction_id", "world_id", "scheduled_end_at", "state_version"])
        }
        ("auction.final_phase_entered", 1)
        | ("auction.early_settlement_requested", 1)
        | ("auction.cancelled", 1)
        | ("auction.settling", 1) => Some(&["auction_id", "state_version"]),
        ("auction.settled", 1) => {
            Some(&["auction_id", "sold_count", "unsold_count", "state_version"])
        }
        ("auction.settled", 2) => Some(&[
            "auction_id",
            "state_version",
            "policy_version",
            "sold_count",
            "unsold_count",
            "obligation_count",
        ]),
        ("lot.bid_accepted", 2) => Some(&[
            "auction_id",
            "lot_id",
            "bid_id",
            "amount",
            "effective_end",
            "state_version",
            "fact_index",
            "mechanism",
        ]),
        _ => None,
    }
}

fn valid_registered_payload(kind: &str, schema_version: i32, payload: &Map<String, Value>) -> bool {
    let auction_id = || valid_uuid_value(payload.get("auction_id"));
    match (kind, schema_version) {
        ("auction.created", 1) => {
            auction_id()
                && valid_world(payload.get("world_id"))
                && positive_value(payload.get("sequence"))
                && positive_value(payload.get("state_version"))
        }
        ("auction.preview_updated", 1) => {
            auction_id()
                && nonnegative_value(payload.get("preview_revision"))
                && nonnegative_value(payload.get("item_count"))
                && payload.get("manual_open").is_some_and(Value::is_boolean)
                && positive_value(payload.get("state_version"))
        }
        ("preview_catalog_changed", 1) => {
            valid_world(payload.get("world"))
                && auction_id()
                && positive_value(payload.get("preview_revision"))
        }
        ("auction.opened", 1) => {
            auction_id()
                && valid_world(payload.get("world_id"))
                && positive_value(payload.get("state_version"))
                && positive_value(payload.get("preview_revision"))
        }
        ("auction.schedule_updated", 1) => {
            auction_id()
                && valid_world(payload.get("world_id"))
                && valid_time(payload.get("scheduled_end_at"))
                && positive_value(payload.get("state_version"))
        }
        (
            "auction.final_phase_entered"
            | "auction.early_settlement_requested"
            | "auction.cancelled"
            | "auction.settling",
            1,
        ) => auction_id() && positive_value(payload.get("state_version")),
        ("auction.settled", 1) => {
            auction_id()
                && nonnegative_value(payload.get("sold_count"))
                && nonnegative_value(payload.get("unsold_count"))
                && positive_value(payload.get("state_version"))
        }
        ("auction.settled", 2) => {
            auction_id()
                && positive_value(payload.get("state_version"))
                && nonnegative_value(payload.get("policy_version"))
                && nonnegative_value(payload.get("sold_count"))
                && nonnegative_value(payload.get("unsold_count"))
                && integer_value(payload.get("sold_count"))
                    .zip(integer_value(payload.get("unsold_count")))
                    .and_then(|(sold, unsold)| sold.checked_add(unsold))
                    .is_some_and(|total| total > 0)
                && positive_value(payload.get("obligation_count"))
        }
        ("lot.bid_accepted", 2) => {
            auction_id()
                && valid_uuid_value(payload.get("lot_id"))
                && valid_uuid_value(payload.get("bid_id"))
                && positive_value(payload.get("amount"))
                && valid_time(payload.get("effective_end"))
                && positive_value(payload.get("state_version"))
                && positive_value(payload.get("fact_index"))
                && matches!(
                    payload.get("mechanism").and_then(Value::as_str),
                    Some("ordinary" | "proxy")
                )
        }
        _ => false,
    }
}

fn integer_value(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_i64)
        .filter(|value| (-MAX_PUBLIC_SAFE_INTEGER..=MAX_PUBLIC_SAFE_INTEGER).contains(value))
}

fn positive_integer(value: i64) -> bool {
    (1..=MAX_PUBLIC_SAFE_INTEGER).contains(&value)
}

fn positive_value(value: Option<&Value>) -> bool {
    integer_value(value).is_some_and(positive_integer)
}

fn nonnegative_value(value: Option<&Value>) -> bool {
    integer_value(value).is_some_and(|value| value >= 0)
}

fn valid_public_uuid(value: Uuid) -> bool {
    !value.is_nil()
        && (1..=5).contains(&value.get_version_num())
        && value.get_variant() == Variant::RFC4122
}

fn valid_public_uuid_string(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| uuid.to_string() == value && valid_public_uuid(uuid))
}

fn valid_uuid_value(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(valid_public_uuid_string)
}

fn valid_world(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|value| {
        !value.is_empty()
            && value.len() <= 64
            && value == value.trim()
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
    })
}

fn valid_time(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty() && DateTime::parse_from_rfc3339(value).is_ok())
}

fn valid_created_at(value: &DateTime<Utc>) -> bool {
    value.timestamp() != GO_ZERO_TIME_UNIX_SECONDS || value.timestamp_subsec_nanos() != 0
}
