//! Worker 投递的领取、发送围栏与传输端口。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::ports::auction::ApplicationFuture;
use crate::ports::core::RequestHash;

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
    /// outbox 创建时间；用于按通知种类记录送达延迟，不参与发送判定。
    pub created_at: DateTime<Utc>,
}

/// 当前 delivery 领取轮次中暂不再领取的 MoogleMail 收件人。
/// 玩家名称只在对应 world 内比较，避免主世界的失败误跳过 Isekai 同名玩家。该值不进入
/// 持久状态；下一轮 Worker 仍会重新判断尚未处理的 delivery。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRecipientExclusion {
    pub world: String,
    pub recipient: String,
}

/// 当前领取轮次的暂时排除项；不修改 delivery 权威状态或冻结负载。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeliveryClaimExclusions {
    /// 当前没有可用传输凭据的世界；下一轮重新检查可用性。
    pub unavailable_worlds: Vec<String>,
    /// 本轮只读准备或发送前交接失败的世界；只暂缓 Send，不阻断独立 Take/Return。
    pub unavailable_send_worlds: Vec<String>,
    /// 本轮已被远端明确报告不存在的收件人。
    pub recipients: Vec<DeliveryRecipientExclusion>,
    /// 已被 runtime 关闭的出站通知分类；匹配的投递保留在 outbox 中，重新开启后补发。
    /// 关闭只暂停通知，不改变已提交的命令、履约义务或审计事实。
    pub suppressed_classes: crate::ports::mail::MailSendClasses,
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
    /// 除远端结果未知外，也包括收件人不存在但退信资产尚未取回等已证明拒绝场景；
    /// 未携带 Credits、CoD 或装备的零资产通知没有可取回的退信资产，收件人不存在只按确定失败收口。
    pub outcome_unknown: bool,
    /// 远端明确失败时仍可能返回的安全引用，例如退信 `mid`。
    pub external_reference: Option<String>,
    /// 由 transport 产生的低基数安全元数据；不得包含请求 payload、Cookie 或原始响应。
    pub result_metadata: Value,
}

/// 已完成只读前置、但尚未开始外部写入的消耗式发送句柄。
/// 句柄持有适配器的凭据独占上下文；仅在持久 sending fence 成功且续租确认后 poll 一次。
/// 丢弃句柄只释放上下文，不执行远端写入。
pub type DeliveryPreparedSend<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<DeliverySendResult, DeliveryTransportError>>
            + Send
            + 'a,
    >,
>;

/// 只读准备结果；Err 一律表示尚未外部写入，应释放 claim 而非记 outcome_unknown。
pub type DeliveryPreparation<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<DeliveryPreparedSend<'a>, DeliveryTransportError>>
            + Send
            + 'a,
    >,
>;

/// 授权准备保留同一独占会话；证据只借用，发送 future 在 fence 前不能 poll。
pub struct AuthorizedDeliverySend<'a> {
    send: DeliveryPreparedSend<'a>,
    evidence: Option<crate::ports::settlement::PreparedSettlementInternalEvidence>,
    expectation: Option<crate::ports::delivery::DeliveryPreparedExpectation>,
    cancellation: DeliveryFenceCancellation,
}

/// 即使legacy没有内部证据，可能提交的fence取消也须撤销同一mail owner。
#[derive(Default)]
struct DeliveryFenceCancellation {
    lease: Option<Box<dyn crate::ports::settlement::InternalEvidenceLease>>,
    armed: bool,
}

impl Drop for DeliveryFenceCancellation {
    fn drop(&mut self) {
        if self.armed
            && let Some(lease) = &self.lease
        {
            lease.revoke();
        }
    }
}

impl<'a> AuthorizedDeliverySend<'a> {
    /// 可信适配器组装；None 只表示没有内部现金证据，不授予绕过 Send/Take 冻结身份的能力。
    pub fn new(
        send: DeliveryPreparedSend<'a>,
        evidence: Option<crate::ports::settlement::PreparedSettlementInternalEvidence>,
    ) -> Self {
        Self {
            send,
            evidence,
            expectation: None,
            cancellation: DeliveryFenceCancellation::default(),
        }
    }

