//! Admin realtime 运维端口。
//! 这里仅暴露事件元数据、lag、failure 的安全投影和带幂等命令的显式 replay。原始事件
//! payload、数据库行和驱动错误留在存储 owner；replay 必须在同一事务中追加新事件并解决
//! 原 failure。

use chrono::{DateTime, Utc};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use super::{ApplicationFuture, RequestHash};

/// Admin realtime 查询允许的最大单页大小。
pub const MAX_REALTIME_ADMIN_PAGE_SIZE: i64 = 200;

/// Admin realtime 数字分页允许的最大偏移量，避免深分页把数据库退化为无界扫描。
pub const MAX_REALTIME_ADMIN_OFFSET: i64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeEventListRequest {
    pub stream_key: Option<String>,
    pub kind: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl RealtimeEventListRequest {
    /// 校验有界筛选与分页；事件 payload 不属于该查询边界。
    pub fn validate(&self) -> bool {
        valid_optional_text(self.stream_key.as_deref(), 256)
            && valid_optional_text(self.kind.as_deref(), 128)
            && self.limit > 0
            && self.limit <= MAX_REALTIME_ADMIN_PAGE_SIZE
            && (0..=MAX_REALTIME_ADMIN_OFFSET).contains(&self.offset)
    }
}

/// 已持久化 realtime 事件的安全元数据投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeEventSnapshot {
    pub event_id: Uuid,
    pub stream_key: String,
    pub stream_sequence: i64,
    pub kind: String,
    pub schema_version: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeEventPage {
    pub entries: Vec<RealtimeEventSnapshot>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// Admin realtime 查询和 replay 的稳定业务错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeAdminError {
    #[error("realtime Admin 命令参数无效")]
    InvalidCommand,
    #[error("realtime failure 不存在")]
    NotFound,
    #[error("realtime failure 已经解决")]
    AlreadyResolved,
    #[error("realtime replay 与原 failure 不匹配")]
    ReplayMismatch,
    #[error("realtime Admin 命令与既有命令冲突")]
    CommandConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeLagListRequest {
    pub stream_prefix: Option<String>,
    pub consumer_key: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl RealtimeLagListRequest {
    /// 校验有界的 lag 查询。
    pub fn validate(&self) -> bool {
        valid_optional_text(self.stream_prefix.as_deref(), 256)
            && valid_optional_text(self.consumer_key.as_deref(), 128)
            && self.limit > 0
            && self.limit <= MAX_REALTIME_ADMIN_PAGE_SIZE
            && (0..=MAX_REALTIME_ADMIN_OFFSET).contains(&self.offset)
    }
}

/// 单个 consumer/stream 的安全 lag 投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeLagSnapshot {
    pub stream_key: String,
    pub consumer_key: String,
    pub offset: i64,
    pub latest_sequence: i64,
    pub lag: i64,
    pub last_advanced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeLagPage {
    pub entries: Vec<RealtimeLagSnapshot>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// realtime 未解决 failure 的安全水位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeFailureWater {
    pub count: i64,
    pub oldest_at: Option<DateTime<Utc>>,
    pub latest_code: String,
}

impl RealtimeFailureWater {
    /// 校验计数、最早时间和最新错误码之间的一致性。
    pub fn validate(&self) -> bool {
        if self.count < 0
            || self.oldest_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
        {
            return false;
        }
        if self.count == 0 {
            self.oldest_at.is_none() && self.latest_code.is_empty()
        } else {
            self.oldest_at.is_some() && valid_failure_code(Some(&self.latest_code))
        }
    }
}

/// realtime dashboard 的全量安全摘要。
/// total_lag 汇总同一只读快照内全部 consumer/stream；lags 只保留有界第一页。
/// failure 只保留未解决水位，不包含正文和事件 payload。详细列表由 realtime Admin 专用端点提供。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeDashboardSnapshot {
    pub observed_at: DateTime<Utc>,
    pub total_lag: i64,
    pub lags: Vec<RealtimeLagSnapshot>,
    pub failure: RealtimeFailureWater,
}

impl RealtimeDashboardSnapshot {
    /// 校验观察时间、全量及明细 lag 数值和稳定排序，防止损坏投影进入 dashboard。
    pub fn validate(&self) -> bool {
        if self.observed_at.timestamp_nanos_opt().is_none()
            || self.total_lag < 0
            || !i64::try_from(self.lags.len())
                .is_ok_and(|lag_count| lag_count <= MAX_REALTIME_ADMIN_PAGE_SIZE)
            || !self.failure.validate()
        {
            return false;
        }
        let mut previous: Option<(&str, &str)> = None;
        let mut page_lag = 0_i64;
        for lag in &self.lags {
            if lag.offset < 0
                || lag.latest_sequence < lag.offset
                || lag.lag != lag.latest_sequence - lag.offset
                || lag.last_advanced_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
                || !valid_required_text(&lag.stream_key, 256)
                || !valid_required_text(&lag.consumer_key, 128)
                || previous.is_some_and(|(consumer, stream)| {
                    lag.consumer_key.as_str() < consumer
                        || (lag.consumer_key == consumer && lag.stream_key.as_str() <= stream)
                })
            {
                return false;
            }
            let Some(next_page_lag) = page_lag.checked_add(lag.lag) else {
                return false;
            };
            page_lag = next_page_lag;
            previous = Some((&lag.consumer_key, &lag.stream_key));
        }
        page_lag <= self.total_lag
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeFailureListRequest {
    pub stream_key: Option<String>,
    pub consumer_key: Option<String>,
    pub code: Option<String>,
    pub resolved: Option<bool>,
    pub limit: i64,
    pub offset: i64,
}

impl RealtimeFailureListRequest {
    /// 校验 failure 筛选和分页边界。
    pub fn validate(&self) -> bool {
        valid_optional_text(self.stream_key.as_deref(), 256)
            && valid_optional_text(self.consumer_key.as_deref(), 128)
            && valid_failure_code(self.code.as_deref())
            && self.limit > 0
            && self.limit <= MAX_REALTIME_ADMIN_PAGE_SIZE
            && (0..=MAX_REALTIME_ADMIN_OFFSET).contains(&self.offset)
    }
}

/// realtime failure 的安全管理投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeFailureSnapshot {
    pub id: Uuid,
    pub event_id: Uuid,
    pub stream_key: String,
    pub consumer_key: String,
    pub code: String,
    pub safe_detail: String,
    pub failed_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

impl RealtimeFailureSnapshot {
    /// 返回是否已经解决；浏览器不需要理解数据库的 nullable 语义。
    pub const fn resolved(&self) -> bool {
        self.resolved_at.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeFailurePage {
    pub entries: Vec<RealtimeFailureSnapshot>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeReplayCommand {
    pub command_id: Uuid,
    pub failure_id: Uuid,
    pub schema_version: i32,
    pub public_payload: Value,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
    pub created_at: DateTime<Utc>,
}

impl RealtimeReplayCommand {
    /// 校验 replay 的幂等标识、审计关联和公共 payload 边界。
    pub fn validate(&self) -> bool {
        !self.command_id.is_nil()
            && !self.failure_id.is_nil()
            && self.schema_version > 0
            && self.public_payload.is_object()
            && valid_required_text(&self.actor_id, 128)
            && valid_optional_text(self.request_id.as_deref(), 128)
            && self.created_at.timestamp_micros() != 0
    }
}

/// replay 成功后的安全事件投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeReplayResult {
    pub command_id: Uuid,
    pub event_id: Uuid,
    pub replay_of_event_id: Uuid,
    pub stream_key: String,
    pub stream_sequence: i64,
    pub kind: String,
    pub schema_version: i32,
    pub created_at: DateTime<Utc>,
}

pub trait RealtimeAdminPort: Send + Sync {
    /// 读取旧版 Admin dashboard 所需的 lag/failure 安全摘要。
    fn dashboard_snapshot(&self) -> ApplicationFuture<'_, RealtimeDashboardSnapshot>;

    /// 按新到旧读取事件元数据；不得返回 payload 或内部来源标识。
    fn list_events(
        &self,
        request: RealtimeEventListRequest,
    ) -> ApplicationFuture<'_, RealtimeEventPage>;

    fn list_lag(&self, request: RealtimeLagListRequest) -> ApplicationFuture<'_, RealtimeLagPage>;

    fn list_failures(
        &self,
        request: RealtimeFailureListRequest,
    ) -> ApplicationFuture<'_, RealtimeFailurePage>;

    fn get_failure(
        &self,
        failure_id: Uuid,
    ) -> ApplicationFuture<'_, Option<RealtimeFailureSnapshot>>;

    fn replay(&self, command: RealtimeReplayCommand)
    -> ApplicationFuture<'_, RealtimeReplayResult>;
}

fn valid_optional_text(value: Option<&str>, max_len: usize) -> bool {
    value.is_none_or(|value| auction_domain::valid_text(value, max_len))
}

fn valid_required_text(value: &str, max_len: usize) -> bool {
    valid_optional_text(Some(value), max_len)
}

fn valid_failure_code(value: Option<&str>) -> bool {
    valid_optional_text(value, 64)
        && value.is_none_or(|value| !value.chars().any(char::is_whitespace))
}
