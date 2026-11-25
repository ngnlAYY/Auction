//! 寄售派发 executor 与补寄消费端的持久来源端口：领取寄售义务、发送请求只读读口、补寄待办读口，
//! 以及在授权前为补寄新轮备好投递的写端。
//!
//! 本端口只回答“从哪里取工作、取哪些冻结事实”；授权、登记尝试、起写与证据仍只经
//! [`super::DispatchStore`]，外写许可只由起写首次提交铸造。返回值不含装备 key、装备 URL、
//! 预约码或任何凭据。

use auction_domain::consignment::FundingAccountingMode;
use auction_domain::consignment::dispatch::{DispatchRoundKey, FrozenDispatchIdentity};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::errors::ConsignmentFuture;

/// 一项已领取的寄售派发工作：寄售义务的履约 claim 与该义务的当前轮。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchWorkClaim {
    /// 被派发的寄售义务。
    pub obligation_id: Uuid,
    /// 领取写在履约行上的 token。
    pub claim_token: Uuid,
    /// 领取写在履约行上的 fence。
    pub claim_fencing: i64,
    /// 要派发的轮（该义务的最新轮）。
    pub key: DispatchRoundKey,
    /// 轮上冻结的派发身份。
    pub frozen: FrozenDispatchIdentity,
}

/// 发送一封已衔接轮投递所需的冻结事实（只读）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchSendRequest {
    /// 轮的投递。
    pub delivery_id: Uuid,
    /// 轮业务键（合同、义务与轮次）。
    pub key: DispatchRoundKey,
    /// 投递冻结的收件人。
    pub recipient: String,
    /// 轮上冻结的派发身份：世界、Bot UID、主题、模板、正文摘要、轮引用、D0、Credits、COD 与附件装备。
    pub frozen: FrozenDispatchIdentity,
    /// 渲染正文所用的装备显示名（挂单登记时的安全快照）；传输须以冻结的正文摘要核对渲染结果，
    /// 不符即不发。
    pub item_name: String,
    /// 成交冻结的资金 UID（Sale 合同才有）：现金腿的发信会话认证 UID 必须等于它，不符不发。
    pub funding_uid: Option<String>,
    /// 成交冻结的资金记账方式（Sale 合同才有）：严格现金须在同一会话取新鲜现金观察，记账模式不读余额。
    pub funding_mode: Option<FundingAccountingMode>,
}

/// 已备好的补寄新轮投递及其冻结身份（授权时原样交给 `authorize`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRedispatch {
    /// 新轮的投递（`prepared`）。
    pub delivery_id: Uuid,
    /// 投递载荷里冻结的身份；其摘要与载荷的 `identity_digest` 一致。
    pub frozen: FrozenDispatchIdentity,
}

/// 一条已排队、尚未建轮的补寄处置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedRedispatch {
    /// 处置行记下的新轮 ID。
    pub dispatch_id: Uuid,
    /// 新轮的业务键（同合同、同义务、上一轮加一）。
    pub key: DispatchRoundKey,
    /// 封闭上一轮的退件接管（处置引用的证据）。
    pub closed_by_return_intake: Uuid,
    /// 新轮投递已备好时的投递与冻结身份；尚未备好为 `None`。
    pub prepared: Option<PreparedRedispatch>,
}

/// 已备好的补寄新轮投递（`prepare_redispatch` 的成功结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedispatchPrepared {
    /// 新轮的投递（`prepared`）。
    pub delivery_id: Uuid,
    /// 新轮冻结身份；授权时原样交给 `authorize`。
    pub frozen: FrozenDispatchIdentity,
    /// 此前已备好、返回的是原投递与原冻结身份。
    pub replayed: bool,
}

/// 派发与补寄工作的持久来源；实现方只委托存储，领取状态全部持久在数据库。
pub trait DispatchWorkStore: Send + Sync {
    /// 领取一项待派发的寄售义务：只领 `contract_id` 非空、ready、前置已满足、claim 空闲或过期，
    /// 且最新轮的投递尚未起写、履约行未指向别的投递的义务。没有时返回 `None`。
    fn claim_dispatch_work(
        &self,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<DispatchWorkClaim>>;

    /// 读取已衔接（履约行指向它、状态 `claimed` 或 `sending`）的轮投递的发送请求；
    /// 冻结身份与载荷摘要不一致或投递未衔接时返回 `None`。只读，不写任何行。
    fn dispatch_send_request(
        &self,
        delivery_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<DispatchSendRequest>>;

    /// 列出已排队、尚未建轮的补寄处置（最多 `limit` 条，按处置时刻）。只读。
    fn queued_redispatches(&self, limit: u32) -> ConsignmentFuture<'_, Vec<QueuedRedispatch>>;

    /// 为已排队的补寄新轮备好 `prepared` 投递（带冻结身份摘要与冻结内容）。新轮冻结身份全部由
    /// 实现方在同一事务里确定：D0 = 备好时的数据库时刻 + 合同冻结的付款期限，正文按冻结模板渲染；
    /// 已备好时重放返回原投递与原冻结身份。处置不存在或已建轮、上一轮不符、渲染内容与上一轮 /
    /// 合同不一致、D0 早于上一轮，或已有投递载荷无法还原时返回 `None`（失败关闭，不建投递）。
    fn prepare_redispatch(
        &self,
        dispatch_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<RedispatchPrepared>>;
}
