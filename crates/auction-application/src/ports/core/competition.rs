//! 竞价事务端口：锁内快照、审计上下文与事务错误。

use std::{future::Future, pin::Pin};

use auction_domain::consignment::{ListingEquipmentIdentity, matches_historical_equipment_alias};
use auction_domain::{
    AuctionRules, AuctionStatus, BidAmount, CommandId, CompetitionPlan, LotBidState, LotId,
    ProxyAuthorization, ProxyAuthorizationId, ProxyStatus, UserId,
};
use auction_time::Timestamp;
use thiserror::Error;

use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::core::ledger::{CommandLedgerError, StoredCommand};
use crate::ports::core::policy::{TransactionPolicy, TransactionPolicyError};

/// 竞价命令在锁内读取到的最小聚合快照。
#[derive(Debug, Clone)]
pub struct CompetitionSnapshot {
    pub rules: AuctionRules,
    /// 锁内读取到的真实拍卖状态；不能用结束时间窗口反推 lifecycle 已经进入 Final。
    pub auction_status: AuctionStatus,
    pub state: LotBidState,
    pub active_proxy: Option<ProxyAuthorization>,
}

/// 在 lot 锁内读取的同一用户代理重入上下文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyUsageContext {
    pub usage_number: u64,
    pub previous_maximum: Option<BidAmount>,
    pub previous_status: Option<ProxyStatus>,
}

/// 锁内读取的一条代理转换历史，用于生成命令账本可逐字重放的完整代理结果 wire。
/// 该结构保留数据库 wire 值，由应用层旧账本防腐层完成闭集和连续性校验；它不进入领域
/// 状态机，也不直接暴露给 HTTP。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompetitionProxyTransitionRecord {
    pub transition_number: i64,
    pub kind: String,
    pub old_maximum_amount: Option<i64>,
    pub new_maximum_amount: i64,
    pub old_mode: Option<String>,
    pub new_mode: String,
    pub old_status: Option<String>,
    pub new_status: String,
    pub reason: Option<String>,
    pub resulting_state_version: i64,
    pub created_at: Timestamp,
}

impl ProxyUsageContext {
    /// 从同一 lot、同一 owner 的完整授权历史建立下一次重入上下文。
    /// 旧版要求 usage 从 1 连续递增；只看最后一行会让损坏的 `1,3` 历史继续生成 4，并使
    /// quota 计数与账户投影分叉，因此这里统一失败关闭。
    pub fn from_owner_history(
        lot_id: LotId,
        user_id: UserId,
        history: &[ProxyAuthorization],
    ) -> Result<Self, CompetitionTransactionError> {
        let mut expected_usage = 1_u64;
        for authorization in history {
            authorization.validate().map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_history",
                    "validate_authorization",
                    DiagnosticContext::default(),
                );

                CompetitionTransactionError::Failed("代理授权历史包含无效快照".to_owned())
            })?;
            if authorization.lot_id != lot_id
                || authorization.user_id != user_id
                || authorization.usage_number != expected_usage
            {
                return Err(CompetitionTransactionError::Failed(
                    "代理授权历史身份或使用次数不连续".to_owned(),
                ));
            }
            expected_usage = expected_usage.checked_add(1).ok_or_else(|| {
                CompetitionTransactionError::Failed("代理授权使用次数溢出".to_owned())
            })?;
        }

        let Some(previous) = history.last() else {
            return Ok(Self { usage_number: 1, previous_maximum: None, previous_status: None });
        };
        Ok(Self {
            usage_number: expected_usage,
            previous_maximum: Some(previous.maximum_amount),
            previous_status: Some(previous.status),
        })
    }
}

/// 外部 Public 竞价命令在同一事务中追加旧版命令级审计所需的安全上下文。
/// 普通出价、代理创建和代理降低各写一条审计，而不是按公开 bid 或代理转换逐条写。
/// 这类已发布审计的 `request_id` 固定为空，因此这里刻意不携带 HTTP 关联 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompetitionCommandAuditContext {
    pub command_id: CommandId,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub kind: CompetitionCommandAuditKind,
}

/// 已冻结的三种外部 Public 竞价审计分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompetitionCommandAuditKind {
    PlaceBid,
    CreateProxy,
    LowerProxy { authorization_id: ProxyAuthorizationId, new_maximum: BidAmount },
}