    /// 发送 future 必须消费生成此身份的同一渲染请求，不在 fence 后重读模板或密钥。
    pub fn with_send_expectation(
        mut self,
        expectation: Arc<crate::ports::delivery::DeliverySendExpectation>,
    ) -> Self {
        self.expectation =
            Some(crate::ports::delivery::DeliveryPreparedExpectation::Send(expectation));
        self
    }

    /// Take future 必须持有生成此身份的原会话，不能在 sending fence 后重新选择账号。
    pub fn with_take_expectation(
        mut self,
        expectation: Arc<crate::ports::delivery::DeliveryTakeExpectation>,
    ) -> Self {
        self.expectation =
            Some(crate::ports::delivery::DeliveryPreparedExpectation::Take(expectation));
        self
    }

    /// 只借用不可变身份；复制枚举只增加其内部单层 Arc 计数，不复制内容或会话。
    pub fn expectation(&self) -> Option<&crate::ports::delivery::DeliveryPreparedExpectation> {
        self.expectation.as_ref()
    }

    /// 使用原session的撤销能力；不取得第二个operation guard。
    pub fn with_fence_lease(
        mut self,
        lease: Box<dyn crate::ports::settlement::InternalEvidenceLease>,
    ) -> Self {
        self.cancellation.lease = Some(lease);
        self
    }

    /// 只读准备已结束，即将进入可能提交的数据库fence。
    pub fn begin_fence(&mut self) {
        self.cancellation.armed = true;
    }

    /// 只用于已确定提交/回滚；提交未知保留armed直到drop同步撤销。
    pub fn confirm_fence(&mut self) {
        self.cancellation.armed = false;
    }

    pub fn evidence_mut(
        &mut self,
    ) -> Option<&mut crate::ports::settlement::PreparedSettlementInternalEvidence> {
        self.evidence.as_mut()
    }

    /// 仅在持久 fence 及续租确认后消费；丢弃证据引用，原 session 仍由发送 future 持有。
    pub fn into_send(self) -> DeliveryPreparedSend<'a> {
        self.send
    }
}

/// 授权准备的返回类型；默认实现包装旧 `prepare`，旧适配器和测试无需改变签名。
pub type AuthorizedDeliveryPreparation<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<AuthorizedDeliverySend<'a>, DeliveryTransportError>>
            + Send
            + 'a,
    >,
>;

/// 只有 Sending 允许 poll；Deferred 已确定提交冻结或确定回滚，不是发送成功。
pub enum DeliveryFenceOutcome {
    Sending,
    Deferred,
}

/// 持久化 delivery outbox 的传输端口。
pub trait DeliveryTransport: Send + Sync {
    /// 返回 transport 在单个 Worker 进程内允许同时进入 `sending` 的上限。
    /// 配置并发仍是全局上限；拥有单一远端可变会话的 transport 可以进一步收紧，避免任务先进入
    /// `sending` 再在适配器内部排队并消耗请求超时。实现必须返回大于零的值。
    fn maximum_concurrency(&self) -> u16 {
        u16::MAX
    }

    /// 在 delivery 进入 `sending` 前判断目标 world 当前是否有可用凭据。
    /// 无效 payload 必须返回 true，让正式发送路径把它收口为确定失败；false 只表示当前外部
    /// 能力暂不可用，Worker 应释放 claim 而不是消耗交付。
    fn available(&self, claim: &DeliveryWorkClaim) -> bool;

