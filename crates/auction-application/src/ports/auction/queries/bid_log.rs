//! Public 竞价日志查询端口。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ApplicationFuture;

/// 公开竞价日志允许的最大页大小。
pub const MAX_PUBLIC_BID_LOG_PAGE_SIZE: i64 = 200;

/// 公开竞价日志兼容页允许的最大 offset。
pub const MAX_PUBLIC_BID_LOG_OFFSET: i64 = 10_000;

/// 单场公开竞价日志的筛选与分页条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBidLogListRequest {
    pub auction_id: Uuid,
    pub world_id: Option<String>,
    pub lot_id: Option<Uuid>,
    pub limit: i64,
    pub offset: i64,
    pub cursor: Option<PublicBidLogCursor>,
}

impl PublicBidLogListRequest {
    /// 在应用端口边界校验资源标识、筛选文本和两种分页模式的互斥关系。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.auction_id.is_nil()
            || self
                .world_id
                .as_deref()
                .is_some_and(|world| world.is_empty() || world.len() > 64 || world != world.trim())
            || self.lot_id.is_some_and(|lot_id| lot_id.is_nil())
            || self.limit < 1
            || self.limit > MAX_PUBLIC_BID_LOG_PAGE_SIZE
            || self.offset < 0
            || self.offset > MAX_PUBLIC_BID_LOG_OFFSET
            || self.cursor.is_some() && self.offset != 0
        {
            return Err("公开竞价日志分页参数无效");
        }
        if let Some(PublicBidLogCursor::Boundary {
            created_at,
            lot_id,
            accepted_state_version,
            fact_index,
            id,
        }) = self.cursor.as_ref()
            && (created_at.timestamp_micros() == 0
                || lot_id.is_nil()
                || *accepted_state_version <= 0
                || *fact_index <= 0
                || id.is_nil())
        {
            return Err("公开竞价日志游标无效");
        }
        Ok(())
    }
}

/// 公开竞价日志的完整降序 keyset 边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicBidLogCursor {
    Start,
    Boundary {
        created_at: DateTime<Utc>,
        lot_id: Uuid,
        accepted_state_version: i64,
        fact_index: i32,
        id: Uuid,
    },
}

/// 公开竞价日志条目；不包含代理上限、Cookie 或其他私有竞价状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBidLogEntry {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub lot_number: i64,
    pub amount: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    pub bidder_name: String,
    /// 展示来源闭集：论坛普通出价为 `forum`，所有代理事实统一为 `proxy`。
    pub source: Option<String>,
    /// 论坛普通出价对应的真实楼层号；历史无法证明时保持为空。
    pub source_post_number: Option<i32>,
    pub valid: bool,
    pub accepted_state_version: i64,
    pub fact_index: i32,
    pub mechanism: String,
    pub created_at: DateTime<Utc>,
}

/// 公开竞价日志分页结果；cursor 页不计算全量 total。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBidLogPage {
    pub entries: Vec<PublicBidLogEntry>,
    pub total: Option<i64>,
    pub limit: i64,
    pub offset: i64,
    pub next_cursor: Option<PublicBidLogCursor>,
    pub has_more: bool,
}

/// Public 竞价日志只读端口。
pub trait PublicBidLogReader: Send + Sync {
    fn list_public_bid_log(
        &self,
        request: PublicBidLogListRequest,
    ) -> ApplicationFuture<'_, Option<PublicBidLogPage>>;
}
