//! operations alert 的来源观察、健康快照和策略模型。

use std::collections::HashSet;
use std::time::Duration;

use auction_domain::{SettlementObligationKind, SettlementReturnTakeUnknownReason};
use auction_time::Timestamp;
use uuid::Uuid;

use super::digest::valid_age_threshold;
use super::signals::OperationsAlertError;
use super::signals::{OperationsAlertKind, OperationsAlertSignal, OperationsAlertSource};
use super::{
    MAX_OPERATIONS_ALERT_BATCH_ITEMS, MAX_OPERATIONS_ALERT_BODY_BYTES, MAX_OPERATIONS_ALERT_SIGNALS,
};

/// 一次来源观察；不可用时保留固定 `source_unavailable`，并可携带已确认的局部异常。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertSourceObservation {
    pub source: OperationsAlertSource,
    pub available: bool,
    pub signals: Vec<OperationsAlertSignal>,
    /// 本轮没有检查的信号种类（例如主机报告未提供演练目录时的 `restore_drill_overdue`）。
    /// 缺席不等于恢复：存储层关闭“本轮未出现”的活动信号时必须跳过这些种类。
    pub unchecked: Vec<OperationsAlertKind>,
}

impl OperationsAlertSourceObservation {
    pub fn available(
        source: OperationsAlertSource,
        signals: Vec<OperationsAlertSignal>,
    ) -> Result<Self, OperationsAlertError> {
        let result = Self { source, available: true, signals, unchecked: Vec::new() };
        result.validate()?;
        Ok(result)
    }

    /// 标记本轮未检查的种类；该种类的活动信号保持原状，既不刷新也不关闭。
    pub fn with_unchecked(
        mut self,
        kind: OperationsAlertKind,
    ) -> Result<Self, OperationsAlertError> {
        if !self.unchecked.contains(&kind) {
            self.unchecked.push(kind);
        }
        self.validate()?;
        Ok(self)
    }

    /// 构造不暴露底层错误的不可用来源观察。
    pub fn unavailable(source: OperationsAlertSource) -> Result<Self, OperationsAlertError> {
        let signal = OperationsAlertSignal::new(
            OperationsAlertKind::SourceUnavailable,
            source.as_str(),
            source,
            1,
            0,
        )?;
        Ok(Self { source, available: false, signals: vec![signal], unchecked: Vec::new() })
    }

    /// 校验来源完整性与 signal 归属。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.signals.len() > MAX_OPERATIONS_ALERT_SIGNALS {
            return Err(OperationsAlertError::Invalid);
        }
        // 未检查的种类不能同时出现在本轮信号里，也不能豁免固定的来源不可用信号。
        if self.unchecked.iter().any(|kind| {
            *kind == OperationsAlertKind::SourceUnavailable
                || self.signals.iter().any(|signal| signal.kind == *kind)
        }) {
            return Err(OperationsAlertError::Invalid);
        }
        let unavailable_count = self
            .signals
            .iter()
            .filter(|signal| signal.kind == OperationsAlertKind::SourceUnavailable)
            .count();
        if unavailable_count != usize::from(!self.available) {
            return Err(OperationsAlertError::Invalid);
        }
        if self
            .signals
            .iter()
            .any(|signal| signal.validate().is_err() || signal.source != self.source)
        {
            return Err(OperationsAlertError::Invalid);
        }
        // 同一轮观察内按 fingerprint 去重：一个信号只能对应一条持久化水位与一次通知，
        // 重复项必须在进入存储前被拒绝，而不是依赖 upsert 顺序。
        let mut fingerprints = HashSet::with_capacity(self.signals.len());
        if self.signals.iter().any(|signal| !fingerprints.insert(signal.fingerprint)) {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }
}

/// Worker 观察到的进程安全状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertProcessState {
    pub role: String,
    pub status: String,
}

/// Process source 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertProcessSnapshot {
    pub processes: Vec<OperationsAlertProcessState>,
    pub audit_failure_count: i64,
}

/// Delivery source 快照；operations alert 自己的 delivery 不应计入此水位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertDeliverySnapshot {
    pub pending: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    pub returned_mail_manual_reconcile: i64,
    pub oldest_pending_age_seconds: i64,
    pub oldest_manual_reconcile_age_seconds: i64,
    pub oldest_returned_mail_age_seconds: i64,
    /// 已领取（claimed/sending）的投递数量；用于识别领取后没有推进的队列延迟。
    pub claimed: i64,
    pub oldest_claimed_age_seconds: i64,
    pub last_update_at: Option<Timestamp>,
}

