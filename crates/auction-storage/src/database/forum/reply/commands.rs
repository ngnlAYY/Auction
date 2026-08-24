//! 论坛回帖竞价命令账本的幂等开始与完成写入。

#[cfg(feature = "postgres")]
use auction_application::forum_world_name;
#[cfg(feature = "postgres")]
use auction_application::{
    ApplicationError, ForumBidCommandRecord, ForumCommandStart, ForumCommandStatus,
};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use super::super::super::PostgresStore;
#[cfg(feature = "postgres")]
use super::validation::storage_error;

#[cfg(feature = "postgres")]
pub(super) async fn begin_command_postgres(
    store: &PostgresStore,
    record: ForumBidCommandRecord,
) -> Result<ForumCommandStart, ApplicationError> {
    use sqlx::Row;

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启论坛命令账本事务失败"))?;
    let inserted = sqlx::query(
        "INSERT INTO public.auction_forum_bid_commands
            (command_id, auction_id, world_id, thread_id, post_id, line_number, lot_code,
             actor, command_kind, amount, status, attempt_count)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'pending', 1)
         ON CONFLICT (command_id) DO NOTHING",
    )
    .bind(record.command_id)
    .bind(record.auction_id.as_uuid())
    .bind(forum_world_name(record.world))
    .bind(&record.thread_id)
    .bind(&record.post_id)
    .bind(record.line_number)
    .bind(&record.lot_code)
    .bind(&record.actor)
    .bind(&record.command_kind)
    .bind(record.amount)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入论坛命令账本失败"))?;
    if inserted.rows_affected() == 1 {
        transaction.commit().await.map_err(|_| storage_error("提交论坛命令账本事务失败"))?;
        return Ok(ForumCommandStart { status: ForumCommandStatus::Pending, should_run: true });
    }

    let row = sqlx::query(
        "SELECT auction_id, world_id, thread_id, post_id, line_number, lot_code,
                actor, command_kind, amount, status
         FROM public.auction_forum_bid_commands
         WHERE command_id = $1
         FOR UPDATE",
    )
    .bind(record.command_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取既有论坛命令账本失败"))?;
    let Some(row) = row else {
        return Err(storage_error("论坛命令账本冲突记录不存在"));
    };
    let existing_auction_id: Uuid =
        row.try_get("auction_id").map_err(|_| storage_error("读取论坛命令拍卖标识失败"))?;
    let existing_world: String =
        row.try_get("world_id").map_err(|_| storage_error("读取论坛命令世界失败"))?;
    let existing_thread_id: String =
        row.try_get("thread_id").map_err(|_| storage_error("读取论坛命令主题标识失败"))?;
    let existing_post_id: String =
        row.try_get("post_id").map_err(|_| storage_error("读取论坛命令帖子标识失败"))?;
    let existing_line_number: i32 =
        row.try_get("line_number").map_err(|_| storage_error("读取论坛命令行号失败"))?;
    let existing_lot_code: String =
        row.try_get("lot_code").map_err(|_| storage_error("读取论坛命令 lot 编码失败"))?;
    let existing_actor: String =
        row.try_get("actor").map_err(|_| storage_error("读取论坛命令操作者失败"))?;
    let existing_kind: String =
        row.try_get("command_kind").map_err(|_| storage_error("读取论坛命令类型失败"))?;
    let existing_amount: i64 =
        row.try_get("amount").map_err(|_| storage_error("读取论坛命令金额失败"))?;
    let stored_status: String =
        row.try_get("status").map_err(|_| storage_error("读取论坛命令状态失败"))?;
    let same = existing_auction_id == record.auction_id.as_uuid()
        && existing_world == forum_world_name(record.world)
        && existing_thread_id == record.thread_id
        && existing_post_id == record.post_id
        && existing_line_number == record.line_number
        && existing_lot_code == record.lot_code
        && existing_actor == record.actor
        && existing_kind == record.command_kind
        && existing_amount == record.amount;
    if !same {
        return Err(storage_error("论坛命令标识与既有负载冲突"));
    }
    let (status, should_run) = match stored_status.as_str() {
        "pending" => {
            sqlx::query(
                "UPDATE public.auction_forum_bid_commands
                 SET attempt_count = attempt_count + 1, updated_at = CURRENT_TIMESTAMP
                 WHERE command_id = $1",
            )
            .bind(record.command_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| storage_error("更新论坛命令重试次数失败"))?;
            (ForumCommandStatus::Pending, true)
        }
        "applied" => (ForumCommandStatus::Applied, false),
        "rejected" => (ForumCommandStatus::Rejected, false),
        _ => return Err(storage_error("论坛命令账本状态不在闭集内")),
    };
    transaction.commit().await.map_err(|_| storage_error("提交既有论坛命令账本事务失败"))?;
    Ok(ForumCommandStart { status, should_run })
}

#[cfg(feature = "postgres")]
pub(super) async fn finish_command_postgres(
    store: &PostgresStore,
    command_id: Uuid,
    status: &str,
    error_code: &str,
    safe_detail: &str,
) -> Result<(), ApplicationError> {
    use sqlx::Row;

    let result = sqlx::query(
        "UPDATE public.auction_forum_bid_commands
         SET status = $2,
             last_error_code = NULLIF($3, ''),
             last_error_detail = NULLIF($4, ''),
             updated_at = CURRENT_TIMESTAMP
         WHERE command_id = $1 AND status = 'pending'",
    )
    .bind(command_id)
    .bind(status)
    .bind(error_code)
    .bind(safe_detail)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("完成论坛命令账本失败"))?;
    if result.rows_affected() == 1 {
        return Ok(());
    }
    let current: Option<String> =
        sqlx::query("SELECT status FROM public.auction_forum_bid_commands WHERE command_id = $1")
            .bind(command_id)
            .fetch_optional(store.pool())
            .await
            .map_err(|_| storage_error("读取论坛命令完成状态失败"))?
            .map(|row| row.try_get("status"))
            .transpose()
            .map_err(|_| storage_error("解析论坛命令完成状态失败"))?;
    match current.as_deref() {
        Some(current) if current == status => Ok(()),
        Some(_) => Err(storage_error("论坛命令已由其他状态完成")),
        None => Err(storage_error("论坛命令账本记录不存在")),
    }
}