/// 慢速代理执行计划在同一事务内写入旧版 Worker 审计所需的安全上下文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlowProxyExecutionContext {
    pub authorization_id: ProxyAuthorizationId,
    pub generation: u64,
    pub due_at: Timestamp,
    pub executed_at: Timestamp,
}

impl SlowProxyExecutionContext {
    /// 防止存储适配器把损坏 generation 或时间写成成功审计。
    pub fn validate(self) -> Result<(), CompetitionTransactionError> {
        if self.authorization_id.is_nil()
            || self.generation == 0
            || self.due_at.timestamp_nanos_opt().is_none()
            || self.executed_at.timestamp_nanos_opt().is_none()
            || self.executed_at < self.due_at
        {
            return Err(CompetitionTransactionError::Failed("慢速代理执行上下文无效".to_owned()));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum CompetitionTransactionError {
    #[error("竞价投影不存在")]
    NotFound,
    #[error("竞价事务中的资源状态发生冲突")]
    Conflict,
    #[error("当前世界没有可接收拍品的草稿场次")]
    AdminIntakeUnavailable,
    /// 收件目标在赛季开始前拒收装备：赛季前最后一场开拍后，旧赛季装备不能进入新赛季场次。
    #[error("赛季开始前已停止接收装备")]
    IntakeClosed,
    #[error("Admin 录入卖家当前处于黑名单")]
    AdminIntakeSellerBlacklisted,
    #[error("Admin 录入装备已经存在")]
    AdminIntakeEquipmentConflict,
    /// 同一世界、同一规范装备编号正被寄售活动预约或代管 position 强占用。
    #[error("装备正被寄售预约或代管持有")]
    EquipmentHeld,
    /// 实物会话锁在预算内没有取得；调用方不重试事务，按暂时不可用返回，客户端可用同一
    /// `command_id` 原样重试。
    #[error("实物锁在预算内未取得")]
    EquipmentLockUnavailable,
    #[error("管理 lot 的状态版本已经过期")]
    StaleState,
    #[error("管理 lot 当前状态不允许该命令")]
    InvalidState,
    #[error("竞价参与用户当前没有资格")]
    BidderIneligible,
    #[error("慢速代理到期 generation 已经过期")]
    StaleSlowProxy,
    #[error("竞价投影读取或写入失败: {0}")]
    Failed(String),
    #[error("竞价事务发生可重试的序列化冲突")]
    SerializationConflict,
    /// 同一场次的出价队列在预算内没有取得排队凭证；调用方不重试事务，按暂时不可用返回，
    /// 客户端可用同一 `command_id` 原样重试。
    #[error("同一场次的出价队列等待超时")]
    QueueWaitTimeout,
    #[error(transparent)]
    CommandLedger(#[from] CommandLedgerError),
}

/// 允许事务 runner 判断一次应用失败是否应按事务策略重试。
pub trait TransactionFailure:
    From<CompetitionTransactionError> + From<TransactionPolicyError> + Send
{
    fn is_serialization_conflict(&self) -> bool;
}

/// 事务回调的异步返回类型，使 runner 能在提交前继续持有事务对象。
pub type TransactionFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// 命令账本的写入结果。
///
/// `Inserted` 表示本事务新写入账本；`Duplicate` 表示同一命令已由并发事务提交，调用方
/// 必须按账本结果重放，绝不能再次应用计划（否则同一命令会写两遍事实）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandInsert {
    Inserted,
    Duplicate(StoredCommand),
}

/// 竞价事务对象。实现必须在同一数据库事务中锁定快照并原子应用计划。
pub trait CompetitionTransaction: Send {
    fn find_command(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CompetitionTransactionError>> + Send;

    /// 锁前门控：判断给定规范实物中是否至少一件正被寄售预约或 position 持有。
    ///
    /// 只读查询，供拍卖准入在开启写事务前决定是否取得实物租约。默认返回 `false`，让
    /// 不关心寄售的测试替身继续走既有 `run` 路径；生产适配器必须按
    /// `(world, 规范 EID)` 读取 `consignment_equipment_claims`。该预判不是授权：
    /// 锁内强占用检查、Serializable 与 lot 守卫仍然必须保留。
    fn any_equipment_held(
        &mut self,
        equipment: &[ListingEquipmentIdentity],
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send {
        let _ = equipment;
        async { Ok(false) }
    }

    /// 锁前续办预判：同一 `(mailbox_key, external_mail_id)` 是否已有拍卖投稿。
    ///
    /// 勘误 E1：既有投稿续办不以实物锁为前提，因此结果只用于选择既有 `run` 路径；
    /// 默认 `false` 供测试替身使用，生产适配器只读查询 `auction_submissions`。
    fn mail_auction_submission_exists(
        &mut self,
        mailbox_key: &str,
        external_mail_id: &str,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send {
        let _ = (mailbox_key, external_mail_id);
        async { Ok(false) }
    }

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<CommandInsert, CompetitionTransactionError>> + Send;

    /// 返回数据库当前时间；调用方在业务锁齐备后读取，截止与事实不使用进程时钟。
    fn current_time(
        &mut self,
    ) -> impl Future<Output = Result<Timestamp, CompetitionTransactionError>> + Send;

    fn load_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<CompetitionSnapshot, CompetitionTransactionError>> + Send;

    /// 在 lot 与活动代理之后锁定参与用户，并核对用户状态和当前黑名单事实。
    /// 调用方必须按 `UserId` 稳定排序后逐个调用，保持普通竞价、代理竞争和生命周期任务使用
    /// 同一套 lot → proxy → user 锁顺序。
    fn participant_is_eligible_for_update(
        &mut self,
        user_id: UserId,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send;

    /// 在已锁定 lot 的事务中读取下一次使用次数及上一份授权上限。
    fn proxy_usage_context(
        &mut self,
        lot_id: LotId,
        user_id: UserId,
    ) -> impl Future<Output = Result<ProxyUsageContext, CompetitionTransactionError>> + Send;

    /// 读取一份已锁定活动授权的完整转换历史，供旧命令结果在同一事务内形成连续闭集。
    fn proxy_transition_history(
        &mut self,
        authorization_id: ProxyAuthorizationId,
    ) -> impl Future<
        Output = Result<Vec<CompetitionProxyTransitionRecord>, CompetitionTransactionError>,
    > + Send;

    /// 读取历史 `accepted + CompetitionPlan` 账本重放所需的冻结代理配额。
    /// 该兼容读取不能复用带当前时间窗 gate 的竞价快照，否则拍卖结束后合法旧命令会无法重放。
    fn proxy_quota_limit(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<u32, CompetitionTransactionError>> + Send;

    /// 原子应用竞价计划；计划只读借用，调用方无需为返回结果复制整份计划。
    fn apply_plan(
        &mut self,
        plan: &CompetitionPlan,
        audit: Option<CompetitionCommandAuditContext>,
        occurred_at: Timestamp,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;

    /// 原子应用慢速代理计划，并写入格式已冻结的 Worker 审计。
    fn apply_slow_proxy_plan(
        &mut self,
        plan: &CompetitionPlan,
        context: SlowProxyExecutionContext,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

/// 事务 runner 端口。实现负责开始、提交/回滚和后端隔离级别映射。
pub trait TransactionRunner: Send + Sync {
    type Transaction: CompetitionTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send;

    /// 同一拍卖的可写命令入口（只知道 lot）：在进入 Serializable 事务**之前**按场次排队。
    ///
    /// 排队只改变并发形状，不改变隔离级别、行锁、命令账本或写入顺序：进入事务后仍然是
    /// Serializable + 显式行锁，同一 `command_id` 仍然只应用一次。默认实现直接执行，供不
    /// 关心跨实例串行的测试替身与不写拍卖的可调用方使用；生产适配器必须让同一场次在
    /// 进程内与跨实例都互斥，否则 SSI 对同页 `lots` 更新的误报仍会耗尽重试。
    ///
    /// 调用方必须把每个会写 `lots`/`auctions` 的**场次级**竞价命令（出价、代理创建/降低、慢速
    /// 代理执行）与 Admin draft lot 修改都放到这个入口上，并与 `run` 使用同一事务策略。
    /// 场次 ID 已知的写入必须使用 [`TransactionRunner::run_in_auction_queue`]，两者落到同一把门。
    fn run_in_auction_lot_queue<T, E, F>(
        &self,
        lot_id: LotId,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        let _ = lot_id;
        self.run(policy, operation)
    }

    /// 同一拍卖的可写命令入口（已知场次 ID）：生命周期迁移与 Admin 排期修改走这里。
    ///
    /// 与 [`TransactionRunner::run_in_auction_lot_queue`] 共用**同一把**按场次的门，因此竞价与
    /// 生命周期/Admin 写入在同一场次内严格串行：先排队、后 `BEGIN`，不存在两条事务同时处于
    /// SSI 冲突窗口，也就没有「竞价队列 ↔ 生命周期」的交叉等待顺序问题。
    ///
    /// 命令只能持有一个场次的门：跨场次写入必须在事务内完成，不能在同一命令里再排另一场次。
    fn run_in_auction_queue<T, E, F>(
        &self,
        auction_id: auction_domain::AuctionId,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        let _ = auction_id;
        self.run(policy, operation)
    }

    /// 拍卖与寄售共用的实物准入入口：先按预读的规范实物身份取得会话锁，再执行既有事务。
    ///
    /// 锁只保护实际争用该实物的写入，不把全站竞价都改成拿装备锁。默认实现直接执行，供不
    /// 关心实物仲裁的测试替身使用；生产适配器必须在开始 Serializable 快照之前取得全部实物
    /// 锁，并持有到事务、提交与提交后处理结束。无法构造规范身份的装备不进入预读集合，调用方
    /// 仍按原有处理继续（与数据库守卫在别名为空时跳过一致）。
    fn run_with_equipment_locks<T, E, F>(
        &self,
        equipment: &[ListingEquipmentIdentity],
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        let _ = equipment;
        self.run(policy, operation)
    }

    /// 按锁前门控结果选择既有 `run` 或带实物锁入口。
    ///
    /// 没有寄售持有时保持原路径，不建立租约、不占出价队列配额；有持有时才进入
    /// [`TransactionRunner::run_with_equipment_locks`]。闭包直接作为参数传入，保持
    /// runner 要求的 higher-ranked lifetime，避免先存成局部变量后丢失 `for<'a>` 约束。
    fn run_with_equipment_gate<T, E, F>(
        &self,
        equipment_held: bool,
        equipment: &[ListingEquipmentIdentity],
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        async move {
            if equipment_held {
                self.run_with_equipment_locks(equipment, policy, operation).await
            } else {
                self.run(policy, operation).await
            }
        }
    }
}

/// 把原文装备编号规范化为寄售实物锁身份。
///
/// 新登记与 `ListingEquipmentIdentity::new` 只接受规范正十进制；拍卖旧路径的原文 EID 允许
/// 前导零（如 `00042`），必须先按数据库 `consignment_eid_alias` 的同一规则去零。这里先按
/// 文本去零，再复用领域的历史别名比较与既有校验，不另造编号规则；全零、非 ASCII 数字或
/// 超过 32 位的结果返回 `None`，调用方按“无法构造身份”跳过实物锁。
pub(crate) fn canonical_equipment_lock_identity(
    world_id: &str,
    raw_equipment_id: &str,
) -> Option<ListingEquipmentIdentity> {
    let canonical = raw_equipment_id.trim_start_matches('0');
    let identity = ListingEquipmentIdentity::new(world_id, canonical).ok()?;
    matches_historical_equipment_alias(&identity, world_id, raw_equipment_id).then_some(identity)
}

#[cfg(test)]
mod tests {
    use super::canonical_equipment_lock_identity;

    #[test]
    fn equipment_lock_identity_uses_the_same_alias_rule_as_the_database() {
        let canonical = canonical_equipment_lock_identity("main", "42").expect("规范身份");
        let padded = canonical_equipment_lock_identity("main", "00042").expect("去零身份");
        assert_eq!(canonical, padded);
        assert_eq!(canonical.world_id(), "main");
        assert_eq!(canonical.external_equipment_id(), "42");
        assert_ne!(
            canonical,
            canonical_equipment_lock_identity("isekai", "00042").expect("不同世界身份")
        );
    }

    #[test]
    fn equipment_lock_identity_skips_values_the_database_alias_cannot_normalize() {
        for raw in ["", "0", "000", "+42", "-42", " 42", "42 ", "４２", "4.2", "x42"] {
            assert!(canonical_equipment_lock_identity("main", raw).is_none(), "{raw}");
        }
        assert!(
            canonical_equipment_lock_identity("main", &"9".repeat(33)).is_none(),
            "超过寄售身份上限的旧编号不取锁"
        );
    }
}