/// Settlement fulfillment source 快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertFulfillmentSnapshot {
    /// 真正可推进的 ready 履约项；正常等待付款/前置、已寄出等待领取的项不计入。
    pub pending: i64,
    /// 非正常等待的可推进受阻项；失败、已满足及正常等待项不计入。
    pub blocked: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    /// 付款期限内等待买家付款的 COD/付款义务；不应作为积压。
    pub waiting_payment: i64,
    /// 等待前置义务完成的卖方货款等；超过策略年龄后才应视为积压。
    pub waiting_prerequisite: i64,
    /// 已寄出、只等收件人在游戏内领取的 Send 义务（COD/载体付款仍按等待付款口径）；
    /// 不计入积压，义务仍未满足。
    pub awaiting_collection: i64,
    /// 等待领取中最近一次 Sent 读取失败（`evidence_unavailable`）的数量；区分“读不到”与
    /// “已读到但未领取”，不能据此推断收件人是否已领取。
    pub awaiting_collection_unreadable: i64,
    pub oldest_backlog_age_seconds: i64,
    pub oldest_prerequisite_wait_age_seconds: i64,
    /// 最早寄出的等待领取义务自投递完成（`deliveries.completed_at`）起的秒数；没有时为 0。
    pub oldest_awaiting_collection_age_seconds: i64,
    /// 等待领取义务中最早进入履约的年龄，与 `oldest_backlog_age_seconds` 同一时刻口径（履约创建，
    /// 缺行时取义务创建）；没有时为 0。只供 Admin 看板缺省响应按已发布口径把等待领取并回积压。
    pub oldest_awaiting_collection_backlog_age_seconds: i64,
    /// 等待领取中寄出时长已达到长期未领取阈值的件数（超期件数），是 `awaiting_collection` 的
    /// 子集；阈值随快照读取给出，未给出时为 0。
    pub collection_overdue: i64,
    /// 已绑定原派发、尚未处理完的结算退件案件数；它们的履约不再计入人工对账与失败水位。
    pub settlement_return_pending: i64,
    /// 最早一件待处理退件案件自建案起的秒数；没有时为 0。
    pub oldest_settlement_return_pending_age_seconds: i64,
    /// 来源或绑定未证明（隔离、待重新核验）、尚未结案的结算退件案件数；Take 结果未知的案件
    /// 按原因单列在 `settlement_return_take_unknown`。
    pub settlement_return_quarantined: i64,
    /// 最早一件隔离退件案件自建案起的秒数；没有时为 0。
    pub oldest_settlement_return_quarantined_age_seconds: i64,
    /// Take 结果未知的结算退件按原因分组的水位；下标与
    /// [`SettlementReturnTakeUnknownReason::ALL`] 同序。
    pub settlement_return_take_unknown:
        [OperationsAlertReturnTakeUnknown; SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS],
}

/// [`SettlementReturnTakeUnknownReason`] 闭集的大小，即按原因分组水位的固定长度。
pub const SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS: usize =
    SettlementReturnTakeUnknownReason::ALL.len();

/// 一个原因下 Take 结果未知的结算退件水位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OperationsAlertReturnTakeUnknown {
    pub count: i64,
    /// 最早一件自建案起的秒数；没有时为 0。
    pub oldest_age_seconds: i64,
}

/// 长期未领取清单的一项：已寄出、寄出时长达到阈值仍未领取的 Send 义务。只含 Admin 可见的业务
/// 标识与展示字段，不含投递载荷、凭据或外部错误原文；只读展示，不授权任何动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertCollectionOverdueItem {
    pub obligation_id: Uuid,
    pub obligation_kind: SettlementObligationKind,
    /// 场次所属世界与届次；与 lot 编号、装备分类一起定位拍品。
    pub auction_world: String,
    pub auction_sequence: i64,
    /// 分类内编号（`lots.category_number`）：拍品代码是分类前缀加该编号，与 Admin lot 页一致；
    /// 不是全场编号。
    pub lot_number: i32,
    pub equipment_category: String,
    pub equipment_name: String,
    /// 投递冻结的收件人游戏名。
    pub recipient: String,
    /// 寄出时刻（投递完成的数据库时刻 `deliveries.completed_at`）。
    pub sent_at: Timestamp,
    /// 自寄出起已等待的秒数。
    pub age_seconds: i64,
    /// 最近一次 Sent 读取失败（`evidence_unavailable`）；只说明读不到，不代表是否已领取。
    /// `false` 只表示没有记录读取失败（含 Sent 找不到与尚未读取），不是“已确认未领取”。
    pub last_read_failed: bool,
}

/// Recovery 每个 world 的余额和 evidence 安全状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertRecoveryWorld {
    pub world: String,
    pub balance_credits: i64,
    pub balance_status: String,
    pub evidence_status: String,
    pub evidence_age_seconds: i64,
    /// 余额读取连续处于同一非正常状态（busy/unavailable）的持续时间；正常时为 0。
    pub balance_stall_age_seconds: i64,
    pub balance_observed_at: Option<Timestamp>,
    pub evidence_last_success_at: Option<Timestamp>,
    pub updated_at: Timestamp,
}

