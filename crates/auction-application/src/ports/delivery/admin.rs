use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::{ApplicationFuture, RequestHash};

/// Admin 交付列表允许的最大单页大小。
pub const MAX_DELIVERY_ADMIN_PAGE_SIZE: i64 = 100;
/// Admin 交付投影允许显示的最大 COD 整数；与旧版 JSON 安全整数边界一致。
pub const MAX_DELIVERY_COD: i64 = 9_007_199_254_740_991;

/// Delivery Admin 查询与命令使用的稳定业务错误。
///
/// 该类型只携带闭集分类，不跨应用边界暴露数据库驱动或持久化正文。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAdminError {
    #[error("delivery Admin 命令参数无效")]
    Invalid,
    #[error("delivery 不存在")]
    NotFound,
    #[error("delivery 状态或 fencing version 已变化")]
    Stale,
    #[error("delivery Admin 命令与既有命令冲突")]
    CommandConflict,
    #[error("delivery Admin 持久化结果损坏")]
    StoredResultCorrupt,
    #[error("delivery Admin 依赖暂不可用")]
    Unavailable,
}

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

    /// 按旧版已发布的字段顺序和 Go `encoding/json` 规则计算 Admin 命令摘要。
    ///
    /// `request_id` 有意不进入摘要。release/retry 只包含 delivery、fence 和 actor；reconcile
    /// 还包含动作及两个互斥结果字段，并把缺失值编码为空字符串。该字节合同必须保持稳定，
    /// 否则新旧实例滚动并存时会把同一命令误判为冲突。
    pub fn request_hash(
        self,
        delivery_id: Uuid,
        expected_fencing_version: i64,
        external_reference: Option<&str>,
        failure_code: Option<&str>,
        actor_id: &str,
    ) -> RequestHash {
        let mut payload = Vec::with_capacity(256);
        payload.extend_from_slice(b"{\"delivery_id\":");
        push_go_json_string(&mut payload, &delivery_id.to_string());
        payload.extend_from_slice(b",\"expected_fencing_version\":");
        payload.extend_from_slice(expected_fencing_version.to_string().as_bytes());

        if let Some(action) = self.reconcile_action_name() {
            payload.extend_from_slice(b",\"action\":");
            push_go_json_string(&mut payload, action);
            payload.extend_from_slice(b",\"external_reference\":");
            push_go_json_string(&mut payload, external_reference.unwrap_or_default());
            payload.extend_from_slice(b",\"failure_code\":");
            push_go_json_string(&mut payload, failure_code.unwrap_or_default());
        }

        payload.extend_from_slice(b",\"actor_id\":");
        push_go_json_string(&mut payload, actor_id);
        payload.push(b'}');
        RequestHash::from_canonical_bytes(&payload)
    }

    const fn reconcile_action_name(self) -> Option<&'static str> {
        match self {
            Self::Release | Self::RetryFailed => None,
            Self::ReconcileRetry => Some("retry"),
            Self::ReconcileSucceeded => Some("succeeded"),
            Self::ReconcileFailed => Some("failed"),
        }
    }
}

/// delivery outbox 的安全 Admin 投影。
///
/// 不包含 payload、命令摘要、lease token 或原始 result metadata；仅保留旧版契约允许的
/// `reason_code`、`take_required` 和 `returned_mail_id` 安全派生字段，避免把外部协议或敏感
/// 路由细节扩散到 UI。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryAdminSnapshot {
    pub id: Uuid,
    pub origin_kind: String,
    pub origin_id: String,
    pub recipient: String,
    /// 仅从版本化 `send` payload 提取的金额，不跨端口传递 payload 原文。
    pub cod: Option<i64>,
    pub status: String,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub fencing_version: i64,
    pub attempt_count: u32,
    pub claimed_at: Option<DateTime<Utc>>,
    pub sending_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub external_reference: Option<String>,
    pub failure_code: Option<String>,
    pub reason_code: Option<String>,
    pub take_required: bool,
    pub returned_mail_id: Option<String>,
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

/// Go `encoding/json.Marshal` 默认启用 HTML-safe string；摘要必须逐字兼容该历史行为。
fn push_go_json_string(output: &mut Vec<u8>, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    output.push(b'"');
    for character in value.chars() {
        match character {
            '"' => output.extend_from_slice(br#"\""#),
            '\\' => output.extend_from_slice(br#"\\"#),
            '\u{0008}' => output.extend_from_slice(br"\b"),
            '\u{000c}' => output.extend_from_slice(br"\f"),
            '\n' => output.extend_from_slice(br"\n"),
            '\r' => output.extend_from_slice(br"\r"),
            '\t' => output.extend_from_slice(br"\t"),
            '<' => output.extend_from_slice(br"\u003c"),
            '>' => output.extend_from_slice(br"\u003e"),
            '&' => output.extend_from_slice(br"\u0026"),
            '\u{2028}' => output.extend_from_slice(br"\u2028"),
            '\u{2029}' => output.extend_from_slice(br"\u2029"),
            character if character <= '\u{001f}' => {
                let value = character as u8;
                output.extend_from_slice(br"\u00");
                output.push(HEX[usize::from(value >> 4)]);
                output.push(HEX[usize::from(value & 0x0f)]);
            }
            character => {
                let mut encoded = [0_u8; 4];
                output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
    output.push(b'"');
}
