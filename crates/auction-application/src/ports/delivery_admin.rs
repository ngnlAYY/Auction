use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApplicationFuture, RequestHash};

/// Admin 交付列表允许的最大单页大小。
pub const MAX_DELIVERY_ADMIN_PAGE_SIZE: i64 = 100;

/// Admin 交付列表查询条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminListRequest {
    pub status: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl DeliveryAdminListRequest {
    /// 校验有界查询，避免 Admin 端通过未限制的 offset 扫描整个 outbox。
    pub fn validate(&self) -> bool {
        self.limit > 0
            && self.limit <= MAX_DELIVERY_ADMIN_PAGE_SIZE
            && self.offset >= 0
            && self.status.as_ref().is_none_or(|status| valid_delivery_status(status))
    }
}

/// 交付 Admin 允许的人工状态动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryAdminAction {
    Release,
    RetryFailed,
    ReconcileRetry,
    ReconcileSucceeded,
    ReconcileFailed,
}

impl DeliveryAdminAction {
    /// 返回持久化命令账本使用的稳定动作名。
    pub const fn command_kind(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::RetryFailed => "retry_failed",
            Self::ReconcileRetry | Self::ReconcileSucceeded | Self::ReconcileFailed => "reconcile",
        }
    }
}

/// delivery outbox 的安全 Admin 投影。
///
/// 不包含 payload、命令摘要、lease token 或原始 result metadata；这些字段不能进入管理端
/// 浏览器响应，避免把外部协议或敏感路由细节扩散到 UI。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryAdminSnapshot {
    pub id: Uuid,
    pub origin_kind: String,
    pub origin_id: String,
    pub recipient: String,
    pub status: String,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub fencing_version: i64,
    pub attempt_count: u32,
    pub claimed_at: Option<DateTime<Utc>>,
    pub sending_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub external_reference: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Admin 交付列表结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminPage {
    pub entries: Vec<DeliveryAdminSnapshot>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// Admin 交付状态命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminCommand {
    pub command_id: Uuid,
    pub delivery_id: Uuid,
    pub expected_fencing_version: i64,
    pub action: DeliveryAdminAction,
    pub external_reference: Option<String>,
    pub failure_code: Option<String>,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

/// Admin 交付命令结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryAdminResult {
    pub command_id: Uuid,
    pub snapshot: DeliveryAdminSnapshot,
}

/// 交付 Admin 查询与人工状态命令端口。
pub trait DeliveryAdminPort: Send + Sync {
    fn list(&self, request: DeliveryAdminListRequest) -> ApplicationFuture<'_, DeliveryAdminPage>;

    fn execute(&self, command: DeliveryAdminCommand) -> ApplicationFuture<'_, DeliveryAdminResult>;
}

fn valid_delivery_status(value: &str) -> bool {
    matches!(
        value,
        "prepared"
            | "pending"
            | "claimed"
            | "sending"
            | "succeeded"
            | "failed"
            | "manual_reconcile"
    )
}
