//! Worker 投递的领取、发送围栏与传输端口。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::errors::diagnostics::SafeErrorKind;
use crate::ports::core::ApplicationFuture;
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

/// 发送前只读准备耗尽 Worker 准备预算的失败码。Worker 调度与传输适配器共用这一个 owner：
/// 调度据此归为超时诊断，通知指标归为准备阻断；改名须同时评估两处归类。
pub const DELIVERY_PREFLIGHT_TIMEOUT: &str = "preflight_timeout";

impl DeliveryTransportError {
    /// 只读准备在预算内未完成：尚未开始远端写入，只能释放 claim 延后重试。超时不证明任何
    /// 资金、凭据或远端状态，不得归为现金授权失败或写入未知。
    pub fn preflight_timeout() -> Self {
        Self {
            failure_code: DELIVERY_PREFLIGHT_TIMEOUT.to_owned(),
            safe_detail: "delivery 只读准备超出预算，尚未开始远端写入".to_owned(),
            outcome_unknown: false,
            external_reference: None,
            result_metadata: serde_json::json!({"stage": "preflight", "write_started": false}),
        }
    }
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

/// 授权准备的返回类型；成功句柄携带可选内部证据与冻结身份，句柄内的发送 future 在 sending fence
/// 前不得 poll。
pub type AuthorizedDeliveryPreparation<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<AuthorizedDeliverySend<'a>, DeliveryTransportError>>
            + Send
            + 'a,
    >,
>;

/// sending fence 的确定结论，含义由存储层按失败原因精确产出；只有 `Sending` 允许 poll。
/// 提交结果未知不在此列，由端口返回 `Err` 表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryFenceOutcome {
    /// sending 已确定提交。
    Sending,
    /// 业务门确定未满足（竞价警告失效、发送节奏未到期、claim 已失效、资金 owner 拒绝，或证据
    /// owner 判定凭据与运行时 owner 在观察之后正常推进），事务确定回滚或只提交了冻结事实：正常
    /// 延期，不是发送成功，也不是错误。
    Deferred,
    /// 内部故障（SQL 错误、冲突重试耗尽、提交预算耗尽、请求与冻结身份或持久不变量不符，以及
    /// 证据结构、generation、时效或证据与 claim、凭据绑定不符）后事务已确定回滚或从未开始：没有
    /// 写入 sending，可以安全释放 claim，但调用方必须按所带安全分类保留错误信号与计数，不能当作
    /// 正常延期。
    Faulted(SafeErrorKind),
}

/// 持久化 delivery outbox 的传输端口。
pub trait DeliveryTransport: Send + Sync {
    /// 返回 transport 在单个 Worker 进程内允许同时进入 `sending` 的上限。
    /// 配置并发仍是全局上限，不需要额外收紧的 transport 返回 `u16::MAX`；拥有单一远端可变
    /// 会话的 transport 应进一步收紧，避免任务先进入 `sending` 再在适配器内部排队并消耗
    /// 请求超时。实现必须返回大于零的值。
    fn maximum_concurrency(&self) -> u16;

    /// 在 delivery 进入 `sending` 前判断目标 world 当前是否有可用凭据。
    /// 无效 payload 必须返回 true，让正式发送路径把它收口为确定失败；false 只表示当前外部
    /// 能力暂不可用，Worker 应释放 claim 而不是消耗交付。
    fn available(&self, claim: &DeliveryWorkClaim) -> bool;

    /// 在 sending fence 前执行只读前置，成功句柄必须保留与后续写入相同的互斥上下文。
    /// 无远端可变 compose 状态的适配器可直接返回发送句柄；非法负载在句柄执行时确定失败。
    fn prepare(&self, claim: DeliveryWorkClaim) -> DeliveryPreparation<'_>;

    /// Worker 的发送前准备入口。`deadline` 是 Worker 准备请求的原绝对截止，覆盖独占 guard 排队
    /// 与全部只读请求，不逐步重置；它不是租约截止——租约由调用方在准备期间持续续租守住，续租
    /// 失败时调用方直接丢弃本 future。整体越过 `deadline` 必须返回
    /// [`DeliveryTransportError::preflight_timeout`]；只有资金证据确实未能证明时才可报现金授权失败，
    /// 现金证据自身的寿命上限仍由适配器在读取证据时另行收紧。
    fn prepare_authorized(
        &self,
        claim: DeliveryWorkClaim,
        deadline: std::time::Instant,
    ) -> AuthorizedDeliveryPreparation<'_>;

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
    /// 订阅提交后的非权威唤醒提示；不具备通知能力的实现返回 `CapabilityUnavailable`，
    /// Worker 只保留周期轮询，不伪造通知能力。
    fn subscribe_wakeups(
        &self,
    ) -> ApplicationFuture<'_, Box<dyn crate::ports::delivery::DeliveryWakeup>>;

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

    /// 每一轮领取前执行一次过期回收与退役扫描。
    /// 清理不改变领取资格，只把已经失效的工作从待发送状态移出。
    fn begin_claim_cycle(
        &self,
        now: DateTime<Utc>,
        sending_grace: std::time::Duration,
    ) -> ApplicationFuture<'_, ()>;

    /// 证据借自原prepared/commit guard。Err表示未确定提交状态，调用方必须撤销owner。
    /// 业务门确定拒绝或冻结用Deferred，内部故障确定回滚用Faulted；None仍须经过数据库legacy
    /// 保护门。实现不得忽略冻结身份而沿用未证明的 Send 或 Take 授权。
    fn start_sending_authorized<'a>(
        &'a self,
        claim: DeliveryWorkClaim,
        started_at: DateTime<Utc>,
        evidence: Option<&'a crate::ports::settlement::SettlementInternalEvidenceSnapshot>,
        expectation: Option<&'a crate::ports::delivery::DeliveryPreparedExpectation>,
    ) -> ApplicationFuture<'a, DeliveryFenceOutcome>;

    /// 现金 fence 沿用原 prepared 单调截止，并收紧至本次租约/提交预算。
    /// 仅期限前确定 Sending 才允许外发；Err/超时须撤销 owner，Deferred 与 Faulted 表示确定结束。
    /// 仅 Send 可进入现金授权；实现不得忽略提交期限或接受 Take。
    fn start_sending_authorized_until<'a>(
        &'a self,
        claim: DeliveryWorkClaim,
        started_at: DateTime<Utc>,
        evidence: &'a crate::ports::settlement::SettlementInternalEvidenceSnapshot,
        deadline: std::time::Instant,
        expectation: Option<&'a crate::ports::delivery::DeliveryPreparedExpectation>,
    ) -> ApplicationFuture<'a, DeliveryFenceOutcome>;

    /// 只用于发送前确定的 invalid_payload；不得伪造 sending 或释放既有资金占用。
    fn reject_preparation(
        &self,
        claim: DeliveryWorkClaim,
        completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

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
