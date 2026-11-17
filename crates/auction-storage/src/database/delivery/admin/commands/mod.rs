//! PostgreSQL delivery Admin 人工命令事务与结算履约联动。
//! 本文件只做有界重试；`attempt` 是单次命令事务（取锁顺序、领域守卫与同事务写入），`ledger`
//! 拥有命令账本的事务锁、重放与冲突识别，`transition` 以 fencing 条件改写投递行，`settlement`
//! 联动结算履约行并生成人工对账证明摘要。

use auction_application::delivery::{
    DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminResult,
};
use sqlx::Postgres;

use self::attempt::execute_transaction;
use super::transaction::wait_before_retry;
use crate::database::TrackedPool;

mod attempt;
mod ledger;
mod settlement;
mod transition;

pub(crate) async fn execute(
    pool: &TrackedPool<Postgres>,
    command: &DeliveryAdminCommand,
) -> Result<DeliveryAdminResult, DeliveryAdminError> {
    pool.observe_transaction(
        auction_observability::DatabaseTransactionOwner::DeliveryAdmin,
        async {
            const MAX_ATTEMPTS: usize = 3;

            for attempt in 0..MAX_ATTEMPTS {
                match execute_transaction(pool, command).await {
                    Ok(result) => return Ok(result),
                    Err(error) if error.is_retryable() && attempt + 1 < MAX_ATTEMPTS => {
                        // 每次重试都从新的事务开始；已执行的写入不能在同一事务中继续使用。
                        wait_before_retry(attempt).await;
                    }
                    Err(error) => {
                        crate::database::observe_retry_exhaustion(
                            attempt + 1,
                            MAX_ATTEMPTS,
                            error.is_retryable(),
                        );
                        return Err(error.into_delivery_error());
                    }
                }
            }
            // 循环的最后一次尝试总会返回；这里只为未来改动留下失败关闭的出口，不以 panic 终止请求。
            Err(DeliveryAdminError::Unavailable)
        },
    )
    .await
}
