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
    /// true 表示不能安全自动重发或终结，必须进入人工 reconcile。
    ///
    /// 除远端结果未知外，也包括收件人不存在但退信资产尚未取回等已证明拒绝场景。
    pub outcome_unknown: bool,
    /// 远端明确失败时仍可能返回的安全引用，例如退信 `mid`。
    pub external_reference: Option<String>,
    /// 由 transport 产生的低基数安全元数据；不得包含请求 payload、Cookie 或原始响应。
    pub result_metadata: Value,
}

/// 持久化 delivery outbox 的传输端口。
pub trait DeliveryTransport: Send + Sync {
    /// 在 delivery 进入 `sending` 前判断目标 world 当前是否有可用凭据。
    ///
    /// 无效 payload 必须返回 true，让正式发送路径把它收口为确定失败；false 只表示当前外部
    /// 能力暂不可用，Worker 应释放 claim 而不是消耗交付。
    fn available(&self, claim: &DeliveryWorkClaim) -> bool;

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
        sending_grace: std::time::Duration,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<DeliveryWorkClaim>>;

    fn start_sending(
        &self,
        claim: DeliveryWorkClaim,
        started_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    /// 在尚未开始外部写入时把有效 claim 释放回 pending。
    fn release(
        &self,
        claim: DeliveryWorkClaim,
        released_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    /// 外部发送尚未完成时续租当前 sending claimant。
    fn renew(
        &self,
        claim: DeliveryWorkClaim,
        renewed_at: DateTime<Utc>,
        lease_until: DateTime<Utc>,
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
