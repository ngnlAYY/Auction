//! 解禁（Unblacklist）的寄售周期边界：先取买家用户锁，再在解禁事务内关闭旧拒收周期、开启新周期。
//!
//! 保护决定 23 与不变量 2、3（D9 §5.2“任一解禁路径原子关闭旧周期、开启新周期”）：
//! - 解禁与寄售购买、拒收、处罚执行器共用同一把买家用户会话锁（`user_lock_key`），锁在开
//!   Serializable 快照之前取得，执行器因此不能插在“解禁”与“周期边界”之间落账号效果；
//! - 周期边界与 `reset_cycle` 账本回执在解禁事务内同提交，旧周期的待执行处罚只能收口为
//!   `cycle_closed`；重放的解禁命令在 Users 账本命中后直接返回原结果，不再重置；
//! - 只有买家已有寄售拒收周期时才走这里；没有任何周期的用户（只参与拍卖）走原解禁路径，不取寄售锁、
//!   不设锁超时、不写寄售行，对既有拍卖零行为变化。周期只由持买家锁的拒收写端创建。

use auction_application::admin::{AdminUserCommandResult, AdminUserMutationWrite};
use auction_application::consignment::reset_cycle_command;
use auction_application::core::{ApplicationError, CompetitionTransactionError};
use auction_domain::UserId;
use sqlx::Connection;
use uuid::Uuid;

use super::mutation::mutate_in_transaction;
use crate::database::accounts::users::admin::postgres::mapping::map_storage_error;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::user_lock_key;
use crate::database::consignment::recovery::{ResetCycle, reset_cycle_in_conn};
use crate::database::{DatabaseStore, TrackedPool, TrackedTransaction};

/// 买家是否已有寄售拒收周期（任一状态）；没有就不需要也不应该触碰寄售周期。
pub(super) async fn buyer_has_refusal_cycle(
    pool: &TrackedPool<sqlx::Postgres>,
    buyer_user_id: Uuid,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar(REFUSAL_CYCLE_EXISTS)
        .bind(buyer_user_id)
        .fetch_one(pool)
        .await
        .map_err(map_storage_error)
}

/// 原解禁路径的快照内复核：周期在预读之后才出现时返回序列化冲突，由事务 owner 重试并改走持锁路径。
pub(super) async fn ensure_no_refusal_cycle(
    transaction: &mut TrackedTransaction<'_, sqlx::Postgres>,
    buyer_user_id: Uuid,
) -> Result<(), ApplicationError> {
    let exists: bool = sqlx::query_scalar(REFUSAL_CYCLE_EXISTS)
        .bind(buyer_user_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
    if exists { Err(CompetitionTransactionError::SerializationConflict.into()) } else { Ok(()) }
}

const REFUSAL_CYCLE_EXISTS: &str = "SELECT EXISTS (
     SELECT 1 FROM public.consignment_refusal_cycles WHERE buyer_user_id = $1)";

/// 在专用连接上持买家用户会话锁执行一次解禁事务；锁在提交或回滚之后才释放。
pub(super) async fn unblacklist_postgres_once(
    store: &DatabaseStore,
    write: &AdminUserMutationWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    let mut connection = store.pool().acquire().await.map_err(map_storage_error)?;
    // 会话锁与会话级超时都要求这条连接不回池：任何中断都以关闭连接解除锁。
    connection.close_on_drop();
    sqlx::query(
        "SELECT set_config('statement_timeout', '5s', false),
                set_config('lock_timeout', '1s', false)",
    )
    .execute(&mut *connection)
    .await
    .map_err(map_storage_error)?;
    let (namespace, key) = user_lock_key(write.user_id);
    sqlx::query("SELECT pg_advisory_lock($1, $2)")
        .bind(namespace)
        .bind(key)
        .execute(&mut *connection)
        .await
        .map_err(map_storage_error)?;
    let result = match connection.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await {
        Ok(transaction) => {
            mutate_in_transaction(TrackedTransaction::borrowed(transaction), write, true).await
        }
        Err(error) => Err(map_storage_error(error)),
    };
    let released = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1, $2)")
        .bind(namespace)
        .bind(key)
        .fetch_one(&mut *connection)
        .await;
    if !matches!(released, Ok(true)) {
        tracing::warn!("无法确认解禁的买家锁已解除，连接将关闭");
    }
    result
}

/// 解禁事务内的周期边界：命令 ID 与摘要由买家与本次解禁命令（原解禁事实）派生。
///
/// 调用方必须是 [`unblacklist_postgres_once`] 开启的事务（已持买家锁），否则周期原语失败关闭。
pub(super) async fn reset_buyer_cycle(
    transaction: &mut TrackedTransaction<'_, sqlx::Postgres>,
    buyer_user_id: Uuid,
    unban_fact_id: Uuid,
) -> Result<(), ApplicationError> {
    let (command_id, request_hash) = reset_cycle_command(buyer_user_id, unban_fact_id);
    let reset = ResetCycle {
        command_id: command_id.as_uuid(),
        request_hash: &request_hash,
        buyer: UserId::from_uuid(buyer_user_id),
    };
    reset_cycle_in_conn(transaction, &reset).await.map(drop).map_err(cycle_error)
}

/// 已确认回滚的序列化冲突交给 Users 事务 owner 有界重试；其余失败关闭，不泄露 SQL 细节。
fn cycle_error(error: WriteError) -> ApplicationError {
    match error {
        WriteError::RetryConflict => CompetitionTransactionError::SerializationConflict.into(),
        other => {
            tracing::warn!(error = %other, "解禁的寄售周期边界写入失败");
            crate::database::storage_error("解禁的寄售周期边界写入失败")
        }
    }
}
