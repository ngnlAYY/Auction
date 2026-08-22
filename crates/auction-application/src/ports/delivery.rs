use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::{ApplicationFuture, RequestHash};

/// Worker 领取的一条 delivery outbox 记录。
///
/// payload 是已经由业务 owner 持久化的无密文 JSON；传输适配器不得从 claim 外寻找凭据，也
/// 不得把 Cookie、密钥或完整响应写入日志。
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryWorkClaim {
    pub delivery_id: Uuid,
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub origin_kind: String,
    pub origin_id: String,
    pub recipient: String,
    pub payload: Value,
    pub lease_token: Uuid,
    pub lease_until: DateTime<Utc>,
    pub fencing_version: i64,
    pub attempt_count: u32,
    pub worker_id: String,
    pub claimed_at: DateTime<Utc>,
}

/// 外部传输成功后允许写入 delivery 的最小结果。
#[derive(Debug, Clone, PartialEq)]
pub struct DeliverySendResult {
    pub external_reference: String,
    pub result_metadata: Value,
}

/// 外部传输失败的安全分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryTransportError {
    pub failure_code: String,
    pub safe_detail: String,
    /// true 表示远端结果未知，不能自动重发，必须进入人工 reconcile。
    pub outcome_unknown: bool,
}

/// 持久化 delivery outbox 的传输端口。
pub trait DeliveryTransport: Send + Sync {
    fn send(
        &self,
        claim: DeliveryWorkClaim,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<DeliverySendResult, DeliveryTransportError>>
                + Send
                + '_,
        >,
    >;
}

/// delivery outbox 的窄存储端口。
pub trait DeliveryWorkStore: Send + Sync {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<DeliveryWorkClaim>>;

    fn start_sending(
        &self,
        claim: DeliveryWorkClaim,
        started_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    fn succeed(
        &self,
        claim: DeliveryWorkClaim,
        result: DeliverySendResult,
        completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    fn fail(
        &self,
        claim: DeliveryWorkClaim,
        error: DeliveryTransportError,
        completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;
}
