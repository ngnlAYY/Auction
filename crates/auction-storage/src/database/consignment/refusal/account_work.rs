//! 每项处罚唯一的待执行账号工作：本期只创建与随周期关闭取消，执行与拉黑归第 7 期。

use auction_domain::UserId;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;

/// 为处罚创建唯一待执行账号工作；`UNIQUE(sanction_id)` 保证每项处罚至多一份（不变量 1、3）。
pub(super) async fn insert_account_work(
    conn: &mut PgConnection,
    sanction_id: Uuid,
    buyer: UserId,
) -> Result<Uuid, WriteError> {
    let work_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.consignment_account_work (id, sanction_id, buyer_user_id)
         VALUES ($1, $2, $3)",
    )
    .bind(work_id)
    .bind(sanction_id)
    .bind(buyer.as_uuid())
    .execute(&mut *conn)
    .await?;
    Ok(work_id)
}

/// 处罚随周期关闭时，取消尚未完成的账号工作（pending 或 claimed）；已完成的保持原状。
pub(super) async fn cancel_open_work(
    conn: &mut PgConnection,
    sanction_id: Uuid,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE public.consignment_account_work SET state = 'cancelled', version = version + 1
         WHERE sanction_id = $1 AND state IN ('pending', 'claimed')",
    )
    .bind(sanction_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}