    /// 在 sending fence 前执行只读前置，成功句柄必须保留与后续写入相同的互斥上下文。
    /// 无远端可变 compose 状态的适配器沿用直接发送；非法负载在句柄执行时确定失败。
    fn prepare(&self, claim: DeliveryWorkClaim) -> DeliveryPreparation<'_> {
        Box::pin(async move { Ok(self.send(claim)) })
    }

    /// deadline 包含独占 guard 排队与全部只读请求，不逐步重置。
    fn prepare_authorized(
        &self,
        claim: DeliveryWorkClaim,
        _deadline: std::time::Instant,
    ) -> AuthorizedDeliveryPreparation<'_> {
        Box::pin(async move {
            self.prepare(claim).await.map(|send| AuthorizedDeliverySend::new(send, None))
        })
    }

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
    /// 订阅提交后的非权威唤醒提示；旧适配器保留周期轮询，不伪造通知能力。
    fn subscribe_wakeups(
        &self,
    ) -> ApplicationFuture<'_, Box<dyn crate::ports::delivery::DeliveryWakeup>> {
        Box::pin(async {
            Err(crate::errors::ApplicationError::CapabilityUnavailable(
                "delivery wakeup subscription",
            ))
        })
    }

    /// 领取可发送任务；compose 安全由 transport 在 sending fence 前实时只读检查。
    /// 历史未知结果保留人工核对，但不能按历史错误永久阻断同 world 新任务。
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        sending_grace: std::time::Duration,
        exclusions: DeliveryClaimExclusions,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<DeliveryWorkClaim>>;

    /// 每一轮领取前执行一次过期回收与退役扫描；默认适配器保持无操作。
    /// 清理不改变领取资格，只把已经失效的工作从待发送状态移出。
    fn begin_claim_cycle(
        &self,
        _now: DateTime<Utc>,
        _sending_grace: std::time::Duration,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    /// 旧入口只允许无需冻结身份的 Return；Send 与 Take 必须经过各自准备身份授权。
    fn start_sending(
        &self,
        claim: DeliveryWorkClaim,
        started_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    /// 证据借自原prepared/commit guard。Err表示未确定提交状态，调用方必须撤销owner。
    /// 确定拒绝或冻结用Deferred；None仍须经过数据库legacy保护门。
    /// 旧适配器不得忽略冻结身份而沿用未证明的 Send 或 Take 授权。
    fn start_sending_authorized<'a>(
        &'a self,
        claim: DeliveryWorkClaim,
        started_at: DateTime<Utc>,
        evidence: Option<&'a crate::ports::settlement::SettlementInternalEvidenceSnapshot>,
        expectation: Option<&'a crate::ports::delivery::DeliveryPreparedExpectation>,
    ) -> ApplicationFuture<'a, DeliveryFenceOutcome> {
        Box::pin(async move {
            if evidence.is_some()
                || expectation.is_some()
                || matches!(
                    claim.payload.get("operation").and_then(Value::as_str),
                    Some("send" | "take")
                )
            {
                return Err(crate::errors::ApplicationError::CapabilityUnavailable(
                    "frozen delivery authorization",
                ));
            }
            self.start_sending(claim, started_at).await?;
            Ok(DeliveryFenceOutcome::Sending)
        })
    }

    /// 现金 fence 沿用原 prepared 单调截止，并收紧至本次租约/提交预算。
    /// 仅期限前确定 Sending 才允许外发；Err/超时须撤销 owner，Deferred 表示确定结束。
    /// 仅 Send 可进入现金授权；默认明确不支持，不因编译兼容而忽略提交期限或接受 Take。
    fn start_sending_authorized_until<'a>(
        &'a self,
        _claim: DeliveryWorkClaim,
        _started_at: DateTime<Utc>,
        _evidence: &'a crate::ports::settlement::SettlementInternalEvidenceSnapshot,
        _deadline: std::time::Instant,
        _expectation: Option<&'a crate::ports::delivery::DeliveryPreparedExpectation>,
    ) -> ApplicationFuture<'a, DeliveryFenceOutcome> {
        Box::pin(async move {
            Err(crate::errors::ApplicationError::CapabilityUnavailable(
                "deadline-bound delivery cash authorization",
            ))
        })
    }

    /// 只用于发送前确定的 invalid_payload；不得伪造 sending 或释放既有资金占用。
    fn reject_preparation(
        &self,
        _claim: DeliveryWorkClaim,
        _completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async {
            Err(crate::errors::ApplicationError::CapabilityUnavailable(
                "delivery preparation rejection",
            ))
        })
    }

    /// 在尚未开始外部写入时把有效 claim 释放回 pending，并撤销本次领取的尝试计数。
    fn release(
        &self,
        claim: DeliveryWorkClaim,
        released_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    /// 为只读准备中的 claimed 或尚未完成的 sending 续租，不改变状态。
    /// 必须匹配当前 token/fencing，且旧租约在调用时刻及数据库实际执行时均有效；
    /// 过期、已更换 owner 或终态记录不得复活。新截止必须仍在未来且不得缩短现有租约。
    /// 调用方在准备总预算内续租，并在 sending fence 前后确认有效租期后才 poll 外部写入。
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
