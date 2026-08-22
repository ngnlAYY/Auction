//! Public 实时事件回放端口。
//!
//! 这里定义事件信封、游标和页面边界，不暴露数据库行、SQL 类型或连接细节。事件注册表
//! 也在应用边界校验，存储适配器遇到未知 schema 时必须要求客户端回读权威快照。

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::ApplicationFuture;

/// Public 事件回放允许的最大页大小。
pub const MAX_PUBLIC_EVENT_PAGE_SIZE: i64 = 200;

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
    ///
    /// 事件负载必须是精确字段集合。这样旧事件、数据库误写字段或未完成迁移的内部事件
    /// 不会被 HTTP 层当作可安全消费的公共消息直接转发。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.event_id.is_nil()
            || self.stream_sequence <= 0
            || self.stream_key.trim().is_empty()
            || self.kind.trim().is_empty()
            || self.schema_version <= 0
            || self.created_at.timestamp_micros() == 0
        {
            return Err("实时事件信封无效");
        }
        if !valid_stream_key(&self.stream_key) {
            return Err("实时事件流名无效");
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
        Ok(())
    }
}

/// Public 事件回放页。
#[derive(Debug, Clone, PartialEq)]
pub struct PublicRealtimePage {
    pub events: Vec<PublicRealtimeEvent>,
    pub next_after: i64,
    pub has_more: bool,
    pub refetch_required: bool,
}

/// Public 持久化实时事件读取端口。
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
    stream_key
        .strip_prefix("auction:")
        .and_then(|id| Uuid::parse_str(id).ok())
        .is_some_and(|id| !id.is_nil())
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
        ("auction.final_phase_entered", 1) | ("auction.cancelled", 1) | ("auction.settling", 1) => {
            Some(&["auction_id", "state_version"])
        }
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
