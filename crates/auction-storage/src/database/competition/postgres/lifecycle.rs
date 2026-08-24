//! PostgreSQL 拍卖生命周期事务适配。
//!
//! 该文件只承载 PostgreSQL 竞价事务的单一职责，事务边界仍由父模块统一装配。

use super::mapping::*;
use super::*;

#[path = "lifecycle/context.rs"]
pub(super) mod context;
#[path = "lifecycle/plan.rs"]
pub(super) mod plan;
#[path = "lifecycle/record.rs"]
pub(super) mod record;

use context::load_lifecycle_context_postgres;
use plan::apply_lifecycle_plan_postgres;
use record::append_lifecycle_record_inner;

#[allow(clippy::manual_async_fn)]
impl AuctionLifecycleTransaction for PostgresCompetitionTransaction {
    fn load_lifecycle_context_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Option<AuctionLifecycleContext>, CompetitionTransactionError>> + Send
    {
        async move { load_lifecycle_context_postgres(self, auction_id).await }
    }

    fn apply_lifecycle_plan(
        &mut self,
        command: &AuctionLifecycleCommand,
        plan: AuctionLifecyclePlan,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        let command = command.clone();
        async move { apply_lifecycle_plan_postgres(self, &command, plan).await }
    }

    fn list_final_lot_ids_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Vec<LotId>, CompetitionTransactionError>> + Send {
        async move {
            let rows = {
                let transaction = self.transaction_mut()?;
                sqlx::query(
                    "SELECT id FROM public.lots
                     WHERE auction_id = $1 AND status = 'live'
                     ORDER BY lot_number
                     FOR UPDATE",
                )
                .bind(auction_id.as_uuid())
                .fetch_all(&mut **transaction)
                .await
                .map_err(map_sql_error)?
            };
            rows.into_iter()
                .map(|row| row.try_get("id").map(LotId::from_uuid).map_err(map_sql_error))
                .collect()
        }
    }

    fn proxy_user_is_eligible_for_update(
        &mut self,
        user_id: UserId,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send {
        async move {
            let status = {
                let transaction = self.transaction_mut()?;
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM public.users WHERE id = $1 FOR UPDATE",
                )
                .bind(user_id.as_uuid())
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
                .ok_or_else(|| invalid_state("活动代理用户不存在"))?
            };
            let blacklist_active = {
                let transaction = self.transaction_mut()?;
                sqlx::query_scalar::<_, bool>(
                    "SELECT revoked_at IS NULL
                     FROM public.user_blacklist
                     WHERE user_id = $1
                     FOR UPDATE",
                )
                .bind(user_id.as_uuid())
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
                .unwrap_or(false)
            };
            Ok(status == "active" && !blacklist_active)
        }
    }

    fn append_lifecycle_record(
        &mut self,
        command: &AuctionLifecycleCommand,
        result: &AuctionLifecycleResult,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { append_lifecycle_record_inner(self, command, result).await }
    }
}
