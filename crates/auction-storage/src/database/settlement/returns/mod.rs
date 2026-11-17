//! PostgreSQL 结算退件案件 owner：案件投影、只追加的命令/事件/领回事实、持久 Take 与补寄授权。
//!
//! 案件与经济义务分开：案件终结只表示本次退件处理已交接或完成；原义务是否满足、资产归谁、是否
//! 仍欠款由履约、资金与对账各自判断。Admin 命令与 Worker 步骤都是单个 Serializable 本地事务，
//! 外部 Take 只经 delivery owner 在本地提交之后执行。Take 结果未知的唯一出口是邮箱巡检在原 Take
//! 会话里观察到的精确事实（`swept`），不接受操作者声明。

mod admission;
mod cases;
mod command;
mod errors;
mod reads;
mod recheck;
mod resend;
mod restart;
mod swept;
mod take;
mod work;

pub(crate) use cases::{current_return_evidence_sql, open_case, return_case_bound};
pub(crate) use errors::ReturnTransactionError;
pub(crate) use swept::{
    SweptTakeOwner, lock_swept_take, requeue_swept_take, settle_swept_take, swept_take_owner,
};
pub(crate) use take::TAKE_SWEEP_REASON;

use auction_application::settlement::{
    SettlementReturnAdminPort, SettlementReturnCommand, SettlementReturnCommandResult,
    SettlementReturnDetail, SettlementReturnListRequest, SettlementReturnPage,
    SettlementReturnProgress, SettlementReturnRecheckClaim, SettlementReturnRecheckOutcome,
    SettlementReturnWorkStore,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::database::DatabaseStore;

/// 只在 `test-db` 下暴露 Admin 列表查询原文给基准/EXPLAIN 夹具，保证测量的是同一份 SQL；默认
/// 构建与发布制品不包含该出口。
#[cfg(feature = "test-db")]
pub mod benchmark_sql {
    /// 列表总数（`$1` 世界、`$2` 证据分类、`$3` 状态、`$4` 义务种类、`$5` 只看未结案、
    /// `$6/$7` 收件时间范围、`$8` 引用）。
    pub fn list_count_sql() -> &'static str {
        super::reads::LIST_COUNT_SQL
    }

    /// 列表当前页：参数同总数，另加 `$9` 页长、`$10` 偏移。
    pub fn list_page_sql() -> &'static str {
        super::reads::LIST_PAGE_SQL
    }
}

impl SettlementReturnAdminPort for DatabaseStore {
    fn list_settlement_returns(
        &self,
        request: SettlementReturnListRequest,
    ) -> auction_application::core::ApplicationFuture<'_, SettlementReturnPage> {
        Box::pin(async move { reads::list_postgres(self.pool(), &request).await })
    }

    fn settlement_return_detail(
        &self,
        return_mail_id: Uuid,
    ) -> auction_application::core::ApplicationFuture<'_, SettlementReturnDetail> {
        Box::pin(async move { reads::detail_postgres(self.pool(), return_mail_id).await })
    }

    fn handle_settlement_return(
        &self,
        command: SettlementReturnCommand,
    ) -> auction_application::core::ApplicationFuture<'_, SettlementReturnCommandResult> {
        Box::pin(async move { command::handle_postgres(self.pool(), &command).await })
    }

    fn open_settlement_return_count(
        &self,
    ) -> auction_application::core::ApplicationFuture<'_, i64> {
        Box::pin(async move { reads::open_count_postgres(self.pool()).await })
    }
}

impl SettlementReturnWorkStore for DatabaseStore {
    fn activate_due_return_take(
        &self,
        now: DateTime<Utc>,
        take_worlds: Vec<String>,
    ) -> auction_application::core::ApplicationFuture<'_, Option<SettlementReturnProgress>> {
        Box::pin(
            async move { work::activate_due_take_postgres(self.pool(), now, &take_worlds).await },
        )
    }

    fn settle_due_return_take(
        &self,
        now: DateTime<Utc>,
    ) -> auction_application::core::ApplicationFuture<'_, Option<SettlementReturnProgress>> {
        Box::pin(async move { work::settle_due_take_postgres(self.pool(), now).await })
    }

    fn complete_due_return_resend(
        &self,
        now: DateTime<Utc>,
    ) -> auction_application::core::ApplicationFuture<'_, Option<SettlementReturnProgress>> {
        Box::pin(async move { resend::complete_due_resend_postgres(self.pool(), now).await })
    }

    fn claim_due_return_recheck(
        &self,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> auction_application::core::ApplicationFuture<'_, Option<SettlementReturnRecheckClaim>>
    {
        Box::pin(
            async move { recheck::claim_recheck_postgres(self.pool(), now, lease_until).await },
        )
    }

    fn record_return_recheck(
        &self,
        claim: SettlementReturnRecheckClaim,
        outcome: SettlementReturnRecheckOutcome,
    ) -> auction_application::core::ApplicationFuture<'_, SettlementReturnProgress> {
        Box::pin(
            async move { recheck::record_recheck_postgres(self.pool(), &claim, &outcome).await },
        )
    }
}