/// Recovery source 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertRecoverySnapshot {
    pub payment_overdue_count: i64,
    /// 付款期限已过、付款投递没有自动或人工发送证明的恢复数量。
    pub payment_mail_unsent_count: i64,
    pub relist_pending_count: i64,
    pub failed_action_count: i64,
    pub manual_reconcile_count: i64,
    pub oldest_overdue_age_seconds: i64,
    pub oldest_payment_mail_unsent_age_seconds: i64,
    pub worlds: Vec<OperationsAlertRecoveryWorld>,
}

/// Mail source 快照：收件轮询水位。Admin 显式暂停的邮箱由读取层排除，不进入该水位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertMailSnapshot {
    pub last_poll_age_seconds: i64,
    pub consecutive_failures: i64,
    /// 最近一次完整 Sent 枚举扫描的页数；None 表示尚无完整枚举观察。
    pub sent_pages_scanned: Option<i64>,
    /// 该次完整枚举生效的页数上限；与 `sent_pages_scanned` 必须同时存在。
    pub sent_page_limit: Option<i64>,
}

/// Credential source 快照：每种凭据当前行的失效数量。
/// 只用稳定种类名，不携带凭据标签、指纹或密文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertCredentialSnapshot {
    pub invalid_mail_cookie: i64,
    pub invalid_external_api: i64,
}

/// Lifecycle source 快照：结束时间已过但尚未推进的场次与慢速代理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertLifecycleSnapshot {
    pub phase_overdue_count: i64,
    pub oldest_phase_overdue_age_seconds: i64,
    pub proxy_expiry_overdue_count: i64,
    pub oldest_proxy_overdue_age_seconds: i64,
    /// 冻结 isekai 换季边界前七天内仍未完成的结算义务数。
    pub season_obligations_pending: i64,
}

/// Forum source 快照：主题发布与私信投递失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertForumSnapshot {
    pub sync_failed_count: i64,
    pub oldest_sync_failed_age_seconds: i64,
    pub message_failed_count: i64,
    pub oldest_message_failed_age_seconds: i64,
    /// 最近一次远端主题扫描发现的最大 HTTP 分页数；不是本地目录固定帖页数。
    pub remote_page_count: Option<i64>,
    /// 该次观察生效的远端主题页数上限；与 `remote_page_count` 必须同时存在。
    pub remote_page_limit: Option<i64>,
}

/// Notification source 快照：非结算通知投递失败与邮资不足。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertNotificationSnapshot {
    pub failed_count: i64,
    pub oldest_failed_age_seconds: i64,
    pub postage_insufficient_count: i64,
    pub oldest_postage_age_seconds: i64,
}

/// 资产对账在单个 world 的最新报告事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertReconciliationWorld {
    pub world: &'static str,
    /// 最近一份 custody 报告的缺失装备数量；报告缺失时为 0，由报告年龄表达缺失。
    pub custody_missing_count: i64,
    /// 最近一份 custody 报告中已寄出、等待领取的在途装备件数；只展示，不产生差异信号。
    pub custody_in_transit_count: i64,
    /// 最近一份 solvency 报告的缺口 credits；报告缺失或余额未知时为 0。
    pub solvency_shortfall_credits: i64,
    /// 最近一份 solvency 报告中已寄出且已支出的在途货款与退款笔数；只展示，不计入缺口。
    pub solvency_in_transit_count: i64,
    /// 上述在途货款与退款的 credits 合计。
    pub solvency_in_transit_credits: i64,
}

/// 资产对账报告投影；`None` 表示该类报告从未追加，不能当作“无差异”。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertReconciliationSnapshot {
    pub custody_report_age_seconds: Option<i64>,
    pub solvency_report_age_seconds: Option<i64>,
    pub worlds: Vec<OperationsAlertReconciliationWorld>,
}

/// 全部告警来源的安全快照；单个来源缺失时 evaluator 会生成 unavailable signal。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OperationsAlertSnapshot {
    pub process: Option<OperationsAlertProcessSnapshot>,
    pub delivery: Option<OperationsAlertDeliverySnapshot>,
    pub fulfillment: Option<OperationsAlertFulfillmentSnapshot>,
    /// 与 `fulfillment` 同一只读快照读出的长期未领取清单：只列寄出时长达到阈值的等待领取义务，
    /// 寄出最早优先，最多 `MAX_OPERATIONS_ALERT_COLLECTION_OVERDUE_ITEMS` 条，总数见
    /// `fulfillment.collection_overdue`；`fulfillment` 缺失或未给出阈值时为空。
    pub collection_overdue_items: Vec<OperationsAlertCollectionOverdueItem>,
    pub recovery: Option<OperationsAlertRecoverySnapshot>,
    pub mail: Option<OperationsAlertMailSnapshot>,
    pub credential: Option<OperationsAlertCredentialSnapshot>,
    pub lifecycle: Option<OperationsAlertLifecycleSnapshot>,
    pub forum: Option<OperationsAlertForumSnapshot>,
    pub notification: Option<OperationsAlertNotificationSnapshot>,
    pub reconciliation: Option<OperationsAlertReconciliationSnapshot>,
}

