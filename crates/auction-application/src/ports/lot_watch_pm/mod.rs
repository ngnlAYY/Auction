//! 装备 PM 独立队列端口；发送与只读核对的持久授权不可互换。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::diagnostics::{DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error};
use crate::{ForumPmMessage, ForumPmReceipt, LotWatchFuture, RequestHash};

/// 与 users 身份约束一致的规范论坛数字 ID，不按显示名投递。
pub fn valid_forum_pm_id(value: &str) -> bool {
    auction_domain::valid_positive_decimal(value, 32)
}

/// 冻结内容摘要，独立于当前账号偏好。
pub fn forum_pm_digest(message: &ForumPmMessage) -> Result<RequestHash, crate::LotWatchError> {
    if message.notification_id.is_nil()
        || !valid_forum_pm_id(&message.recipient_forum_account_id)
        || !message.title.starts_with("Outbid ")
        || message.title.len() > 160
        || !message.title.is_ascii()
        || message.title.contains(['[', ']', '<', '>'])
        || message.body.len() < 2
        || message.body.len() > 2048
        || !message.body.is_ascii()
        || message.body.contains(['[', ']', '<', '>'])
        || !message.body.contains(&format!("Notification: {}.", message.notification_id))
    {
        return Err(crate::LotWatchError::Invalid);
    }
    RequestHash::from_request("lot_watch_pm", message.notification_id, message).map_err(|error| {
        record_error(
            DiagnosticLevel::Error,
            error.diagnostic_kind(),
            "forum_pm",
            "hash_notification",
            DiagnosticContext::default(),
        );
        crate::LotWatchError::Invalid
    })
}

/// 持久领取证明，不能只凭队列 ID 完成发送。
#[derive(Debug, Clone)]
pub struct LotWatchPmClaim {
    pub message: ForumPmMessage,
    pub lot_id: Uuid,
    /// 来源 lot 所属拍卖的权威世界；不改已冻结 PM 正文或历史摘要。
    pub world: auction_domain::SettlementWorld,
    pub digest: Vec<u8>,
    pub token: Uuid,
    pub fence: i64,
    pub sender: Option<String>,
    pub reconcile: bool,
}

/// 每次领取最多一项；时间以数据库为准。
#[derive(Debug, Clone)]
pub struct LotWatchPmClaimRequest {
    pub worker_id: String,
    pub lease_seconds: i64,
    pub reconcile: bool,
    /// 按实际论坛 Cookie scope 过滤候选；必须在队列 LIMIT 前应用。
    pub available_worlds: Vec<auction_domain::SettlementWorld>,
}

/// PM 存储只允许未 sending 的释放；过期 sending 永不重新发送。
pub trait LotWatchPmWorkStore: Send + Sync {
    fn claim(&self, request: LotWatchPmClaimRequest)
    -> LotWatchFuture<'_, Option<LotWatchPmClaim>>;
    fn start_sending(&self, claim: LotWatchPmClaim, sender: String) -> LotWatchFuture<'_, bool>;
    fn release_unsent(&self, claim: LotWatchPmClaim) -> LotWatchFuture<'_, ()>;
    /// receipt 为 None 表示未知，不表示失败可重试；只允许原 sender 的精确副本。
    fn finish(
        &self,
        claim: LotWatchPmClaim,
        receipt: Option<ForumPmReceipt>,
    ) -> LotWatchFuture<'_, ()>;
}

/// 运维安全投影，不返回正文、收件人、摘要或租约 token。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LotWatchPmSnapshot {
    pub notification_id: Uuid,
    pub lot_id: Uuid,
    pub status: String,
    pub fencing_version: i64,
    pub sender_forum_account_id: Option<String>,
    pub failure_code: Option<String>,
    pub sending_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub reconcile_requested_at: Option<DateTime<Utc>>,
}

/// 运维命令仅申请只读核对，不创建第二个论坛客户端。
#[derive(Debug, Clone, Serialize)]
pub struct RequestLotWatchPmReconcile {
    pub command_id: auction_domain::CommandId,
    pub notification_id: Uuid,
    pub expected_fencing_version: i64,
    pub actor: String,
    /// 主机身份不改变原命令的幂等摘要。
    #[serde(skip)]
    pub operator_identity: Option<crate::CliOperatorIdentity>,
}

/// 无重发、强制成功或废弃动作。
pub trait LotWatchPmOperations: Send + Sync {
    fn list(
        &self,
        status: Option<String>,
        limit: i64,
        offset: i64,
    ) -> LotWatchFuture<'_, Vec<LotWatchPmSnapshot>>;
    fn request_reconcile(
        &self,
        command: RequestLotWatchPmReconcile,
    ) -> LotWatchFuture<'_, LotWatchPmSnapshot>;
}