/// 一份来源快照里长期未领取的读取范围。阈值必须取自评估这份快照的同一告警策略；计数与清单
/// 在同一只读快照、同一次履约分类里得出，不会互相矛盾。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationsAlertCollectionOverdueRead {
    /// 不评估长期未领取：超期件数为 0，不读清单。
    Skip,
    /// 只计超期件数（最久寄出时长已在水位里），不读展示清单；Worker 告警评估只需要这些。
    Count(Duration),
    /// 计数并读出长期未领取清单，供 Admin 看板展示。
    CountWithItems(Duration),
}

impl OperationsAlertCollectionOverdueRead {
    /// 评估所用的阈值；`Skip` 时没有阈值。
    pub fn threshold(self) -> Option<Duration> {
        match self {
            Self::Skip => None,
            Self::Count(threshold) | Self::CountWithItems(threshold) => Some(threshold),
        }
    }

    /// 是否读出长期未领取清单。
    pub fn with_items(self) -> bool {
        matches!(self, Self::CountWithItems(_))
    }
}

/// 告警阈值与批次边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertPolicy {
    pub source_timeout: Duration,
    pub batch_window: Duration,
    pub cooldown: Duration,
    pub claim_lease: Duration,
    pub retry_delay: Duration,
    pub max_signals_per_scan: usize,
    pub max_batch_items: usize,
    pub max_body_bytes: usize,
    pub delivery_pending_count_threshold: i64,
    pub delivery_pending_age_threshold: Duration,
    pub delivery_manual_reconcile_count_threshold: i64,
    pub fulfillment_backlog_count_threshold: i64,
    pub fulfillment_backlog_age_threshold: Duration,
    /// 已寄出等待领取的寄出时长达到该阈值即计为超期，有超期件即产生
    /// `fulfillment_collection_overdue`；读取快照时传给来源端口。
    pub fulfillment_collection_overdue_age_threshold: Duration,
    pub recovery_overdue_count_threshold: i64,
    pub recovery_overdue_age_threshold: Duration,
}

impl OperationsAlertPolicy {
    /// 校验时间、数量和正文边界，避免配置扩大外部通知或数据库扫描范围。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.source_timeout < Duration::from_millis(100)
            || self.source_timeout > Duration::from_secs(30)
            || self.batch_window < Duration::from_secs(1)
            || self.batch_window > Duration::from_secs(3_600)
            || self.cooldown < self.batch_window
            || self.cooldown > Duration::from_secs(7 * 24 * 3_600)
            || self.claim_lease < self.source_timeout.saturating_mul(4)
            || self.claim_lease > Duration::from_secs(24 * 3_600)
            || self.retry_delay < Duration::from_secs(1)
            || self.retry_delay > Duration::from_secs(24 * 3_600)
            || self.max_signals_per_scan == 0
            || self.max_signals_per_scan > MAX_OPERATIONS_ALERT_SIGNALS
            || self.max_batch_items == 0
            || self.max_batch_items > MAX_OPERATIONS_ALERT_BATCH_ITEMS
            || self.max_body_bytes < 256
            || self.max_body_bytes > MAX_OPERATIONS_ALERT_BODY_BYTES
            || self.delivery_pending_count_threshold < 1
            || self.delivery_manual_reconcile_count_threshold < 1
            || self.fulfillment_backlog_count_threshold < 1
            || self.recovery_overdue_count_threshold < 1
            || !valid_age_threshold(self.delivery_pending_age_threshold)
            || !valid_age_threshold(self.fulfillment_backlog_age_threshold)
            || !valid_age_threshold(self.fulfillment_collection_overdue_age_threshold)
            || !valid_age_threshold(self.recovery_overdue_age_threshold)
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }
}

/// 固定告警收件目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertTarget {
    pub world_id: String,
    pub recipient: String,
}

impl OperationsAlertTarget {
    /// 校验 world 和收件人边界；收件人不会进入 signal fingerprint。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if !matches!(self.world_id.as_str(), "main" | "isekai")
            || !auction_domain::valid_text(&self.recipient, 256)
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }
}
